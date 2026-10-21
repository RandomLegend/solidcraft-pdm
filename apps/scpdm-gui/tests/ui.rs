//! Drives the real window over a demo vault. With `SCPDM_SCREENSHOTS=<dir>` it also renders
//! screenshots there (needs a GPU or software Vulkan/GL adapter).

use std::fs;
use std::io::Write;
use std::path::Path;

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
