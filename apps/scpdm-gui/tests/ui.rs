//! Drives the real window over a demo vault. With `SCPDM_SCREENSHOTS=<dir>` it also renders
//! screenshots there (needs a GPU or software Vulkan/GL adapter).

use std::fs;
use std::io::Write;
use std::path::Path;

use eframe::egui;
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use scpdm_gui::app::App;
use vault::Vault;

const FIXTURES: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../crates/scformat/tests/fixtures");

fn fixture(name: &str) -> Vec<u8> {
    fs::read(Path::new(FIXTURES).join(name)).unwrap()
}

fn json_design(fixture_name: &str, edit: impl FnOnce(&mut serde_json::Value)) -> Vec<u8> {
    let mut doc: serde_json::Value = serde_json::from_slice(&fixture(fixture_name)).unwrap();
    edit(&mut doc);
    serde_json::to_vec_pretty(&doc).unwrap()
}

/// The `document.json` of a container fixture.
fn container_document(fixture_name: &str) -> serde_json::Value {
    let mut zip = zip::ZipArchive::new(std::io::Cursor::new(fixture(fixture_name))).unwrap();
    serde_json::from_reader(zip.by_name("document.json").unwrap()).unwrap()
}

fn container(document: &[u8], thumbnail: Option<&[u8]>) -> Vec<u8> {
    let mut zip = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    let mut add = |name: &str, bytes: &[u8]| {
        zip.start_file(name, zip::write::SimpleFileOptions::default()).unwrap();
        zip.write_all(bytes).unwrap();
    };
    add("container.json", br#"{"format":"solidcraft-container/1"}"#);
    add("document.json", document);
    if let Some(png) = thumbnail {
        add("cache/thumbnail.png", png);
    }
    zip.finish().unwrap().into_inner()
}

/// A 240×150 picture of a shaded box, like SolidCraft's isometric thumbnail.
fn box_thumbnail(hue: [u8; 3]) -> Vec<u8> {
    let (w, h) = (240u32, 150u32);
    let shade = |f: f32| image::Rgba([(hue[0] as f32 * f) as u8, (hue[1] as f32 * f) as u8, (hue[2] as f32 * f) as u8, 255]);
    let inside = |p: (f32, f32), poly: &[(f32, f32)]| {
        let mut odd = false;
        for i in 0..poly.len() {
            let (a, b) = (poly[i], poly[(i + 1) % poly.len()]);
            if (a.1 > p.1) != (b.1 > p.1) && p.0 < (b.0 - a.0) * (p.1 - a.1) / (b.1 - a.1) + a.0 {
                odd = !odd;
            }
        }
        odd
    };
    let top = [(120.0, 20.0), (190.0, 50.0), (120.0, 80.0), (50.0, 50.0)];
    let left = [(50.0, 50.0), (120.0, 80.0), (120.0, 135.0), (50.0, 105.0)];
    let right = [(120.0, 80.0), (190.0, 50.0), (190.0, 105.0), (120.0, 135.0)];
    let img = image::RgbaImage::from_fn(w, h, |x, y| {
        let p = (x as f32 + 0.5, y as f32 + 0.5);
        if inside(p, &top) {
            shade(1.0)
        } else if inside(p, &left) {
            shade(0.75)
        } else if inside(p, &right) {
            shade(0.55)
        } else {
            image::Rgba([0, 0, 0, 0])
        }
    });
    let mut png = Vec::new();
    img.write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png).unwrap();
    png
}

fn set_metadata(doc: &mut serde_json::Value, pairs: &[(&str, &str)]) {
    let meta = doc.as_object_mut().unwrap().entry("metadata").or_insert_with(|| serde_json::json!({}));
    for (k, v) in pairs {
        meta[*k] = serde_json::Value::String((*v).into());
    }
}

