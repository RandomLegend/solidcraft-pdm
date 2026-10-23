//! Opening designs in SolidCraft, other files in their usual program, and showing files in the
//! file manager.
//!
//! Linux: `xdg-open` is not used for designs. SolidCraft's AppImage installs a desktop entry
//! (`ai.storyteller.solidcraft.desktop`, `Exec="/path/to/SolidCraft.AppImage" %F`) but no MIME
//! type for `*.solidcraft`, so `xdg-open` guesses from the contents (a ZIP, or JSON) and opens
//! whatever handles those. Instead SolidCraft's desktop entry is found the way a launcher finds
//! it, and its `Exec` line is run.
//!
//! Windows: SolidCraft's installer puts `solidcraft.exe` in `Program Files\SolidCraft` and
//! associates `.solidcraft` with it, so the program is looked for there, and the association
//! is the fallback.

use std::path::{Path, PathBuf};
use std::process::Command;

#[cfg(not(windows))]
const DESKTOP_IDS: [&str; 2] = ["ai.storyteller.solidcraft.desktop", "ai.storyteller.solvecraft.desktop"];
#[cfg(not(windows))]
const MIME_TYPE: &str = "application/x-solidcraft";

/// How a design gets opened.
#[derive(Clone, Debug, PartialEq)]
pub struct Launcher {
    /// Program and arguments; `%f` (and the other file codes) is replaced by the design.
    pub command: Vec<String>,
    /// Where it came from, for Settings.
    pub source: String,
}

impl Launcher {
    /// Starts SolidCraft with `file`.
    pub fn open(&self, file: &Path) -> std::io::Result<()> {
        let args = expand(&self.command, file);
        let (program, args) = args.split_first().ok_or(std::io::ErrorKind::InvalidInput)?;
        spawn(Command::new(program).args(args))
    }
}

/// The user's own command: a path to the program (spaces allowed), or a command line with
/// arguments in desktop-entry quoting (`"/my apps/SolidCraft.AppImage" --flag %f`).
pub fn custom(command: &str) -> Option<Launcher> {
    let command = command.trim();
    if command.is_empty() {
        return None;
    }
    let path = expand_home(command);
    let mut words = if path.is_file() { vec![path.to_string_lossy().into_owned()] } else { split_exec(command)? };
    if let Some(first) = words.first_mut() {
        *first = expand_home(first).to_string_lossy().into_owned();
    }
    Some(Launcher { command: words, source: "your setting".into() })
}

/// SolidCraft as installed: its desktop entry (the AppImage's own, or one from Gear Lever,
/// AppImageLauncher, appimaged, Flatpak, a package), else `solidcraft` on `PATH`.
#[cfg(not(windows))]
pub fn detect() -> Option<Launcher> {
    detect_in(&application_dirs(), std::env::var_os("PATH").as_deref())
}

/// SolidCraft as installed: `solidcraft.exe` where the installer puts it, else whatever Windows
/// opens `.solidcraft` files with.
#[cfg(windows)]
pub fn detect() -> Option<Launcher> {
    let roots = ["ProgramW6432", "ProgramFiles", "ProgramFiles(x86)"]
        .into_iter()
        .filter_map(std::env::var_os)
        .map(PathBuf::from)
        .chain(std::env::var_os("LOCALAPPDATA").map(|d| PathBuf::from(d).join("Programs")));
    for root in roots {
        let exe = root.join("SolidCraft").join("solidcraft.exe");
        if exe.is_file() {
            let source = exe.display().to_string();
            return Some(Launcher { command: vec![source.clone()], source });
        }
    }
    Some(Launcher { command: vec!["explorer.exe".into()], source: "the Windows file association".into() })
}

