//! Files the PDM writes, opened and re-saved by SolidCraft itself.
//!
//! Runs only when `SOLIDCRAFT_CLI` names SolidCraft's command line (`solidcraft-cli`, e.g. from
//! an extracted AppImage: `SolidCraft.AppImage --appimage-extract`, then
//! `squashfs-root/usr/bin/solidcraft-cli`); otherwise it passes without doing anything.

use std::path::Path;
use std::process::Command;

fn fixture(name: &str) -> Vec<u8> {
    std::fs::read(Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures").join(name)).unwrap()
}

/// SolidCraft opens `file` and saves it again; returns what it saved.
fn resave(cli: &str, file: &Path) -> scformat::DesignInfo {
    let out = file.with_extension("resaved.solidcraft");
    let status = Command::new(cli).arg("bench-open").arg(file).arg("--save").arg(&out).output().unwrap();
    assert!(status.status.success(), "SolidCraft refused {}: {}", file.display(), String::from_utf8_lossy(&status.stderr));
    scformat::read_file(&out).unwrap()
}

#[test]
fn solidcraft_opens_what_the_pdm_writes() {
    let Ok(cli) = std::env::var("SOLIDCRAFT_CLI") else {
        eprintln!("SOLIDCRAFT_CLI not set: skipped");
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let mark = |doc: &mut serde_json::Value| {
        doc["metadata"] = serde_json::json!({"part_number": "PDM-EDIT-1", "description": "edited by the PDM"});
    };

    // A new design.
    let new = dir.path().join("new.solidcraft");
    std::fs::write(&new, scformat::new_design("fresh", &[("part_number", "NEW-1")])).unwrap();
    assert_eq!(resave(&cli, &new).metadata.part_number.as_deref(), Some("NEW-1"));

    // An edited container and an edited plain JSON design.
    for (name, fixture_name) in [("container.solidcraft", "container-cubeish.solidcraft"), ("json.solidcraft", "json-pi-case.solidcraft")] {
        let file = dir.path().join(name);
        std::fs::write(&file, scformat::edit_document(&fixture(fixture_name), mark).unwrap()).unwrap();
        let saved = resave(&cli, &file);
        assert_eq!(saved.metadata.part_number.as_deref(), Some("PDM-EDIT-1"), "{name}");
        assert_eq!(saved.metadata.description.as_deref(), Some("edited by the PDM"), "{name}");
    }
}
