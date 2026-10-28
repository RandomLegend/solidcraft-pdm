//! Two people, Anna and Ben, working on one server vault from workspaces of their own.

use std::fs;
use std::path::Path;

use protocol::{Action, ErrorCode};
use remote::{Client, RemoteStatus, Workspace};
use scpdm_server::Server;

struct Team {
    _dirs: Vec<tempfile::TempDir>,
    url: String,
    anna: Workspace,
    ben: Workspace,
}

fn design(part_number: &str, features: usize) -> String {
    let features = vec![r#"{"id": 1, "type": "sketch"}"#; features].join(",");
    format!(r#"{{"format": "solidcraft/29", "name": "part", "units": "mm", "metadata": {{"part_number": "{part_number}"}}, "features": [{features}]}}"#)
}

fn assembly(lid: &str) -> String {
    format!(
        r#"{{"format": "solidcraft/29", "name": "box", "components": [{{"id": 2, "name": "Lid", "parent": 0}}],
            "links": {{"2": {{"path": "{lid}", "feature": 1}}}}}}"#
    )
}

fn login(url: &str, user: &str, password: &str) -> Client {
    let mut client = Client::new(url, None);
    client.login(user, password).unwrap();
    client
}

fn workspace(url: &str, user: &str, password: &str) -> (tempfile::TempDir, Workspace) {
    let dir = tempfile::tempdir().unwrap();
    let ws = Workspace::create(dir.path(), login(url, user, password), user, "workshop").unwrap();
    (dir, ws)
}

/// A server with a vault, Anna and Ben (both may change it), and a lid and a box (linking the
/// lid) added by Anna.
fn team() -> Team {
    let data = tempfile::tempdir().unwrap();
    let server = Server::open(data.path(), "Test").unwrap();
    server.add_vault("workshop").unwrap();
    for (user, admin) in [("anna", false), ("ben", false), ("root", true), ("reader", false)] {
        server.accounts.add_user(user, &format!("{user}-password"), admin).unwrap();
    }
    server.accounts.grant("anna", "workshop", protocol::Access::Write).unwrap();
    server.accounts.grant("ben", "workshop", protocol::Access::Write).unwrap();
    server.accounts.grant("reader", "workshop", protocol::Access::Read).unwrap();
    let url = scpdm_server::spawn_for_tests(server);

    let (a, mut anna) = workspace(&url, "anna", "anna-password");
    let (b, ben) = workspace(&url, "ben", "ben-password");
    write(&anna, "parts/lid.solidcraft", &design("LID-1", 1));
    write(&anna, "asm/box.solidcraft", &assembly("../parts/lid.solidcraft"));
    anna.add("parts/lid.solidcraft", "First lid").unwrap();
    anna.add("asm/box.solidcraft", "The box").unwrap();
    Team { _dirs: vec![data, a, b], url, anna, ben }
}

/// Writes a file in a workspace like SolidCraft does: a new file renamed over the old one.
/// That works on a read-only file on Unix; on Windows someone would first have to clear the
/// read-only flag (which is the hint that it is not checked out), as done here.
fn write(ws: &Workspace, path: &str, text: &str) {
    let file = ws.absolute(path);
    fs::create_dir_all(file.parent().unwrap()).unwrap();
    #[cfg(windows)]
    if let Ok(meta) = fs::metadata(&file) {
        let mut perms = meta.permissions();
        #[allow(clippy::permissions_set_readonly_false)]
        perms.set_readonly(false);
        fs::set_permissions(&file, perms).unwrap();
    }
    let tmp = file.with_extension("tmp");
    fs::write(&tmp, text).unwrap();
    fs::rename(&tmp, &file).unwrap();
}

fn status(ws: &mut Workspace, path: &str) -> RemoteStatus {
    ws.status().unwrap().designs.into_iter().find(|e| e.path == path).map(|e| e.status).unwrap_or(RemoteStatus::LocalOnly)
}

