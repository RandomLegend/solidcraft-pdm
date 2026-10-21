//! Scanning the vault off the UI thread: hashing changed files and reading their properties.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc::{Receiver, channel};

use scformat::DesignInfo;
use vault::{Entry, Vault};

/// A design file's properties, or why it could not be read.
pub type Info = Arc<Result<DesignInfo, String>>;

pub struct Scan {
    pub entries: Vec<Entry>,
    /// Every folder, also empty ones.
    pub folders: Vec<String>,
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
            let mut infos = HashMap::new();
            for e in &entries {
                let Some(hash) = &e.hash else { continue };
                if known.contains(hash) || infos.contains_key(hash) {
                    continue;
                }
                let info = scformat::read_file(vault.absolute(&e.path)).map_err(|e| e.to_string());
                infos.insert(hash.clone(), Arc::new(info));
            }
            Ok::<_, vault::Error>(Scan { entries, folders, infos })
        })();
        let _ = tx.send(result.map_err(|e| e.to_string()));
        ctx.request_repaint();
    });
    rx
}
