//! The object store: `.pdm/objects/<first two hex digits>/<rest of the SHA-256>`.
//!
//! Every checked-in file version and thumbnail is one immutable object named by the SHA-256 of
//! its bytes, so identical content is stored once and corruption is detectable.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use sha2::{Digest, Sha256};

use crate::{Error, Result};

pub(crate) struct Store {
    dir: PathBuf,
}

/// Lowercase hex SHA-256.
pub fn hash(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut hex = String::with_capacity(64);
    for b in digest {
        hex.push_str(&format!("{b:02x}"));
    }
    hex
}

impl Store {
    pub(crate) fn new(dir: PathBuf) -> Self {
        Store { dir }
    }

    fn path(&self, hash: &str) -> PathBuf {
        self.dir.join(&hash[..2]).join(&hash[2..])
    }

    /// Stores `bytes` (whose hash is `hash`) unless already stored.
    pub(crate) fn put(&self, hash: &str, bytes: &[u8]) -> Result<()> {
        let path = self.path(hash);
        if path.exists() {
            return Ok(());
        }
        let dir = path.parent().expect("object paths have a folder");
        fs::create_dir_all(dir)?;
        write_atomic(&path, bytes, None)?;
        let mut perms = fs::metadata(&path)?.permissions();
        perms.set_readonly(true);
        fs::set_permissions(&path, perms)?;
        Ok(())
    }

    pub(crate) fn contains(&self, hash: &str) -> bool {
        self.path(hash).is_file()
    }

    /// Reads an object back, checking its hash.
    pub(crate) fn get(&self, hash: &str) -> Result<Vec<u8>> {
        let bytes = fs::read(self.path(hash)).map_err(|e| match e.kind() {
            std::io::ErrorKind::NotFound => Error::Corrupt(format!("object {hash} is missing")),
            _ => e.into(),
        })?;
        if self::hash(&bytes) != hash {
            return Err(Error::Corrupt(format!("object {hash} does not match its hash")));
        }
        Ok(bytes)
    }
}

/// Writes `path` so that a crash leaves either the old file or the new one: a temporary file in
/// the same folder, flushed, then renamed over. `like` gives the permissions to keep.
pub(crate) fn write_atomic(path: &Path, bytes: &[u8], like: Option<&fs::Permissions>) -> Result<()> {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let dir = path.parent().unwrap_or(Path::new("."));
    let name = path.file_name().map(|n| n.to_string_lossy()).unwrap_or_default();
    let tmp = dir.join(format!(
        ".{name}.pdm-tmp-{}-{}",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    let result = (|| {
        let mut file = fs::File::create(&tmp)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        if let Some(perms) = like {
            fs::set_permissions(&tmp, perms.clone())?;
        }
        fs::rename(&tmp, path)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    Ok(result?)
}
