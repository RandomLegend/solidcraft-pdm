//! Thumbnails drawn by SolidCraft itself (`solidcraft-cli snapshot`): shaded, with the model's
//! edges outlined, as on SolidCraft's start page, instead of the plainer picture SolidCraft saves
//! in the file (which stays the fallback).
//!
//! The program is found once, on a worker thread: `$SCPDM_SOLIDCRAFT_CLI`, `solidcraft-cli` on
//! `PATH`, next to the SolidCraft program the app opens designs with, or taken out of
//! SolidCraft's AppImage (`--appimage-extract`, into the cache). Pictures are kept by the
//! design's content hash in `~/.cache/solidcraft-pdm/thumbnails/` (Windows: `%LOCALAPPDATA%`),
//! so each version is drawn once. A design the program cannot draw (a newer format, say) is
//! remembered as such until the program changes.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use eframe::egui;

/// The picture's size: the list's and the details panel's thumbnails at 1.6 : 1, sharp at 150 %.
const WIDTH: u32 = 640;
const HEIGHT: u32 = 400;
/// A drawing that takes longer than this is given up.
const TIMEOUT: Duration = Duration::from_secs(60);
const WORKERS: usize = 2;

/// What to draw: a working file (drawn only if it still has the content asked for), or bytes.
pub enum Source {
    File(PathBuf),
    Bytes(Vec<u8>),
}

/// Whether the program was found.
#[derive(Clone, Debug, Default, PartialEq)]
pub enum Found {
    #[default]
    NotYet,
    Program(PathBuf),
    Missing,
}

struct Job {
    hash: String,
    source: Source,
}

pub struct Renderer {
    jobs: Sender<Job>,
    done: Receiver<(String, Option<Arc<[u8]>>)>,
    asked: HashSet<String>,
    cache: PathBuf,
    found: Arc<Mutex<Found>>,
}

/// Where the app keeps what it can make again.
pub fn cache_root() -> Option<PathBuf> {
    let base = if cfg!(windows) {
        std::env::var_os("LOCALAPPDATA").map(PathBuf::from).map(|p| p.join("SolidCraft PDM"))
    } else {
        std::env::var_os("XDG_CACHE_HOME")
            .map(PathBuf::from)
            .filter(|p| p.is_absolute())
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cache")))
            .map(|p| p.join("solidcraft-pdm"))
    };
    base
}

impl Renderer {
    /// Starts the workers; `solidcraft`: the program the app opens designs with, if known.
    pub fn start(ctx: &egui::Context, solidcraft: Option<PathBuf>) -> Option<Renderer> {
        let root = cache_root()?;
        let cache = root.join("thumbnails");
        std::fs::create_dir_all(cache.join("tmp")).ok()?;
        let (jobs, rx) = channel::<Job>();
        let (tx, done) = channel();
        let rx = Arc::new(Mutex::new(rx));
        let found = Arc::new(Mutex::new(Found::NotYet));
        let program: Arc<Mutex<Option<Option<PathBuf>>>> = Arc::new(Mutex::new(None));
        for _ in 0..WORKERS {
            let (rx, tx, ctx, cache, root, found, program, solidcraft) =
                (rx.clone(), tx.clone(), ctx.clone(), cache.clone(), root.clone(), found.clone(), program.clone(), solidcraft.clone());
            std::thread::spawn(move || {
                loop {
                    let job = match rx.lock().unwrap_or_else(|e| e.into_inner()).recv() {
                        Ok(job) => job,
                        Err(_) => return,
                    };
                    // The program, looked for by the first worker that needs it.
                    let cli = {
                        let mut program = program.lock().unwrap_or_else(|e| e.into_inner());
                        if program.is_none() {
                            let cli = find_cli(solidcraft.as_deref(), &root);
                            *found.lock().unwrap_or_else(|e| e.into_inner()) = match &cli {
                                Some(p) => Found::Program(p.clone()),
                                None => Found::Missing,
                            };
                            *program = Some(cli);
                        }
                        program.clone().flatten()
                    };
                    let png = cli.and_then(|cli| draw(&cli, &cache, &job.hash, job.source));
                    if tx.send((job.hash, png)).is_err() {
                        return;
                    }
                    ctx.request_repaint();
                }
            });
        }
        Some(Renderer { jobs, done, asked: HashSet::new(), cache, found })
    }

