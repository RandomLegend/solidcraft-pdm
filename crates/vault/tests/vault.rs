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
fn designs_are_sc_solidcraft_or_solvecraft_files() {
    let (dir, mut vault) = setup();
    fs::write(dir.path().join("enclosure/lid.sc"), design("P-2", 1)).unwrap();
    fs::write(dir.path().join("enclosure/old.SolveCraft"), design("P-3", 1)).unwrap();
    fs::write(dir.path().join("enclosure/script.scala"), "not a design").unwrap();
    let paths: Vec<String> = vault.scan().unwrap().into_iter().map(|e| e.path).collect();
    assert_eq!(paths, ["enclosure/case.solidcraft", "enclosure/lid.sc", "enclosure/old.SolveCraft"]);
    assert_eq!(vault.check_in("enclosure/lid.sc", "v1").unwrap().number, 1);
    assert_eq!(vault::design_stem("lid.sc"), "lid");
    assert_eq!(vault::design_stem("case.solidcraft"), "case");
    assert_eq!(vault::design_stem("photo.png"), "photo.png");
    assert_eq!(vault::design_stem(".sc"), ".sc");
    assert_eq!(vault::new_design_name("bracket"), "bracket.sc");
    assert_eq!(vault::new_design_name("bracket.solidcraft"), "bracket.solidcraft");
    assert_eq!(vault::new_design_name("v1.2"), "v1.2.sc");
}

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

    let moved = vault.move_to_folder(CASE, "archive/2026").unwrap().to;
    assert_eq!(moved, "archive/2026/case.solidcraft");
    assert!(matches!(vault.remove_folder("archive/2026"), Err(Error::FolderNotEmpty(_))));
    assert_eq!(vault.move_to_folder(&moved, "").unwrap().to, "case.solidcraft");
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
    assert_eq!(vault.move_to_folder("sketch.webp", "enclosure").unwrap().to, "enclosure/sketch.webp");
    assert!(dir.path().join("enclosure/sketch.webp").is_file());
}

/// A design linking components 2 and 5 to other design files.
fn assembly(links: &[(u64, &str)]) -> String {
    let links: Vec<String> = links.iter().map(|(c, p)| format!(r#""{c}": {{"path": "{p}", "feature": 1}}"#)).collect();
    format!(
        r#"{{"format": "solidcraft/29", "name": "asm", "units": "mm", "features": [{{"id": 1, "w": 0.1}}],
            "components": [{{"id": 2, "name": "Lid", "parent": 0}}, {{"id": 5, "name": "Base", "parent": 0}}],
            "links": {{{}}}}}"#,
        links.join(", ")
    )
}

fn links_of(dir: &Path, path: &str) -> Vec<(u64, String)> {
    scformat::read_file(dir.join(path)).unwrap().links.into_iter().map(|l| (l.component, l.path)).collect()
}

fn linked_vault() -> (tempfile::TempDir, Vault) {
    let dir = tempfile::tempdir().unwrap();
    for d in ["asm", "parts"] {
        fs::create_dir_all(dir.path().join(d)).unwrap();
    }
    fs::write(dir.path().join("parts/lid.solidcraft"), design("LID", 1)).unwrap();
    fs::write(dir.path().join("parts/base.solidcraft"), design("BASE", 1)).unwrap();
    fs::write(dir.path().join("asm/box.solidcraft"), assembly(&[(2, "../parts/lid.solidcraft"), (5, "../parts/base.solidcraft")])).unwrap();
    let mut vault = Vault::init(dir.path()).unwrap();
    for e in vault.scan().unwrap() {
        vault.check_in(&e.path, "v1").unwrap();
    }
    (dir, vault)
}