/// A vault with folders, history, a modified and an untracked design, thumbnails and links.
fn demo_vault(root: &Path) {
    for d in ["enclosure", "assembly", "parts/fasteners"] {
        fs::create_dir_all(root.join(d)).unwrap();
    }
    let case = |rev: &str| {
        json_design("json-pi-case.solidcraft", |d| {
            set_metadata(d, &[
                ("part_number", "PI4-CASE-010"),
                ("revision", rev),
                ("project", "Raspberry Pi 4 enclosure"),
                ("description", "Vented base for the Pi 4, PETG"),
                ("author", "Workshop"),
            ])
        })
    };
    let lid = |rev: &str, hue| {
        let mut doc = container_document("container-cubeish.solidcraft");
        set_metadata(&mut doc, &[("part_number", "PI4-LID-020"), ("revision", rev), ("project", "Raspberry Pi 4 enclosure")]);
        doc["name"] = "pi4-lid".into();
        container(&serde_json::to_vec(&doc).unwrap(), Some(&box_thumbnail(hue)))
    };
    fs::write(root.join("enclosure/pi4-case.solidcraft"), case("A")).unwrap();
    fs::write(root.join("enclosure/pi4-lid.solidcraft"), lid("A", [90, 140, 220])).unwrap();
    let assembly = json_design("json-components.solidcraft", |d| {
        set_metadata(d, &[("part_number", "PI4-ASM-001"), ("revision", "A"), ("description", "Case, lid and screws")]);
        d["links"] = serde_json::json!({
            "2": {"path": "../enclosure/pi4-case.solidcraft", "feature": 1},
            "7": {"path": "../enclosure/pi4-lid.solidcraft", "feature": 2},
        });
    });
    fs::write(root.join("assembly/pi4-enclosure.solidcraft"), assembly).unwrap();
    fs::write(root.join("parts/fasteners/m2.5-standoff.solidcraft"), fixture("container-cubeish.solidcraft")).unwrap();

    let mut vault = Vault::init(root).unwrap();
    for e in vault.scan().unwrap() {
        vault.check_in(&e.path, "Initial import").unwrap();
    }
    fs::write(root.join("enclosure/pi4-lid.solidcraft"), lid("B", [220, 140, 70])).unwrap();
    vault.check_in("enclosure/pi4-lid.solidcraft", "Deeper snap fit, orange PETG").unwrap();
    fs::write(root.join("enclosure/pi4-case.solidcraft"), case("B")).unwrap(); // modified, not checked in
    fs::write(root.join("parts/fasteners/m3-screw.solidcraft"), fixture("json-move-bodies.solidcraft")).unwrap();
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

fn screenshot(harness: &mut Harness<'_, App>, name: &str) {
    let Some(dir) = std::env::var_os("SCPDM_SCREENSHOTS") else { return };
    let image = harness.render().expect("render (needs a GPU adapter)");
    image.save(Path::new(&dir).join(format!("{name}.png"))).unwrap();
}

#[test]
fn browses_a_vault() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    demo_vault(&root);

    let mut harness = Harness::builder()
        .with_size([1280.0, 800.0])
        .build_eframe(|cc| App::new(cc, Some(root.clone())));
    settle(&mut harness);

    harness.get_by_label("pi4-case.solidcraft");
    harness.get_by_label("m3-screw.solidcraft");
    harness.get_by_label("All designs  (5)");
    harness.get_by_label("Modified  (1)");
    harness.get_by_label("Untracked  (1)");
    screenshot(&mut harness, "1-vault");

    harness.state_mut().select("enclosure/pi4-lid.solidcraft");
    settle(&mut harness);
    harness.get_by_label("History (2)");
    harness.get_by_label("Deeper snap fit, orange PETG");
    harness.get_by_label("Used in (1)");
    screenshot(&mut harness, "2-history");

    harness.state_mut().select("assembly/pi4-enclosure.solidcraft");
    settle(&mut harness);
    harness.get_by_label("Uses (2)");
    screenshot(&mut harness, "3-assembly");

    harness.state_mut().select("enclosure/pi4-case.solidcraft");
    settle(&mut harness);
    harness.get_by_label("⬆ Check in as new version");
    screenshot(&mut harness, "4-modified");
}

#[test]
fn checks_in_from_the_details_panel() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    demo_vault(&root);

    let mut harness = Harness::builder()
        .with_size([1280.0, 800.0])
        .build_eframe(|cc| App::new(cc, Some(root.clone())));
    settle(&mut harness);
    harness.state_mut().select("enclosure/pi4-case.solidcraft");
    settle(&mut harness);

    harness.get_by_label("⬆ Check in as new version").click();
    settle(&mut harness);

    harness.get_by_label("History (2)");
    harness.get_by_label("5 designs · 0 modified · 1 untracked · 0 missing");
    assert!(harness.query_by_label("Modified  (1)").is_none());
}

