//! Read-only access to SolidCraft design files.
//!
//! A `.solidcraft` file is one of (see SolidCraft's `docs/design-container.md`):
//!
//! - a **container**: a ZIP holding `container.json` (`{"format":"solidcraft-container/1"}`),
//!   `document.json` (the design), `blobs/…` (STEP, mesh and image payloads) and optionally
//!   `cache/…` (model cache, preview, `cache/thumbnail.png`);
//! - **plain JSON**: the design itself, `{"format": "solidcraft/N", …}`. Very old designs have no
//!   `format` field.
//!
//! The format is detected from the contents (ZIP signature `50 4b 03 04`), never from the name.
//!
//! A **drawing** (`.scdwg`, SolidCraft 0.41) is read the same way and described by the same
//! [`DesignInfo`] ([`Kind::Drawing`]): a container whose `container.json` says
//! `"content": "drawing"` and holds `drawing.json`, or plain JSON with
//! `"format": "solidcraft-drawing/N"`. Its properties are the design properties' rules (kept under
//! `properties`), and the designs it draws (`sources`) are its [`Link`]s.
//!
//! This crate reads only what a PDM needs (and writes new, empty designs, [`new_design`]): the design properties (`metadata`), name, units,
//! counts, components, links to other design files and the thumbnail. It never evaluates
//! geometry and does not depend on SolidCraft's crates, so it keeps working across SolidCraft
//! releases: unknown fields are ignored.

pub mod compare;

use std::collections::BTreeMap;
use std::fs::File;
use std::io::{Read, Seek};
use std::path::Path;

use serde::Deserialize;
use serde::de::IgnoredAny;

/// SolidCraft refuses parts larger than this; so do we.
pub const MAX_PART_BYTES: u64 = 256 * 1024 * 1024;
/// SolidCraft bounds the portable thumbnail to 256 KiB.
pub const MAX_THUMBNAIL_BYTES: u64 = 256 * 1024;

const ZIP_SIGNATURE: [u8; 4] = [0x50, 0x4b, 0x03, 0x04];
const CONTAINER_FORMAT_PREFIX: [&str; 2] = ["solidcraft-container/", "solvecraft-container/"];

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("I/O: {0}")]
    Io(#[from] std::io::Error),
    #[error("not a valid container: {0}")]
    Zip(#[from] zip::result::ZipError),
    #[error("invalid design JSON: {0}")]
    Json(#[from] serde_json::Error),
    #[error("{0} is larger than {MAX_PART_BYTES} bytes")]
    TooLarge(&'static str),
    #[error("unsupported container format {0:?}")]
    UnsupportedContainer(String),
    #[error("container has no {0}")]
    MissingPart(&'static str),
}

pub type Result<T> = std::result::Result<T, Error>;

/// How the design is stored on disk.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Encoding {
    Container,
    Json,
}

/// The well-known design properties (File → Design Properties in SolidCraft).
///
/// SolidCraft's own keys are unprefixed text. Other tools keep their data under one reverse-DNS
/// key each (`com.example.tool`), holding an object; those are in `extensions`. A well-known key
/// holding something other than text reads as absent, as in SolidCraft.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Metadata {
    pub project: Option<String>,
    pub part_number: Option<String>,
    pub revision: Option<String>,
    pub author: Option<String>,
    pub organization: Option<String>,
    pub description: Option<String>,
    pub license: Option<String>,
    /// A component's bill of materials type (`bom_type`, part properties only), as stored: read it
    /// with [`BomType::parse`].
    pub bom_type: Option<String>,
    /// Every other key, as stored (other tools' data and keys from newer SolidCraft versions).
    pub extensions: BTreeMap<String, serde_json::Value>,
}

impl Metadata {
    fn from_map(map: BTreeMap<String, serde_json::Value>) -> Self {
        let mut m = Metadata::default();
        for (key, value) in map {
            let slot = match key.as_str() {
                "project" => &mut m.project,
                "part_number" => &mut m.part_number,
                "revision" => &mut m.revision,
                "author" => &mut m.author,
                "organization" => &mut m.organization,
                "description" => &mut m.description,
                "license" => &mut m.license,
                "bom_type" => &mut m.bom_type,
                _ => {
                    m.extensions.insert(key, value);
                    continue;
                }
            };
            *slot = value.as_str().map(str::to_owned);
        }
        m
    }

    pub fn is_empty(&self) -> bool {
        *self == Metadata::default()
    }

    /// The bill of materials type: normal when missing, or a word this crate doesn't know (as
    /// SolidCraft reads a newer build's type).
    pub fn bom_type(&self) -> BomType {
        self.bom_type.as_deref().and_then(BomType::parse).unwrap_or_default()
    }
}

fn metadata_map<'de, D: serde::Deserializer<'de>>(d: D) -> std::result::Result<Metadata, D::Error> {
    // Anything but an object reads as no properties, as SolidCraft reads it.
    let value = serde_json::Value::deserialize(d)?;
    let map = match value {
        serde_json::Value::Object(m) => m.into_iter().collect(),
        _ => BTreeMap::new(),
    };
    Ok(Metadata::from_map(map))
}

/// How a component counts in a bill of materials (SolidCraft 0.41's `bom_type` part property).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum BomType {
    /// A row of its own; every placement adds to its quantity.
    #[default]
    Normal,
    /// No row of its own: what it holds counts as if placed in its parent.
    Phantom,
    /// Shown in the model, counted nowhere, and nothing inside it is counted either.
    Reference,
    /// Bought: one row, nothing inside it counted.
    Purchased,
    /// Made as one item (a welded frame): one row; of what it holds only purchased parts count.
    Inseparable,
}

impl BomType {
    pub const ALL: [BomType; 5] = [BomType::Normal, BomType::Phantom, BomType::Reference, BomType::Purchased, BomType::Inseparable];

    /// The word the design file uses.
    pub fn as_str(self) -> &'static str {
        match self {
            BomType::Normal => "normal",
            BomType::Phantom => "phantom",
            BomType::Reference => "reference",
            BomType::Purchased => "purchased",
            BomType::Inseparable => "inseparable",
        }
    }

    pub fn parse(word: &str) -> Option<BomType> {
        BomType::ALL.into_iter().find(|t| t.as_str() == word)
    }
}

