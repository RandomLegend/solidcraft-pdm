//! Opening designs in SolidCraft and showing files in the file manager.
//!
//! `xdg-open` is not used for designs: SolidCraft's AppImage installs a desktop entry
//! (`ai.storyteller.solidcraft.desktop`, `Exec="/path/to/SolidCraft.AppImage" %F`) but no MIME
//! type for `*.solidcraft`, so `xdg-open` guesses from the contents (a ZIP, or JSON) and opens
//! whatever handles those. Instead SolidCraft's desktop entry is found the way a launcher finds
//! it, and its `Exec` line is run.

use std::path::{Path, PathBuf};
use std::process::Command;

const DESKTOP_IDS: [&str; 2] = ["ai.storyteller.solidcraft.desktop", "ai.storyteller.solvecraft.desktop"];
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
        Command::new(program).args(args).spawn().map(|_| ())
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
pub fn detect() -> Option<Launcher> {
    detect_in(&application_dirs(), std::env::var_os("PATH").as_deref())
}

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

/// Opens the file manager with `file` selected (FileManager1 over D-Bus: Dolphin, Nautilus,
/// Nemo…), else opens its folder.
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
    Command::new("xdg-open").arg(dir).spawn().map(|_| ())
}

/// 3 for SolidCraft's own desktop ids, else 0.
fn file_rank(file: &Path) -> u8 {
    let id = file.file_name().and_then(|n| n.to_str()).unwrap_or("");
    if DESKTOP_IDS.contains(&id) { 3 } else { 0 }
}

struct DesktopEntry {
    name: Option<String>,
    exec: Option<String>,
    mime_types: Vec<String>,
    hidden: bool,
    is_application: bool,
}

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

    #[test]
    fn file_uris_are_percent_encoded() {
        assert_eq!(file_uri(Path::new("/home/me/A b/ü.solidcraft")), "file:///home/me/A%20b/%C3%BC.solidcraft");
    }
}