fn open_demo() -> (tempfile::TempDir, std::path::PathBuf, Harness<'static, App>) {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    demo_vault(&root);
    let start = root.clone();
    // 60 fps, so two clicks fit egui's 0.3 s double-click window (the default step is 0.25 s).
    // 1280 × 900: the app at its usual 150 % on a 1440 px screen, where a design's history fits.
    let mut harness = Harness::builder()
        .with_size([1280.0, 900.0])
        .with_step_dt(1.0 / 60.0)
        .build_eframe(move |cc| App::new(cc, Some(start)));
    settle(&mut harness);
    (dir, root, harness)
}

fn type_text(harness: &mut Harness<'_, App>, text: &str) {
    harness.event(egui::Event::Text(text.into()));
    harness.run_steps(2);
}

#[test]
fn search_finds_designs_in_every_folder() {
    let (_dir, _root, mut harness) = open_demo();
    // A folder is selected, but a search looks through the whole vault.
    harness.get_by_label("assembly  (1)").click();
    harness.run_steps(2);
    assert!(harness.query_by_label("pi4-case.solidcraft").is_none());

    harness.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::F);
    harness.run_steps(2);
    type_text(&mut harness, "vented"); // only in the case's description
    harness.get_by_label("1 match in the whole vault");
    harness.get_by_label("pi4-case.solidcraft");

    harness.key_press(egui::Key::Escape); // clears the search and leaves the field
    harness.run_steps(2);
    harness.get_by_label("pi4-enclosure.solidcraft"); // the selected folder again
    harness.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::F);
    harness.run_steps(2);
    type_text(&mut harness, "p4lid"); // fuzzy: letters in order
    harness.get_by_label("pi4-lid.solidcraft");
    assert!(harness.query_by_label("pi4-case.solidcraft").is_none());
    screenshot(&mut harness, "5-search");
}

#[test]
fn creates_folders() {
    let (_dir, root, mut harness) = open_demo();
    harness.get_by_label("+ New folder").click();
    harness.run_steps(3);
    type_text(&mut harness, "drawings");
    harness.key_press(egui::Key::Enter);
    settle(&mut harness);
    assert!(root.join("drawings").is_dir());
    harness.get_by_label("drawings  (0)");
}

#[test]
fn dragging_a_design_onto_a_folder_moves_it() {
    let (_dir, root, mut harness) = open_demo();
    let from = harness.get_by_label("m3-screw.solidcraft").rect().center();
    let to = harness.get_by_label("enclosure  (2)").rect().center();
    harness.hover_at(from);
    harness.drag_at(from);
    harness.step();
    for i in 1..=8 {
        harness.hover_at(from + (to - from) * (i as f32 / 8.0));
        harness.step();
    }
    harness.drop_at(to);
    settle(&mut harness);

    assert!(root.join("enclosure/m3-screw.solidcraft").is_file());
    assert!(!root.join("parts/fasteners/m3-screw.solidcraft").exists());
    harness.get_by_label("enclosure  (3)");
}

/// An "open" program that records what it was asked to open.
#[cfg(unix)]
fn recording_opener(dir: &Path) -> (std::path::PathBuf, std::path::PathBuf) {
    use std::os::unix::fs::PermissionsExt;
    let log = dir.join("opened.log");
    let script = dir.join("opener.sh");
    fs::write(&script, format!("#!/bin/sh\necho \"$1\" >> '{}'\n", log.display())).unwrap();
    fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();
    (script, log)
}