#[cfg(not(windows))]
pub(crate) fn detect_in(dirs: &[PathBuf], path_var: Option<&std::ffi::OsStr>) -> Option<Launcher> {
    let mut best: Option<(u8, Launcher)> = None;
    for dir in dirs {
        for file in desktop_files(dir) {
            let Ok(text) = std::fs::read_to_string(&file) else { continue };
            let Some(entry) = DesktopEntry::parse(&text) else { continue };
            let rank = file_rank(&file).max(entry.rank());
            if rank == 0 || entry.hidden || !entry.is_application {
                continue;
            }
            let Some(command) = entry.exec.as_deref().and_then(split_exec) else { continue };
            if !command.first().is_some_and(|p| exists(p, path_var)) {
                continue;
            }
            // Earlier folders (the user's own) win between equal ranks.
            if best.as_ref().is_none_or(|(r, _)| rank > *r) {
                best = Some((rank, Launcher { command, source: file.display().to_string() }));
            }
        }
    }
    best.map(|(_, l)| l).or_else(|| {
        exists("solidcraft", path_var)
            .then(|| Launcher { command: vec!["solidcraft".into()], source: "solidcraft on PATH".into() })
    })
}

/// A program found installed that opens a kind of file (a slicer for STL, say).
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct FileApp {
    /// As the program calls itself ("PrusaSlicer").
    pub name: String,
    /// Its command line, as [`custom`] takes it (`%f` stands for the file).
    pub command: String,
}

impl FileApp {
    /// A program chosen as a file: named after it.
    pub fn from_program(path: &Path) -> FileApp {
        let name = path.file_stem().map_or_else(|| path.display().to_string(), |n| n.to_string_lossy().into_owned());
        // Quoted, so a path with spaces stays one word.
        FileApp { name, command: format!("\"{}\" %f", path.display().to_string().replace('"', "\\\"")) }
    }

    /// Starts the program with `file`.
    pub fn open(&self, file: &Path) -> std::io::Result<()> {
        custom(&self.command).ok_or(std::io::ErrorKind::InvalidInput)?.open(file)
    }
}

/// The MIME types programs declare for a file extension (production files).
#[cfg(not(windows))]
fn mime_types(extension: &str) -> &'static [&'static str] {
    match extension {
        "stl" => &["model/stl", "model/x.stl-ascii", "model/x.stl-binary", "application/sla", "application/vnd.ms-pki.stl"],
        "3mf" => &["model/3mf", "application/vnd.ms-3mfdocument", "application/vnd.ms-package.3dmanufacturing-3dmodel+xml"],
        "step" | "stp" => &["model/step", "application/step", "model/step+xml", "model/step-xml+zip", "model/step+zip", "application/x-step"],
        _ => &[],
    }
}

/// Installed programs that open files with `extension` (their desktop entries say so), by name.
#[cfg(not(windows))]
pub fn apps_for(extension: &str) -> Vec<FileApp> {
    apps_in(&application_dirs(), std::env::var_os("PATH").as_deref(), extension)
}

/// Windows keeps its programs per type in the registry; there the system's choice is used, or a
/// program picked by hand.
#[cfg(windows)]
pub fn apps_for(_extension: &str) -> Vec<FileApp> {
    Vec::new()
}

#[cfg(not(windows))]
pub(crate) fn apps_in(dirs: &[PathBuf], path_var: Option<&std::ffi::OsStr>, extension: &str) -> Vec<FileApp> {
    let wanted = mime_types(extension);
    let mut found: Vec<FileApp> = Vec::new();
    let mut seen_ids = std::collections::HashSet::new();
    for dir in dirs {
        for file in desktop_files(dir) {
            // The first folder with an id wins (the user's own over the system's).
            let id = file.strip_prefix(dir).map_or_else(|_| file.clone(), Path::to_path_buf);
            if !seen_ids.insert(id) {
                continue;
            }
            let Ok(text) = std::fs::read_to_string(&file) else { continue };
            let Some(entry) = DesktopEntry::parse(&text) else { continue };
            if entry.hidden || !entry.is_application || !entry.mime_types.iter().any(|m| wanted.contains(&m.as_str())) {
                continue;
            }
            let (Some(name), Some(exec)) = (entry.name, entry.exec) else { continue };
            if !split_exec(&exec).and_then(|c| c.into_iter().next()).is_some_and(|p| exists(&p, path_var)) {
                continue;
            }
            if !found.iter().any(|a| a.name == name) {
                found.push(FileApp { name, command: exec });
            }
        }
    }
    found.sort_by_key(|a| a.name.to_lowercase());
    found
}

