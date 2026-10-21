//! The README's screenshots (`docs/images/`), of a vault made from SolidCraft's own sample
//! designs with thumbnails SolidCraft rendered, and the same designs on a team server. Run by
//! `docs/make-screenshots.sh`; skipped unless `SCPDM_README_INPUT` (designs and renders) and
//! `SCPDM_README_OUT` are set.

use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use eframe::egui;
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use scpdm_gui::app::App;
use remote::{Client, Workspace};
use vault::Vault;

/// A design of the demo vault: where it goes, which sample it is (and its earlier version),
/// and its properties.
struct Demo {
    folder: &'static str,
    file: &'static str,
    source: &'static str,
    earlier: Option<(&'static str, &'static str)>,
    part_number: &'static str,
    description: &'static str,
    comment: &'static str,
}

const PROJECT: &str = "Workshop";

const DEMO: &[Demo] = &[
    Demo {
        folder: "Enclosures",
        file: "pi4-case",
        source: "pi-case-0.8",
        earlier: Some(("pi-case", "First print, red PETG")),
        part_number: "PI4-CASE-010",
        description: "Vented case for the Raspberry Pi 4, PETG",
        comment: "Vents, port cut-outs and Pi 4 lettering",
    },
    Demo { folder: "Enclosures", file: "pi4-lid-frame", source: "issue-616-lid-ribs", earlier: None, part_number: "PI4-LID-020", description: "Lid frame with stiffening ribs", comment: "Initial version" },
    Demo {
        folder: "Power",
        file: "battery-case",
        source: "battery-tpu-case-v2",
        earlier: Some(("battery-tpu-case", "First fit test")),
        part_number: "BAT-CASE-100",
        description: "TPU sleeve for the 4S battery pack",
        comment: "Pleats for a tighter grip",
    },
    Demo { folder: "Power", file: "battery-tray", source: "modern-plastic", earlier: None, part_number: "BAT-TRAY-110", description: "Open tray, 2 mm walls", comment: "Initial version" },
    Demo { folder: "Hardware", file: "hinge", source: "joints-hinge", earlier: None, part_number: "HW-HINGE-01", description: "Butt hinge, 40 mm leaves", comment: "Initial version" },
    Demo { folder: "Hardware", file: "spacer-block", source: "m0-box-fillet-hole", earlier: None, part_number: "HW-SPACER-02", description: "Spacer block, M4 through hole", comment: "Initial version" },
    Demo { folder: "Hardware", file: "mounting-plate", source: "modern-path-pattern", earlier: None, part_number: "HW-PLATE-03", description: "Mounting plate with a hole pattern", comment: "Initial version" },
    Demo { folder: "Hardware", file: "z-bracket", source: "modern-sheet-metal", earlier: None, part_number: "HW-BRKT-04", description: "Sheet metal Z bracket, 1.5 mm", comment: "Initial version" },
    Demo { folder: "Experiments", file: "logo", source: "logo-up-down-join", earlier: None, part_number: "", description: "The SolidCraft logo, joined", comment: "Initial version" },
    Demo { folder: "Experiments", file: "cube-frame", source: "cubeish-chamfer-too-large", earlier: None, part_number: "", description: "How far a chamfer goes", comment: "Initial version" },
];

/// A design with properties and a thumbnail, as a container.
fn prepared(input: &Path, source: &str, demo: &Demo, revision: &str) -> Vec<u8> {
    let design = fs::read(input.join(format!("{source}.solidcraft"))).unwrap();
    let design = scformat::edit_document(&design, |doc| {
        let mut metadata = serde_json::json!({"project": PROJECT, "revision": revision, "description": demo.description});
        if !demo.part_number.is_empty() {
            metadata["part_number"] = demo.part_number.into();
        }
        doc["metadata"] = metadata;
        doc["name"] = demo.file.into();
    })
    .unwrap();
    let thumbnail = fs::read(input.join(format!("{source}.png"))).unwrap();
    with_thumbnail(&design, &thumbnail)
}

