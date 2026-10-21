use std::io::{Cursor, Write};

use scformat::{Encoding, Error};

fn fixture(name: &str) -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures").join(name)
}

fn container(entries: &[(&str, &[u8])]) -> Cursor<Vec<u8>> {
    let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
    for (name, bytes) in entries {
        zip.start_file(*name, zip::write::SimpleFileOptions::default()).unwrap();
        zip.write_all(bytes).unwrap();
    }
    let mut cursor = zip.finish().unwrap();
    cursor.set_position(0);
    cursor
}

const CONTAINER_JSON: &[u8] = br#"{"format":"solidcraft-container/1"}"#;
const PNG: &[u8] = b"\x89PNG\r\n\x1a\nrest-of-png";

#[test]
fn reads_a_container() {
    let info = scformat::read_file(fixture("container-cubeish.solidcraft")).unwrap();
    assert_eq!(info.encoding, Encoding::Container);
    assert_eq!(info.format_version(), Some(19));
    assert_eq!(info.name.as_deref(), Some("cubeish"));
    assert_eq!(info.feature_count, 9);
    assert!(info.metadata.is_empty());
    assert!(info.thumbnail_png.is_none());
}

#[test]
fn reads_plain_json() {
    let info = scformat::read_file(fixture("json-pi-case.solidcraft")).unwrap();
    assert_eq!(info.encoding, Encoding::Json);
    assert_eq!(info.format_version(), Some(16));
    assert_eq!(info.units.as_deref(), Some("mm"));
    assert!(info.param_count > 0);
}

#[test]
fn reads_components() {
    let info = scformat::read_file(fixture("json-components.solidcraft")).unwrap();
    let names: Vec<_> = info.components.iter().map(|c| c.name.as_str()).collect();
    assert_eq!(names, ["Arm", "Arm (copy)"]);
}

#[test]
fn reads_designs_older_than_the_format_string() {
    let doc = br#"{"name": "old", "units": "mm", "features": [{"id": 1}], "next_id": 2}"#;
    let info = scformat::read(Cursor::new(&doc[..])).unwrap();
    assert_eq!(info.format, None);
    assert_eq!(info.format_version(), None);
    assert_eq!(info.feature_count, 1);
}

#[test]
fn reads_json_with_format_after_other_fields() {
    let info = scformat::read_file(fixture("json-move-bodies.solidcraft")).unwrap();
    assert_eq!(info.encoding, Encoding::Json);
    assert!(info.format_version().is_some());
    assert!(info.feature_count > 0);
}

#[test]
fn reads_properties_links_and_thumbnail() {
    let doc = br#"{
        "format": "solidcraft/28", "name": "case", "units": "mm",
        "metadata": {
            "part_number": "ACME-1042", "revision": "B", "author": 42,
            "org.printfarm": {"internal_id": 4}
        },
        "components": [{"id": 3, "name": "Lid", "parent": 0}],
        "links": {"3": {"path": "../parts/lid.solidcraft", "feature": 7}}
    }"#;
    let info = scformat::read(container(&[
        ("container.json", CONTAINER_JSON),
        ("document.json", doc),
        ("blobs/ab.step", b"ISO-10303-21;"),
        ("cache/thumbnail.png", PNG),
    ]))
    .unwrap();

    assert_eq!(info.metadata.part_number.as_deref(), Some("ACME-1042"));
    assert_eq!(info.metadata.revision.as_deref(), Some("B"));
    assert_eq!(info.metadata.author, None, "non-text well-known keys read as absent");
    assert!(info.metadata.extensions.contains_key("org.printfarm"));
    assert_eq!(info.links.len(), 1);
    assert_eq!(info.links[0].component, 3);
    assert_eq!(info.links[0].path, "../parts/lid.solidcraft");
    assert_eq!(info.blob_count, 1);
    assert_eq!(info.thumbnail_png.as_deref(), Some(PNG));
}