/// Opens `file` with the system's program for its type (images: the image viewer).
pub fn open_with_system(file: &Path) -> std::io::Result<()> {
    #[cfg(windows)]
    let program = "explorer.exe";
    #[cfg(target_os = "macos")]
    let program = "open";
    #[cfg(not(any(windows, target_os = "macos")))]
    let program = "xdg-open";
    spawn(Command::new(program).arg(file))
}

/// Opens Explorer with `file` selected.
#[cfg(windows)]
pub fn show_in_folder(file: &Path) -> std::io::Result<()> {
    use std::os::windows::process::CommandExt;
    // Explorer wants the quotes around the path only, after the comma.
    spawn(Command::new("explorer.exe").raw_arg(format!("/select,\"{}\"", file.display())))
}

/// Opens the file manager with `file` selected (FileManager1 over D-Bus: Dolphin, Nautilus,
/// Nemo…), else opens its folder.
#[cfg(not(windows))]
pub fn show_in_folder(file: &Path) -> std::io::Result<()> {
    let selected = Command::new("dbus-send")
        .args([
            "--session",
            "--print-reply",
            "--dest=org.freedesktop.FileManager1",
            "--type=method_call",
            "/org/freedesktop/FileManager1",
            "org.freedesktop.FileManager1.ShowItems",
        ])
        .arg(format!("array:string:{}", file_uri(file)))
        .arg("string:")
        .output()
        .is_ok_and(|o| o.status.success());
    if selected {
        return Ok(());
    }
    let dir = file.parent().unwrap_or(file);
    spawn(Command::new("xdg-open").arg(dir))
}

/// Starts a program without waiting for it; a thread reaps it when it exits.
fn spawn(command: &mut Command) -> std::io::Result<()> {
    let mut child = command.spawn()?;
    std::thread::spawn(move || child.wait());
    Ok(())
}

/// 3 for SolidCraft's own desktop ids, else 0.
#[cfg(not(windows))]
fn file_rank(file: &Path) -> u8 {
    let id = file.file_name().and_then(|n| n.to_str()).unwrap_or("");
    if DESKTOP_IDS.contains(&id) { 3 } else { 0 }
}

#[cfg(not(windows))]
struct DesktopEntry {
    name: Option<String>,
    exec: Option<String>,
    mime_types: Vec<String>,
    hidden: bool,
    is_application: bool,
}

#[cfg(not(windows))]
impl DesktopEntry {
    /// The `[Desktop Entry]` group (other groups are actions and are skipped).
    fn parse(text: &str) -> Option<Self> {
        let mut in_main = false;
        let mut entry =
            DesktopEntry { name: None, exec: None, mime_types: Vec::new(), hidden: false, is_application: false };
        let mut seen_main = false;
        for line in text.lines() {
            let line = line.trim();
            if line.starts_with('[') {
                in_main = line == "[Desktop Entry]";
                seen_main |= in_main;
                continue;
            }
            if !in_main || line.starts_with('#') {
                continue;
            }
            let Some((key, value)) = line.split_once('=') else { continue };
            let (key, value) = (key.trim(), value.trim());
            match key {
                "Name" => entry.name = Some(value.to_owned()),
                "Exec" => entry.exec = Some(value.to_owned()),
                "MimeType" => entry.mime_types = value.split(';').filter(|s| !s.is_empty()).map(str::to_owned).collect(),
                "Hidden" => entry.hidden = value == "true",
                "Type" => entry.is_application = value == "Application",
                _ => {}
            }
        }
        seen_main.then_some(entry)
    }

    /// 2: opens SolidCraft designs; 1: named SolidCraft; 0: something else.
    fn rank(&self) -> u8 {
        let name = self.name.as_deref().unwrap_or("").to_lowercase();
        if name.contains("pdm") {
            // Never this app itself.
            0
        } else if self.mime_types.iter().any(|m| m == MIME_TYPE || m == "application/x-solvecraft") {
            2
        } else if name == "solidcraft" || name.starts_with("solidcraft ") {
            1
        } else {
            0
        }
    }
}