#[cfg(unix)]
/// What the opener recorded, waiting (up to a second) for `n` lines: it runs as its own process.
fn opened_lines(log: &Path, n: usize) -> Vec<String> {
    let read = || fs::read_to_string(log).map(|t| t.lines().map(str::to_owned).collect::<Vec<_>>()).unwrap_or_default();
    for _ in 0..100 {
        if read().len() >= n {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    read()
}

/// Two clicks in a row (raw events: `drop_at` would also take the pointer away in between).
#[cfg_attr(not(unix), allow(dead_code))] // its tests use a shell script
fn double_click_at<S>(harness: &mut Harness<'_, S>, pos: egui::Pos2) {
    harness.hover_at(pos);
    harness.step();
    for _ in 0..2 {
        for pressed in [true, false] {
            harness.event(egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed,
                modifiers: egui::Modifiers::NONE,
            });
            harness.step();
        }
    }
    harness.run_steps(2);
}

#[cfg(unix)] // the stand-in for SolidCraft is a shell script
#[test]
fn double_clicking_a_row_opens_the_design() {
    let tools = tempfile::tempdir().unwrap();
    let (opener, log) = recording_opener(tools.path());
    let (_dir, root, mut harness) = open_demo();
    harness.state_mut().set_open_command(&opener.to_string_lossy());
    let file = root.join("enclosure/pi4-case.solidcraft").display().to_string();

    let name = harness.get_by_label("pi4-case.solidcraft").rect();
    double_click_at(&mut harness, name.center());
    harness.run_steps(60); // a second apart, or egui counts a triple click
    double_click_at(&mut harness, egui::pos2(name.left() - 60.0, name.center().y)); // the thumbnail
    assert_eq!(opened_lines(&log, 2), [file.clone(), file]);
}

#[test]
fn columns_can_be_hidden() {
    let (_dir, _root, mut harness) = open_demo();
    harness.get_by_label("Ver");
    harness.get_by_label("Rev");
    harness.get_by_label("View").click();
    harness.run_steps(2);
    harness.get_by_label("Version").click();
    harness.run_steps(2);
    harness.get_by_label("Revision").click();
    harness.run_steps(2);
    harness.key_press(egui::Key::Escape);
    harness.run_steps(2);
    assert!(harness.query_by_label("Ver").is_none());
    assert!(harness.query_by_label("Rev").is_none());
    harness.get_by_label("Part number");
    screenshot(&mut harness, "6-columns");
}

#[cfg(unix)] // the stand-in for SolidCraft is a shell script
#[test]
fn after_a_restore_only_the_newest_version_is_the_working_file() {
    let tools = tempfile::tempdir().unwrap();
    let (opener, log) = recording_opener(tools.path());
    let (_dir, root, mut harness) = open_demo();
    harness.state_mut().set_open_command(&opener.to_string_lossy());
    // v1 (blue), v2 (orange); restoring v1 makes v3 with v1's content.
    Vault::open(&root).unwrap().restore("enclosure/pi4-lid.solidcraft", 1).unwrap();
    harness.key_press(egui::Key::F5); // restored behind the app's back: rescan now
    settle(&mut harness);
    harness.state_mut().select("enclosure/pi4-lid.solidcraft");
    settle(&mut harness);
    harness.get_by_label("Properties").click(); // collapsed, so the whole history fits
    harness.run_steps(4);
    screenshot(&mut harness, "7-restored");
    harness.get_by_label("History (3)");
    assert_eq!(harness.query_all_by_label("working file").count(), 1);
    harness.get_by_label("same as working file");

    // Double-clicks: the preview and the working version open the file, v2 a read-only copy.
    let file = root.join("enclosure/pi4-lid.solidcraft").display().to_string();
    let path = harness.get_by_label("enclosure/pi4-lid.solidcraft").rect(); // under the heading, under the preview
    harness.run_steps(60); // a second after clicking Properties, or egui counts a triple click
    double_click_at(&mut harness, egui::pos2(path.left() + 100.0, path.top() - 90.0));
    harness.run_steps(60);
    let v3 = harness.get_by_label("Restored v1").rect().center();
    double_click_at(&mut harness, v3);
    harness.run_steps(60);
    let v2 = harness.get_by_label("Deeper snap fit, orange PETG").rect().center();
    double_click_at(&mut harness, v2);
    let lines = opened_lines(&log, 3);
    assert_eq!(lines[..2], [file.clone(), file]);
    assert!(lines[2].ends_with("pi4-lid (v2, read-only).solidcraft"), "{lines:?}");
    let copy = Path::new(&lines[2]);
    assert!(fs::metadata(copy).unwrap().permissions().readonly());
    assert!(fs::metadata(copy.parent().unwrap()).unwrap().permissions().readonly(), "SolidCraft cannot save next to it");
}

#[cfg(unix)] // the stand-in for SolidCraft is a shell script
#[test]
fn creates_a_new_design_and_opens_it() {
    let tools = tempfile::tempdir().unwrap();
    let (opener, log) = recording_opener(tools.path());
    let (_dir, root, mut harness) = open_demo();
    harness.state_mut().set_open_command(&opener.to_string_lossy());
    harness.get_by_label("enclosure  (2)").click();
    harness.run_steps(2);
    harness.get_by_label("+ New design").click();
    harness.run_steps(3);
    type_text(&mut harness, "wall-bracket");
    harness.get_by_label("Creates enclosure/wall-bracket.solidcraft as v1");
    harness.key_press(egui::Key::Enter);
    settle(&mut harness);

    let file = root.join("enclosure/wall-bracket.solidcraft");
    let info = scformat::read_file(&file).unwrap();
    assert_eq!(info.name.as_deref(), Some("wall-bracket"));
    assert_eq!(opened_lines(&log, 1), [file.display().to_string()]);
    harness.get_by_label("History (1)");
    harness.get_by_label("New design");
}

#[test]
fn restore_buttons_respond() {
    let (_dir, _root, mut harness) = open_demo();
    harness.state_mut().select("enclosure/pi4-lid.solidcraft");
    settle(&mut harness);
    harness.get_by_label("Properties").click();
    harness.run_steps(4);
    // v2 is the working file; v1 has a Restore button that works.
    let buttons: Vec<egui::Rect> = harness.query_all_by_label("Restore…").map(|n| n.rect()).collect();
    assert_eq!(buttons.len(), 2);
    let v1 = buttons[1];
    for (i, pos) in [v1.center(), v1.left_center() + egui::vec2(4.0, 0.0), v1.right_center() - egui::vec2(4.0, 0.0)].into_iter().enumerate() {
        harness.hover_at(pos);
        harness.step();
        for pressed in [true, false] {
            harness.event(egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed, modifiers: egui::Modifiers::NONE });
            harness.step();
        }
        harness.run_steps(2);
        assert!(harness.query_by_label("Restore v1?").is_some(), "click {i} at {pos:?} opened no dialog");
        harness.get_by_label("Cancel").click();
        harness.run_steps(30);
    }
}