#[test]
fn moving_a_part_updates_the_assemblies_that_link_to_it() {
    let (dir, mut vault) = linked_vault();
    let report = vault.rename("parts/lid.solidcraft", "archive/2026/lid-old.solidcraft").unwrap();
    assert_eq!(report.updated, [vault::LinkUpdate { path: "asm/box.solidcraft".into(), version: Some(2) }]);
    assert!(report.failed.is_empty());
    assert_eq!(links_of(dir.path(), "asm/box.solidcraft"), [(2, "../archive/2026/lid-old.solidcraft".into()), (5, "../parts/base.solidcraft".into())]);
    let history = vault.history("asm/box.solidcraft").unwrap();
    assert_eq!(history[0].comment, "Link to archive/2026/lid-old.solidcraft updated (moved from parts/lid.solidcraft)");
    // The part itself has no links: moved, not changed.
    assert_eq!(vault.history("archive/2026/lid-old.solidcraft").unwrap().len(), 1);
    // Everything else in the assembly is as it was.
    let doc: serde_json::Value = serde_json::from_slice(&fs::read(dir.path().join("asm/box.solidcraft")).unwrap()).unwrap();
    assert_eq!(doc["features"][0]["w"].as_f64(), Some(0.1));
    assert_eq!(doc["components"][1]["name"], "Base");
}

#[test]
fn a_moved_assembly_keeps_its_own_links_working() {
    let (dir, mut vault) = linked_vault();
    let report = vault.move_to_folder("asm/box.solidcraft", "").unwrap();
    assert_eq!(report.to, "box.solidcraft");
    assert_eq!(report.updated.len(), 1);
    assert_eq!(links_of(dir.path(), "box.solidcraft"), [(2, "parts/lid.solidcraft".into()), (5, "parts/base.solidcraft".into())]);
    assert_eq!(vault.history("box.solidcraft").unwrap()[0].comment, "Links updated after moving from asm/box.solidcraft");
    let statuses: Vec<_> = vault.scan().unwrap().into_iter().map(|e| e.status).collect();
    assert!(statuses.iter().all(|s| *s == Status::Current), "{statuses:?}");
}

#[test]
fn unsaved_changes_are_kept_and_not_checked_in() {
    let (dir, mut vault) = linked_vault();
    let asm = dir.path().join("asm/box.solidcraft");
    let edited = assembly(&[(2, "../parts/lid.solidcraft"), (5, "../parts/base.solidcraft")]).replace("\"asm\"", "\"asm-edited\"");
    fs::write(&asm, edited).unwrap();
    let report = vault.rename("parts/base.solidcraft", "parts/base-v2.solidcraft").unwrap();
    assert_eq!(report.updated, [vault::LinkUpdate { path: "asm/box.solidcraft".into(), version: None }]);
    assert_eq!(scformat::read_file(&asm).unwrap().name.as_deref(), Some("asm-edited"), "the user's change survives");
    assert_eq!(links_of(dir.path(), "asm/box.solidcraft")[1].1, "../parts/base-v2.solidcraft");
    assert_eq!(vault.history("asm/box.solidcraft").unwrap().len(), 1, "no version for the user's unsaved work");
}

