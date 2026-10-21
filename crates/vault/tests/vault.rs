use std::fs;
use std::path::Path;

use vault::{Error, Status, Vault};

fn design(part_number: &str, features: usize) -> String {
    let features = vec![r#"{"id": 1}"#; features].join(",");
    format!(
        r#"{{"format": "solidcraft/28", "name": "case", "units": "mm",
            "metadata": {{"part_number": "{part_number}", "revision": "A"}},
            "features": [{features}]}}"#
    )
}

fn setup() -> (tempfile::TempDir, Vault) {
    let dir = tempfile::tempdir().unwrap();
    fs::create_dir_all(dir.path().join("enclosure")).unwrap();
    fs::write(dir.path().join("enclosure/case.solidcraft"), design("P-1", 1)).unwrap();
    fs::write(dir.path().join("notes.txt"), "not a design").unwrap();
    let vault = Vault::init(dir.path()).unwrap();
    (dir, vault)
}

fn status(vault: &mut Vault, path: &str) -> Option<Status> {
    vault.scan().unwrap().into_iter().find(|e| e.path == path).map(|e| e.status)
}

const CASE: &str = "enclosure/case.solidcraft";

#[test]
fn scan_lists_designs_only() {
    let (_dir, mut vault) = setup();
    let entries = vault.scan().unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].path, CASE);
    assert_eq!(entries[0].status, Status::Untracked);
    assert_eq!(vault::design_folders(&entries), ["enclosure"]);
}

#[test]
fn check_in_tracks_versions_and_properties() {
    let (dir, mut vault) = setup();
    let v1 = vault.check_in(CASE, "first").unwrap();
    assert_eq!(v1.number, 1);
    assert_eq!(v1.part_number.as_deref(), Some("P-1"));
    assert_eq!(v1.revision.as_deref(), Some("A"));
    assert_eq!(status(&mut vault, CASE), Some(Status::Current));

    assert!(matches!(vault.check_in(CASE, "again"), Err(Error::Unchanged(_))));

    fs::write(dir.path().join(CASE), design("P-2", 2)).unwrap();
    assert_eq!(status(&mut vault, CASE), Some(Status::Modified));
    let v2 = vault.check_in(CASE, "  second \n").unwrap();
    assert_eq!((v2.number, v2.comment.as_str()), (2, "second"));

    let history = vault.history(CASE).unwrap();
    assert_eq!(history.iter().map(|v| v.number).collect::<Vec<_>>(), [2, 1]);
}

#[test]
fn restore_records_a_new_version_and_keeps_unsaved_work() {
    let (dir, mut vault) = setup();
    let file = dir.path().join(CASE);
    vault.check_in(CASE, "v1").unwrap();
    fs::write(&file, design("P-2", 2)).unwrap();
    vault.check_in(CASE, "v2").unwrap();
    fs::write(&file, design("P-3", 3)).unwrap(); // not checked in

    let restored = vault.restore(CASE, 1).unwrap();
    assert_eq!(restored.number, 4);
    assert_eq!(restored.comment, "Restored v1");
    assert_eq!(fs::read_to_string(&file).unwrap(), design("P-1", 1));

    let history = vault.history(CASE).unwrap();
    assert_eq!(history[1].comment, "Automatic check-in before restoring v1");
    assert_eq!(vault.version_bytes(&history[1]).unwrap(), design("P-3", 3).as_bytes());
    assert_eq!(status(&mut vault, CASE), Some(Status::Current));
}

#[test]
fn restore_brings_back_a_deleted_file() {
    let (dir, mut vault) = setup();
    vault.check_in(CASE, "v1").unwrap();
    fs::remove_file(dir.path().join(CASE)).unwrap();
    assert_eq!(status(&mut vault, CASE), Some(Status::Missing));
    vault.restore(CASE, 1).unwrap();
    assert_eq!(status(&mut vault, CASE), Some(Status::Current));
}

#[test]
fn rename_keeps_history() {
    let (dir, mut vault) = setup();
    vault.check_in(CASE, "v1").unwrap();
    vault.rename(CASE, "archive/old-case.solidcraft").unwrap();
    assert!(dir.path().join("archive/old-case.solidcraft").is_file());
    assert_eq!(vault.history("archive/old-case.solidcraft").unwrap().len(), 1);
    assert!(matches!(vault.history(CASE), Err(Error::NotTracked(_))));
}

#[test]
fn a_file_moved_outside_the_tool_is_recognised_and_relinked() {
    let (dir, mut vault) = setup();
    vault.check_in(CASE, "v1").unwrap();
    fs::rename(dir.path().join(CASE), dir.path().join("moved.solidcraft")).unwrap();

    let entries = vault.scan().unwrap();
    let moved = entries.iter().find(|e| e.path == "moved.solidcraft").unwrap();
    assert_eq!(moved.status, Status::Untracked);
    assert_eq!(moved.moved_from.as_deref(), Some(CASE));

    vault.relink(CASE, "moved.solidcraft").unwrap();
    assert_eq!(status(&mut vault, "moved.solidcraft"), Some(Status::Current));
    assert_eq!(status(&mut vault, CASE), None);
}