/// `$XDG_DATA_HOME/applications`, then each of `$XDG_DATA_DIRS`.
#[cfg(not(windows))]
fn application_dirs() -> Vec<PathBuf> {
    let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default();
    let data_home = std::env::var_os("XDG_DATA_HOME").map(PathBuf::from).unwrap_or_else(|| home.join(".local/share"));
    let data_dirs = std::env::var("XDG_DATA_DIRS")
        .ok()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "/usr/local/share:/usr/share".into());
    std::iter::once(data_home)
        .chain(data_dirs.split(':').filter(|s| !s.is_empty()).map(PathBuf::from))
        .map(|d| d.join("applications"))
        .collect()
}

#[cfg(not(windows))]
fn desktop_files(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(read) = std::fs::read_dir(&d) else { continue };
        for entry in read.flatten() {
            let p = entry.path();
            if p.is_dir() {
                stack.push(p);
            } else if p.extension().is_some_and(|e| e == "desktop") {
                out.push(p);
            }
        }
    }
    out.sort();
    out
}

/// Splits an `Exec` value into words: the desktop-entry escapes (`\s`, `\\`…) and then
/// double-quoted arguments, in which `\"`, `` \` ``, `\$` and `\\` are escapes.
pub(crate) fn split_exec(exec: &str) -> Option<Vec<String>> {
    let mut unescaped = String::new();
    let mut chars = exec.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            match chars.next() {
                Some('s') => unescaped.push(' '),
                Some('n') => unescaped.push('\n'),
                Some('t') => unescaped.push('\t'),
                Some('r') => unescaped.push('\r'),
                Some(other) => {
                    // Kept for the quoting pass (`\"` inside quotes).
                    unescaped.push('\\');
                    unescaped.push(other);
                }
                None => return None,
            }
        } else {
            unescaped.push(c);
        }
    }

    let mut words = Vec::new();
    let mut word = String::new();
    let mut in_word = false;
    let mut chars = unescaped.chars();
    while let Some(c) = chars.next() {
        match c {
            '"' => {
                in_word = true;
                loop {
                    match chars.next()? {
                        '"' => break,
                        '\\' => word.push(chars.next()?),
                        c => word.push(c),
                    }
                }
            }
            c if c.is_whitespace() => {
                if in_word {
                    words.push(std::mem::take(&mut word));
                    in_word = false;
                }
            }
            '\\' => {
                in_word = true;
                word.push(chars.next()?);
            }
            c => {
                in_word = true;
                word.push(c);
            }
        }
    }
    if in_word {
        words.push(word);
    }
    (!words.is_empty()).then_some(words)
}

/// The command for `file`: `%f %F %u %U` become the file (appended if none is present),
/// `%%` is `%`, and the other field codes are dropped.
pub(crate) fn expand(command: &[String], file: &Path) -> Vec<String> {
    let file = file.to_string_lossy();
    let mut placed = false;
    let mut out = Vec::new();
    for word in command {
        if matches!(word.as_str(), "%f" | "%F" | "%u" | "%U") {
            out.push(file.to_string());
            placed = true;
            continue;
        }
        if matches!(word.as_str(), "%i" | "%c" | "%k") {
            continue;
        }
        let mut expanded = String::new();
        let mut chars = word.chars();
        while let Some(c) = chars.next() {
            if c != '%' {
                expanded.push(c);
                continue;
            }
            match chars.next() {
                Some('%') => expanded.push('%'),
                Some('f' | 'F' | 'u' | 'U') => {
                    expanded.push_str(&file);
                    placed = true;
                }
                _ => {}
            }
        }
        out.push(expanded);
    }
    if !placed {
        out.push(file.into_owned());
    }
    out
}

#[cfg(not(windows))]
fn exists(program: &str, path_var: Option<&std::ffi::OsStr>) -> bool {
    if program.contains('/') {
        return Path::new(program).is_file();
    }
    path_var.is_some_and(|paths| std::env::split_paths(paths).any(|d| d.join(program).is_file()))
}

fn expand_home(path: &str) -> PathBuf {
    match (path.strip_prefix("~/"), std::env::var_os("HOME")) {
        (Some(rest), Some(home)) => PathBuf::from(home).join(rest),
        _ => PathBuf::from(path),
    }
}