#[test]
fn moving_into_a_new_folder_adds_no_version() {
    let (_dir, root, mut harness) = open_demo();
    harness.get_by_label("+ New folder").click();
    harness.run_steps(3);
    type_text(&mut harness, "archive");
    harness.key_press(egui::Key::Enter);
    settle(&mut harness);
    harness.get_by_label("All designs  (5)").click();
    settle(&mut harness);
    let from = harness.get_by_label("pi4-lid.solidcraft").rect().center();
    let to = harness.get_by_label("archive  (0)").rect().center();
    harness.hover_at(from);
    harness.drag_at(from);
    harness.step();
    for i in 1..=8 {
        harness.hover_at(from + (to - from) * (i as f32 / 8.0));
        harness.step();
    }
    harness.drop_at(to);
    settle(&mut harness);
    harness.run_steps(60);
    settle(&mut harness);
    let history = Vault::open(&root).unwrap().history("archive/pi4-lid.solidcraft").unwrap();
    assert_eq!(history.iter().map(|v| v.number).collect::<Vec<_>>(), [2, 1], "{history:#?}");
}

fn write_image(path: &Path, width: u32, height: u32, rgb: [u8; 3]) {
    image::RgbImage::from_pixel(width, height, image::Rgb(rgb)).save(path).unwrap();
}

