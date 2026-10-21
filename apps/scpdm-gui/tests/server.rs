//! The window on a workspace of a server vault: an in-process server, Anna in the window, Ben
//! through the client library.

use std::fs;
use std::path::Path;

use eframe::egui;
use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT, Queryable};
use remote::{Client, Workspace};
use scpdm_gui::app::App;
use scpdm_server::Server;

const FIXTURES: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../crates/scformat/tests/fixtures");
const CASE: &str = "Enclosures/pi4-case.solidcraft";

struct Setup {
    _dirs: Vec<tempfile::TempDir>,
    url: String,
    anna_root: std::path::PathBuf,
    ben: Workspace,
}

fn login(url: &str, user: &str) -> Client {
    let mut c = Client::new(url, None);
    c.login(user, &format!("{user}-password")).unwrap();
    c
}

/// A server whose vault has the Pi case (added by Ben), Anna's workspace (nothing fetched yet).
fn setup() -> Setup {
    let data = tempfile::tempdir().unwrap();
    let server = Server::open(data.path(), "Workshop server").unwrap();
    server.add_vault("workshop").unwrap();
    for user in ["anna", "ben"] {
        server.accounts.add_user(user, &format!("{user}-password"), false).unwrap();
        server.accounts.grant(user, "workshop", protocol::Access::Write).unwrap();
    }
    let url = scpdm_server::spawn_for_tests(server);
    let (a, b) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let mut ben = Workspace::create(b.path(), login(&url, "ben"), "ben", "workshop").unwrap();
    fs::create_dir_all(ben.absolute("Enclosures")).unwrap();
    fs::copy(Path::new(FIXTURES).join("json-pi-case.solidcraft"), ben.absolute(CASE)).unwrap();
    ben.add(CASE, "First case").unwrap();
    let anna = Workspace::create(a.path(), login(&url, "anna"), "anna", "workshop").unwrap();
    let anna_root = anna.root().to_path_buf();
    Setup { _dirs: vec![data, a, b], url, anna_root, ben }
}

fn settle(harness: &mut Harness<'_, App>) {
    for _ in 0..500 {
        harness.step();
        if !harness.state().is_scanning() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    harness.run_steps(4);
}

fn open(root: &Path) -> Harness<'static, App> {
    let start = root.to_path_buf();
    let mut harness = Harness::builder()
        .with_size([1280.0, 800.0])
        .with_step_dt(1.0 / 60.0)
        .build_eframe(move |cc| App::new(cc, Some(start)));
    settle(&mut harness);
    harness
}

fn rescan(harness: &mut Harness<'_, App>) {
    harness.key_press(egui::Key::F5);
    settle(harness);
}

/// In the list and the details panel, say.
fn shown(h: &Harness<'_, App>, label: &str) -> bool {
    h.query_all_by_label(label).next().is_some()
}

fn screenshot(harness: &mut Harness<'_, App>, name: &str) {
    let Some(dir) = std::env::var_os("SCPDM_SCREENSHOTS") else { return };
    harness.render().expect("render").save(Path::new(&dir).join(format!("{name}.png"))).unwrap();
}

#[test]
fn checks_out_and_in_through_the_window() {
    let mut s = setup();
    let mut h = open(&s.anna_root);
    h.get_by_label_contains("workshop on http://");
    h.get_by_label("On the server");
    h.state_mut().select(CASE);
    settle(&mut h);
    h.get_by_label("Get").click();
    settle(&mut h);
    assert!(s.anna_root.join(CASE).is_file(), "fetched");
    h.get_by_label("Check out").click();
    settle(&mut h);
    h.get_by_label_contains("Checked out by you since");

    // Ben cannot have it now.
    s.ben.status().unwrap();
    assert!(s.ben.checkout(CASE).is_err());

    // Anna changes it (as SolidCraft saves) and checks it in from the panel.
    let file = s.anna_root.join(CASE);
    let changed = fs::read_to_string(&file).unwrap().replace(r#""board_w""#, r#""board_width""#);
    fs::write(file.with_extension("tmp"), changed).unwrap();
    fs::rename(file.with_extension("tmp"), &file).unwrap();
    rescan(&mut h);
    assert!(shown(&h, "Checked out · changed"));
    screenshot(&mut h, "server-checked-out");
    h.get_by_label("⬆ Check in").click();
    settle(&mut h);
    let history = s.ben.history(CASE).unwrap();
    assert_eq!((history[0].number, history[0].author.as_str()), (2, "anna"));
    assert!(shown(&h, "Up to date"));
}

#[test]
fn shows_who_has_a_design_checked_out() {
    let mut s = setup();
    Workspace::open(&s.anna_root).unwrap().get(CASE).unwrap();
    s.ben.checkout(CASE).unwrap();
    let mut h = open(&s.anna_root);
    h.get_by_label("Checked out by ben");
    h.state_mut().select(CASE);
    settle(&mut h);
    h.get_by_label_contains("Checked out by ben since");
    h.get_by_label_contains("ben is working on it");
    assert!(h.get_by_label("Release as A…").accesskit_node().is_disabled(), "the server releases only what nobody has");
    assert!(h.get_by_label("Edit properties…").accesskit_node().is_disabled());
    screenshot(&mut h, "server-locked");
}

/// Types into the text field right of a label (in the dialog, drawn last).
fn type_into(h: &mut Harness<'_, App>, label: &str, text: &str) {
    let rect = h.query_all_by_label(label).last().unwrap().rect();
    let pos = egui::pos2(rect.right() + 80.0, rect.center().y);
    h.hover_at(pos);
    h.step();
    for pressed in [true, false] {
        h.event(egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed, modifiers: egui::Modifiers::NONE });
        h.step();
    }
    h.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::A);
    h.event(egui::Event::Text(text.into()));
    h.run_steps(3);
}

#[test]
fn connects_to_a_server_and_opens_a_vault() {
    let s = setup();
    let folder = tempfile::tempdir().unwrap();
    let mut h = Harness::builder().with_size([1280.0, 800.0]).with_step_dt(1.0 / 60.0).build_eframe(|cc| App::new(cc, None));
    h.run_steps(3);
    h.get_by_label("Vault").click();
    h.run_steps(2);
    h.get_by_label("Connect to a server…").click();
    h.run_steps(3);
    type_into(&mut h, "Server", &s.url);
    type_into(&mut h, "User", "anna");
    type_into(&mut h, "Password", "anna-password");
    h.get_by_label("Connect").click();
    h.run_steps(4);
    h.get_by_label("workshop  (read and change)");
    let target = folder.path().join("workshop-ws").display().to_string();
    let rect = h.get_by_label("Browse…").rect();
    let field = egui::pos2(rect.left() - 120.0, rect.center().y);
    h.hover_at(field);
    h.step();
    for pressed in [true, false] {
        h.event(egui::Event::PointerButton { pos: field, button: egui::PointerButton::Primary, pressed, modifiers: egui::Modifiers::NONE });
        h.step();
    }
    h.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::A);
    h.event(egui::Event::Text(target.clone()));
    h.run_steps(3);
    h.get_by_label("Open the vault").click();
    settle(&mut h);
    assert!(Workspace::is_workspace(Path::new(&target)));
    h.get_by_label("pi4-case.solidcraft");
    h.get_by_label("On the server");
}