/// The design as a container carrying `png` as its thumbnail.
fn with_thumbnail(design: &[u8], png: &[u8]) -> Vec<u8> {
    let mut out = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    let options = zip::write::SimpleFileOptions::default();
    if design.starts_with(b"PK") {
        let mut zip = zip::ZipArchive::new(std::io::Cursor::new(design)).unwrap();
        for i in 0..zip.len() {
            let entry = zip.by_index_raw(i).unwrap();
            if !entry.name().starts_with("cache/thumbnail") {
                out.raw_copy_file(entry).unwrap();
            }
        }
    } else {
        out.start_file("container.json", options).unwrap();
        out.write_all(br#"{"format":"solidcraft-container/1"}"#).unwrap();
        out.start_file("document.json", options).unwrap();
        out.write_all(design).unwrap();
    }
    out.start_file("cache/thumbnail.png", options.compression_method(zip::CompressionMethod::Stored)).unwrap();
    out.write_all(png).unwrap();
    out.finish().unwrap().into_inner()
}

/// Not in the vault yet.
const UNTRACKED: Demo = Demo {
    folder: "Experiments",
    file: "odd-cuts",
    source: "crazy-geom-cuts",
    earlier: None,
    part_number: "",
    description: "Boolean cuts from every side",
    comment: "",
};

fn demo_vault(input: &Path, root: &Path) {
    for demo in DEMO {
        fs::create_dir_all(root.join(demo.folder)).unwrap();
    }
    let mut vault = Vault::init(root).unwrap();
    for demo in DEMO {
        let path = format!("{}/{}.solidcraft", demo.folder, demo.file);
        if let Some((earlier, comment)) = demo.earlier {
            fs::write(root.join(&path), prepared(input, earlier, demo, "A")).unwrap();
            vault.check_in(&path, comment).unwrap();
            vault.release(&path, "").unwrap();
            fs::write(root.join(&path), prepared(input, demo.source, demo, "A")).unwrap();
            vault.check_in(&path, demo.comment).unwrap();
        } else {
            fs::write(root.join(&path), prepared(input, demo.source, demo, "A")).unwrap();
            vault.check_in(&path, demo.comment).unwrap();
        }
    }
    vault.release("Enclosures/pi4-case.solidcraft", "For the workshop").unwrap();
    vault.release("Hardware/hinge.solidcraft", "").unwrap();
    vault.set_obsolete("Experiments/cube-frame.solidcraft", true).unwrap();
    // Work in progress: changed and not checked in; one design not in the vault yet.
    vault.edit_properties("Hardware/mounting-plate.solidcraft", None, &[("description", Some("Mounting plate, 6 x M3"))]).unwrap();
    let odd = prepared(input, UNTRACKED.source, &UNTRACKED, "A");
    fs::write(root.join(format!("{}/{}.solidcraft", UNTRACKED.folder, UNTRACKED.file)), odd).unwrap();
    fs::copy(input.join("reference-sketch.png"), root.join("Enclosures/board-dimensions.png")).unwrap();
}

fn settle(harness: &mut Harness<'_, App>) {
    for _ in 0..500 {
        harness.step();
        if !harness.state().is_scanning() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    harness.run_steps(6);
}

fn shot(harness: &mut Harness<'_, App>, out: &Path, name: &str) {
    harness.remove_cursor();
    harness.run_steps(4);
    // Thumbnails are decoded in the background: no spinners in the pictures.
    for _ in 0..300 {
        if !harness.ctx.has_pending_images() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
        harness.step();
    }
    harness.run_steps(2);
    harness.render().expect("render (needs a GPU or software adapter)").save(out.join(format!("{name}.png"))).unwrap();
}

fn type_text(harness: &mut Harness<'_, App>, text: &str) {
    harness.event(egui::Event::Text(text.into()));
    harness.run_steps(3);
}

#[test]
fn readme_screenshots() {
    let (Some(input), Some(out)) = (std::env::var_os("SCPDM_README_INPUT"), std::env::var_os("SCPDM_README_OUT")) else {
        eprintln!("SCPDM_README_INPUT / SCPDM_README_OUT not set: skipped");
        return;
    };
    let (input, out) = (PathBuf::from(input), PathBuf::from(out));
    fs::create_dir_all(&out).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("CAD");
    demo_vault(&input, &root);

    let start = root.clone();
    let mut harness = Harness::builder()
        .with_size([1400.0, 860.0])
        .with_pixels_per_point(1.5)
        .with_step_dt(1.0 / 60.0)
        .build_eframe(move |cc| App::new(cc, Some(start)));
    settle(&mut harness);

    // 1. The vault, with a released design and its history.
    harness.state_mut().select("Enclosures/pi4-case.solidcraft");
    settle(&mut harness);
    shot(&mut harness, &out, "vault");

    // 2. Fuzzy search through every folder.
    harness.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::F);
    harness.run_steps(2);
    type_text(&mut harness, "case");
    harness.state_mut().select("Power/battery-case.solidcraft");
    settle(&mut harness);
    shot(&mut harness, &out, "search");
    harness.key_press(egui::Key::Escape);
    harness.run_steps(3);

    // 3. What changed between the two versions of the Pi case.
    harness.state_mut().select("Enclosures/pi4-case.solidcraft");
    settle(&mut harness);
    // Clicks through accessibility: pointer positions are off at 1.5 pixels per point.
    harness.get_by_label("Properties").click_accesskit(); // so v1 is in view
    harness.run_steps(60);
    harness.query_all_by_label("Compare…").last().unwrap().click_accesskit();
    harness.run_steps(6);
    shot(&mut harness, &out, "compare");
    harness.get_by_label("Close").click_accesskit();
    harness.run_steps(3);

    // 4. A folder with its reference image.
    harness.get_by_label("Enclosures  (2)").click_accesskit();
    settle(&mut harness);
    harness.state_mut().select("Enclosures/board-dimensions.png");
    harness.run_steps(6);
    shot(&mut harness, &out, "folder");

    let mut check = Vec::new();
    fs::File::open(out.join("vault.png")).unwrap().read_to_end(&mut check).unwrap();
    assert!(check.starts_with(b"\x89PNG"));
}

/// Writes a design into a workspace as SolidCraft saves it (a new file renamed over the old).
fn save(ws: &Workspace, path: &str, bytes: &[u8]) {
    let file = ws.absolute(path);
    fs::create_dir_all(file.parent().unwrap()).unwrap();
    let tmp = file.with_extension("tmp");
    fs::write(&tmp, bytes).unwrap();
    fs::rename(&tmp, &file).unwrap();
}

fn described(bytes: &[u8], description: &str) -> Vec<u8> {
    scformat::edit_document(bytes, |doc| doc["metadata"]["description"] = description.into()).unwrap()
}

/// The README's picture of the app on a team server: Anna's workspace, with designs Ben has
/// checked out or changed, one she is changing, and one she has not fetched.
#[test]
fn readme_server_screenshot() {
    let (Some(input), Some(out)) = (std::env::var_os("SCPDM_README_INPUT"), std::env::var_os("SCPDM_README_OUT")) else {
        eprintln!("SCPDM_README_INPUT / SCPDM_README_OUT not set: skipped");
        return;
    };
    let (input, out) = (PathBuf::from(input), PathBuf::from(out));
    fs::create_dir_all(&out).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let server = scpdm_server::Server::open(&dir.path().join("server"), "Workshop server").unwrap();
    server.add_vault("workshop").unwrap();
    for user in ["anna", "ben"] {
        server.accounts.add_user(user, &format!("{user}-password"), false).unwrap();
        server.accounts.grant(user, "workshop", protocol::Access::Write).unwrap();
    }
    let url = scpdm_server::spawn_for_tests(server).replace("127.0.0.1", "localhost");
    let login = |user: &str| {
        let mut client = Client::new(&url, None);
        client.login(user, &format!("{user}-password")).unwrap();
        client
    };

    let mut ben = Workspace::create(&dir.path().join("ben"), login("ben"), "ben", "workshop").unwrap();
    for demo in DEMO {
        let path = format!("{}/{}.solidcraft", demo.folder, demo.file);
        match demo.earlier {
            Some((earlier, comment)) => {
                save(&ben, &path, &prepared(&input, earlier, demo, "A"));
                ben.add(&path, comment).unwrap();
                ben.checkout(&path).unwrap();
                save(&ben, &path, &prepared(&input, demo.source, demo, "A"));
                ben.checkin(&path, demo.comment, false).unwrap();
            }
            None => {
                save(&ben, &path, &prepared(&input, demo.source, demo, "A"));
                ben.add(&path, demo.comment).unwrap();
            }
        }
    }
    let release = |path: &str| protocol::Action::Release { path: path.into(), revision: "A".into(), note: String::new() };
    ben.act_on_design("Enclosures/pi4-case.solidcraft", release("Enclosures/pi4-case.solidcraft")).unwrap();
    ben.act_on_design("Hardware/hinge.solidcraft", release("Hardware/hinge.solidcraft")).unwrap();

    let root = dir.path().join("PDM").join("workshop");
    let mut anna = Workspace::create(&root, login("anna"), "anna", "workshop").unwrap();
    for demo in DEMO.iter().filter(|d| d.file != "logo") {
        anna.get(&format!("{}/{}.solidcraft", demo.folder, demo.file)).unwrap();
    }
    // Ben: one design checked out, one changed since Anna fetched it.
    ben.checkout("Experiments/cube-frame.solidcraft").unwrap();
    let plate = "Hardware/mounting-plate.solidcraft";
    ben.checkout(plate).unwrap();
    save(&ben, plate, &described(&fs::read(ben.absolute(plate)).unwrap(), "Mounting plate, 6 x M3"));
    ben.checkin(plate, "Hole pattern for M3", false).unwrap();
    // Anna: working on the lid frame.
    let lid = "Enclosures/pi4-lid-frame.solidcraft";
    anna.checkout(lid).unwrap();
    save(&anna, lid, &described(&fs::read(anna.absolute(lid)).unwrap(), "Lid frame, ribs 1.6 mm"));
    drop(anna);

    let start = root.clone();
    let mut harness = Harness::builder()
        .with_size([1400.0, 860.0])
        .with_pixels_per_point(1.5)
        .with_step_dt(1.0 / 60.0)
        .build_eframe(move |cc| App::new(cc, Some(start)));
    settle(&mut harness);
    harness.state_mut().select(lid);
    settle(&mut harness);
    shot(&mut harness, &out, "server-app");
}