/// A component of the design (`parent` 0 is the root): a definition, placed by occurrences.
#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct Component {
    pub id: u64,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub parent: u64,
    /// The definition's part properties (part number, description, revision, `bom_type`), shared
    /// by every occurrence. Insert Design seeds them from the linked design's properties.
    #[serde(default, deserialize_with = "metadata_map")]
    pub metadata: Metadata,
}

/// A placement of a component (`parent`: the component it sits in, 0 the root).
#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct Occurrence {
    #[serde(default)]
    pub id: u64,
    pub component: u64,
    #[serde(default)]
    pub parent: u64,
}

/// A component that is a standard part (a fastener, pin, bearing), rebuilt from a catalogue.
#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct StandardPart {
    #[serde(default)]
    pub family: String,
    #[serde(default)]
    pub size: String,
    #[serde(default)]
    pub length: Option<f64>,
}

impl StandardPart {
    /// As a bill of materials names it: `ISO 4762 M3 × 12`.
    pub fn label(&self) -> String {
        let mut out = format!("{} {}", self.family.trim(), self.size.trim()).trim().to_owned();
        if let Some(l) = self.length.filter(|l| l.is_finite() && *l > 0.0) {
            out.push_str(&format!(" × {}", l));
        }
        out
    }
}

/// A component linked to another design file, or a design a drawing draws.
#[derive(Clone, Debug, PartialEq)]
pub struct Link {
    /// The linked component (0 for a drawing's source).
    pub component: u64,
    /// Relative to the linking file's folder, `/` separated (as SolidCraft stores it).
    pub path: String,
    /// A drawing's source: its id, as stored.
    pub source: Option<String>,
    /// The SHA-256 of the linked file's bytes as the assembly last read them (SolidCraft 0.41,
    /// format 46): a file that no longer has it shows as changed, for Update Links.
    pub file_sha256: Option<String>,
}

impl Link {
    /// Writes a new `path` for this link into a design's or drawing's document (as
    /// [`edit_document`] hands it over).
    pub fn set_path(&self, doc: &mut serde_json::Value, path: &str) {
        let entry = match &self.source {
            Some(id) => doc
                .get_mut("sources")
                .and_then(serde_json::Value::as_array_mut)
                .and_then(|sources| sources.iter_mut().find(|s| source_id(&s["id"]).as_deref() == Some(id))),
            None => doc.get_mut("links").and_then(|l| l.get_mut(self.component.to_string())),
        };
        if let Some(entry) = entry.filter(|e| e.is_object()) {
            entry["path"] = serde_json::Value::String(path.to_owned());
        }
    }

