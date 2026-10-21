//! Updates from the project's GitLab releases, for an app not connected to a server (a
//! connected app is to follow its server, later).
//!
//! - **Checking:** the latest release from GitLab's public API, at most once a day, and only
//!   for the AppImage and the Windows installation (a build from source is updated with git).
//!   It can be switched off in the settings.
//! - **Verifying:** the release's file for this platform must match its line in the release's
//!   `SHA256SUMS`. That catches broken and cut-off downloads; the sums come from the same
//!   place as the file, so they are no signature.
//! - **Installing:** the AppImage is written next to the running one and renamed into place
//!   (the version in its file name updated, the menu entry following it), then started. On
//!   Windows the installer runs silently once the app has quit, and starts it again.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use serde::Deserialize;
use sha2::{Digest, Sha256};

/// The project on gitlab.com; `$SCPDM_UPDATE_API` replaces it (for trying a release out).
pub const PROJECT_API: &str = "https://gitlab.com/api/v4/projects/87922247";
pub const CHECKSUMS: &str = "SHA256SUMS";
const MAX_DOWNLOAD: u64 = 1 << 30;

pub type Version = (u32, u32, u32);

/// "0.2.1" or "v0.2.1".
pub fn parse_version(text: &str) -> Option<Version> {
    let mut parts = text.trim().trim_start_matches('v').split('.').map(|p| p.parse::<u32>().ok());
    let version = (parts.next()??, parts.next()??, parts.next()??);
    parts.next().is_none().then_some(version)
}

pub fn current() -> Version {
    parse_version(env!("CARGO_PKG_VERSION")).expect("the package version is x.y.z")
}

pub fn version_text((a, b, c): Version) -> String {
    format!("{a}.{b}.{c}")
}

/// How this copy of the app was installed, which decides how it is updated.
#[derive(Clone, Debug, PartialEq)]
pub enum Kind {
    /// The running AppImage file.
    AppImage(PathBuf),
    /// Installed by the Windows installer (its uninstaller is next to the program).
    Installer,
    /// Built from source, or a copied program: updates are only pointed out.
    Other,
}

impl Kind {
    pub fn detect() -> Kind {
        if let Some(file) = std::env::var_os("APPIMAGE").map(PathBuf::from).filter(|p| p.is_file()) {
            return Kind::AppImage(file);
        }
        let installed = std::env::current_exe().ok().and_then(|exe| exe.parent().map(|d| d.join("unins000.exe").is_file()));
        if cfg!(windows) && installed == Some(true) { Kind::Installer } else { Kind::Other }
    }

