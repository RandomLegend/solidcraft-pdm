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