#[test]
fn a_bad_thumbnail_is_a_miss() {
    let info = scformat::read(container(&[
        ("container.json", CONTAINER_JSON),
        ("document.json", br#"{"format":"solidcraft/28"}"#),
        ("cache/thumbnail.png", b"not a png"),
    ]))
    .unwrap();
    assert!(info.thumbnail_png.is_none());
}

#[test]
fn refuses_containers_without_a_document() {
    let err = scformat::read(container(&[("container.json", CONTAINER_JSON)])).unwrap_err();
    assert!(matches!(err, Error::MissingPart("document.json")), "{err}");
}

#[test]
fn refuses_unknown_containers() {
    let err = scformat::read(container(&[
        ("container.json", br#"{"format":"something-else/1"}"#),
        ("document.json", b"{}"),
    ]))
    .unwrap_err();
    assert!(matches!(err, Error::UnsupportedContainer(_)), "{err}");
}

#[test]
fn new_designs_read_back() {
    let bytes = scformat::new_design("bracket", &[("part_number", " BR-001 "), ("description", "")]);
    let info = scformat::read(Cursor::new(bytes)).unwrap();
    assert_eq!(info.encoding, Encoding::Json);
    assert_eq!(info.format_version(), Some(1));
    assert_eq!(info.name.as_deref(), Some("bracket"));
    assert_eq!(info.units.as_deref(), Some("mm"));
    assert_eq!(info.metadata.part_number.as_deref(), Some("BR-001"));
    assert_eq!(info.metadata.description, None, "empty values are left out");
    assert_eq!(info.feature_count, 0);
}

const TRICKY: &str = r#"{
  "format": "solidcraft/29",
  "name": "bracket",
  "params": [{"name": "w", "value": 0.1}, {"name": "z", "value": -0.0}, {"name": "tiny", "value": 1e-300},
             {"name": "long", "value": 123456789.12345679}, {"name": "third", "value": 0.30000000000000004}],
  "next_id": 18446744073709551615,
  "zeta": 1,
  "alpha": 2,
  "links": {"3": {"path": "../parts/lid.solidcraft", "feature": 7}}
}
"#;

fn set_link(doc: &mut serde_json::Value) {
    doc["links"]["3"]["path"] = "lid.solidcraft".into();
}

#[test]
fn editing_plain_json_keeps_everything_else_exactly() {
    let out = scformat::edit_document(TRICKY.as_bytes(), set_link).unwrap();
    let text = String::from_utf8(out).unwrap();
    let before: serde_json::Value = serde_json::from_str(TRICKY).unwrap();
    let after: serde_json::Value = serde_json::from_str(&text).unwrap();
    for (i, p) in before["params"].as_array().unwrap().iter().enumerate() {
        let (a, b) = (p["value"].as_f64().unwrap(), after["params"][i]["value"].as_f64().unwrap());
        assert_eq!(a.to_bits(), b.to_bits(), "{} changed: {a} -> {b}", p["name"]);
    }
    assert_eq!(after["next_id"].as_u64(), Some(u64::MAX));
    let keys: Vec<&str> = after.as_object().unwrap().keys().map(String::as_str).collect();
    assert_eq!(keys, ["format", "name", "params", "next_id", "zeta", "alpha", "links"], "key order kept");
    assert_eq!(after["links"]["3"]["path"], "lid.solidcraft");
    assert_eq!(after["links"]["3"]["feature"], 7);
    assert!(text.contains("\n  \"name\""), "pretty layout kept");
    assert!(text.ends_with('\n'));
}

#[test]
fn editing_a_container_copies_the_other_entries_byte_for_byte() {
    let blob: Vec<u8> = (0..=255u8).cycle().take(10_000).collect();
    let png = b"\x89PNG\r\n\x1a\nthumbnail";
    let original = container(&[
        ("container.json", CONTAINER_JSON),
        ("document.json", TRICKY.as_bytes()),
        ("blobs/ab.mesh", &blob),
        ("cache/thumbnail.png", png),
    ])
    .into_inner();
    let edited = scformat::edit_document(&original, set_link).unwrap();

    let info = scformat::read(Cursor::new(edited.clone())).unwrap();
    assert_eq!(info.links[0].path, "lid.solidcraft");
    assert_eq!(info.thumbnail_png.as_deref(), Some(&png[..]));
    let mut before = zip::ZipArchive::new(Cursor::new(original)).unwrap();
    let mut after = zip::ZipArchive::new(Cursor::new(edited)).unwrap();
    let names = |z: &zip::ZipArchive<Cursor<Vec<u8>>>| z.file_names().map(str::to_owned).collect::<Vec<_>>();
    assert_eq!(names(&before), names(&after));
    for name in ["container.json", "blobs/ab.mesh", "cache/thumbnail.png"] {
        let read = |z: &mut zip::ZipArchive<Cursor<Vec<u8>>>| {
            let mut v = Vec::new();
            std::io::Read::read_to_end(&mut z.by_name(name).unwrap(), &mut v).unwrap();
            v
        };
        assert_eq!(read(&mut before), read(&mut after), "{name}");
    }
}

#[test]
fn editing_refuses_what_is_not_a_design() {
    assert!(scformat::edit_document(b"not json", |_| {}).is_err());
    let no_doc = container(&[("container.json", CONTAINER_JSON)]).into_inner();
    assert!(matches!(scformat::edit_document(&no_doc, |_| {}), Err(Error::MissingPart("document.json"))));
}

#[test]
fn compares_two_versions() {
    use scformat::compare::{Change, FeatureChangeKind};
    let before = br#"{"format": "solidcraft/28", "name": "case", "units": "mm",
        "metadata": {"part_number": "P-1", "revision": "A"},
        "params": [{"name": "height", "expr": "24 mm", "unit": "mm"}, {"name": "wall", "expr": "2 mm"}],
        "features": [
            {"id": 1, "name": "Sketch1", "type": "sketch", "suppressed": false, "sketch": {"lines": 4}},
            {"id": 2, "name": "Extrude1", "type": "extrude", "suppressed": false, "extent": {"distance": "height"}},
            {"id": 3, "name": "Fillet1", "type": "fillet", "suppressed": false, "radius": "1 mm"}],
        "components": [{"id": 7, "name": "Lid", "parent": 0}],
        "links": {"7": {"path": "lid.solidcraft", "feature": 2}},
        "appearances": {"bodies": {}}}"#;
    let after = br#"{"format": "solidcraft/29", "name": "case", "units": "mm",
        "metadata": {"part_number": "P-1", "revision": "B", "description": "vented"},
        "params": [{"name": "height", "expr": "32 mm", "unit": "mm"}, {"name": "vent", "expr": "5 mm"}],
        "features": [
            {"id": 1, "name": "Sketch1", "type": "sketch", "suppressed": false, "sketch": {"lines": 5}},
            {"id": 2, "name": "Base", "type": "extrude", "suppressed": true, "extent": {"distance": "height"}},
            {"id": 4, "name": "Shell1", "type": "shell", "suppressed": false}],
        "components": [{"id": 7, "name": "Cover", "parent": 0}],
        "links": {"7": {"path": "parts/lid.solidcraft", "feature": 2}},
        "appearances": {"bodies": {"Body1": {"color": [1, 0, 0]}}}}"#;
    let c = scformat::compare::compare(before, after).unwrap();
    let ch = |what: &str, b: Option<&str>, a: Option<&str>| Change { what: what.into(), before: b.map(Into::into), after: a.map(Into::into) };

    assert_eq!(c.properties, [ch("format", Some("solidcraft/28"), Some("solidcraft/29")), ch("description", None, Some("vented")), ch("revision", Some("A"), Some("B"))]);
    assert_eq!(c.parameters, [ch("height", Some("24 mm"), Some("32 mm")), ch("vent", None, Some("5 mm")), ch("wall", Some("2 mm"), None)]);
    let features: Vec<_> = c.features.iter().map(|f| (f.id, f.name.as_str(), f.change.clone())).collect();
    assert_eq!(
        features,
        [
            (1, "Sketch1", FeatureChangeKind::Edited { renamed_from: None, suppressed: None, settings: vec!["sketch".into()] }),
            (2, "Base", FeatureChangeKind::Edited { renamed_from: Some("Extrude1".into()), suppressed: Some(true), settings: vec![] }),
            (4, "Shell1", FeatureChangeKind::Added),
            (3, "Fillet1", FeatureChangeKind::Removed),
        ]
    );
    assert!(!c.features_reordered);
    assert_eq!(c.components, [ch("Cover", Some("Lid"), Some("Cover"))]);
    assert_eq!(c.links, [ch("Cover", Some("lid.solidcraft"), Some("parts/lid.solidcraft"))]);
    assert_eq!(c.other, ["appearances"]);

    assert!(scformat::compare::compare(before, before).unwrap().is_empty());
}

#[test]
fn compares_across_storage_types() {
    // The same document as plain JSON and in a container: no differences.
    let json = std::fs::read(fixture("json-pi-case.solidcraft")).unwrap();
    let boxed = container(&[("container.json", CONTAINER_JSON), ("document.json", &json)]).into_inner();
    assert!(scformat::compare::compare(&json, &boxed).unwrap().is_empty());
}
