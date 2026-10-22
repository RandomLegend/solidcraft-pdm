//! Scanning the vault off the UI thread: hashing changed files and reading their properties.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::mpsc::{Receiver, channel};
use std::sync::{Arc, Mutex};

use scformat::DesignInfo;
use remote::{RemoteStatus, Workspace};
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
    /// In a workspace: how each design and image relates to the server, by path.
    pub remote: HashMap<String, RemoteInfo>,
    /// In a workspace whose server could not be reached: why (the rest is as last seen).
    pub offline: Option<String>,
    /// Moves by others that the workspace followed.
    pub followed_moves: Vec<(String, String)>,
    /// Designs deleted on the server whose unchanged copies were removed here.
    pub followed_deletes: Vec<String>,
}

/// A workspace file and the server.
#[derive(Clone, Debug)]
pub struct RemoteInfo {
    pub status: RemoteStatus,
    pub lock: Option<protocol::LockInfo>,
    /// The latest version on the server, and the version fetched here.
    pub server_version: Option<u32>,
    pub base_version: Option<u32>,
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
            Ok::<_, vault::Error>(Scan { entries, folders, images, image_thumbs, infos, remote: HashMap::new(), offline: None, followed_moves: Vec::new(), followed_deletes: Vec::new() })
        })();
        let _ = tx.send(result.map_err(|e| e.to_string()));
        ctx.request_repaint();
    });
    rx
}

/// Scans a workspace on its own thread: the server's state and the local files. An expired login
/// is reported as [`LOGIN_NEEDED`].
pub fn spawn_remote(ws: Arc<Mutex<Workspace>>, known: HashSet<String>, ctx: eframe::egui::Context) -> Receiver<Result<Scan, String>> {
    let (tx, rx) = channel();
    std::thread::spawn(move || {
        let result = scan_remote(&ws, &known);
        let _ = tx.send(result);
        ctx.request_repaint();
    });
    rx
}

/// What the window shows for "log in again".
pub const LOGIN_NEEDED: &str = "The login has expired: log in again.";

fn scan_remote(ws: &Arc<Mutex<Workspace>>, known: &HashSet<String>) -> Result<Scan, String> {
    let (status, root) = {
        let mut ws = ws.lock().unwrap_or_else(|e| e.into_inner());
        let status = ws.status().map_err(|e| if e.needs_login() { LOGIN_NEEDED.to_owned() } else { e.to_string() })?;
        (status, ws.root().to_path_buf())
    };
    let mut remote = HashMap::new();
    let mut entries = Vec::new();
    let mut infos = HashMap::new();
    for e in status.designs {
        let latest = e.server.as_ref().map(|d| crate::backend::version(d.latest.clone()));
        remote.insert(e.path.clone(), RemoteInfo {
            status: e.status.clone(),
            lock: e.server.as_ref().and_then(|d| d.lock.clone()),
            server_version: e.server.as_ref().map(|d| d.latest.number),
            base_version: e.base.as_ref().map(|b| b.version),
        });
        if let Some(hash) = &e.local_hash {
            if !known.contains(hash) && !infos.contains_key(hash) {
                let info = scformat::read_file(root.join(&e.path)).map_err(|e| e.to_string());
                infos.insert(hash.clone(), Arc::new(info));
            }
        }
        entries.push(Entry {
            status: crate::backend::vault_status(&e.status),
            latest,
            hash: e.local_hash,
            size: e.size,
            modified: e.modified,
            moved_from: None,
            state: e.server.as_ref().map(|d| crate::backend::state(d.state)),
            release: e.server.as_ref().and_then(|d| d.release.clone()).map(crate::backend::release),
            path: e.path,
        });
    }
    let mut images = Vec::new();
    let mut image_thumbs = HashMap::new();
    for i in status.images {
        let file = ImageFile::new(OtherFile { path: i.path.clone(), size: i.size.unwrap_or(0), modified: i.modified });
        let status = match (&i.server, &i.local) {
            (Some(_), None) => RemoteStatus::NotDownloaded,
            (None, Some(_)) => RemoteStatus::LocalOnly,
            (Some(s), Some(l)) if s != l => RemoteStatus::NewerOnServer,
            _ => RemoteStatus::UpToDate,
        };
        remote.insert(i.path.clone(), RemoteInfo { status, lock: None, server_version: None, base_version: None });
        if i.local.is_some() && !known.contains(&file.key) && !image_thumbs.contains_key(&file.key) {
            image_thumbs.insert(file.key.clone(), thumbnail(&root.join(&i.path)));
        }
        images.push(file);
    }
    Ok(Scan {
        entries,
        folders: status.folders,
        images,
        image_thumbs,
        infos,
        remote,
        offline: status.offline,
        followed_moves: status.followed_moves,
        followed_deletes: status.followed_deletes,
    })
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
