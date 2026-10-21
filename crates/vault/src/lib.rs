//! The PDM vault: a folder of SolidCraft designs whose versions are kept in `.pdm/`.
//!
//! ```text
//! my-designs/
//!   .pdm/vault.db          SQLite: documents, versions, cached hashes of working files
//!   .pdm/objects/ab/cdef…  every checked-in version and thumbnail, by SHA-256 (see `store`)
//!   enclosure/case.solidcraft
//!   …
//! ```
//!
//! The working files stay ordinary files that SolidCraft opens and saves as usual. A *document*
//! is a tracked file; *checking in* records its current content as the next version. Paths are
//! relative to the vault root with `/` separators.
//!
//! Single user for now: there is no check-out or locking.

mod store;

use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::Cursor;
use std::path::{Component, Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::{Connection, OptionalExtension, params};

pub use store::hash;
use store::{Store, write_atomic};

pub const PDM_DIR: &str = ".pdm";
const SCHEMA_VERSION: i32 = 1;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("I/O: {0}")]
    Io(#[from] std::io::Error),
    #[error("database: {0}")]
    Db(#[from] rusqlite::Error),
    #[error("{} is not inside a vault (no {PDM_DIR} folder found); create one with init", .0.display())]
    NotAVault(PathBuf),
    #[error("{} is already a vault", .0.display())]
    AlreadyAVault(PathBuf),
    #[error("vault was made by a newer version of this tool (schema {0})")]
    NewerSchema(i32),
    #[error("{} is outside the vault", .0.display())]
    OutsideVault(PathBuf),
    #[error("{0} is not tracked; check it in first")]
    NotTracked(String),
    #[error("{0} has no version {1}")]
    NoSuchVersion(String, u32),
    #[error("{0} has not changed since its last version")]
    Unchanged(String),
    #[error("{0} does not exist")]
    Missing(String),
    #[error("{0} already exists")]
    Exists(String),
    #[error("{0:?} is not a valid path in the vault (use folder/name; no \"..\", no hidden names)")]
    InvalidPath(String),
    #[error("folder {0} is not empty")]
    FolderNotEmpty(String),
    #[error("vault store is damaged: {0}")]
    Corrupt(String),
}

pub type Result<T> = std::result::Result<T, Error>;

/// One recorded version of a document.
#[derive(Clone, Debug, PartialEq)]
pub struct Version {
    /// 1, 2, 3… per document.
    pub number: u32,
    pub hash: String,
    pub size: u64,
    pub comment: String,
    pub author: String,
    /// Unix seconds.
    pub created_at: i64,
    /// Where the file was when this version was checked in.
    pub path: String,
    /// Properties read from the design at check-in (absent when it was not a readable design).
    pub name: Option<String>,
    pub part_number: Option<String>,
    pub revision: Option<String>,
    pub format: Option<String>,
    /// Object hash of the design's saved thumbnail.
    pub thumbnail: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Status {
    /// A design file the vault does not track yet.
    Untracked,
    /// Same content as the latest version.
    Current,
    /// Changed since the latest version.
    Modified,
    /// Tracked, but the file is gone (deleted, or moved outside the vault tool).
    Missing,
}

/// A file in the vault and how it relates to its history.
#[derive(Clone, Debug)]
pub struct Entry {
    pub path: String,
    pub status: Status,
    pub latest: Option<Version>,
    /// SHA-256 of the working file (absent when missing).
    pub hash: Option<String>,
    pub size: Option<u64>,
    pub modified: Option<SystemTime>,
    /// For an untracked file: a missing document whose latest version has exactly this
    /// content, so the file was probably moved there (see [`Vault::relink`]).
    pub moved_from: Option<String>,
}

pub struct Vault {
    root: PathBuf,
    db: Connection,
    store: Store,
}

impl Vault {
    /// Makes `root` a vault.
    pub fn init(root: impl AsRef<Path>) -> Result<Vault> {
        let root = root.as_ref().canonicalize()?;
        let pdm = root.join(PDM_DIR);
        if pdm.exists() {
            return Err(Error::AlreadyAVault(root));
        }
        fs::create_dir_all(pdm.join("objects"))?;
        Vault::open(root)
    }

    /// Opens the vault whose root is `root`.
    pub fn open(root: impl AsRef<Path>) -> Result<Vault> {
        let root = root.as_ref().canonicalize()?;
        let pdm = root.join(PDM_DIR);
        if !pdm.is_dir() {
            return Err(Error::NotAVault(root));
        }
        let db = Connection::open(pdm.join("vault.db"))?;
        db.busy_timeout(std::time::Duration::from_secs(5))?;
        // Readers (the GUI's background scan, the CLI) don't block a writer, and vice versa.
        db.query_row("PRAGMA journal_mode = WAL", [], |_| Ok(()))?;
        migrate(&db)?;
        Ok(Vault { store: Store::new(pdm.join("objects")), root, db })
    }

    /// Opens the vault containing `path` (itself or the nearest folder above with `.pdm`).
    pub fn discover(path: impl AsRef<Path>) -> Result<Vault> {
        let start = path.as_ref().canonicalize()?;
        let mut dir = Some(start.as_path());
        while let Some(d) = dir {
            if d.join(PDM_DIR).is_dir() {
                return Vault::open(d);
            }
            dir = d.parent();
        }
        Err(Error::NotAVault(start))
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// The vault path (`a/b.solidcraft`) of a file given absolute or relative to the current folder.
    pub fn relative(&self, path: impl AsRef<Path>) -> Result<String> {
        let path = path.as_ref();
        let abs = if path.is_absolute() { path.to_path_buf() } else { std::env::current_dir()?.join(path) };
        // The file itself may not exist (yet): resolve its folder, keep its name.
        let abs = normalize(&abs);
        let (dir, name) = match (abs.parent(), abs.file_name()) {
            (Some(d), Some(n)) => (d, n),
            _ => return Err(Error::OutsideVault(abs)),
        };
        let dir = dir.canonicalize().unwrap_or_else(|_| dir.to_path_buf());
        let rel = dir.join(name);
        let rel = rel.strip_prefix(&self.root).map_err(|_| Error::OutsideVault(abs.clone()))?;
        let parts: Vec<_> = rel.components().map(|c| c.as_os_str().to_string_lossy().into_owned()).collect();
        if parts.is_empty() || parts[0] == PDM_DIR {
            return Err(Error::OutsideVault(abs));
        }
        Ok(parts.join("/"))
    }

    /// The absolute path of a vault path.
    pub fn absolute(&self, path: &str) -> PathBuf {
        self.root.join(path)
    }

    /// Every design file in the vault and every tracked document, sorted by path.
    pub fn scan(&mut self) -> Result<Vec<Entry>> {
        let mut files = Vec::new();
        find_designs(&self.root, &mut files)?;
        let mut latest = self.all_latest()?;
        let mut entries = Vec::new();
        let tx = self.db.transaction()?;
        for file in files {
            let Ok(rel) = file.strip_prefix(&self.root) else { continue };
            let path = rel.components().map(|c| c.as_os_str().to_string_lossy()).collect::<Vec<_>>().join("/");
            let meta = match fs::metadata(&file) {
                Ok(m) => m,
                Err(_) => continue, // vanished during the scan
            };
            let hash = match cached_hash(&tx, &path, &file, &meta)? {
                Some(h) => h,
                None => continue,
            };
            let latest = latest.remove(&path);
            let status = match &latest {
                None => Status::Untracked,
                Some(v) if v.hash == hash => Status::Current,
                Some(_) => Status::Modified,
            };
            entries.push(Entry {
                path,
                status,
                latest,
                hash: Some(hash),
                size: Some(meta.len()),
                modified: meta.modified().ok(),
                moved_from: None,
            });
        }
        tx.commit()?;

        // What is left of `latest` is missing. An untracked file with a missing document's
        // exact content was most likely moved.
        let missing_by_hash: HashMap<&str, &str> =
            latest.iter().map(|(path, v)| (v.hash.as_str(), path.as_str())).collect();
        for e in entries.iter_mut().filter(|e| e.status == Status::Untracked) {
            e.moved_from = e.hash.as_deref().and_then(|h| missing_by_hash.get(h)).map(|p| p.to_string());
        }
        entries.extend(latest.into_iter().map(|(path, v)| Entry {
            path,
            status: Status::Missing,
            latest: Some(v),
            hash: None,
            size: None,
            modified: None,
            moved_from: None,
        }));
        entries.sort_by(|a, b| a.path.cmp(&b.path));
        Ok(entries)
    }

    /// Records the file's current content as its next version (tracking it if it is new).
    pub fn check_in(&mut self, path: &str, comment: &str) -> Result<Version> {
        let bytes = self.read_working(path)?;
        let hash = hash(&bytes);
        if let Some(doc) = self.document_id(path)? {
            if self.latest(doc)?.is_some_and(|v| v.hash == hash) {
                return Err(Error::Unchanged(path.to_owned()));
            }
        }
        self.record(path, &bytes, &hash, comment)
    }

    /// Writes a new design file (never over an existing one) and checks it in as v1.
    pub fn add_new(&mut self, path: &str, bytes: &[u8], comment: &str) -> Result<Version> {
        check_path(path)?;
        let file = self.absolute(path);
        if let Some(dir) = file.parent() {
            fs::create_dir_all(dir)?;
        }
        let mut out = fs::OpenOptions::new().write(true).create_new(true).open(&file).map_err(|e| match e.kind() {
            std::io::ErrorKind::AlreadyExists => Error::Exists(path.to_owned()),
            _ => e.into(),
        })?;
        std::io::Write::write_all(&mut out, bytes)?;
        out.sync_all()?;
        self.check_in(path, comment)
    }

    /// Every version of a document, newest first.
    pub fn history(&self, path: &str) -> Result<Vec<Version>> {
        let doc = self.document_id(path)?.ok_or_else(|| Error::NotTracked(path.to_owned()))?;
        let mut stmt = self.db.prepare(&format!("{VERSION_SELECT} WHERE document_id = ?1 ORDER BY number DESC"))?;
        let versions = stmt.query_map([doc], version_from_row)?.collect::<rusqlite::Result<_>>()?;
        Ok(versions)
    }

    /// Puts version `number`'s content back into the working file, recorded as a new version
    /// ("Restored vN"), so history only grows. Changes not yet checked in are checked in first
    /// rather than lost.
    pub fn restore(&mut self, path: &str, number: u32) -> Result<Version> {
        let doc = self.document_id(path)?.ok_or_else(|| Error::NotTracked(path.to_owned()))?;
        let target = self.version(doc, number)?.ok_or_else(|| Error::NoSuchVersion(path.to_owned(), number))?;
        let file = self.absolute(path);

        let current = match fs::read(&file) {
            Ok(bytes) => Some(bytes),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
            Err(e) => return Err(e.into()),
        };
        if let Some(bytes) = &current {
            let h = hash(bytes);
            if self.latest(doc)?.is_none_or(|v| v.hash != h) {
                self.record(path, bytes, &h, &format!("Automatic check-in before restoring v{number}"))?;
            }
        }
        let latest = self.latest(doc)?.expect("a tracked document has a version");
        if latest.hash == target.hash && current.is_some() {
            return Ok(latest); // already that content
        }

        let bytes = self.store.get(&target.hash)?;
        if let Some(dir) = file.parent() {
            fs::create_dir_all(dir)?;
        }
        let perms = fs::metadata(&file).ok().map(|m| m.permissions());
        write_atomic(&file, &bytes, perms.as_ref())?;
        self.record(path, &bytes, &target.hash, &format!("Restored v{number}"))
    }

    /// The stored bytes of a version (to save a copy of it elsewhere).
    pub fn version_bytes(&self, version: &Version) -> Result<Vec<u8>> {
        self.store.get(&version.hash)
    }

    /// A version's thumbnail PNG, if its design carried one.
    pub fn thumbnail(&self, version: &Version) -> Result<Option<Vec<u8>>> {
        version.thumbnail.as_deref().map(|h| self.store.get(h)).transpose()
    }

    /// Moves or renames a file inside the vault, keeping its history.
    pub fn rename(&mut self, from: &str, to: &str) -> Result<()> {
        check_path(to)?;
        let (src, dst) = (self.absolute(from), self.absolute(to));
        if !src.is_file() {
            return Err(Error::Missing(from.to_owned()));
        }
        if dst.exists() || self.document_id(to)?.is_some() {
            return Err(Error::Exists(to.to_owned()));
        }
        if let Some(dir) = dst.parent() {
            fs::create_dir_all(dir)?;
        }
        fs::rename(&src, &dst)?;
        self.db.execute("UPDATE documents SET path = ?2 WHERE path = ?1", params![from, to])?;
        self.db.execute("DELETE FROM stat_cache WHERE path = ?1", [from])?;
        Ok(())
    }

    /// Moves a design into `folder` ("" for the top level), keeping its name and history.
    /// Returns the new path.
    pub fn move_to_folder(&mut self, path: &str, folder: &str) -> Result<String> {
        let name = path.rsplit_once('/').map_or(path, |(_, n)| n);
        let to = if folder.is_empty() { name.to_owned() } else { format!("{folder}/{name}") };
        if to != path {
            self.rename(path, &to)?;
        }
        Ok(to)
    }

    /// Creates a folder (and missing parents).
    pub fn create_folder(&self, folder: &str) -> Result<()> {
        check_path(folder)?;
        let dir = self.absolute(folder);
        if dir.exists() {
            return Err(Error::Exists(folder.to_owned()));
        }
        fs::create_dir_all(dir)?;
        Ok(())
    }

    /// Removes an empty folder.
    pub fn remove_folder(&self, folder: &str) -> Result<()> {
        check_path(folder)?;
        let dir = self.absolute(folder);
        if fs::read_dir(&dir)?.next().is_some() {
            return Err(Error::FolderNotEmpty(folder.to_owned()));
        }
        fs::remove_dir(dir)?;
        Ok(())
    }

    /// Every folder in the vault (also empty ones), as vault paths, sorted.
    pub fn folders(&self) -> Result<Vec<String>> {
        let mut out = Vec::new();
        let mut stack = vec![(self.root.clone(), String::new())];
        while let Some((dir, prefix)) = stack.pop() {
            for entry in fs::read_dir(&dir)? {
                let entry = entry?;
                let name = entry.file_name().to_string_lossy().into_owned();
                if name.starts_with('.') || !entry.file_type()?.is_dir() {
                    continue;
                }
                let path = if prefix.is_empty() { name } else { format!("{prefix}/{name}") };
                stack.push((entry.path(), path.clone()));
                out.push(path);
            }
        }
        out.sort();
        Ok(out)
    }

    /// Attaches a missing document's history to the file now at `to` (moved outside the tool).
    pub fn relink(&mut self, missing: &str, to: &str) -> Result<()> {
        check_path(to)?;
        if self.document_id(missing)?.is_none() {
            return Err(Error::NotTracked(missing.to_owned()));
        }
        if self.absolute(missing).exists() {
            return Err(Error::Exists(missing.to_owned()));
        }
        if !self.absolute(to).is_file() {
            return Err(Error::Missing(to.to_owned()));
        }
        if self.document_id(to)?.is_some() {
            return Err(Error::Exists(to.to_owned()));
        }
        self.db.execute("UPDATE documents SET path = ?2 WHERE path = ?1", params![missing, to])?;
        Ok(())
    }

    fn read_working(&self, path: &str) -> Result<Vec<u8>> {
        fs::read(self.absolute(path)).map_err(|e| match e.kind() {
            std::io::ErrorKind::NotFound => Error::Missing(path.to_owned()),
            _ => e.into(),
        })
    }

    fn record(&mut self, path: &str, bytes: &[u8], hash: &str, comment: &str) -> Result<Version> {
        self.store.put(hash, bytes)?;
        let info = scformat::read(Cursor::new(bytes)).ok();
        let thumbnail = match info.as_ref().and_then(|i| i.thumbnail_png.as_deref()) {
            Some(png) => {
                let h = store::hash(png);
                self.store.put(&h, png)?;
                Some(h)
            }
            None => None,
        };
        let metadata = info.as_ref().map(|i| &i.metadata);
        let version = Version {
            number: 0,
            hash: hash.to_owned(),
            size: bytes.len() as u64,
            comment: comment.trim().to_owned(),
            author: author(),
            created_at: now(),
            path: path.to_owned(),
            name: info.as_ref().and_then(|i| i.name.clone()),
            part_number: metadata.and_then(|m| m.part_number.clone()),
            revision: metadata.and_then(|m| m.revision.clone()),
            format: info.as_ref().and_then(|i| i.format.clone()),
            thumbnail,
        };

        let tx = self.db.transaction()?;
        let doc: i64 = match tx.query_row("SELECT id FROM documents WHERE path = ?1", [path], |r| r.get(0)).optional()? {
            Some(id) => id,
            None => {
                tx.execute("INSERT INTO documents (path, created_at) VALUES (?1, ?2)", params![path, version.created_at])?;
                tx.last_insert_rowid()
            }
        };
        let number: u32 =
            tx.query_row("SELECT COALESCE(MAX(number), 0) + 1 FROM versions WHERE document_id = ?1", [doc], |r| r.get(0))?;
        tx.execute(
            "INSERT INTO versions (document_id, number, hash, size, comment, author, created_at, path,
                                   name, part_number, revision, format, thumbnail)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
            params![
                doc,
                number,
                version.hash,
                version.size as i64,
                version.comment,
                version.author,
                version.created_at,
                version.path,
                version.name,
                version.part_number,
                version.revision,
                version.format,
                version.thumbnail,
            ],
        )?;
        tx.commit()?;
        Ok(Version { number, ..version })
    }

    fn document_id(&self, path: &str) -> Result<Option<i64>> {
        Ok(self.db.query_row("SELECT id FROM documents WHERE path = ?1", [path], |r| r.get(0)).optional()?)
    }

    fn latest(&self, doc: i64) -> Result<Option<Version>> {
        let sql = format!("{VERSION_SELECT} WHERE document_id = ?1 ORDER BY number DESC LIMIT 1");
        Ok(self.db.query_row(&sql, [doc], version_from_row).optional()?)
    }

    fn version(&self, doc: i64, number: u32) -> Result<Option<Version>> {
        let sql = format!("{VERSION_SELECT} WHERE document_id = ?1 AND number = ?2");
        Ok(self.db.query_row(&sql, params![doc, number], version_from_row).optional()?)
    }

    /// Every document's latest version, by the document's current path.
    fn all_latest(&self) -> Result<HashMap<String, Version>> {
        let mut stmt = self.db.prepare(
            "SELECT d.path, v.number, v.hash, v.size, v.comment, v.author, v.created_at, v.path,
                    v.name, v.part_number, v.revision, v.format, v.thumbnail
             FROM documents d JOIN versions v ON v.document_id = d.id
             WHERE v.number = (SELECT MAX(number) FROM versions WHERE document_id = d.id)",
        )?;
        let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, version_from_row_at(r, 1)?)))?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }
}