    /// Writes the fingerprint of the linked file (lower-case hex SHA-256) into a design's
    /// document, as SolidCraft's Update Links does when the file's bodies are unchanged. A link
    /// that keeps none (saved before format 46) is left without.
    pub fn set_file_sha256(&self, doc: &mut serde_json::Value, digest: &str) {
        if self.source.is_some() {
            return;
        }
        if let Some(link) = doc.get_mut("links").and_then(|l| l.get_mut(self.component.to_string())) {
            if link.get("file_sha256").is_some_and(serde_json::Value::is_string) {
                link["file_sha256"] = serde_json::Value::String(digest.to_owned());
            }
        }
    }
}

fn source_id(id: &serde_json::Value) -> Option<String> {
    match id {
        serde_json::Value::String(s) => Some(s.clone()),
        serde_json::Value::Number(n) => Some(n.to_string()),
        _ => None,
    }
}

/// What a SolidCraft file holds.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Kind {
    #[default]
    Design,
    /// A drawing (`.scdwg`): sheets that draw designs.
    Drawing,
}

/// The format tag of drawings: `solidcraft-drawing/N`.
pub const DRAWING_FORMAT_PREFIX: &str = "solidcraft-drawing/";
const DRAWING_PART: &str = "drawing.json";

/// The key a document keeps its properties under: `metadata` in a design, `properties` in a
/// drawing.
pub fn properties_key(doc: &serde_json::Value) -> &'static str {
    if is_drawing_document(doc) { "properties" } else { "metadata" }
}

fn is_drawing_document(doc: &serde_json::Value) -> bool {
    doc.get("format").and_then(serde_json::Value::as_str).is_some_and(|f| f.starts_with(DRAWING_FORMAT_PREFIX))
}

/// What the PDM knows about one design file.
#[derive(Clone, Debug)]
pub struct DesignInfo {
    pub kind: Kind,
    pub encoding: Encoding,
    /// `solidcraft/N`; `None` for designs older than the format string.
    pub format: Option<String>,
    pub name: Option<String>,
    pub units: Option<String>,
    pub metadata: Metadata,
    pub feature_count: usize,
    pub param_count: usize,
    pub components: Vec<Component>,
    /// Where components are placed. A design saved before occurrences existed places each
    /// component once, in its parent: those are filled in here as SolidCraft does on upgrade.
    pub occurrences: Vec<Occurrence>,
    /// Components that are standard parts, by component id.
    pub standard_parts: BTreeMap<u64, StandardPart>,
    pub links: Vec<Link>,
    /// Entries under `blobs/` (containers only).
    pub blob_count: usize,
    /// A drawing's sheets.
    pub sheet_count: usize,
    /// Whether the container carries SolidCraft's evaluated model cache.
    pub has_model_cache: bool,
    /// The portable 240×150 PNG, when the container carries one.
    pub thumbnail_png: Option<Vec<u8>>,
}

impl DesignInfo {
    /// The `N` of `solidcraft/N` (`solidcraft-drawing/N` for a drawing).
    pub fn format_version(&self) -> Option<u32> {
        let format = self.format.as_deref()?;
        format.strip_prefix("solidcraft/").or_else(|| format.strip_prefix(DRAWING_FORMAT_PREFIX))?.parse().ok()
    }
}

/// A new, empty design: plain JSON in format 1, which every SolidCraft release opens and upgrades
/// (checked with SolidCraft 0.18.3, 0.20.0 and 0.25.1, also named `.sc`: they open such a file,
/// save it as a container in their format, up to `solidcraft/41`, and keep the properties). `properties` are
/// well-known `metadata` keys such as `part_number`; empty values are left out.
pub fn new_design(name: &str, properties: &[(&str, &str)]) -> Vec<u8> {
    let metadata: serde_json::Map<String, serde_json::Value> = properties
        .iter()
        .filter(|(_, v)| !v.trim().is_empty())
        .map(|(k, v)| ((*k).to_owned(), serde_json::Value::String(v.trim().to_owned())))
        .collect();
    let mut doc = serde_json::json!({
        "format": "solidcraft/1",
        "name": name,
        "units": "mm",
        "params": [],
        "features": [],
        "next_id": 1,
    });
    if !metadata.is_empty() {
        doc["metadata"] = serde_json::Value::Object(metadata);
    }
    let mut bytes = serde_json::to_vec_pretty(&doc).expect("a JSON value serializes");
    bytes.push(b'\n');
    bytes
}

