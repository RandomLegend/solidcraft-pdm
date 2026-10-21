//! Backups of a server: complete, restorable, small when nothing changed, pruned, careful with
//! the target.

use std::fs;
use std::path::Path;
use std::sync::Arc;

use scpdm_server::Server;
use scpdm_server::backup::Settings;

const LID: &str = "parts/lid.solidcraft";

fn design(n: usize) -> Vec<u8> {
    format!(r#"{{"format": "solidcraft/29", "name": "lid", "features": [{}]}}"#, vec![r#"{"id": 1}"#; n].join(",")).into_bytes()
}

/// A server with a user and a vault holding a design in two versions and a reference image.
fn server(data: &Path, backups: Option<&Path>) -> Arc<Server> {
    let server = Server::open_with_backups(data, "Test", backups.map(Path::to_path_buf)).unwrap();
    server.accounts.add_user("anna", "anna-password", false).unwrap();
    server.add_vault("workshop").unwrap();
    let mut vault = vault::Vault::open(data.join("vaults/workshop")).unwrap();
    vault.check_in_bytes(LID, &design(1), "First").unwrap();
    vault.check_in_bytes(LID, &design(2), "Second").unwrap();
    vault.put_file("parts/sketch.png", b"\x89PNG sketch").unwrap();
    server
}

fn backups_in(target: &Path) -> Vec<String> {
    let mut names: Vec<String> =
        fs::read_dir(target).unwrap().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).filter(|n| n.starts_with("pdm-backup-")).collect();
    names.sort();
    names
}

#[test]
fn a_backup_is_a_data_folder_the_server_runs_from() {
    let (data, target) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let server = server(data.path(), Some(target.path()));
    assert!(server.backup_settings().unwrap().enabled, "on when started with a backup folder");
    let run = server.back_up("test").unwrap();
    assert!(run.ok, "{}", run.message);
    assert!(run.files >= 6 && run.copied == run.bytes, "{run:?}");
    let name = run.name.unwrap();
    assert_eq!(backups_in(target.path()), [name.clone()]);
    assert!(target.path().join(".solidcraft-pdm-backups").is_file());

    assert!(!target.path().join(&name).join("vaults/workshop/.pdm/vault.db-wal").exists(), "a whole database, no journal");

    // Restored: the backup folder as a server's data folder.
    let restored = Server::open(&target.path().join(&name), "Restored").unwrap();
    assert!(restored.accounts.login("anna", "anna-password").unwrap().is_some(), "users came along");
    let vault = vault::Vault::open(target.path().join(&name).join("vaults/workshop")).unwrap();
    let history = vault.history(LID).unwrap();
    assert_eq!(history.len(), 2);
    assert_eq!(vault.version_bytes(&history[1]).unwrap(), design(1), "old versions too");
    assert_eq!(fs::read(target.path().join(&name).join("vaults/workshop").join(LID)).unwrap(), design(2));
    assert!(target.path().join(&name).join("vaults/workshop/parts/sketch.png").is_file(), "reference images");
}

#[test]
fn unchanged_versions_are_linked_not_copied() {
    let (data, target) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let server = server(data.path(), Some(target.path()));
    let first = server.back_up("test").unwrap();
    let second = server.back_up("test").unwrap();
    assert!(second.ok, "{}", second.message);
    assert_eq!(second.bytes, first.bytes);
    assert!(second.copied < first.copied, "{} of {} written again", second.copied, second.bytes);
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let objects = |name: &str| target.path().join(name).join("vaults/workshop/.pdm/objects");
        let object = fs::read_dir(objects(first.name.as_deref().unwrap())).unwrap().flatten().next().unwrap();
        let inner = fs::read_dir(object.path()).unwrap().flatten().next().unwrap();
        let rel = Path::new(&object.file_name()).join(inner.file_name());
        let (a, b) = (fs::metadata(objects(first.name.as_deref().unwrap()).join(&rel)).unwrap(), fs::metadata(objects(second.name.as_deref().unwrap()).join(&rel)).unwrap());
        assert_eq!(a.ino(), b.ino(), "the same file");
    }
}

#[test]
fn old_backups_are_pruned_and_nothing_else_is_touched() {
    let (data, target) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let server = server(data.path(), Some(target.path()));
    fs::create_dir(target.path().join("my-photos")).unwrap();
    fs::create_dir(target.path().join("pdm-backup-but-not-ours")).unwrap();
    server.set_backup_settings(&Settings { enabled: true, keep_count: 2, ..Settings::default() }).unwrap();
    let names: Vec<String> = (0..3).map(|_| server.back_up("test").unwrap().name.unwrap()).collect();
    let mut kept = backups_in(target.path());
    kept.retain(|n| n != "pdm-backup-but-not-ours");
    assert_eq!(kept, names[1..], "the newest two");
    assert!(target.path().join("my-photos").is_dir() && target.path().join("pdm-backup-but-not-ours").is_dir());
    assert_eq!(server.stored_backups().unwrap().len(), 2);
    assert_eq!(server.backup_counts().unwrap(), (3, 3));
}

#[test]
fn a_missing_target_fails_and_is_not_created() {
    let (data, parent) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let share = parent.path().join("nas"); // not mounted
    let server = server(data.path(), Some(&share));
    let run = server.back_up("schedule").unwrap();
    assert!(!run.ok);
    assert!(run.message.contains("does not exist"), "{}", run.message);
    assert!(!share.exists(), "not created");
    assert_eq!(server.backup_counts().unwrap(), (1, 0));
    assert_eq!(server.backup_runs(10).unwrap()[0].trigger, "schedule");
    // The next try comes within the hour.
    let next = server.next_backup().unwrap().unwrap();
    assert!(next <= run.started_at + 3600, "{next}");
}

#[test]
fn the_page_sets_the_target_unless_it_is_fixed() {
    let (data, target) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let server = server(data.path(), None);
    assert!(!server.backup_settings().unwrap().enabled, "off without a backup folder");
    assert_eq!(server.next_backup().unwrap(), None);
    let with = |t: &str| Settings { enabled: true, target: Some(t.into()), ..Settings::default() };
    assert!(server.set_backup_settings(&with("relative/path")).is_err());
    assert!(server.set_backup_settings(&with("/no/such/folder")).is_err());
    let inside = data.path().join("vaults").display().to_string();
    assert!(server.set_backup_settings(&with(&inside)).is_err(), "not inside the data folder");
    assert!(server.set_backup_settings(&Settings { interval_hours: 0, ..with(&target.path().display().to_string()) }).is_err());
    server.set_backup_settings(&with(&target.path().display().to_string())).unwrap();
    assert_eq!(server.backup_target().unwrap().as_deref(), Some(target.path()));
    assert!(server.next_backup().unwrap().is_some());
    assert!(server.back_up("page").unwrap().ok);
}
