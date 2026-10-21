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
//! This crate reads only what a PDM needs: the design properties (`metadata`), name, units,
//! counts, components, links to other design files and the thumbnail. It never evaluates
//! geometry and does not depend on SolidCraft's crates, so it keeps working across SolidCraft
//! releases: unknown fields are ignored.

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
}

/// A component of the design (`parent` 0 is the root).
#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct Component {
    pub id: u64,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub parent: u64,
}

/// A component linked to another design file.
#[derive(Clone, Debug, PartialEq)]
pub struct Link {
    pub component: u64,
    /// Relative to the linking design's folder, `/` separated (as SolidCraft stores it).
    pub path: String,
}

/// What the PDM knows about one design file.
#[derive(Clone, Debug)]
pub struct DesignInfo {
    pub encoding: Encoding,
    /// `solidcraft/N`; `None` for designs older than the format string.
    pub format: Option<String>,
    pub name: Option<String>,
    pub units: Option<String>,
    pub metadata: Metadata,
    pub feature_count: usize,
    pub param_count: usize,
    pub components: Vec<Component>,
    pub links: Vec<Link>,
    /// Entries under `blobs/` (containers only).
    pub blob_count: usize,
    /// Whether the container carries SolidCraft's evaluated model cache.
    pub has_model_cache: bool,
    /// The portable 240×150 PNG, when the container carries one.
    pub thumbnail_png: Option<Vec<u8>>,
}

impl DesignInfo {
    /// The `N` of `solidcraft/N`.
    pub fn format_version(&self) -> Option<u32> {
        self.format.as_deref()?.strip_prefix("solidcraft/")?.parse().ok()
    }
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
        let doc: RawDocument = serde_json::from_slice(&bytes)?;
        Ok(doc.into_info(Encoding::Json))
    }
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
    /// Component id (as a JSON object key) → link.
    #[serde(default)]
    links: BTreeMap<String, RawLink>,
}

#[derive(Deserialize)]
struct RawLink {
    path: String,
}

impl RawDocument {
    fn into_info(self, encoding: Encoding) -> DesignInfo {
        let links = self
            .links
            .into_iter()
            .filter_map(|(id, link)| Some(Link { component: id.parse().ok()?, path: link.path }))
            .collect();
        DesignInfo {
            encoding,
            format: self.format,
            name: self.name,
            units: self.units,
            metadata: Metadata::from_map(self.metadata),
            feature_count: self.features.len(),
            param_count: self.params.len(),
            components: self.components,
            links,
            blob_count: 0,
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