/// `file://` URI with everything but unreserved characters and `/` percent-encoded.
#[cfg(not(windows))]
fn file_uri(path: &Path) -> String {
    use std::os::unix::ffi::OsStrExt;
    let mut uri = String::from("file://");
    for &b in path.as_os_str().as_bytes() {
        if b.is_ascii_alphanumeric() || b"-._~/".contains(&b) {
            uri.push(b as char);
        } else {
            uri.push_str(&format!("%{b:02X}"));
        }
    }
    uri
}

#[cfg(test)]
mod tests {
    use super::*;

    fn words(s: &[&str]) -> Vec<String> {
        s.iter().map(|w| w.to_string()).collect()
    }

    #[test]
    fn splits_exec_lines() {
        assert_eq!(split_exec(r#""/home/me/My Apps/SolidCraft.AppImage" %F"#).unwrap(), words(&["/home/me/My Apps/SolidCraft.AppImage", "%F"]));
        assert_eq!(split_exec("solidcraft %f").unwrap(), words(&["solidcraft", "%f"]));
        assert_eq!(split_exec(r#"sh -c "echo \"hi\"""#).unwrap(), words(&["sh", "-c", r#"echo "hi""#]));
        // `\s` is a space; like any space it separates words unless quoted.
        assert_eq!(split_exec(r#""/opt/a\sb/run" %f"#), Some(words(&["/opt/a b/run", "%f"])));
        assert_eq!(split_exec(r#""unterminated"#), None);
    }

    #[test]
    fn expands_field_codes() {
        let file = Path::new("/v/a b.solidcraft");
        assert_eq!(expand(&words(&["app", "%F"]), file), words(&["app", "/v/a b.solidcraft"]));
        assert_eq!(expand(&words(&["app", "--icon", "%i", "%c"]), file), words(&["app", "--icon", "/v/a b.solidcraft"]));
        assert_eq!(expand(&words(&["app", "--file=%f", "100%%"]), file), words(&["app", "--file=/v/a b.solidcraft", "100%"]));
    }

    #[cfg(not(windows))]
    #[test]
    fn finds_programs_for_production_files() {
        let dir = tempfile::tempdir().unwrap();
        let (user, system) = (dir.path().join("user/applications"), dir.path().join("system/applications"));
        std::fs::create_dir_all(&user).unwrap();
        std::fs::create_dir_all(&system).unwrap();
        let slicer = dir.path().join("Apps/PrusaSlicer.AppImage");
        std::fs::create_dir_all(slicer.parent().unwrap()).unwrap();
        std::fs::write(&slicer, "").unwrap();
        let entry = |name: &str, exec: &str, mime: &str| format!("[Desktop Entry]\nType=Application\nName={name}\nExec={exec}\nMimeType={mime}\n");
        std::fs::write(user.join("prusa.desktop"), entry("PrusaSlicer", &format!("\"{}\" %F", slicer.display()), "model/stl;application/vnd.ms-3mfdocument;")).unwrap();
        // The same id in the system folder is the user's entry's, overridden.
        std::fs::write(system.join("prusa.desktop"), entry("Old PrusaSlicer", &format!("\"{}\" %F", slicer.display()), "model/stl;")).unwrap();
        std::fs::write(system.join("gone.desktop"), entry("Gone Slicer", "/gone/slicer %f", "model/stl;")).unwrap();
        std::fs::write(system.join("viewer.desktop"), entry("Image viewer", &format!("\"{}\" %f", slicer.display()), "image/png;")).unwrap();

        let dirs = [user, system];
        let stl = apps_in(&dirs, None, "stl");
        assert_eq!(stl.iter().map(|a| a.name.as_str()).collect::<Vec<_>>(), ["PrusaSlicer"]);
        assert_eq!(custom(&stl[0].command).unwrap().command, words(&[&slicer.to_string_lossy(), "%F"]));
        assert_eq!(apps_in(&dirs, None, "3mf").len(), 1);
        assert!(apps_in(&dirs, None, "step").is_empty());

        let chosen = FileApp::from_program(Path::new("/opt/My Slicer/orca-slicer"));
        assert_eq!(chosen.name, "orca-slicer");
        assert_eq!(expand(&custom(&chosen.command).unwrap().command, Path::new("/tmp/a b.stl")), words(&["/opt/My Slicer/orca-slicer", "/tmp/a b.stl"]));
    }

    #[cfg(not(windows))]
    #[test]
    fn finds_the_appimage_entry() {
        let dir = tempfile::tempdir().unwrap();
        let user = dir.path().join("user/applications");
        let system = dir.path().join("system/applications");
        std::fs::create_dir_all(&user).unwrap();
        std::fs::create_dir_all(&system).unwrap();
        let appimage = dir.path().join("Apps/SolidCraft x86_64.AppImage");
        std::fs::create_dir_all(appimage.parent().unwrap()).unwrap();
        std::fs::write(&appimage, "").unwrap();

        // An unrelated app that claims ZIP files (what xdg-open picked) and a stale entry.
        std::fs::write(system.join("calibre-ebook.desktop"), "[Desktop Entry]\nType=Application\nName=E-book manager\nExec=ebook %f\nMimeType=application/zip;\n").unwrap();
        std::fs::write(system.join("old-solidcraft.desktop"), "[Desktop Entry]\nType=Application\nName=SolidCraft\nExec=/gone/SolidCraft.AppImage %F\n").unwrap();
        assert_eq!(detect_in(&[user.clone(), system.clone()], None), None);

        std::fs::write(
            user.join("ai.storyteller.solidcraft.desktop"),
            format!(
                "[Desktop Entry]\nType=Application\nName=SolidCraft\nExec=\"{}\" %F\nMimeType=application/x-solidcraft;model/step;\n\n[Desktop Action new]\nExec=other\n",
                appimage.display()
            ),
        )
        .unwrap();
        let found = detect_in(&[user.clone(), system], None).unwrap();
        assert_eq!(found.command, words(&[&appimage.to_string_lossy(), "%F"]));
        assert!(found.source.ends_with("ai.storyteller.solidcraft.desktop"));
    }

    #[cfg(not(windows))]
    #[test]
    fn a_launcher_entry_with_another_name_is_found_by_its_mime_type() {
        let dir = tempfile::tempdir().unwrap();
        let apps = dir.path().join("applications");
        std::fs::create_dir_all(&apps).unwrap();
        let bin = dir.path().join("SolidCraft.AppImage");
        std::fs::write(&bin, "").unwrap();
        std::fs::write(
            apps.join("gearlever_solidcraft_abc123.desktop"),
            format!("[Desktop Entry]\nType=Application\nName=SolidCraft\nExec={} %U\nMimeType=application/x-solidcraft;\n", bin.display()),
        )
        .unwrap();
        // Our own entry must never be picked.
        std::fs::write(apps.join("solidcraft-pdm.desktop"), "[Desktop Entry]\nType=Application\nName=SolidCraft PDM\nExec=/bin/sh\nMimeType=application/x-solidcraft;\n").unwrap();
        let found = detect_in(&[apps], None).unwrap();
        assert_eq!(found.command[0], bin.to_string_lossy());
    }

    #[test]
    fn custom_commands_accept_plain_paths_with_spaces() {
        let dir = tempfile::tempdir().unwrap();
        let bin = dir.path().join("My Apps/SolidCraft.AppImage");
        std::fs::create_dir_all(bin.parent().unwrap()).unwrap();
        std::fs::write(&bin, "").unwrap();
        assert_eq!(custom(&bin.to_string_lossy()).unwrap().command, words(&[&bin.to_string_lossy()]));
        assert_eq!(custom("snap run solidcraft").unwrap().command, words(&["snap", "run", "solidcraft"]));
        assert_eq!(custom("  "), None);
    }

    #[cfg(not(windows))]
    #[test]
    fn file_uris_are_percent_encoded() {
        assert_eq!(file_uri(Path::new("/home/me/A b/ü.solidcraft")), "file:///home/me/A%20b/%C3%BC.solidcraft");
    }
}