    /// The end of the release file's name for this kind.
    fn asset_suffix(&self) -> Option<&'static str> {
        match self {
            Kind::AppImage(_) => Some("-x86_64.AppImage"),
            Kind::Installer => Some("-setup.exe"),
            Kind::Other => None,
        }
    }

    /// Whether the app looks for updates by itself.
    pub fn checks(&self) -> bool {
        *self != Kind::Other
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Release {
    pub version: Version,
    /// The release notes (Markdown, from the changelog).
    pub notes: String,
    /// The release's web page.
    pub page: String,
    /// The file for this kind of installation, and the checksums (`None`: the release has
    /// none, then it can only be downloaded by hand).
    pub asset: Option<Asset>,
    pub checksums: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Asset {
    pub name: String,
    pub url: String,
}

#[derive(Deserialize)]
struct ApiRelease {
    tag_name: String,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    _links: Option<ApiLinks>,
    #[serde(default)]
    assets: Option<ApiAssets>,
}

#[derive(Deserialize)]
struct ApiLinks {
    #[serde(rename = "self")]
    page: Option<String>,
}

#[derive(Deserialize)]
struct ApiAssets {
    #[serde(default)]
    links: Vec<ApiLink>,
}

#[derive(Deserialize)]
struct ApiLink {
    name: String,
    url: String,
    #[serde(default)]
    direct_asset_url: Option<String>,
}

fn agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .timeout_connect(Some(Duration::from_secs(10)))
        .timeout_global(Some(Duration::from_secs(30 * 60)))
        .user_agent(concat!("solidcraft-pdm/", env!("CARGO_PKG_VERSION")))
        .build()
        .into()
}

pub fn api() -> String {
    std::env::var("SCPDM_UPDATE_API").ok().filter(|a| !a.is_empty()).unwrap_or_else(|| PROJECT_API.to_owned())
}

/// The latest release, with its file for `kind`.
pub fn latest(api: &str, kind: &Kind) -> Result<Release, String> {
    let url = format!("{}/releases/permalink/latest", api.trim_end_matches('/'));
    let mut response = agent().get(&url).call().map_err(|e| format!("Could not ask for the latest release: {e}"))?;
    let release: ApiRelease = response.body_mut().read_json().map_err(|e| format!("Unexpected answer about the latest release: {e}"))?;
    let version = parse_version(&release.tag_name).ok_or_else(|| format!("The latest release is called {}, not vX.Y.Z", release.tag_name))?;
    let links = release.assets.map(|a| a.links).unwrap_or_default();
    let link = |l: &ApiLink| Asset { name: l.name.clone(), url: l.direct_asset_url.clone().unwrap_or_else(|| l.url.clone()) };
    let asset = kind.asset_suffix().and_then(|suffix| links.iter().find(|l| l.name.ends_with(suffix))).map(link);
    let checksums = links.iter().find(|l| l.name == CHECKSUMS).map(|l| link(l).url);
    Ok(Release {
        version,
        notes: release.description.unwrap_or_default(),
        page: release._links.and_then(|l| l.page).unwrap_or_else(|| "https://gitlab.com/randomlegend/solidcraft-pdm/-/releases".into()),
        asset,
        checksums,
    })
}

/// Bytes downloaded, and of how many (0: not known yet).
#[derive(Default)]
pub struct Progress {
    pub done: AtomicU64,
    pub total: AtomicU64,
}

impl Progress {
    pub fn fraction(&self) -> Option<f32> {
        let total = self.total.load(Ordering::Relaxed);
        (total > 0).then(|| self.done.load(Ordering::Relaxed) as f32 / total as f32)
    }
}

/// `name`'s SHA-256 in a `sha256sum` listing.
fn checksum_of(listing: &str, name: &str) -> Option<String> {
    listing.lines().find_map(|line| {
        let (hash, file) = line.trim().split_once(char::is_whitespace)?;
        let file = file.trim_start().trim_start_matches('*');
        (file == name && hash.len() == 64).then(|| hash.to_ascii_lowercase())
    })
}

/// Downloads the release's file into `dir` (as a hidden `.part` file), checked against the
/// release's checksums.
fn download(release: &Release, dir: &Path, progress: &Progress) -> Result<PathBuf, String> {
    let asset = release.asset.as_ref().ok_or("This release has no file for this installation")?;
    let sums_url = release.checksums.as_ref().ok_or("This release has no checksums, so it cannot be installed from here")?;
    let agent = agent();
    let listing = agent
        .get(sums_url)
        .call()
        .and_then(|mut r| r.body_mut().read_to_string())
        .map_err(|e| format!("Could not download the checksums: {e}"))?;
    let expected = checksum_of(&listing, &asset.name).ok_or_else(|| format!("The checksums do not list {}", asset.name))?;

    let mut response = agent.get(&asset.url).call().map_err(|e| format!("Could not download {}: {e}", asset.name))?;
    if let Some(length) = response.body().content_length() {
        progress.total.store(length, Ordering::Relaxed);
    }
    let part = dir.join(format!(".{}.part", asset.name));
    let result = (|| -> Result<(), String> {
        let mut file = std::fs::File::create(&part).map_err(|e| format!("Cannot write into {}: {e}", dir.display()))?;
        let mut reader = response.body_mut().with_config().limit(MAX_DOWNLOAD).reader();
        let mut hasher = Sha256::new();
        let mut buffer = vec![0; 1 << 16];
        loop {
            let n = reader.read(&mut buffer).map_err(|e| format!("The download broke off: {e}"))?;
            if n == 0 {
                break;
            }
            hasher.update(&buffer[..n]);
            file.write_all(&buffer[..n]).map_err(|e| format!("Cannot write {}: {e}", part.display()))?;
            progress.done.fetch_add(n as u64, Ordering::Relaxed);
        }
        file.sync_all().map_err(|e| e.to_string())?;
        let got: String = hasher.finalize().iter().map(|b| format!("{b:02x}")).collect();
        if got != expected {
            return Err(format!("{} is damaged (its checksum does not match); nothing was changed", asset.name));
        }
        Ok(())
    })();
    match result {
        Ok(()) => Ok(part),
        Err(e) => {
            let _ = std::fs::remove_file(&part);
            Err(e)
        }
    }
}

/// The AppImage's new name: the version in it replaced (`SolidCraft-PDM-0.1.0-x86_64.AppImage`
/// becomes `…-0.2.0-…`); other names are kept.
fn renamed(current: &Path, from: Version, to: Version) -> PathBuf {
    let name = current.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let (old, new) = (format!("-{}-", version_text(from)), format!("-{}-", version_text(to)));
    if name.contains(&old) { current.with_file_name(name.replacen(&old, &new, 1)) } else { current.to_path_buf() }
}

/// Puts the release's AppImage in place of `current`; the new AppImage's path.
pub fn install_appimage(release: &Release, current: &Path, progress: &Progress) -> Result<PathBuf, String> {
    let dir = current.parent().ok_or("The AppImage has no folder")?;
    let part = download(release, dir, progress)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&part, std::fs::Permissions::from_mode(0o755)).map_err(|e| e.to_string())?;
    }
    let target = renamed(current, self::current(), release.version);
    std::fs::rename(&part, &target).map_err(|e| format!("Cannot put the new AppImage in place: {e}"))?;
    if target != current {
        // The running AppImage stays mounted until it quits.
        let _ = std::fs::remove_file(current);
        #[cfg(not(windows))]
        if let Some(data) = crate::menu_entry::data_home() {
            if crate::menu_entry::installed(&data).as_deref() == Some(current) {
                let _ = crate::menu_entry::install(&data, &target);
            }
        }
    }
    Ok(target)
}