fn readonly(path: &Path) -> bool {
    fs::metadata(path).unwrap().permissions().readonly()
}

const LID: &str = "parts/lid.solidcraft";
const BOX: &str = "asm/box.solidcraft";

#[test]
fn logins_and_access() {
    let t = team();
    let mut client = Client::new(&t.url, None);
    let err = client.login("anna", "wrong").unwrap_err();
    assert_eq!(err.code(), Some(ErrorCode::Unauthorized));
    assert_eq!(client.info().unwrap().protocol, protocol::PROTOCOL_VERSION);
    let anna = login(&t.url, "anna", "anna-password");
    assert_eq!(anna.vaults().unwrap()[0].name, "workshop");
    assert!(Client::new(&t.url, Some("forged".into())).vaults().unwrap_err().needs_login());

    let reader = login(&t.url, "reader", "reader-password");
    assert_eq!(reader.tree("workshop", None).unwrap().designs.len(), 2, "a reader sees the vault");
    let err = reader.act("workshop", &Action::Checkout { path: LID.into() }).unwrap_err();
    assert_eq!(err.code(), Some(ErrorCode::Forbidden), "but changes nothing");
}

#[test]
fn get_check_out_change_check_in() {
    let mut t = team();
    assert_eq!(status(&mut t.ben, LID), RemoteStatus::NotDownloaded);
    t.ben.get(LID).unwrap();
    assert_eq!(status(&mut t.ben, LID), RemoteStatus::UpToDate);
    assert!(readonly(&t.ben.absolute(LID)), "not checked out: read-only");

    t.ben.checkout(LID).unwrap();
    assert!(!readonly(&t.ben.absolute(LID)));
    let err = t.anna.checkout(LID).unwrap_err();
    assert_eq!(err.code(), Some(ErrorCode::Locked));
    assert!(err.to_string().contains("ben"), "{err}");
    assert_eq!(status(&mut t.anna, LID), RemoteStatus::CheckedOutByOther("ben".into()));

    write(&t.ben, LID, &design("LID-2", 2));
    assert_eq!(status(&mut t.ben, LID), RemoteStatus::CheckedOut { modified: true });
    let v2 = t.ben.checkin(LID, "Deeper", false).unwrap();
    assert_eq!((v2.number, v2.author.as_str()), (2, "ben"), "recorded as ben's");
    assert!(readonly(&t.ben.absolute(LID)));

    assert_eq!(status(&mut t.anna, LID), RemoteStatus::NewerOnServer);
    t.anna.get(LID).unwrap();
    assert_eq!(fs::read_to_string(t.anna.absolute(LID)).unwrap(), design("LID-2", 2));
    assert_eq!(status(&mut t.anna, LID), RemoteStatus::UpToDate);
}

#[test]
fn changes_without_check_out_are_caught_and_never_overwritten() {
    let mut t = team();
    write(&t.anna, LID, &design("LID-X", 3));
    assert_eq!(status(&mut t.anna, LID), RemoteStatus::ChangedWithoutCheckout);
    let err = t.anna.checkin(LID, "sneaky", false).unwrap_err();
    assert_eq!(err.code(), Some(ErrorCode::NotCheckedOut));

    // Ben changes it meanwhile: Anna's copy is now in conflict, and stays as she made it.
    t.ben.get(LID).unwrap();
    t.ben.checkout(LID).unwrap();
    write(&t.ben, LID, &design("LID-B", 2));
    t.ben.checkin(LID, "Ben's", false).unwrap();
    assert_eq!(status(&mut t.anna, LID), RemoteStatus::Conflict);
    assert!(t.anna.get(LID).is_err(), "a local change is never overwritten");
    assert_eq!(fs::read_to_string(t.anna.absolute(LID)).unwrap(), design("LID-X", 3));

    // Checking out and in on the old version is refused as stale.
    t.anna.checkout(LID).unwrap();
    let err = t.anna.checkin(LID, "mine", false).unwrap_err();
    assert_eq!(err.code(), Some(ErrorCode::Stale));
    // Undoing with discard takes her back to what she had fetched; then she can get the latest.
    t.anna.undo_checkout(LID, true).unwrap();
    t.anna.get(LID).unwrap();
    assert_eq!(fs::read_to_string(t.anna.absolute(LID)).unwrap(), design("LID-B", 2));
}