const VERSION_SELECT: &str = "SELECT number, hash, size, comment, author, created_at, path,
                                     name, part_number, revision, format, thumbnail FROM versions";

fn version_from_row(r: &rusqlite::Row) -> rusqlite::Result<Version> {
    version_from_row_at(r, 0)
}

fn version_from_row_at(r: &rusqlite::Row, i: usize) -> rusqlite::Result<Version> {
    Ok(Version {
        number: r.get(i)?,
        hash: r.get(i + 1)?,
        size: r.get::<_, i64>(i + 2)? as u64,
        comment: r.get(i + 3)?,
        author: r.get(i + 4)?,
        created_at: r.get(i + 5)?,
        path: r.get(i + 6)?,
        name: r.get(i + 7)?,
        part_number: r.get(i + 8)?,
        revision: r.get(i + 9)?,
        format: r.get(i + 10)?,
        thumbnail: r.get(i + 11)?,
    })
}

fn migrate(db: &Connection) -> Result<()> {
    let version: i32 = db.query_row("PRAGMA user_version", [], |r| r.get(0))?;
    if version > SCHEMA_VERSION {
        return Err(Error::NewerSchema(version));
    }
    if version < 1 {
        db.execute_batch(
            "BEGIN;
             CREATE TABLE documents (
                 id         INTEGER PRIMARY KEY,
                 path       TEXT NOT NULL UNIQUE,
                 created_at INTEGER NOT NULL
             );
             CREATE TABLE versions (
                 id          INTEGER PRIMARY KEY,
                 document_id INTEGER NOT NULL REFERENCES documents(id),
                 number      INTEGER NOT NULL,
                 hash        TEXT NOT NULL,
                 size        INTEGER NOT NULL,
                 comment     TEXT NOT NULL,
                 author      TEXT NOT NULL,
                 created_at  INTEGER NOT NULL,
                 path        TEXT NOT NULL,
                 name        TEXT,
                 part_number TEXT,
                 revision    TEXT,
                 format      TEXT,
                 thumbnail   TEXT,
                 UNIQUE (document_id, number)
             );
             -- Hashes of working files, valid while their size and modification time match.
             CREATE TABLE stat_cache (
                 path     TEXT PRIMARY KEY,
                 size     INTEGER NOT NULL,
                 mtime_ns INTEGER NOT NULL,
                 hash     TEXT NOT NULL
             );
             PRAGMA user_version = 1;
             COMMIT;",
        )?;
    }
    Ok(())
}