#[test]
fn shows_reference_images_in_folders() {
    let (_dir, root, mut harness) = open_demo();
    write_image(&root.join("enclosure/case-ref.jpg"), 400, 200, [200, 60, 60]);
    write_image(&root.join("enclosure/lid-sketch.webp"), 300, 300, [60, 160, 90]);
    write_image(&root.join("overview.png"), 320, 200, [60, 90, 200]);
    fs::write(root.join("enclosure/broken.png"), b"not an image").unwrap();
    harness.key_press(egui::Key::F5);
    settle(&mut harness);

    // Not among "All designs"…
    assert!(harness.query_by_label("case-ref.jpg").is_none());
    // …but in their folder, next to the designs.
    harness.get_by_label("enclosure  (2)").click();
    settle(&mut harness);
    harness.get_by_label("case-ref.jpg");
    harness.get_by_label("lid-sketch.webp");
    harness.get_by_label("pi4-case.solidcraft");
    harness.get_by_label_contains("Reference image  ·  400 × 200");
    screenshot(&mut harness, "8-images");

    harness.state_mut().select("enclosure/case-ref.jpg");
    harness.run_steps(3);
    harness.get_by_label("400 × 200 px");
    harness.get_by_label("Reference image (shown, not versioned)");
    screenshot(&mut harness, "9-image-details");
    harness.state_mut().select("enclosure/broken.png");
    harness.run_steps(3);
    harness.get_by_label("This image could not be read.");

    // And found by the search, wherever they are.
    harness.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::F);
    harness.run_steps(2);
    type_text(&mut harness, "overview");
    harness.get_by_label("overview.png");
    assert!(Vault::open(&root).unwrap().history("enclosure/case-ref.jpg").is_err(), "not versioned");
}

#[test]
fn new_design_from_a_right_click_in_the_list() {
    let (_dir, _root, mut harness) = open_demo();
    harness.get_by_label("assembly  (1)").click();
    settle(&mut harness);
    // Below the only row.
    let row = harness.get_by_label("pi4-enclosure.solidcraft").rect();
    let below = egui::pos2(row.center().x, row.bottom() + 200.0);
    harness.hover_at(below);
    harness.step();
    for pressed in [true, false] {
        harness.event(egui::Event::PointerButton { pos: below, button: egui::PointerButton::Secondary, pressed, modifiers: egui::Modifiers::NONE });
        harness.step();
    }
    harness.run_steps(2);
    harness.get_by_label("New design here…").click();
    harness.run_steps(3);
    type_text(&mut harness, "gasket");
    harness.get_by_label("Creates assembly/gasket.solidcraft as v1");
}

#[test]
fn moving_a_linked_part_updates_the_assembly() {
    let (_dir, root, mut harness) = open_demo();
    let from = harness.get_by_label("pi4-lid.solidcraft").rect().center();
    let to = harness.get_by_label("parts  (2)").rect().center();
    harness.hover_at(from);
    harness.drag_at(from);
    harness.step();
    for i in 1..=8 {
        harness.hover_at(from + (to - from) * (i as f32 / 8.0));
        harness.step();
    }
    harness.drop_at(to);
    settle(&mut harness);

    let links = scformat::read_file(root.join("assembly/pi4-enclosure.solidcraft")).unwrap().links;
    let paths: Vec<&str> = links.iter().map(|l| l.path.as_str()).collect();
    assert!(paths.contains(&"../parts/pi4-lid.solidcraft"), "{paths:?}");
    harness.get_by_label_contains("links updated in assembly/pi4-enclosure.solidcraft (v2)");
    let history = Vault::open(&root).unwrap().history("assembly/pi4-enclosure.solidcraft").unwrap();
    assert_eq!(history[0].comment, "Link to parts/pi4-lid.solidcraft updated (moved from enclosure/pi4-lid.solidcraft)");
}

#[test]
fn compares_a_version_with_the_working_file() {
    let (_dir, _root, mut harness) = open_demo();
    harness.state_mut().select("enclosure/pi4-lid.solidcraft");
    settle(&mut harness);
    harness.get_by_label("Properties").click();
    harness.run_steps(4);
    // v1 (revision A, blue) against the working file (v2: revision B, orange).
    harness.query_all_by_label("Compare…").last().unwrap().click();
    harness.run_steps(4);
    harness.get_by_label("Compare pi4-lid.solidcraft");
    harness.get_by_label("~ revision");
    harness.get_by_label("Properties (1)");
    screenshot(&mut harness, "10-compare");
    harness.get_by_label("Close").click();
    harness.run_steps(2);
    assert!(harness.query_by_label("Compare pi4-lid.solidcraft").is_none());
}