#[test]
fn moves_update_links_and_other_workspaces_follow() {
    let mut t = team();
    t.ben.get(LID).unwrap();
    t.ben.get(BOX).unwrap();

    // Refused while the box (whose link would change) is checked out.
    t.ben.checkout(BOX).unwrap();
    let err = t.anna.move_file(LID, "parts/cover.solidcraft").unwrap_err();
    assert_eq!(err.code(), Some(ErrorCode::Locked), "{err}");
    t.ben.undo_checkout(BOX, false).unwrap();

    let updated = t.anna.move_file(LID, "parts/cover.solidcraft").unwrap();
    assert_eq!(updated, [BOX]);
    assert!(t.anna.absolute("parts/cover.solidcraft").is_file());
    let links = scformat::read_file(t.anna.absolute(BOX)).unwrap().links;
    assert_eq!(links[0].path, "../parts/cover.solidcraft", "Anna's box was fetched again");

    let status = t.ben.status().unwrap();
    assert_eq!(status.followed_moves, [(LID.to_owned(), "parts/cover.solidcraft".to_owned())]);
    assert!(t.ben.absolute("parts/cover.solidcraft").is_file() && !t.ben.absolute(LID).exists());
    let box_status = status.designs.iter().find(|e| e.path == BOX).unwrap().status.clone();
    assert_eq!(box_status, RemoteStatus::NewerOnServer);
}

#[test]
fn releases_and_restores_on_the_server() {
    let mut t = team();
    let err = {
        t.ben.checkout(LID).unwrap();
        t.anna.act_on_design(LID, Action::Release { path: LID.into(), revision: "A".into(), note: String::new() }).unwrap_err()
    };
    assert_eq!(err.code(), Some(ErrorCode::Locked), "not while checked out");
    t.ben.undo_checkout(LID, false).unwrap();

    let done = t.anna.act_on_design(LID, Action::Release { path: LID.into(), revision: "A".into(), note: "first".into() }).unwrap();
    assert_eq!(done.release.unwrap().revision, "A");
    assert_eq!(scformat::read_file(t.anna.absolute(LID)).unwrap().metadata.revision.as_deref(), Some("A"), "fetched again");
    assert_eq!(status(&mut t.anna, LID), RemoteStatus::UpToDate);
    let design = t.anna.server_design(LID).unwrap().clone();
    assert_eq!(design.state, protocol::State::Released);

    let restored = t.anna.restore(LID, 1).unwrap();
    assert_eq!(restored.comment, "Restored v1");
    assert_eq!(fs::read_to_string(t.anna.absolute(LID)).unwrap(), design_v1());
    assert_eq!(t.anna.server_design(LID).unwrap().lock, None, "checked in again");
}