    pub fn found(&self) -> Found {
        self.found.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    /// A picture drawn before (this or an earlier run).
    pub fn cached(&self, hash: &str) -> Option<Arc<[u8]>> {
        if !vault::is_hash(hash) {
            return None;
        }
        std::fs::read(self.cache.join(format!("{hash}.png"))).ok().map(Arc::from)
    }

    /// Asks for a picture of the content `hash` (once); it arrives through [`Renderer::finished`].
    /// `source` is only called when it is not drawn yet.
    pub fn request(&mut self, hash: &str, source: impl FnOnce() -> Option<Source>) {
        if !vault::is_hash(hash) || self.asked.contains(hash) || self.found() == Found::Missing {
            return;
        }
        self.asked.insert(hash.to_owned());
        if let Some(source) = source() {
            let _ = self.jobs.send(Job { hash: hash.to_owned(), source });
        }
    }

    /// Pictures finished since the last call (`None`: could not be drawn).
    pub fn finished(&mut self) -> Vec<(String, Option<Arc<[u8]>>)> {
        self.done.try_iter().collect()
    }
}

/// Draws the design into the cache; its PNG.
fn draw(cli: &Path, cache: &Path, hash: &str, source: Source) -> Option<Arc<[u8]>> {
    let out = cache.join(format!("{hash}.png"));
    if let Ok(png) = std::fs::read(&out) {
        return Some(Arc::from(png));
    }
    // Not drawable with this program: remembered until the program changes.
    let failed = cache.join(format!("{hash}.failed"));
    let program = identity(cli);
    if std::fs::read_to_string(&failed).ok().as_deref() == Some(program.as_str()) {
        return None;
    }
    let bytes = match source {
        Source::Bytes(b) => b,
        Source::File(path) => std::fs::read(path).ok()?,
    };
    if vault::hash(&bytes) != hash {
        // Changed since it was asked for: the next scan asks again.
        return None;
    }
    let tmp = cache.join("tmp");
    let design = tmp.join(format!("{hash}.solidcraft"));
    let picture = tmp.join(format!("{hash}.png"));
    std::fs::write(&design, &bytes).ok()?;
    let ok = run(
        Command::new(cli)
            .arg("snapshot")
            .arg("--in")
            .arg(&design)
            .arg("--out")
            .arg(&picture)
            .args(["--width", &WIDTH.to_string(), "--height", &HEIGHT.to_string(), "--view", "iso", "--no-grid", "--no-axes"]),
    );
    let _ = std::fs::remove_file(&design);
    let png = ok.then(|| std::fs::read(&picture).ok()).flatten().filter(|p| p.starts_with(b"\x89PNG"));
    match png {
        Some(png) => {
            let _ = std::fs::rename(&picture, &out);
            Some(Arc::from(png))
        }
        None => {
            let _ = std::fs::remove_file(&picture);
            let _ = std::fs::write(&failed, program);
            None
        }
    }
}

/// Runs a command to its end (at most [`TIMEOUT`]); whether it succeeded.
fn run(command: &mut Command) -> bool {
    let Ok(mut child) = command.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn() else { return false };
    let start = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return status.success(),
            Ok(None) if start.elapsed() < TIMEOUT => std::thread::sleep(Duration::from_millis(50)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return false;
            }
        }
    }
}

/// The program as it is (path, size, time): a new SolidCraft may draw what an old one could not.
fn identity(cli: &Path) -> String {
    let meta = std::fs::metadata(cli).ok();
    let size = meta.as_ref().map_or(0, |m| m.len());
    let time = meta.and_then(|m| m.modified().ok()).and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map_or(0, |d| d.as_secs());
    format!("{} {size} {time}", cli.display())
}

fn cli_name() -> &'static str {
    if cfg!(windows) { "solidcraft-cli.exe" } else { "solidcraft-cli" }
}

/// SolidCraft's command line program: see the module documentation.
pub fn find_cli(solidcraft: Option<&Path>, cache_root: &Path) -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("SCPDM_SOLIDCRAFT_CLI").map(PathBuf::from).filter(|p| p.is_file()) {
        return Some(p);
    }
    if let Some(p) = std::env::var_os("PATH").and_then(|path| std::env::split_paths(&path).map(|d| d.join(cli_name())).find(|p| p.is_file())) {
        return Some(p);
    }
    let program = solidcraft.and_then(|p| std::fs::canonicalize(p).ok())?;
    let is_appimage = program.extension().is_some_and(|e| e.eq_ignore_ascii_case("appimage"));
    if is_appimage {
        return from_appimage(&program, &cache_root.join("solidcraft-cli"));
    }
    program.parent().map(|dir| dir.join(cli_name())).filter(|p| p.is_file())
}

