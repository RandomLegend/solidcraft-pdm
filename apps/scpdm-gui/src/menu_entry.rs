//! Putting SolidCraft PDM into the desktop's application menu (Linux, freedesktop desktops).
//!
//! An AppImage installs nothing, so it is started from its folder and Wayland docks show no
//! icon for it (they look the icon up by the window's app id, `solidcraft-pdm`, in a desktop
//! entry of that name). This writes `$XDG_DATA_HOME/applications/solidcraft-pdm.desktop`,
//! pointing at the AppImage (or the running program), and the icons next to it.

use std::fs;
use std::path::{Path, PathBuf};

const DESKTOP_FILE: &str = "solidcraft-pdm.desktop";
const ICONS: [(&str, &[u8]); 4] = [
    ("256x256/apps/solidcraft-pdm.png", include_bytes!("../../../assets/solidcraft-pdm-256.png")),
    ("128x128/apps/solidcraft-pdm.png", include_bytes!("../../../assets/solidcraft-pdm-128.png")),
    ("48x48/apps/solidcraft-pdm.png", include_bytes!("../../../assets/solidcraft-pdm-48.png")),
    ("scalable/apps/solidcraft-pdm.svg", include_bytes!("../../../assets/solidcraft-pdm.svg")),
];

/// The program a menu entry should start: the AppImage file when running from one, else this
/// executable.
pub fn program() -> Option<PathBuf> {
    std::env::var_os("APPIMAGE").map(PathBuf::from).filter(|p| p.is_file()).or_else(|| std::env::current_exe().ok())
}

/// `$XDG_DATA_HOME`, by default `~/.local/share`.
pub fn data_home() -> Option<PathBuf> {
    std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share")))
}

/// The program the installed menu entry starts, if there is one.
pub fn installed(data_home: &Path) -> Option<PathBuf> {
    let text = fs::read_to_string(data_home.join("applications").join(DESKTOP_FILE)).ok()?;
    let exec = text.lines().find_map(|l| l.strip_prefix("Exec="))?;
    crate::launcher::split_exec(exec)?.into_iter().next().map(PathBuf::from)
}

/// Writes the menu entry for `program`, and the icons.
pub fn install(data_home: &Path, program: &Path) -> Result<(), String> {
    let path = program.to_str().ok_or("the program's path is not valid UTF-8")?;
    // Exec= would need escaping (or could gain lines) for these; SolidCraft's AppImage refuses
    // them for the same reason.
    if path.chars().any(|c| matches!(c, '"' | '`' | '$' | '\\' | '%' | '\n' | '\r')) {
        return Err(format!("{path} contains characters a menu entry cannot hold; move or rename it"));
    }
    let entry = format!(
        "[Desktop Entry]\n\
         Type=Application\n\
         Name=SolidCraft PDM\n\
         GenericName=CAD data management\n\
         Comment=Manage SolidCraft designs: folders, versions, thumbnails, search\n\
         Exec=\"{path}\" %f\n\
         Icon=solidcraft-pdm\n\
         Terminal=false\n\
         StartupWMClass=solidcraft-pdm\n\
         Categories=Graphics;Engineering;\n\
         Keywords=solidcraft;cad;pdm;versions;\n"
    );
    let io = |e: std::io::Error| e.to_string();
    let apps = data_home.join("applications");
    fs::create_dir_all(&apps).map_err(io)?;
    for (rel, bytes) in ICONS {
        let file = data_home.join("icons/hicolor").join(rel);
        fs::create_dir_all(file.parent().expect("icon paths have folders")).map_err(io)?;
        fs::write(&file, bytes).map_err(io)?;
    }
    // Icons first: a launcher that sees the entry finds them.
    let tmp = apps.join(format!(".{DESKTOP_FILE}.tmp"));
    fs::write(&tmp, entry).map_err(io)?;
    fs::rename(&tmp, apps.join(DESKTOP_FILE)).map_err(io)?;
    refresh(&apps);
    Ok(())
}

/// Removes the menu entry and the icons.
pub fn remove(data_home: &Path) -> Result<(), String> {
    let apps = data_home.join("applications");
    match fs::remove_file(apps.join(DESKTOP_FILE)) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(e.to_string()),
    }
    for (rel, _) in ICONS {
        let _ = fs::remove_file(data_home.join("icons/hicolor").join(rel));
    }
    refresh(&apps);
    Ok(())
}

/// Lets desktops that keep a cache of desktop entries know (most watch the folder anyway).
fn refresh(apps: &Path) {
    let _ = std::process::Command::new("update-desktop-database")
        .arg(apps)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn installs_and_removes_the_menu_entry() {
        let dir = tempfile::tempdir().unwrap();
        let data = dir.path().join("share");
        let program = dir.path().join("My Apps/SolidCraft-PDM-0.1.0-x86_64.AppImage");
        assert_eq!(installed(&data), None);

        install(&data, &program).unwrap();
        let entry = fs::read_to_string(data.join("applications/solidcraft-pdm.desktop")).unwrap();
        assert!(entry.contains(&format!("Exec=\"{}\" %f\n", program.display())));
        assert!(entry.contains("StartupWMClass=solidcraft-pdm\n"));
        assert_eq!(installed(&data), Some(program.clone()), "read back, spaces and all");
        assert!(data.join("icons/hicolor/256x256/apps/solidcraft-pdm.png").is_file());
        assert!(data.join("icons/hicolor/scalable/apps/solidcraft-pdm.svg").is_file());

        remove(&data).unwrap();
        assert_eq!(installed(&data), None);
        assert!(!data.join("icons/hicolor/256x256/apps/solidcraft-pdm.png").exists());
        remove(&data).unwrap(); // twice is fine
    }

    #[test]
    fn refuses_paths_a_desktop_entry_cannot_hold() {
        let dir = tempfile::tempdir().unwrap();
        for bad in ["/tmp/a\"b/app", "/tmp/$HOME/app", "/tmp/100%/app"] {
            assert!(install(dir.path(), Path::new(bad)).is_err(), "{bad}");
        }
        assert!(!dir.path().join("applications/solidcraft-pdm.desktop").exists());
    }
}