/// The file's hash, from the cache when its size and modification time are unchanged.
/// `None` when the file vanished.
fn cached_hash(db: &Connection, path: &str, file: &Path, meta: &fs::Metadata) -> Result<Option<String>> {
    let size = meta.len() as i64;
    let mtime = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_nanos() as i64);
    let cached: Option<String> = db
        .query_row(
            "SELECT hash FROM stat_cache WHERE path = ?1 AND size = ?2 AND mtime_ns = ?3",
            params![path, size, mtime],
            |r| r.get(0),
        )
        .optional()?;
    if cached.is_some() {
        return Ok(cached);
    }
    // Stat was taken before reading: if the file changes after this read, its new modification
    // time no longer matches the cached one and it is hashed again.
    let bytes = match fs::read(file) {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e.into()),
    };
    let h = hash(&bytes);
    db.execute(
        "INSERT OR REPLACE INTO stat_cache (path, size, mtime_ns, hash) VALUES (?1, ?2, ?3, ?4)",
        params![path, size, mtime, h],
    )?;
    Ok(Some(h))
}

/// A vault path must be `name` or `folder/…/name`: no empty, `.`, `..` or hidden (`.pdm`)
/// components, no backslashes.
fn check_path(path: &str) -> Result<()> {
    let ok = !path.is_empty()
        && !path.contains('\\')
        && path.split('/').all(|c| !c.is_empty() && !c.starts_with('.'));
    if ok { Ok(()) } else { Err(Error::InvalidPath(path.to_owned())) }
}