/// `solidcraft-cli` out of SolidCraft's AppImage, extracted once per AppImage (path, size, time).
fn from_appimage(appimage: &Path, dir: &Path) -> Option<PathBuf> {
    let id = vault::hash(identity(appimage).as_bytes());
    let folder = dir.join(&id[..16]);
    let cli = folder.join("squashfs-root/usr/bin/solidcraft-cli");
    if cli.is_file() {
        return Some(cli);
    }
    std::fs::create_dir_all(&folder).ok()?;
    let ok = run(Command::new(appimage).arg("--appimage-extract").arg("usr/bin/solidcraft-cli").current_dir(&folder));
    // Older extractions of other AppImages are no longer needed.
    if ok && cli.is_file() {
        if let Ok(entries) = std::fs::read_dir(dir) {
            for entry in entries.flatten() {
                if entry.path() != folder && entry.file_type().is_ok_and(|t| t.is_dir()) {
                    let _ = std::fs::remove_dir_all(entry.path());
                }
            }
        }
        return Some(cli);
    }
    None
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    const PNG: &[u8] = b"\x89PNG\r\n\x1a\nfake picture";

    fn script(path: &Path, body: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, format!("#!/bin/sh\n{body}\n")).unwrap();
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    /// A stand-in for solidcraft-cli: writes a "picture" to --out unless the design says "bad".
    fn fake_cli(path: &Path) {
        script(
            path,
            r#"while [ $# -gt 0 ]; do case "$1" in --in) shift; in="$1";; --out) shift; out="$1";; esac; shift; done
grep -q bad "$in" && exit 1
printf '\211PNG\r\n\032\nfake picture' > "$out""#,
        );
    }

    #[test]
    fn takes_the_program_out_of_an_appimage_once() {
        let dir = tempfile::tempdir().unwrap();
        let appimage = dir.path().join("SolidCraft-0.19.0-x86_64.AppImage");
        // Like the AppImage runtime: --appimage-extract PATTERN writes squashfs-root/PATTERN here.
        script(
            &appimage,
            r#"[ "$1" = --appimage-extract ] || exit 2
mkdir -p squashfs-root/usr/bin && echo extracted >> ../count && printf '#!/bin/sh\nexit 0\n' > squashfs-root/usr/bin/solidcraft-cli && chmod +x squashfs-root/usr/bin/solidcraft-cli"#,
        );
        let cache = dir.path().join("cache");
        let first = from_appimage(&appimage, &cache.join("solidcraft-cli")).unwrap();
        assert!(first.is_file());
        let again = from_appimage(&appimage, &cache.join("solidcraft-cli")).unwrap();
        assert_eq!(first, again);
        let count = std::fs::read_to_string(cache.join("solidcraft-cli/count")).unwrap();
        assert_eq!(count.lines().count(), 1, "extracted once");
    }

    #[test]
    fn draws_once_and_remembers_what_it_cannot_draw() {
        let dir = tempfile::tempdir().unwrap();
        let cli = dir.path().join("bin/solidcraft-cli");
        fake_cli(&cli);
        let cache = dir.path().join("thumbnails");
        std::fs::create_dir_all(cache.join("tmp")).unwrap();
        let good = b"a design".to_vec();
        let hash = vault::hash(&good);
        assert_eq!(draw(&cli, &cache, &hash, Source::Bytes(good.clone())).as_deref(), Some(PNG));
        assert!(cache.join(format!("{hash}.png")).is_file(), "kept");
        // From the cache, without the program.
        assert_eq!(draw(Path::new("/nonexistent"), &cache, &hash, Source::Bytes(good)).as_deref(), Some(PNG));

        let bad = b"a bad design".to_vec();
        let bad_hash = vault::hash(&bad);
        assert!(draw(&cli, &cache, &bad_hash, Source::Bytes(bad.clone())).is_none());
        assert!(cache.join(format!("{bad_hash}.failed")).is_file(), "remembered");
        // A working file that changed in between is not drawn under the old hash.
        let file = dir.path().join("part.solidcraft");
        std::fs::write(&file, b"changed meanwhile").unwrap();
        assert!(draw(&cli, &cache, &hash.replace('a', "b"), Source::File(file)).is_none());
        assert_eq!(std::fs::read_dir(cache.join("tmp")).unwrap().count(), 0, "no leftovers");
    }
}