#[test]
fn releases_keep_production_files_on_the_server() {
    let mut t = team();
    let done = t.anna.act_on_design(LID, Action::Release { path: LID.into(), revision: "A".into(), note: String::new() }).unwrap();
    let version = done.release.unwrap().version;
    let files = [("lid-A.step".to_owned(), b"ISO-10303-21;".to_vec()), ("lid-A.stl".to_owned(), b"solid lid".to_vec())];
    // Reading is not enough.
    let (_r, mut reader) = workspace(&t.url, "reader", "reader-password");
    assert_eq!(reader.add_release_files(LID, version, &files).unwrap_err().code(), Some(ErrorCode::Forbidden));
    assert_eq!(t.anna.add_release_files(LID, version + 1, &files).unwrap_err().code(), Some(ErrorCode::Conflict), "not a release");

    let done = t.anna.add_release_files(LID, version, &files).unwrap();
    assert_eq!(done.release.unwrap().files.len(), 2);
    // Everyone sees them, and can fetch them.
    t.ben.status().unwrap();
    let release = t.ben.server_design(LID).unwrap().release.clone().unwrap();
    assert_eq!(release.files.iter().map(|f| (f.name.as_str(), f.size)).collect::<Vec<_>>(), [("lid-A.step", 13), ("lid-A.stl", 9)]);
    assert_eq!(t.ben.object(&release.files[1].hash).unwrap(), b"solid lid");
}

#[test]
fn sc_designs_go_through_the_server() {
    let mut t = team();
    // SolidCraft 0.25 names designs `.sc`.
    write(&t.anna, "parts/clip.sc", &design("CLIP-1", 1));
    t.anna.add("parts/clip.sc", "First clip").unwrap();
    t.ben.status().unwrap();
    assert!(t.ben.server_design("parts/clip.sc").is_some());
    t.ben.get("parts/clip.sc").unwrap();
    assert_eq!(fs::read_to_string(t.ben.absolute("parts/clip.sc")).unwrap(), design("CLIP-1", 1));
}

fn design_v1() -> String {
    design("LID-1", 1)
}

#[test]
fn reference_images_and_folders_are_shared() {
    let mut t = team();
    fs::write(t.anna.absolute("parts/ref.png"), b"\x89PNG fake").unwrap();
    t.anna.put_image("parts/ref.png").unwrap();
    t.anna.act(Action::CreateFolder { path: "drawings".into() }).unwrap();

    let status = t.ben.status().unwrap();
    assert!(status.folders.contains(&"drawings".to_owned()));
    let image = status.images.iter().find(|i| i.path == "parts/ref.png").unwrap();
    assert!(image.server.is_some() && image.local.is_none());
    t.ben.get("parts/ref.png").unwrap();
    assert_eq!(fs::read(t.ben.absolute("parts/ref.png")).unwrap(), b"\x89PNG fake");
}

#[test]
fn a_workspace_reopens_with_its_login() {
    let t = team();
    let root = t.anna.root().to_path_buf();
    drop(t.anna);
    let mut again = Workspace::open(&root).unwrap();
    assert_eq!((again.user(), again.vault()), ("anna", "workshop"));
    assert_eq!(again.status().unwrap().designs.len(), 2);
    assert!(Workspace::is_workspace(&root));
    let inside = Workspace::discover(&root.join("parts")).unwrap();
    assert_eq!(inside.root(), root);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = fs::metadata(root.join(remote::WORKSPACE_DIR).join("session")).unwrap().permissions().mode();
        assert_eq!(mode & 0o077, 0, "only the owner can read the login");
    }
}