#[test]
fn thumbnails_are_stored_per_version() {
    let (dir, mut vault) = setup();
    let png = b"\x89PNG\r\n\x1a\nfake";
    let mut zip = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    for (name, bytes) in [
        ("container.json", &br#"{"format":"solidcraft-container/1"}"#[..]),
        ("document.json", design("P-9", 1).as_bytes()),
        ("cache/thumbnail.png", &png[..]),
    ] {
        zip.start_file(name, zip::write::SimpleFileOptions::default()).unwrap();
        std::io::Write::write_all(&mut zip, bytes).unwrap();
    }
    fs::write(dir.path().join("boxed.solidcraft"), zip.finish().unwrap().into_inner()).unwrap();

    let v = vault.check_in("boxed.solidcraft", "").unwrap();
    assert_eq!(v.part_number.as_deref(), Some("P-9"));
    assert_eq!(vault.thumbnail(&v).unwrap().as_deref(), Some(&png[..]));
}

#[test]
fn paths_resolve_inside_the_vault_only() {
    let (dir, vault) = setup();
    assert_eq!(vault.relative(dir.path().join("enclosure/../enclosure/case.solidcraft")).unwrap(), CASE);
    assert!(matches!(vault.relative(dir.path().join(".pdm/vault.db")), Err(Error::OutsideVault(_))));
    assert!(matches!(vault.relative(Path::new("/etc/passwd")), Err(Error::OutsideVault(_))));
}

#[test]
fn discover_finds_the_vault_from_a_subfolder() {
    let (dir, _vault) = setup();
    let found = Vault::discover(dir.path().join("enclosure")).unwrap();
    assert_eq!(found.root(), dir.path().canonicalize().unwrap());
    assert!(matches!(Vault::init(dir.path()), Err(Error::AlreadyAVault(_))));
}

#[test]
fn stored_objects_are_verified() {
    let (dir, mut vault) = setup();
    let v = vault.check_in(CASE, "").unwrap();
    let object = dir.path().join(".pdm/objects").join(&v.hash[..2]).join(&v.hash[2..]);
    let mut perms = fs::metadata(&object).unwrap().permissions();
    assert!(perms.readonly());
    #[allow(clippy::permissions_set_readonly_false)]
    perms.set_readonly(false);
    fs::set_permissions(&object, perms).unwrap();
    fs::write(&object, "tampered").unwrap();
    assert!(matches!(vault.version_bytes(&v), Err(Error::Corrupt(_))));
}

#[test]
fn folders_can_be_created_listed_and_removed() {
    let (dir, mut vault) = setup();
    vault.create_folder("archive/2026").unwrap();
    assert!(dir.path().join("archive/2026").is_dir());
    assert_eq!(vault.folders().unwrap(), ["archive", "archive/2026", "enclosure"]);
    assert!(matches!(vault.create_folder("archive"), Err(Error::Exists(_))));

    let moved = vault.move_to_folder(CASE, "archive/2026").unwrap();
    assert_eq!(moved, "archive/2026/case.solidcraft");
    assert!(matches!(vault.remove_folder("archive/2026"), Err(Error::FolderNotEmpty(_))));
    assert_eq!(vault.move_to_folder(&moved, "").unwrap(), "case.solidcraft");
    vault.remove_folder("archive/2026").unwrap();
    assert_eq!(vault.folders().unwrap(), ["archive", "enclosure"]);
}

#[test]
fn paths_cannot_leave_the_vault_or_hide() {
    let (_dir, mut vault) = setup();
    for bad in ["../outside.solidcraft", "a/../../x.solidcraft", ".pdm/x.solidcraft", "/abs.solidcraft", "a//b.solidcraft", ""] {
        assert!(matches!(vault.rename(CASE, bad), Err(Error::InvalidPath(_))), "{bad}");
    }
    for bad in ["..", ".hidden", "a/./b"] {
        assert!(matches!(vault.create_folder(bad), Err(Error::InvalidPath(_))), "{bad}");
    }
    // Nothing was moved.
    assert_eq!(vault.scan().unwrap()[0].path, CASE);
}

#[test]
fn new_designs_are_added_as_v1_and_never_overwrite() {
    let (dir, mut vault) = setup();
    let v = vault.add_new("drafts/bracket.solidcraft", &design("BR-1", 0).into_bytes(), "New design").unwrap();
    assert_eq!((v.number, v.part_number.as_deref()), (1, Some("BR-1")));
    assert!(dir.path().join("drafts/bracket.solidcraft").is_file());
    assert!(matches!(vault.add_new(CASE, b"{}", ""), Err(Error::Exists(_))));
    assert_eq!(fs::read_to_string(dir.path().join(CASE)).unwrap(), design("P-1", 1), "untouched");
    assert!(matches!(vault.add_new("../x.solidcraft", b"{}", ""), Err(Error::InvalidPath(_))));
}

#[test]
fn reference_images_are_listed_but_not_versioned() {
    let (dir, mut vault) = setup();
    fs::write(dir.path().join("enclosure/case-ref.JPG"), b"jpeg").unwrap();
    fs::write(dir.path().join("sketch.webp"), b"webp").unwrap();
    fs::write(dir.path().join("enclosure/notes.pdf"), b"pdf").unwrap();
    let images: Vec<String> = vault.images().unwrap().into_iter().map(|i| i.path).collect();
    assert_eq!(images, ["enclosure/case-ref.JPG", "sketch.webp"]);
    assert_eq!(vault.scan().unwrap().len(), 1, "scan lists designs only");
    // They move like designs.
    assert_eq!(vault.move_to_folder("sketch.webp", "enclosure").unwrap(), "enclosure/sketch.webp");
    assert!(dir.path().join("enclosure/sketch.webp").is_file());
}