#[test]
fn links_in_containers_are_updated_too() {
    let (dir, mut vault) = linked_vault();
    let mut zip = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    let blob: Vec<u8> = (0..=255u8).cycle().take(5000).collect();
    for (name, bytes) in [
        ("container.json", &br#"{"format":"solidcraft-container/1"}"#[..]),
        ("document.json", assembly(&[(2, "../parts/lid.solidcraft")]).as_bytes()),
        ("blobs/aa.mesh", &blob[..]),
    ] {
        zip.start_file(name, zip::write::SimpleFileOptions::default()).unwrap();
        std::io::Write::write_all(&mut zip, bytes).unwrap();
    }
    fs::write(dir.path().join("asm/packed.solidcraft"), zip.finish().unwrap().into_inner()).unwrap();
    vault.check_in("asm/packed.solidcraft", "v1").unwrap();

    vault.rename("parts/lid.solidcraft", "lid.solidcraft").unwrap();
    assert_eq!(links_of(dir.path(), "asm/packed.solidcraft"), [(2, "../lid.solidcraft".into())]);
    let mut packed = zip::ZipArchive::new(fs::File::open(dir.path().join("asm/packed.solidcraft")).unwrap()).unwrap();
    let mut kept = Vec::new();
    std::io::Read::read_to_end(&mut packed.by_name("blobs/aa.mesh").unwrap(), &mut kept).unwrap();
    assert_eq!(kept, blob);
}

#[test]
fn renaming_in_place_updates_links_and_images_just_move() {
    let (dir, mut vault) = linked_vault();
    vault.rename("parts/lid.solidcraft", "parts/cover.solidcraft").unwrap();
    assert_eq!(links_of(dir.path(), "asm/box.solidcraft")[0].1, "../parts/cover.solidcraft");
    fs::write(dir.path().join("parts/ref.png"), b"png").unwrap();
    let report = vault.rename("parts/ref.png", "asm/ref.png").unwrap();
    assert!(report.updated.is_empty());
}

#[test]
fn releasing_assigns_revisions_and_records_them() {
    use vault::State;
    let (dir, mut vault) = setup();
    vault.check_in(CASE, "v1").unwrap();
    assert_eq!(vault.lifecycle(CASE).unwrap().state, State::Wip);
    assert_eq!(vault.next_revision(CASE).unwrap(), "A");

    // The design says revision A already: v1 is revision A, no new version.
    let a = vault.release(CASE, "first batch").unwrap();
    assert_eq!((a.revision.as_str(), a.version, a.note.as_str()), ("A", 1, "first batch"));
    assert_eq!(scformat::read_file(dir.path().join(CASE)).unwrap().metadata.revision.as_deref(), Some("A"));
    assert_eq!(vault.history(CASE).unwrap().len(), 1);
    let entry = vault.scan().unwrap().into_iter().find(|e| e.path == CASE).unwrap();
    assert_eq!((entry.state, entry.release.map(|r| r.revision)), (Some(State::Released), Some("A".into())));
    assert!(matches!(vault.release(CASE, ""), Err(Error::AlreadyReleased(_, r)) if r == "A"));

    // Changed: must be checked in first; the check-in starts work on B.
    fs::write(dir.path().join(CASE), design("P-1", 2)).unwrap();
    assert!(matches!(vault.release(CASE, ""), Err(Error::NotCheckedIn(_))));
    vault.check_in(CASE, "more holes").unwrap();
    assert_eq!(vault.lifecycle(CASE).unwrap().state, State::Wip);
    let b = vault.release(CASE, "").unwrap();
    assert_eq!((b.revision.as_str(), b.version), ("B", 3));
    assert_eq!(vault.history(CASE).unwrap()[0].comment, "Released revision B");
    assert_eq!(scformat::read_file(dir.path().join(CASE)).unwrap().metadata.revision.as_deref(), Some("B"));
    let releases: Vec<String> = vault.lifecycle(CASE).unwrap().releases.into_iter().map(|r| r.revision).collect();
    assert_eq!(releases, ["B", "A"]);

    vault.revise(CASE).unwrap();
    assert_eq!(vault.lifecycle(CASE).unwrap().state, State::Wip);
    assert!(matches!(vault.revise(CASE), Err(Error::NotReleased(_))));
}

#[test]
fn obsolete_designs_cannot_be_released_until_revived() {
    use vault::State;
    let (_dir, mut vault) = setup();
    vault.check_in(CASE, "v1").unwrap();
    vault.set_obsolete(CASE, true).unwrap();
    assert_eq!(vault.lifecycle(CASE).unwrap().state, State::Obsolete);
    assert!(matches!(vault.release(CASE, ""), Err(Error::Obsolete(_))));
    vault.set_obsolete(CASE, false).unwrap();
    assert_eq!(vault.lifecycle(CASE).unwrap().state, State::Wip);
}

#[test]
fn the_revision_scheme_can_be_numbers() {
    let (_dir, mut vault) = setup();
    vault.check_in(CASE, "v1").unwrap();
    vault.set_revision_scheme(vault::RevisionScheme::Numbers).unwrap();
    assert_eq!(vault.release(CASE, "").unwrap().revision, "01");
    assert_eq!(vault::Vault::open(vault.root()).unwrap().revision_scheme().unwrap(), vault::RevisionScheme::Numbers);
}

#[test]
fn a_released_assembly_stays_released_when_only_its_links_are_updated() {
    let (_dir, mut vault) = linked_vault();
    vault.release("asm/box.solidcraft", "").unwrap();
    vault.rename("parts/lid.solidcraft", "parts/cover.solidcraft").unwrap();
    assert_eq!(vault.lifecycle("asm/box.solidcraft").unwrap().state, vault::State::Released);
}

#[test]
fn design_properties_are_written_into_the_file() {
    let (dir, mut vault) = setup();
    vault.check_in(CASE, "v1").unwrap();
    vault
        .edit_properties(CASE, Some("Case, vented"), &[("part_number", Some("P-77")), ("description", Some("PETG")), ("revision", None)])
        .unwrap();
    let info = scformat::read_file(dir.path().join(CASE)).unwrap();
    assert_eq!(info.name.as_deref(), Some("Case, vented"));
    assert_eq!(info.metadata.part_number.as_deref(), Some("P-77"));
    assert_eq!(info.metadata.description.as_deref(), Some("PETG"));
    assert_eq!(info.metadata.revision, None);
    assert_eq!(vault.scan().unwrap()[0].status, Status::Modified, "not checked in by itself");

    // Removing every property leaves no metadata at all.
    vault.edit_properties(CASE, None, &[("part_number", None), ("description", Some("  "))]).unwrap();
    let doc: serde_json::Value = serde_json::from_slice(&fs::read(dir.path().join(CASE)).unwrap()).unwrap();
    assert!(doc.get("metadata").is_none());
    assert!(matches!(vault.edit_properties(CASE, None, &[("description", Some(&"x".repeat(5000)))]), Err(Error::TooLong(_))));
}

#[test]
fn vaults_from_the_first_schema_are_upgraded() {
    let (dir, mut vault) = setup();
    vault.check_in(CASE, "v1").unwrap();
    drop(vault);
    // As the first version of the tool left it: no state, no releases, no settings.
    let db = rusqlite::Connection::open(dir.path().join(".pdm/vault.db")).unwrap();
    db.execute_batch(
        "DROP TABLE release_files; ALTER TABLE documents DROP COLUMN deleted_at; ALTER TABLE documents DROP COLUMN deleted_by; ALTER TABLE documents DROP COLUMN deleted_from; DROP TABLE locks; DROP TABLE releases; DROP TABLE settings; ALTER TABLE documents DROP COLUMN state; PRAGMA user_version = 1;",
    )
    .unwrap();
    drop(db);
    let mut vault = Vault::open(dir.path()).unwrap();
    assert_eq!(vault.history(CASE).unwrap().len(), 1);
    assert_eq!(vault.lifecycle(CASE).unwrap().state, vault::State::Wip);
    assert_eq!(vault.release(CASE, "").unwrap().revision, "A");
}

#[test]
fn a_revision_set_by_hand_is_proposed_and_revisions_are_never_reused() {
    let (dir, mut vault) = setup();
    // The design says revision A already (set in SolidCraft before using the PDM).
    vault.check_in(CASE, "v1").unwrap();
    assert_eq!(vault.suggested_revision(CASE).unwrap(), "A");
    vault.edit_properties(CASE, None, &[("revision", Some("D"))]).unwrap();
    vault.check_in(CASE, "rev D by hand").unwrap();
    assert_eq!(vault.suggested_revision(CASE).unwrap(), "D");
    assert_eq!(vault.release_as(CASE, "D", "").unwrap().revision, "D");
    vault.revise(CASE).unwrap();
    assert_eq!(vault.suggested_revision(CASE).unwrap(), "E", "after a release: the next one");
    assert!(matches!(vault.release_as(CASE, "d", ""), Err(Error::RevisionTaken(_, _))));
    assert!(dir.path().join(CASE).is_file());
}

#[test]
fn check_outs_belong_to_one_user_and_follow_moves() {
    let (_dir, mut vault) = linked_vault();
    let part = "parts/lid.solidcraft";
    let lock = vault.lock(part, "anna").unwrap();
    assert_eq!(lock.user, "anna");
    assert_eq!(vault.lock(part, "anna").unwrap(), lock, "again by the same user: fine");
    assert!(matches!(vault.lock(part, "ben"), Err(Error::Locked(_, by)) if by == "anna"));
    assert!(matches!(vault.unlock(part, "ben", false), Err(Error::Locked(_, _))));

    vault.rename(part, "parts/cover.solidcraft").unwrap();
    assert_eq!(vault.lock_of("parts/cover.solidcraft").unwrap().map(|l| l.user), Some("anna".into()));
    assert_eq!(vault.locks().unwrap().len(), 1);

    vault.unlock("parts/cover.solidcraft", "ben", true).unwrap(); // an administrator
    assert_eq!(vault.lock_of("parts/cover.solidcraft").unwrap(), None);
    assert!(matches!(vault.lock("parts/nothing.solidcraft", "anna"), Err(Error::NotTracked(_))));
}

#[test]
fn a_server_vault_checks_in_bytes_and_serves_objects() {
    let (dir, mut vault) = setup();
    let v = vault.check_in_bytes("new/part.solidcraft", design("NEW", 1).as_bytes(), "from a workspace").unwrap();
    assert_eq!(v.number, 1);
    assert_eq!(fs::read_to_string(dir.path().join("new/part.solidcraft")).unwrap(), design("NEW", 1));
    assert_eq!(vault.object(&v.hash).unwrap(), design("NEW", 1).as_bytes());

    let hash = vault.put_object(b"uploaded").unwrap();
    assert!(vault.has_object(&hash));
    assert!(!vault.has_object("../../etc/passwd"));
    assert!(matches!(vault.object("../../etc/passwd"), Err(Error::Corrupt(_))));
    assert!(vault::is_hash(&hash) && !vault::is_hash(&hash.to_uppercase()));

    let ids = vault.document_ids().unwrap();
    vault.rename("new/part.solidcraft", "part.solidcraft").unwrap();
    assert_eq!(vault.document_ids().unwrap()["part.solidcraft"], ids["new/part.solidcraft"], "ids survive moves");
}

#[test]
fn a_move_tells_which_designs_it_would_change() {
    let (_dir, vault) = linked_vault();
    assert_eq!(vault.move_affects("parts/lid.solidcraft", "lid.solidcraft").unwrap(), ["asm/box.solidcraft"]);
    assert!(vault.move_affects("asm/box.solidcraft", "box.solidcraft").unwrap().is_empty(), "only its own links");
}

#[test]
fn vaults_from_the_second_schema_get_locks() {
    let (dir, mut vault) = setup();
    vault.check_in(CASE, "v1").unwrap();
    drop(vault);
    let db = rusqlite::Connection::open(dir.path().join(".pdm/vault.db")).unwrap();
    db.execute_batch("DROP TABLE release_files; ALTER TABLE documents DROP COLUMN deleted_at; ALTER TABLE documents DROP COLUMN deleted_by; ALTER TABLE documents DROP COLUMN deleted_from; DROP TABLE locks; PRAGMA user_version = 2;").unwrap();
    drop(db);
    let mut vault = Vault::open(dir.path()).unwrap();
    assert_eq!(vault.lock(CASE, "anna").unwrap().user, "anna");
}

#[test]
fn versions_can_be_by_another_author() {
    let (_dir, mut vault) = setup();
    vault.set_author(Some("anna".into()));
    assert_eq!(vault.check_in(CASE, "").unwrap().author, "anna");
}

#[test]
fn deleted_designs_are_kept_until_purged_and_can_come_back() {
    let (dir, mut vault) = setup();
    let path = "parts/lid.solidcraft";
    fs::create_dir_all(dir.path().join("parts")).unwrap();
    fs::write(dir.path().join(path), design("LID-1", 1)).unwrap();
    vault.check_in(path, "first").unwrap();
    // Changed and not checked in: kept as a version before it goes.
    fs::write(dir.path().join(path), design("LID-2", 2)).unwrap();
    vault.delete(path, Some("anna")).unwrap();
    assert!(!dir.path().join(path).exists());
    assert_eq!(status(&mut vault, path), None, "gone from the vault's list");
    assert!(vault.history(path).is_err());
    let deleted = vault.deleted().unwrap();
    assert_eq!(deleted.len(), 1);
    assert_eq!((deleted[0].path.as_str(), deleted[0].deleted_by.as_str()), (path, "anna"));
    assert_eq!(deleted[0].latest.as_ref().unwrap().comment, "Before deleting");

    // The name is free for a new design meanwhile.
    fs::write(dir.path().join(path), design("NEW", 1)).unwrap();
    vault.check_in(path, "another lid").unwrap();
    let restored = vault.restore_deleted(deleted[0].id).unwrap();
    assert_eq!(restored, "parts/lid (restored).solidcraft");
    assert_eq!(fs::read_to_string(dir.path().join(&restored)).unwrap(), design("LID-2", 2), "as it was deleted");
    assert_eq!(vault.history(&restored).unwrap().len(), 2, "with its history");
    assert_eq!(status(&mut vault, &restored), Some(Status::Current));
    assert!(vault.deleted().unwrap().is_empty());

    // Purged: gone for good.
    vault.delete(&restored, None).unwrap();
    let an_hour_ago = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs() as i64 - 3600;
    assert_eq!(vault.purge_deleted(an_hour_ago).unwrap(), 0, "not old enough");
    assert_eq!(vault.purge_deleted(i64::MAX).unwrap(), 1);
    assert!(vault.deleted().unwrap().is_empty());
    assert_eq!(status(&mut vault, path), Some(Status::Current), "the other lid is untouched");
}

#[test]
fn a_new_design_is_checked_in_before_it_is_deleted() {
    let (dir, mut vault) = setup();
    fs::write(dir.path().join("sketch.solidcraft"), design("S", 1)).unwrap();
    vault.delete("sketch.solidcraft", None).unwrap();
    let deleted = vault.deleted().unwrap();
    assert_eq!(deleted[0].latest.as_ref().unwrap().number, 1);
    assert_eq!(vault.restore_deleted(deleted[0].id).unwrap(), "sketch.solidcraft");
    assert!(matches!(vault.delete("nothing.solidcraft", None), Err(Error::NotTracked(_))));
}

#[test]
fn releases_keep_their_production_files() {
    let (_dir, mut vault) = setup();
    vault.check_in(CASE, "v1").unwrap();
    assert!(matches!(vault.add_release_file(CASE, 1, "case-A.step", b"ISO-10303-21;"), Err(Error::NotReleased(_))));
    let release = vault.release(CASE, "").unwrap();
    let step = vault.add_release_file(CASE, release.version, "case-A.step", b"ISO-10303-21;").unwrap();
    vault.add_release_file(CASE, release.version, "case-A.stl", b"solid case").unwrap();
    assert!(matches!(vault.add_release_file(CASE, release.version, "../case.step", b"x"), Err(Error::InvalidPath(_))));
    // A file of the same name is replaced.
    vault.add_release_file(CASE, release.version, "case-A.stl", b"solid case, again").unwrap();

    let files = &vault.lifecycle(CASE).unwrap().releases[0].files;
    assert_eq!(files.iter().map(|f| f.name.as_str()).collect::<Vec<_>>(), ["case-A.step", "case-A.stl"]);
    assert_eq!(files[1].size, 17);
    assert_eq!(vault.release_file_bytes(&step).unwrap(), b"ISO-10303-21;");
    let entry = vault.scan().unwrap().into_iter().find(|e| e.path == CASE).unwrap();
    assert_eq!(entry.release.unwrap().files.len(), 2, "the scan has them too");

    // Gone with the design when it is purged.
    vault.delete(CASE, None).unwrap();
    let id = vault.deleted().unwrap()[0].id;
    vault.purge_one(id).unwrap();
    assert!(vault.has_object(&step.hash), "younger than a day: kept for now");
}