/// Downloads the Windows installer (into the temporary folder); its path.
pub fn download_installer(release: &Release, progress: &Progress) -> Result<PathBuf, String> {
    let dir = std::env::temp_dir();
    let part = download(release, &dir, progress)?;
    let name = release.asset.as_ref().map_or("SolidCraft-PDM-setup.exe", |a| a.name.as_str());
    let target = dir.join(name);
    std::fs::rename(&part, &target).map_err(|e| e.to_string())?;
    Ok(target)
}

/// Starts the new version: the AppImage, with this run's arguments, or the installer (which
/// waits for this app to quit, and starts it when done). The caller then quits.
pub fn start(kind: &Kind, program: &Path) -> std::io::Result<()> {
    let mut command = std::process::Command::new(program);
    match kind {
        // The AppImage runtime sets these for the running AppImage; the new one sets its own.
        Kind::AppImage(_) => {
            command.args(std::env::args_os().skip(1));
            for var in ["APPIMAGE", "APPDIR", "ARGV0", "OWD"] {
                command.env_remove(var);
            }
        }
        _ => {
            command.args(["/SP-", "/SILENT", "/SUPPRESSMSGBOXES", "/NORESTART"]);
        }
    }
    command.spawn().map(drop)
}

/// A check or an installation on its own thread.
pub fn spawn<T: Send + 'static>(ctx: &eframe::egui::Context, work: impl FnOnce() -> T + Send + 'static) -> std::sync::mpsc::Receiver<T> {
    let (tx, rx) = std::sync::mpsc::channel();
    let ctx = ctx.clone();
    std::thread::spawn(move || {
        let _ = tx.send(work());
        ctx.request_repaint();
    });
    rx
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;

    const IMAGE: &str = "SolidCraft-PDM-9.0.0-x86_64.AppImage";

    /// Serves `routes(base URL)` (path → body) over HTTP until the test ends.
    fn serve(routes: impl FnOnce(&str) -> Vec<(String, Vec<u8>)>) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let routes = routes(&url);
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let mut stream = stream.unwrap();
                let mut request = Vec::new();
                let mut byte = [0];
                while !request.ends_with(b"\r\n\r\n") && stream.read(&mut byte).unwrap_or(0) == 1 {
                    request.push(byte[0]);
                }
                let text = String::from_utf8_lossy(&request).into_owned();
                let path = text.split(' ').nth(1).unwrap_or_default().to_owned();
                let (status, body) = match routes.iter().find(|(p, _)| *p == path) {
                    Some((_, body)) => ("200 OK", body.clone()),
                    None => ("404 Not Found", b"not found".to_vec()),
                };
                let head = format!("HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len());
                let _ = stream.write_all(head.as_bytes());
                let _ = stream.write_all(&body);
            }
        });
        url
    }

    fn sha256(bytes: &[u8]) -> String {
        Sha256::digest(bytes).iter().map(|b| format!("{b:02x}")).collect()
    }

    /// A GitLab API with release v9.0.0: an AppImage with `image` and, unless `None`, a
    /// checksum listing.
    fn gitlab(image: &'static [u8], sums: Option<String>) -> String {
        serve(move |base| {
            let mut links = vec![serde_json::json!({"name": IMAGE, "url": "http://unused", "direct_asset_url": format!("{base}/{IMAGE}")})];
            if sums.is_some() {
                links.push(serde_json::json!({"name": CHECKSUMS, "url": format!("{base}/{CHECKSUMS}")}));
            }
            let release = serde_json::json!({
                "tag_name": "v9.0.0",
                "description": "### Big\n\n- Things",
                "_links": {"self": "https://example.org/release"},
                "assets": {"links": links},
            });
            let mut routes = vec![("/releases/permalink/latest".to_owned(), release.to_string().into_bytes()), (format!("/{IMAGE}"), image.to_vec())];
            if let Some(sums) = sums {
                routes.push((format!("/{CHECKSUMS}"), sums.into_bytes()));
            }
            routes
        })
    }

    fn current_appimage(dir: &Path) -> PathBuf {
        let file = dir.join(format!("SolidCraft-PDM-{}-x86_64.AppImage", env!("CARGO_PKG_VERSION")));
        std::fs::write(&file, b"old").unwrap();
        file
    }

    #[test]
    fn finds_the_latest_release_and_its_file() {
        let api = gitlab(b"new", Some(String::new()));
        let kind = Kind::AppImage("/x".into());
        let release = latest(&api, &kind).unwrap();
        assert_eq!((release.version, release.page.as_str()), ((9, 0, 0), "https://example.org/release"));
        assert!(release.notes.contains("Things"));
        assert_eq!(release.asset.as_ref().unwrap().url, format!("{api}/{IMAGE}"), "the direct link");
        assert!(latest(&api, &Kind::Installer).unwrap().asset.is_none(), "no Windows file in it");
        assert!(latest(&api, &Kind::Other).unwrap().asset.is_none());
    }

    #[test]
    fn installs_a_verified_appimage_under_its_new_name() {
        let api = gitlab(b"new version", Some(format!("{}  {IMAGE}\n", sha256(b"new version"))));
        let dir = tempfile::tempdir().unwrap();
        let current = current_appimage(dir.path());
        let kind = Kind::AppImage(current.clone());
        let progress = Progress::default();
        let installed = install_appimage(&latest(&api, &kind).unwrap(), &current, &progress).unwrap();
        assert_eq!(installed, dir.path().join(IMAGE));
        assert_eq!(std::fs::read(&installed).unwrap(), b"new version");
        assert!(!current.exists(), "the old one is gone");
        assert_eq!(progress.fraction(), Some(1.0));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(&installed).unwrap().permissions().mode() & 0o111, 0o111, "executable");
        }
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1, "no leftovers");
    }

    #[test]
    fn a_damaged_or_unlisted_download_changes_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let current = current_appimage(dir.path());
        let kind = Kind::AppImage(current.clone());
        for (sums, why) in [
            (Some(format!("{}  {IMAGE}\n", sha256(b"something else"))), "damaged"),
            (Some(format!("{}  other-file\n", sha256(b"new"))), "do not list"),
            (None, "no checksums"),
        ] {
            let api = gitlab(b"new", sums);
            let err = install_appimage(&latest(&api, &kind).unwrap(), &current, &Progress::default()).unwrap_err();
            assert!(err.contains(why), "{err}");
            assert_eq!(std::fs::read(&current).unwrap(), b"old");
            assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1, "no leftovers");
        }
    }

    /// The real project (network): `cargo test -p scpdm-gui -- --ignored gitlab`.
    #[test]
    #[ignore]
    fn the_latest_release_on_gitlab() {
        let release = latest(PROJECT_API, &Kind::AppImage("/x".into())).unwrap();
        assert!(release.version >= (0, 1, 0));
        let asset = release.asset.expect("an AppImage");
        let response = agent().head(&asset.url).call().unwrap();
        assert!(response.body().content_length().unwrap_or(0) > 1_000_000 || response.status() == 200, "{asset:?}");
    }

    #[test]
    fn versions() {
        assert_eq!(parse_version("v0.12.3"), Some((0, 12, 3)));
        assert_eq!(parse_version("1.2"), None);
        assert_eq!(parse_version("1.2.3.4"), None);
        assert!(parse_version("v0.2.0") > parse_version("0.1.9"));
        assert!(parse_version("0.10.0") > parse_version("0.9.0"), "numbers, not text");
    }

    #[test]
    fn checksum_listings() {
        let hash = "a".repeat(64);
        let listing = format!("{hash}  SolidCraft-PDM-1.0.0-x86_64.AppImage\n{}  *other.exe\n", "B".repeat(64));
        assert_eq!(checksum_of(&listing, "SolidCraft-PDM-1.0.0-x86_64.AppImage"), Some(hash));
        assert_eq!(checksum_of(&listing, "other.exe"), Some("b".repeat(64)));
        assert_eq!(checksum_of(&listing, "missing"), None);
    }

    #[test]
    fn appimage_names_follow_the_version() {
        let p = Path::new("/apps/SolidCraft-PDM-0.1.0-x86_64.AppImage");
        assert_eq!(renamed(p, (0, 1, 0), (0, 2, 0)), Path::new("/apps/SolidCraft-PDM-0.2.0-x86_64.AppImage"));
        assert_eq!(renamed(Path::new("/apps/pdm.AppImage"), (0, 1, 0), (0, 2, 0)), Path::new("/apps/pdm.AppImage"));
    }
}