/// Whether `path` names a SolidCraft design (`.solidcraft`, or the legacy `.solvecraft`).
pub fn is_design(path: &Path) -> bool {
    matches!(path.extension().and_then(|e| e.to_str()), Some("solidcraft" | "solvecraft"))
}

/// Every design file below `dir`, skipping hidden files and folders (including `.pdm`).
pub fn find_designs(dir: &Path, out: &mut Vec<PathBuf>) -> std::io::Result<()> {
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        if entry.file_name().to_string_lossy().starts_with('.') {
            continue;
        }
        let path = entry.path();
        if entry.file_type()?.is_dir() {
            find_designs(&path, out)?;
        } else if is_design(&path) {
            out.push(path);
        }
    }
    Ok(())
}

/// Folders below the vault root that contain designs (for a folder tree), sorted.
pub fn design_folders(entries: &[Entry]) -> Vec<String> {
    let mut set = HashSet::new();
    for e in entries {
        let mut p = e.path.as_str();
        while let Some(i) = p.rfind('/') {
            p = &p[..i];
            if !set.insert(p.to_owned()) {
                break;
            }
        }
    }
    let mut v: Vec<_> = set.into_iter().collect();
    v.sort();
    v
}

fn normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for c in path.components() {
        match c {
            Component::ParentDir => {
                out.pop();
            }
            Component::CurDir => {}
            c => out.push(c),
        }
    }
    out
}

fn author() -> String {
    std::env::var("USER").or_else(|_| std::env::var("USERNAME")).unwrap_or_else(|_| "unknown".into())
}

fn now() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64)
}
