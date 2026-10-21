//! What a vault needs when it is a server's storage (`apps/scpdm-server`): check-out locks,
//! checking in bytes that came over the network, and objects by hash.
//!
//! On a server, a vault's working files are the latest versions (its "head"); people work in
//! workspaces of their own and check in through the server.

use std::collections::HashMap;
use std::fs;

use rusqlite::{OptionalExtension, params};

use crate::store::write_atomic;
use crate::{Error, Result, VERSION_SELECT, Vault, Version, check_path, store, version_from_row};

/// Who has a design checked out, and since when (Unix seconds).
#[derive(Clone, Debug, PartialEq)]
pub struct Lock {
    pub user: String,
    pub since: i64,
}

/// A vault in numbers, for a server's overview.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Stats {
    pub designs: u64,
    pub versions: u64,
    pub releases: u64,
    pub checked_out: u64,
    /// The versions' bytes, each distinct content counted once (as stored).
    pub stored_bytes: u64,
    /// The newest version's time (Unix seconds).
    pub last_change: Option<i64>,
}

/// Whether `hash` is a lowercase hex SHA-256 (and so a safe object name).
pub fn is_hash(hash: &str) -> bool {
    hash.len() == 64 && hash.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

impl Vault {
    /// A vault path from elsewhere (a request), checked: `folder/…/name`, no `..` or hidden
    /// parts; surrounding `/` removed.
    pub fn relative_path(path: &str) -> Result<String> {
        let path = path.trim_matches('/');
        check_path(path)?;
        Ok(path.to_owned())
    }

    /// Checks a design out to `user`. Checking out again by the same user is fine.
    pub fn lock(&mut self, path: &str, user: &str) -> Result<Lock> {
        let doc = self.document_id(path)?.ok_or_else(|| Error::NotTracked(path.to_owned()))?;
        if let Some(lock) = self.lock_of(path)? {
            if lock.user != user {
                return Err(Error::Locked(path.to_owned(), lock.user));
            }
            return Ok(lock);
        }
        let lock = Lock { user: user.to_owned(), since: crate::now() };
        self.db.execute("INSERT INTO locks (document_id, user, since) VALUES (?1, ?2, ?3)", params![doc, lock.user, lock.since])?;
        Ok(lock)
    }

    /// Releases `user`'s check-out (`force`: anyone's, for administrators).
    pub fn unlock(&mut self, path: &str, user: &str, force: bool) -> Result<()> {
        let doc = self.document_id(path)?.ok_or_else(|| Error::NotTracked(path.to_owned()))?;
        if let Some(lock) = self.lock_of(path)? {
            if lock.user != user && !force {
                return Err(Error::Locked(path.to_owned(), lock.user));
            }
        }
        self.db.execute("DELETE FROM locks WHERE document_id = ?1", [doc])?;
        Ok(())
    }

    pub fn lock_of(&self, path: &str) -> Result<Option<Lock>> {
        Ok(self
            .db
            .query_row(
                "SELECT l.user, l.since FROM locks l JOIN documents d ON d.id = l.document_id WHERE d.path = ?1",
                [path],
                |r| Ok(Lock { user: r.get(0)?, since: r.get(1)? }),
            )
            .optional()?)
    }

    /// Every check-out, by path.
    pub fn locks(&self) -> Result<HashMap<String, Lock>> {
        let mut stmt = self.db.prepare("SELECT d.path, l.user, l.since FROM locks l JOIN documents d ON d.id = l.document_id")?;
        let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, Lock { user: r.get(1)?, since: r.get(2)? })))?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Every document's id, by path. Ids stay the same when a document moves, so workspaces can
    /// follow moves.
    pub fn document_ids(&self) -> Result<HashMap<String, i64>> {
        let mut stmt = self.db.prepare("SELECT path, id FROM documents")?;
        let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Writes `bytes` as the file at `path` and checks it in (tracking it if it is new).
    pub fn check_in_bytes(&mut self, path: &str, bytes: &[u8], comment: &str) -> Result<Version> {
        check_path(path)?;
        let file = self.absolute(path);
        if let Some(dir) = file.parent() {
            fs::create_dir_all(dir)?;
        }
        let perms = fs::metadata(&file).ok().map(|m| m.permissions());
        write_atomic(&file, bytes, perms.as_ref())?;
        self.check_in(path, comment)
    }

    /// Writes a plain (not versioned) file, such as a reference image.
    pub fn put_file(&self, path: &str, bytes: &[u8]) -> Result<()> {
        check_path(path)?;
        let file = self.absolute(path);
        if let Some(dir) = file.parent() {
            fs::create_dir_all(dir)?;
        }
        write_atomic(&file, bytes, None)
    }

    /// Removes a plain (not versioned) file.
    pub fn remove_file(&self, path: &str) -> Result<()> {
        check_path(path)?;
        if self.document_id(path)?.is_some() {
            return Err(Error::Exists(format!("{path} is a tracked design")));
        }
        fs::remove_file(self.absolute(path)).map_err(|e| match e.kind() {
            std::io::ErrorKind::NotFound => Error::Missing(path.to_owned()),
            _ => e.into(),
        })
    }

    /// An object (a version's bytes, a thumbnail) by its SHA-256.
    pub fn object(&self, hash: &str) -> Result<Vec<u8>> {
        if !is_hash(hash) {
            return Err(Error::Corrupt(format!("{hash:?} is not an object name")));
        }
        self.store.get(hash)
    }

    pub fn has_object(&self, hash: &str) -> bool {
        is_hash(hash) && self.store.contains(hash)
    }

    /// Stores bytes as an object; returns its SHA-256.
    pub fn put_object(&self, bytes: &[u8]) -> Result<String> {
        let hash = store::hash(bytes);
        self.store.put(&hash, bytes)?;
        Ok(hash)
    }

    pub fn stats(&self) -> Result<Stats> {
        let count = |sql: &str| -> Result<u64> { Ok(self.db.query_row(sql, [], |r| r.get::<_, i64>(0))? as u64) };
        Ok(Stats {
            designs: count("SELECT COUNT(*) FROM documents")?,
            versions: count("SELECT COUNT(*) FROM versions")?,
            releases: count("SELECT COUNT(*) FROM releases")?,
            checked_out: count("SELECT COUNT(*) FROM locks")?,
            stored_bytes: count("SELECT COALESCE(SUM(size), 0) FROM (SELECT DISTINCT hash, size FROM versions)")?,
            last_change: self.db.query_row("SELECT MAX(created_at) FROM versions", [], |r| r.get(0))?,
        })
    }

    /// The newest versions of any design, newest first.
    pub fn recent_versions(&self, limit: usize) -> Result<Vec<Version>> {
        let mut stmt = self.db.prepare(&format!("{VERSION_SELECT} ORDER BY created_at DESC, id DESC LIMIT ?1"))?;
        let rows = stmt.query_map([limit as i64], version_from_row)?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// How many versions were made per day (Unix day number, UTC) since `since`.
    pub fn versions_per_day(&self, since: i64) -> Result<Vec<(i64, u64)>> {
        let mut stmt = self.db.prepare("SELECT created_at / 86400, COUNT(*) FROM versions WHERE created_at >= ?1 GROUP BY 1 ORDER BY 1")?;
        let rows = stmt.query_map([since], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)? as u64)))?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// The designs (besides `from` itself) whose links a move of `from` to `to` would update.
    pub fn move_affects(&self, from: &str, to: &str) -> Result<Vec<String>> {
        Ok(self.link_changes(from, to)?.into_iter().map(|c| c.design).filter(|d| d != from).collect())
    }
}