#[test]
fn releases_a_design_from_the_details_panel() {
    let (_dir, root, mut harness) = open_demo();
    harness.state_mut().select("enclosure/pi4-lid.solidcraft");
    settle(&mut harness);
    // The lid says revision B already (set by hand): that is proposed.
    harness.get_by_label("Release as B…").click();
    harness.run_steps(3);
    harness.get_by_label("Release B").click();
    settle(&mut harness);

    harness.get_by_label("Revision B");
    assert_eq!(scformat::read_file(root.join("enclosure/pi4-lid.solidcraft")).unwrap().metadata.revision.as_deref(), Some("B"));
    harness.get_by_label("Revise");
    harness.get_by_label("Released  (1)");
    screenshot(&mut harness, "11-released");
    let lifecycle = Vault::open(&root).unwrap().lifecycle("enclosure/pi4-lid.solidcraft").unwrap();
    // The file said B already: nothing to write, the release is v2 itself.
    assert_eq!((lifecycle.state, lifecycle.releases[0].version), (vault::State::Released, 2));
}

#[test]
fn edits_design_properties() {
    let (_dir, root, mut harness) = open_demo();
    harness.state_mut().select("enclosure/pi4-lid.solidcraft");
    settle(&mut harness);
    harness.get_by_label("Edit properties…").click();
    harness.run_steps(3);
    harness.get_by_label("Design properties");
    // The dialog's part number field, right of its label (drawn last, above everything else).
    let label = harness.query_all_by_label("Part number").last().unwrap().rect();
    let field = egui::pos2(label.right() + 60.0, label.center().y);
    harness.hover_at(field);
    harness.step();
    for pressed in [true, false] {
        harness.event(egui::Event::PointerButton { pos: field, button: egui::PointerButton::Primary, pressed, modifiers: egui::Modifiers::NONE });
        harness.step();
    }
    harness.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::A);
    harness.run_steps(2);
    type_text(&mut harness, "PI4-LID-021");
    screenshot(&mut harness, "12-properties");
    harness.get_by_label("Save").click();
    settle(&mut harness);

    let info = scformat::read_file(root.join("enclosure/pi4-lid.solidcraft")).unwrap();
    assert_eq!(info.metadata.part_number.as_deref(), Some("PI4-LID-021"));
    assert_eq!(info.metadata.project.as_deref(), Some("Raspberry Pi 4 enclosure"), "untouched fields kept");
    let history = Vault::open(&root).unwrap().history("enclosure/pi4-lid.solidcraft").unwrap();
    assert_eq!((history[0].number, history[0].comment.as_str()), (3, "Edited properties"));
}

/// Makes a file look saved `secs` seconds ago.
fn saved_ago(file: &Path, secs: u64) {
    let when = std::time::SystemTime::now() - std::time::Duration::from_secs(secs);
    fs::File::options().write(true).open(file).unwrap().set_modified(when).unwrap();
}

#[test]
fn checks_in_by_itself_when_asked_to() {
    let (_dir, root, mut harness) = open_demo();
    harness.state_mut().set_auto_checkin(true);
    // Saved a while ago: the modified case and the new screw; the lid was saved just now.
    saved_ago(&root.join("enclosure/pi4-case.solidcraft"), 30);
    saved_ago(&root.join("parts/fasteners/m3-screw.solidcraft"), 30);
    fs::write(root.join("enclosure/pi4-lid.solidcraft"), fixture("container-cubeish.solidcraft")).unwrap();
    harness.key_press(egui::Key::F5);
    settle(&mut harness);
    settle(&mut harness);

    let vault = Vault::open(&root).unwrap();
    let case = vault.history("enclosure/pi4-case.solidcraft").unwrap();
    assert_eq!((case.len(), case[0].comment.as_str()), (2, "Saved (checked in automatically)"));
    assert_eq!(vault.history("parts/fasteners/m3-screw.solidcraft").unwrap().len(), 1, "new designs too");
    assert_eq!(vault.history("enclosure/pi4-lid.solidcraft").unwrap().len(), 2, "not while it may still be saving");
    harness.get_by_label_contains("Checked in automatically:");

    // A moment later it is taken too.
    saved_ago(&root.join("enclosure/pi4-lid.solidcraft"), 30);
    harness.key_press(egui::Key::F5);
    settle(&mut harness);
    settle(&mut harness);
    assert_eq!(Vault::open(&root).unwrap().history("enclosure/pi4-lid.solidcraft").unwrap().len(), 3);
}
