//! Scanning the vault off the UI thread: hashing changed files and reading their properties.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc::{Receiver, channel};

use scformat::DesignInfo;
use vault::{Entry, OtherFile, Vault};

/// A design file's properties, or why it could not be read.
pub type Info = Arc<Result<DesignInfo, String>>;

pub struct Scan {
    pub entries: Vec<Entry>,
    /// Every folder, also empty ones.
    pub folders: Vec<String>,
    /// Reference images, with the key of their thumbnail.
    pub images: Vec<ImageFile>,
    /// Thumbnails of images whose key was not in `known` (`None`: not a readable image).
    pub image_thumbs: HashMap<String, Option<Thumb>>,
    /// Properties of working files whose content was not in `known`, by content hash.
    pub infos: HashMap<String, Info>,
}

/// Scans the vault at `root` on its own thread (with its own database connection). Files whose
/// content hash is in `known` are not read again.
pub fn spawn(root: PathBuf, known: HashSet<String>, ctx: eframe::egui::Context) -> Receiver<Result<Scan, String>> {
    let (tx, rx) = channel();
    std::thread::spawn(move || {
        let result = (|| {
            let mut vault = Vault::open(&root)?;
            let entries = vault.scan()?;
            let folders = vault.folders()?;
            let images: Vec<ImageFile> = vault.images()?.into_iter().map(ImageFile::new).collect();
            let mut image_thumbs = HashMap::new();
            for image in &images {
                if !known.contains(&image.key) && !image_thumbs.contains_key(&image.key) {
                    image_thumbs.insert(image.key.clone(), thumbnail(&vault.absolute(&image.file.path)));
                }
            }
            let mut infos = HashMap::new();
            for e in &entries {
                let Some(hash) = &e.hash else { continue };
                if known.contains(hash) || infos.contains_key(hash) {
                    continue;
                }
                let info = scformat::read_file(vault.absolute(&e.path)).map_err(|e| e.to_string());
                infos.insert(hash.clone(), Arc::new(info));
            }
            Ok::<_, vault::Error>(Scan { entries, folders, images, image_thumbs, infos })
        })();
        let _ = tx.send(result.map_err(|e| e.to_string()));
        ctx.request_repaint();
    });
    rx
}

/// A reference image and the key of its thumbnail (its path, size and modification time, so an
/// edited image gets a new one).
#[derive(Clone)]
pub struct ImageFile {
    pub file: OtherFile,
    pub key: String,
}

impl ImageFile {
    fn new(file: OtherFile) -> Self {
        let mtime = file
            .modified
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map_or(0, |d| d.as_nanos());
        let key = format!("img:{}:{}:{mtime}", file.path, file.size);
        ImageFile { file, key }
    }
}

/// A downscaled PNG of an image and the image's own size.
#[derive(Clone)]
pub struct Thumb {
    pub png: Arc<[u8]>,
    pub width: u32,
    pub height: u32,
}

/// Decodes an image and shrinks it to at most 640×400 (full-size photos would take tens of MB
/// each as textures).
fn thumbnail(path: &std::path::Path) -> Option<Thumb> {
    let image = image::ImageReader::open(path).ok()?.with_guessed_format().ok()?.decode().ok()?;
    let (width, height) = (image.width(), image.height());
    let mut png = Vec::new();
    image.thumbnail(640, 400).write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png).ok()?;
    Some(Thumb { png: png.into(), width, height })
}