/// Reads a design file's information.
pub fn read_file(path: impl AsRef<Path>) -> Result<DesignInfo> {
    read(File::open(path)?)
}

/// Reads a design from any seekable source (a file, a memory cursor).
pub fn read<R: Read + Seek>(mut source: R) -> Result<DesignInfo> {
    let mut head = [0u8; 4];
    let n = read_up_to(&mut source, &mut head)?;
    source.rewind()?;
    if n == 4 && head == ZIP_SIGNATURE {
        read_container(source)
    } else {
        let bytes = read_bounded(source, MAX_PART_BYTES, "design")?;
        #[derive(Deserialize)]
        struct Probe {
            #[serde(default)]
            format: Option<serde_json::Value>,
        }
        let probe: Probe = serde_json::from_slice(&bytes)?;
        if probe.format.as_ref().and_then(serde_json::Value::as_str).is_some_and(|f| f.starts_with(DRAWING_FORMAT_PREFIX)) {
            let drawing: RawDrawing = serde_json::from_slice(&bytes)?;
            return Ok(drawing.into_info(Encoding::Json));
        }
        let doc: RawDocument = serde_json::from_slice(&bytes)?;
        Ok(doc.into_info(Encoding::Json))
    }
}

/// Whether `bytes` hold a design or a drawing, cheaply (the container's `container.json`, or the
/// plain JSON's `format`); anything unreadable counts as a design, for the full read to refuse.
pub fn kind_of(bytes: &[u8]) -> Kind {
    if bytes.starts_with(&ZIP_SIGNATURE) {
        let Ok(mut zip) = zip::ZipArchive::new(std::io::Cursor::new(bytes)) else { return Kind::Design };
        let content = zip
            .by_name("container.json")
            .ok()
            .and_then(|e| read_bounded(e, 4096, "container.json").ok())
            .and_then(|b| serde_json::from_slice::<RawContainer>(&b).ok())
            .and_then(|c| c.content);
        return if content.as_deref() == Some("drawing") { Kind::Drawing } else { Kind::Design };
    }
    #[derive(Deserialize)]
    struct Probe {
        #[serde(default)]
        format: Option<serde_json::Value>,
    }
    match serde_json::from_slice::<Probe>(bytes) {
        Ok(Probe { format: Some(serde_json::Value::String(f)) }) if f.starts_with(DRAWING_FORMAT_PREFIX) => Kind::Drawing,
        _ => Kind::Design,
    }
}

/// A design's whole document (`document.json` of a container, or the plain JSON design).
pub fn read_document(bytes: &[u8]) -> Result<serde_json::Value> {
    if bytes.starts_with(&ZIP_SIGNATURE) {
        let mut zip = zip::ZipArchive::new(std::io::Cursor::new(bytes))?;
        let part = document_part(&zip);
        return match zip.by_name(part) {
            Ok(entry) => Ok(serde_json::from_slice(&read_bounded(entry, MAX_PART_BYTES, part)?)?),
            Err(zip::result::ZipError::FileNotFound) => Err(Error::MissingPart(part)),
            Err(e) => Err(e.into()),
        };
    }
    if bytes.len() as u64 > MAX_PART_BYTES {
        return Err(Error::TooLarge("design"));
    }
    Ok(serde_json::from_slice(bytes)?)
}

/// Changes a design's document (`document.json` of a container, or the plain JSON design) and
/// returns the new file. Everything else is kept exactly: the other entries of a container are
/// copied byte for byte, key order and every number (bit for bit) survive the round trip, and
/// a plain JSON design keeps its pretty or compact layout.
///
/// SolidCraft's model cache and thumbnail key in a container record the hash of the old
/// document, so they read as stale: SolidCraft rebuilds the model on the next open (a cache
/// miss, never an error). The geometry is unchanged by the edits the PDM makes, so the
/// thumbnail stays correct.
///
/// An edit that leaves the document as it was returns the file unchanged.
pub fn edit_document(bytes: &[u8], edit: impl FnOnce(&mut serde_json::Value)) -> Result<Vec<u8>> {
    if bytes.len() as u64 > MAX_PART_BYTES {
        return Err(Error::TooLarge("design"));
    }
    if bytes.starts_with(&ZIP_SIGNATURE) {
        return edit_container(bytes, edit);
    }
    let mut doc: serde_json::Value = serde_json::from_slice(bytes)?;
    let before = doc.clone();
    edit(&mut doc);
    if doc == before {
        return Ok(bytes.to_vec());
    }
    let pretty = bytes.windows(2).take(4096).any(|w| w == b"\n ");
    let mut out = if pretty { serde_json::to_vec_pretty(&doc)? } else { serde_json::to_vec(&doc)? };
    if bytes.ends_with(b"\n") {
        out.push(b'\n');
    }
    Ok(out)
}

