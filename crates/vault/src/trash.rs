//! Deleted designs, kept for a while: the working file goes, the document and its versions stay
//! (under the path `/deleted/<id>`, never a vault path, so the name is free again) until they are
//! restored or purged. A design is checked in before it is deleted if it has changes (or is new),
//! so what comes back is what was deleted.

use std::collections::HashSet;
use std::fs;

use rusqlite::{OptionalExtension, params};

use crate::store::write_atomic;
use crate::{Error, Result, Vault, Version, check_path, now};

/// A deleted design.
#[derive(Clone, Debug, PartialEq)]
pub struct Deleted {
    pub id: i64,
    /// Where it was.
    pub path: String,
    pub deleted_at: i64,
    pub deleted_by: String,
    pub latest: Option<Version>,
}

impl Vault {
    /// Deletes a design: its file goes, its history is kept until [`Vault::purge_deleted`].
    /// `by`: who (a server's user; `None`: the user running the program).
    pub fn delete(&mut self, path: &str, by: Option<&str>) -> Result<()> {
        check_path(path)?;
        let file = self.absolute(path);
        // What is deleted can come back: unsaved changes and new designs are checked in first.
        if file.is_file() {
            match self.check_in(path, "Before deleting") {
                Ok(_) | Err(Error::Unchanged(_)) => {}
                Err(e) => return Err(e),
            }
        }
        let doc = self.document_id(path)?.ok_or_else(|| Error::NotTracked(path.to_owned()))?;
        let by = by.map(str::to_owned).or_else(|| self.author.clone()).unwrap_or_else(crate::author);
        let tx = self.db.transaction()?;
        tx.execute(
            "UPDATE documents SET path = ?2, deleted_at = ?3, deleted_by = ?4, deleted_from = ?5 WHERE id = ?1",
            params![doc, format!("/deleted/{doc}"), now(), by, path],
        )?;
        tx.execute("DELETE FROM locks WHERE document_id = ?1", [doc])?;
        tx.execute("DELETE FROM stat_cache WHERE path = ?1", [path])?;
        tx.commit()?;
        match fs::remove_file(&file) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => {
                // Read-only (a hint in workspaces; Windows refuses to delete those).
                let mut perms = fs::metadata(&file)?.permissions();
                #[allow(clippy::permissions_set_readonly_false)]
                perms.set_readonly(false);
                fs::set_permissions(&file, perms)?;
                Ok(fs::remove_file(&file)?)
            }
            Err(e) => Err(e.into()),
        }
    }

    /// The deleted designs, most recently deleted first.
    pub fn deleted(&self) -> Result<Vec<Deleted>> {
        let mut stmt = self.db.prepare(
            "SELECT id, deleted_from, deleted_at, deleted_by FROM documents WHERE deleted_at IS NOT NULL ORDER BY deleted_at DESC, id DESC",
        )?;
        let rows: Vec<(i64, String, i64, Option<String>)> =
            stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))?.collect::<rusqlite::Result<_>>()?;
        drop(stmt);
        let mut out = Vec::new();
        for (id, path, deleted_at, by) in rows {
            out.push(Deleted { id, path, deleted_at, deleted_by: by.unwrap_or_default(), latest: self.latest(id)? });
        }
        Ok(out)
    }

    /// Brings a deleted design back with its history: to where it was, or beside it as
    /// "name (restored).solidcraft" when that name is taken meanwhile. The path it got.
    pub fn restore_deleted(&mut self, id: i64) -> Result<String> {
        let from: Option<String> = self
            .db
            .query_row("SELECT deleted_from FROM documents WHERE id = ?1 AND deleted_at IS NOT NULL", [id], |r| r.get(0))
            .optional()?;
        let from = from.ok_or_else(|| Error::NotTracked(format!("deleted design {id}")))?;
        let latest = self.latest(id)?.ok_or_else(|| Error::NotTracked(from.clone()))?;
        let bytes = self.version_bytes(&latest)?;
        let path = self.free_path(&from)?;
        let file = self.absolute(&path);
        if let Some(dir) = file.parent() {
            fs::create_dir_all(dir)?;
        }
        write_atomic(&file, &bytes, None)?;
        self.db.execute(
            "UPDATE documents SET path = ?2, deleted_at = NULL, deleted_by = NULL, deleted_from = NULL WHERE id = ?1",
            params![id, path],
        )?;
        Ok(path)
    }

    /// `path`, or "name (restored).ext", "name (restored 2).ext" … if that is taken.
    fn free_path(&self, path: &str) -> Result<String> {
        let taken = |p: &str| -> Result<bool> { Ok(self.absolute(p).exists() || self.document_id(p)?.is_some()) };
        if !taken(path)? {
            return Ok(path.to_owned());
        }
        let (stem, ext) = path.rsplit_once('.').map_or((path, ""), |(s, e)| (s, e));
        for n in 1..1000 {
            let suffix = if n == 1 { " (restored)".to_owned() } else { format!(" (restored {n})") };
            let candidate = if ext.is_empty() { format!("{stem}{suffix}") } else { format!("{stem}{suffix}.{ext}") };
            if !taken(&candidate)? {
                return Ok(candidate);
            }
        }
        Err(Error::Exists(path.to_owned()))
    }

    /// Removes for good what was deleted before `before` (Unix seconds): the documents, their
    /// versions and releases, and the stored files nothing refers to any more. How many designs.
    pub fn purge_deleted(&mut self, before: i64) -> Result<usize> {
        let ids: Vec<i64> = {
            let mut stmt = self.db.prepare("SELECT id FROM documents WHERE deleted_at IS NOT NULL AND deleted_at < ?1")?;
            stmt.query_map([before], |r| r.get(0))?.collect::<rusqlite::Result<_>>()?
        };
        self.purge(&ids)
    }

    /// Removes one deleted design for good, now.
    pub fn purge_one(&mut self, id: i64) -> Result<()> {
        let deleted: Option<i64> =
            self.db.query_row("SELECT id FROM documents WHERE id = ?1 AND deleted_at IS NOT NULL", [id], |r| r.get(0)).optional()?;
        let id = deleted.ok_or_else(|| Error::NotTracked(format!("deleted design {id}")))?;
        self.purge(&[id]).map(drop)
    }

    fn purge(&mut self, ids: &[i64]) -> Result<usize> {
        let tx = self.db.transaction()?;
        for id in ids {
            tx.execute("DELETE FROM release_files WHERE document_id = ?1", [id])?;
            tx.execute("DELETE FROM releases WHERE document_id = ?1", [id])?;
            tx.execute("DELETE FROM versions WHERE document_id = ?1", [id])?;
            tx.execute("DELETE FROM locks WHERE document_id = ?1", [id])?;
            tx.execute("DELETE FROM documents WHERE id = ?1", [id])?;
        }
        tx.commit()?;
        if !ids.is_empty() {
            self.remove_unused_objects()?;
        }
        Ok(ids.len())
    }

    /// Removes stored files no version refers to (older than a day: a newer one may belong to a
    /// check-in under way).
    pub fn remove_unused_objects(&mut self) -> Result<(usize, u64)> {
        let mut keep = HashSet::new();
        let mut stmt = self.db.prepare("SELECT hash, thumbnail FROM versions")?;
        for row in stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, Option<String>>(1)?)))? {
            let (hash, thumbnail) = row?;
            keep.insert(hash);
            keep.extend(thumbnail);
        }
        drop(stmt);
        let mut stmt = self.db.prepare("SELECT hash FROM release_files")?;
        for hash in stmt.query_map([], |r| r.get::<_, String>(0))? {
            keep.insert(hash?);
        }
        drop(stmt);
        self.store.remove_unused(&keep, std::time::Duration::from_secs(24 * 3600))
    }
}