#[test]
fn a_vault_without_check_outs_takes_changes_directly() {
    let data = tempfile::tempdir().unwrap();
    let server = Server::open(data.path(), "Test").unwrap();
    server.add_vault("workshop").unwrap();
    vault::Vault::open(data.path().join("vaults/workshop")).unwrap().set_checkout_required(false).unwrap();
    for user in ["anna", "ben"] {
        server.accounts.add_user(user, &format!("{user}-password"), false).unwrap();
        server.accounts.grant(user, "workshop", protocol::Access::Write).unwrap();
    }
    let url = scpdm_server::spawn_for_tests(server);
    let (_a, mut anna) = workspace(&url, "anna", "anna-password");
    let (_b, mut ben) = workspace(&url, "ben", "ben-password");
    write(&anna, LID, &design("LID-1", 1));
    anna.add(LID, "First lid").unwrap();
    assert!(!anna.checkout_required());
    assert!(!readonly(&anna.absolute(LID)), "no hint: nothing to check out");

    // Saved in SolidCraft, checked in as it is.
    write(&anna, LID, &design("LID-2", 2));
    assert_eq!(status(&mut anna, LID), RemoteStatus::Changed);
    let v2 = anna.checkin(LID, "Saved", false).unwrap();
    assert_eq!(v2.number, 2);
    assert!(!readonly(&anna.absolute(LID)));
    assert_eq!(status(&mut anna, LID), RemoteStatus::UpToDate);

    // A check-out still keeps others out, and an old base is still refused.
    ben.get(LID).unwrap();
    ben.checkout(LID).unwrap();
    write(&anna, LID, &design("LID-3", 3));
    assert_eq!(anna.checkin(LID, "mine", false).unwrap_err().code(), Some(ErrorCode::Locked));
    write(&ben, LID, &design("LID-B", 4));
    ben.checkin(LID, "Ben's", false).unwrap();
    assert_eq!(status(&mut anna, LID), RemoteStatus::Conflict);
    assert_eq!(anna.checkin(LID, "stale", false).unwrap_err().code(), Some(ErrorCode::Stale));
}

#[test]
fn deleted_designs_go_for_everyone_and_come_back() {
    let data = tempfile::tempdir().unwrap();
    let server = Server::open(data.path(), "Test").unwrap();
    server.add_vault("workshop").unwrap();
    for user in ["anna", "ben"] {
        server.accounts.add_user(user, &format!("{user}-password"), false).unwrap();
        server.accounts.grant(user, "workshop", protocol::Access::Write).unwrap();
    }
    let url = scpdm_server::spawn_for_tests(server.clone());
    let (_a, mut anna) = workspace(&url, "anna", "anna-password");
    let (_b, mut ben) = workspace(&url, "ben", "ben-password");
    write(&anna, LID, &design("LID-1", 1));
    anna.add(LID, "First lid").unwrap();
    ben.get(LID).unwrap();

    // Not while someone else has it checked out.
    ben.checkout(LID).unwrap();
    assert_eq!(anna.delete(LID).unwrap_err().code(), Some(ErrorCode::Locked));
    ben.undo_checkout(LID, false).unwrap();

    anna.delete(LID).unwrap();
    assert!(!anna.absolute(LID).exists());
    assert!(anna.status().unwrap().designs.is_empty(), "gone here");
    let status = ben.status().unwrap();
    assert_eq!(status.followed_deletes, [LID]);
    assert!(!ben.absolute(LID).exists() && status.designs.is_empty(), "and at ben's");

    // The administrator brings it back; the workspaces get it again.
    let deleted = server.deleted_designs().unwrap();
    assert_eq!((deleted[0].path.as_str(), deleted[0].deleted_by.as_str()), (LID, "anna"));
    assert_eq!(server.restore_deleted("workshop", deleted[0].id).unwrap(), LID);
    assert_eq!(status_of(&mut ben, LID), RemoteStatus::NotDownloaded);
    ben.get(LID).unwrap();
    assert_eq!(fs::read_to_string(ben.absolute(LID)).unwrap(), design("LID-1", 1));
    assert_eq!(ben.history(LID).unwrap().len(), 1, "with its history");
}

#[test]
fn a_changed_copy_of_a_deleted_design_stays() {
    let mut t = team();
    t.ben.get(LID).unwrap();
    write(&t.ben, LID, &design("BEN", 5));
    t.anna.delete(LID).unwrap();
    let status = t.ben.status().unwrap();
    assert!(status.followed_deletes.is_empty());
    assert_eq!(status.designs.iter().find(|e| e.path == LID).unwrap().status, RemoteStatus::RemovedOnServer);
    assert_eq!(fs::read_to_string(t.ben.absolute(LID)).unwrap(), design("BEN", 5), "his work is kept");
}

fn status_of(ws: &mut Workspace, path: &str) -> RemoteStatus {
    status(ws, path)
}