fn edit_container(bytes: &[u8], edit: impl FnOnce(&mut serde_json::Value)) -> Result<Vec<u8>> {
    let mut zip = zip::ZipArchive::new(std::io::Cursor::new(bytes))?;
    let part = document_part(&zip);
    let mut doc: serde_json::Value = match zip.by_name(part) {
        Ok(entry) => serde_json::from_slice(&read_bounded(entry, MAX_PART_BYTES, part)?)?,
        Err(zip::result::ZipError::FileNotFound) => return Err(Error::MissingPart(part)),
        Err(e) => return Err(e.into()),
    };
    let before = doc.clone();
    edit(&mut doc);
    if doc == before {
        return Ok(bytes.to_vec());
    }
    let document = serde_json::to_vec(&doc)?;

    let mut out = zip::ZipWriter::new(std::io::Cursor::new(Vec::with_capacity(bytes.len())));
    for i in 0..zip.len() {
        let entry = zip.by_index_raw(i)?;
        if entry.name() == part {
            // As SolidCraft writes it: Deflate level 1.
            let options = zip::write::SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Deflated)
                .compression_level(Some(1))
                .last_modified_time(entry.last_modified().unwrap_or_default());
            drop(entry);
            out.start_file(part, options)?;
            std::io::Write::write_all(&mut out, &document)?;
        } else {
            out.raw_copy_file(entry)?;
        }
    }
    Ok(out.finish()?.into_inner())
}

/// The part a container keeps its document in: `drawing.json` in a drawing, else `document.json`.
fn document_part<R: Read + Seek>(zip: &zip::ZipArchive<R>) -> &'static str {
    let names: Vec<&str> = zip.file_names().collect();
    if names.contains(&DRAWING_PART) && !names.contains(&"document.json") { DRAWING_PART } else { "document.json" }
}

fn read_container<R: Read + Seek>(source: R) -> Result<DesignInfo> {
    let mut zip = zip::ZipArchive::new(source)?;

    let container: RawContainer = match zip.by_name("container.json") {
        Ok(entry) => serde_json::from_slice(&read_bounded(entry, MAX_PART_BYTES, "container.json")?)?,
        Err(zip::result::ZipError::FileNotFound) => return Err(Error::MissingPart("container.json")),
        Err(e) => return Err(e.into()),
    };
    if !CONTAINER_FORMAT_PREFIX.iter().any(|p| container.format.starts_with(p)) {
        return Err(Error::UnsupportedContainer(container.format));
    }
    match container.content.as_deref() {
        None => {}
        Some("drawing") => {
            let drawing: RawDrawing = match zip.by_name(DRAWING_PART) {
                Ok(entry) => serde_json::from_slice(&read_bounded(entry, MAX_PART_BYTES, DRAWING_PART)?)?,
                Err(zip::result::ZipError::FileNotFound) => return Err(Error::MissingPart(DRAWING_PART)),
                Err(e) => return Err(e.into()),
            };
            return Ok(drawing.into_info(Encoding::Container));
        }
        // A later kind of content that still holds a design reads as one, as in SolidCraft.
        Some(_) if zip.file_names().any(|n| n == "document.json") => {}
        Some(other) => return Err(Error::UnsupportedContainer(format!("{} holding {other}", container.format))),
    }

    let doc: RawDocument = match zip.by_name("document.json") {
        Ok(entry) => serde_json::from_slice(&read_bounded(entry, MAX_PART_BYTES, "document.json")?)?,
        Err(zip::result::ZipError::FileNotFound) => return Err(Error::MissingPart("document.json")),
        Err(e) => return Err(e.into()),
    };

    let blob_count = zip.file_names().filter(|n| n.starts_with("blobs/")).count();
    let has_model_cache = zip.file_names().any(|n| n == "cache/model.json");

    // The thumbnail is optional: anything wrong with it is a miss, never a failed read.
    let thumbnail_png = zip
        .by_name("cache/thumbnail.png")
        .ok()
        .filter(|e| e.size() <= MAX_THUMBNAIL_BYTES)
        .and_then(|e| read_bounded(e, MAX_THUMBNAIL_BYTES, "thumbnail").ok())
        .filter(|png| png.starts_with(b"\x89PNG\r\n\x1a\n"));

    let mut info = doc.into_info(Encoding::Container);
    info.blob_count = blob_count;
    info.has_model_cache = has_model_cache;
    info.thumbnail_png = thumbnail_png;
    Ok(info)
}

#[derive(Deserialize)]
struct RawContainer {
    format: String,
    #[serde(default)]
    content: Option<String>,
}

/// The subset of SolidCraft's `Drawing` the PDM reads.
#[derive(Deserialize)]
struct RawDrawing {
    #[serde(default)]
    format: Option<String>,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    properties: Option<serde_json::Value>,
    #[serde(default)]
    sources: Vec<serde_json::Value>,
    #[serde(default)]
    sheets: Vec<IgnoredAny>,
}

impl RawDrawing {
    fn into_info(self, encoding: Encoding) -> DesignInfo {
        let properties = match self.properties {
            Some(serde_json::Value::Object(m)) => m.into_iter().collect(),
            _ => BTreeMap::new(),
        };
        let links = self
            .sources
            .iter()
            .filter_map(|s| {
                Some(Link { component: 0, path: s.get("path")?.as_str()?.to_owned(), source: Some(source_id(s.get("id")?)?), file_sha256: None })
            })
            .collect();
        DesignInfo {
            kind: Kind::Drawing,
            encoding,
            format: self.format,
            name: self.name,
            units: None,
            metadata: Metadata::from_map(properties),
            feature_count: 0,
            param_count: 0,
            components: Vec::new(),
            occurrences: Vec::new(),
            standard_parts: BTreeMap::new(),
            links,
            blob_count: 0,
            sheet_count: self.sheets.len(),
            has_model_cache: false,
            thumbnail_png: None,
        }
    }
}

/// The subset of SolidCraft's `Document` the PDM reads. Everything else is skipped unparsed.
#[derive(Deserialize)]
struct RawDocument {
    #[serde(default)]
    format: Option<String>,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    units: Option<String>,
    #[serde(default)]
    metadata: BTreeMap<String, serde_json::Value>,
    #[serde(default)]
    features: Vec<IgnoredAny>,
    #[serde(default)]
    params: Vec<IgnoredAny>,
    #[serde(default)]
    components: Vec<Component>,
    #[serde(default)]
    occurrences: Option<Vec<Occurrence>>,
    /// Component id (as a JSON object key) → standard part.
    #[serde(default)]
    parts: BTreeMap<String, StandardPart>,
    /// Component id (as a JSON object key) → link.
    #[serde(default)]
    links: BTreeMap<String, RawLink>,
}

#[derive(Deserialize)]
struct RawLink {
    path: String,
    #[serde(default)]
    file_sha256: Option<String>,
}

impl RawDocument {
    fn into_info(self, encoding: Encoding) -> DesignInfo {
        let links = self
            .links
            .into_iter()
            .filter_map(|(id, link)| Some(Link { component: id.parse().ok()?, path: link.path, source: None, file_sha256: link.file_sha256 }))
            .collect();
        let occurrences = self.occurrences.unwrap_or_else(|| {
            self.components.iter().map(|c| Occurrence { id: 0, component: c.id, parent: c.parent }).collect()
        });
        let standard_parts = self.parts.into_iter().filter_map(|(id, p)| Some((id.parse().ok()?, p))).collect();
        DesignInfo {
            kind: Kind::Design,
            encoding,
            format: self.format,
            name: self.name,
            units: self.units,
            metadata: Metadata::from_map(self.metadata),
            feature_count: self.features.len(),
            param_count: self.params.len(),
            components: self.components,
            occurrences,
            standard_parts,
            links,
            blob_count: 0,
            sheet_count: 0,
            has_model_cache: false,
            thumbnail_png: None,
        }
    }
}

fn read_up_to(r: &mut impl Read, buf: &mut [u8]) -> std::io::Result<usize> {
    let mut filled = 0;
    while filled < buf.len() {
        match r.read(&mut buf[filled..])? {
            0 => break,
            n => filled += n,
        }
    }
    Ok(filled)
}

fn read_bounded(r: impl Read, limit: u64, what: &'static str) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    r.take(limit + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > limit {
        return Err(Error::TooLarge(what));
    }
    Ok(bytes)
}
