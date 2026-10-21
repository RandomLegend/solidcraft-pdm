//! A design's lifecycle (work in progress → released → obsolete), its revisions, and editing its
//! design properties. For one person: releasing records which version a revision is, nobody
//! approves anything.
//!
//! Releasing writes the next revision (A, B, C… or 01, 02…) into the design's `revision`
//! property, checks that in and records it. A released design that changes and is checked in
//! again is work in progress on the next revision. Released files are not made read-only:
//! SolidCraft saves by renaming a new file over the old one, which a read-only file does not
//! prevent, so the PDM shows a changed released design instead.

use std::fs;

use rusqlite::{OptionalExtension, params};

use crate::store::write_atomic;
use crate::{Error, Result, Vault, Version};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum State {
    /// Being worked on (every new or changed design).
    Wip,
    Released,
    /// No longer used; kept with its history.
    Obsolete,
}

impl State {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            State::Wip => "wip",
            State::Released => "released",
            State::Obsolete => "obsolete",
        }
    }

    pub(crate) fn parse(s: &str) -> State {
        match s {
            "released" => State::Released,
            "obsolete" => State::Obsolete,
            _ => State::Wip,
        }
    }
}

/// How revisions are counted.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RevisionScheme {
    /// A, B, C … Y, AA, AB … without I, O, Q, S, X and Z, which read like digits (ASME Y14.35).
    Letters,
    /// 01, 02, 03 …
    Numbers,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Release {
    pub revision: String,
    /// The version that is this revision.
    pub version: u32,
    /// Unix seconds.
    pub released_at: i64,
    pub note: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Lifecycle {
    pub state: State,
    /// Newest first.
    pub releases: Vec<Release>,
}

/// The letters a revision uses.
const LETTERS: &[u8] = b"ABCDEFGHJKLMNPRTUVWY";

/// The revision after `last` (the first one when there is none, or when `last` is not in this
/// scheme, e.g. after switching schemes).
pub fn next_revision(scheme: RevisionScheme, last: Option<&str>) -> String {
    match scheme {
        RevisionScheme::Numbers => {
            let next = last.and_then(|l| l.trim().parse::<u32>().ok()).map_or(1, |n| n + 1);
            format!("{next:02}")
        }
        RevisionScheme::Letters => {
            // Bijective base-20 over LETTERS: A = 1 … Y = 20, AA = 21 …
            let index = last.and_then(letters_index).map_or(1, |n| n + 1);
            letters(index)
        }
    }
}

fn letters_index(revision: &str) -> Option<u64> {
    let revision = revision.trim();
    if revision.is_empty() {
        return None;
    }
    revision.bytes().try_fold(0u64, |n, c| {
        let digit = LETTERS.iter().position(|&l| l == c.to_ascii_uppercase())? as u64 + 1;
        n.checked_mul(LETTERS.len() as u64)?.checked_add(digit)
    })
}

fn letters(mut index: u64) -> String {
    let base = LETTERS.len() as u64;
    let mut out = Vec::new();
    while index > 0 {
        index -= 1;
        out.push(LETTERS[(index % base) as usize]);
        index /= base;
    }
    out.reverse();
    String::from_utf8(out).expect("ASCII letters")
}

/// SolidCraft's limit for a property's text.
const MAX_PROPERTY_BYTES: usize = 4096;

impl Vault {
    pub fn lifecycle(&self, path: &str) -> Result<Lifecycle> {
        let doc = self.document_id(path)?.ok_or_else(|| Error::NotTracked(path.to_owned()))?;
        let state: String = self.db.query_row("SELECT state FROM documents WHERE id = ?1", [doc], |r| r.get(0))?;
        Ok(Lifecycle { state: State::parse(&state), releases: self.releases(doc)? })
    }

    fn releases(&self, doc: i64) -> Result<Vec<Release>> {
        let mut stmt = self.db.prepare(
            "SELECT revision, version, released_at, note FROM releases WHERE document_id = ?1 ORDER BY id DESC",
        )?;
        let rows = stmt.query_map([doc], |r| {
            Ok(Release { revision: r.get(0)?, version: r.get(1)?, released_at: r.get(2)?, note: r.get(3)? })
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// The revision the next release of `path` gets.
    pub fn next_revision(&self, path: &str) -> Result<String> {
        let lifecycle = self.lifecycle(path)?;
        Ok(next_revision(self.revision_scheme()?, lifecycle.releases.first().map(|r| r.revision.as_str())))
    }

    /// The revision to propose for the next release: for a design never released here, the
    /// revision its file already has (set by hand before); else the next one in the scheme.
    pub fn suggested_revision(&self, path: &str) -> Result<String> {
        if self.lifecycle(path)?.releases.is_empty() {
            let own = scformat::read_file(self.absolute(path)).ok().and_then(|i| i.metadata.revision);
            if let Some(own) = own.map(|r| r.trim().to_owned()).filter(|r| !r.is_empty()) {
                return Ok(own);
            }
        }
        self.next_revision(path)
    }

    /// Releases the design as its next revision (see [`Vault::release_as`]).
    pub fn release(&mut self, path: &str, note: &str) -> Result<Release> {
        let revision = self.next_revision(path)?;
        self.release_as(path, &revision, note)
    }

    /// Releases the design as `revision`: writes it into the design's `revision` property,
    /// checks that in, and records the release. The design must be checked in, and the
    /// revision new for it.
    pub fn release_as(&mut self, path: &str, revision: &str, note: &str) -> Result<Release> {
        let revision = revision.trim().to_owned();
        if revision.is_empty() || revision.len() > 64 {
            return Err(Error::Design(format!("{revision:?} is not a revision")));
        }
        let doc = self.document_id(path)?.ok_or_else(|| Error::NotTracked(path.to_owned()))?;
        let lifecycle = self.lifecycle(path)?;
        if lifecycle.state == State::Obsolete {
            return Err(Error::Obsolete(path.to_owned()));
        }
        // Before "already released": checking the changes in starts the next revision.
        if !self.is_current(path)? {
            return Err(Error::NotCheckedIn(path.to_owned()));
        }
        if lifecycle.state == State::Released {
            let revision = lifecycle.releases.first().map_or(String::new(), |r| r.revision.clone());
            return Err(Error::AlreadyReleased(path.to_owned(), revision));
        }
        if lifecycle.releases.iter().any(|r| r.revision.eq_ignore_ascii_case(&revision)) {
            return Err(Error::RevisionTaken(path.to_owned(), revision));
        }

        self.edit_properties(path, None, &[("revision", Some(&revision))])?;
        let note = note.trim();
        let comment = if note.is_empty() { format!("Released revision {revision}") } else { format!("Released revision {revision}: {note}") };
        let version = match self.check_in(path, &comment) {
            Ok(v) => v.number,
            // The design already said this revision.
            Err(Error::Unchanged(_)) => self.latest(doc)?.map_or(0, |v| v.number),
            Err(e) => return Err(e),
        };
        let release = Release { revision, version, released_at: crate::now(), note: note.to_owned() };
        let tx = self.db.transaction()?;
        tx.execute(
            "INSERT INTO releases (document_id, version, revision, released_at, note) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![doc, release.version, release.revision, release.released_at, release.note],
        )?;
        tx.execute("UPDATE documents SET state = 'released' WHERE id = ?1", [doc])?;
        tx.commit()?;
        Ok(release)
    }

    /// Takes a released design back to work in progress (on its next revision).
    pub fn revise(&mut self, path: &str) -> Result<()> {
        if self.lifecycle(path)?.state != State::Released {
            return Err(Error::NotReleased(path.to_owned()));
        }
        self.set_state(path, State::Wip)
    }

    /// Marks a design obsolete, or (`false`) back to work in progress.
    pub fn set_obsolete(&mut self, path: &str, obsolete: bool) -> Result<()> {
        let lifecycle = self.lifecycle(path)?;
        let state = match (obsolete, lifecycle.state) {
            (true, _) => State::Obsolete,
            (false, State::Obsolete) => State::Wip,
            (false, state) => state,
        };
        self.set_state(path, state)
    }

    fn set_state(&mut self, path: &str, state: State) -> Result<()> {
        self.db.execute("UPDATE documents SET state = ?2 WHERE path = ?1", params![path, state.as_str()])?;
        Ok(())
    }

    pub fn revision_scheme(&self) -> Result<RevisionScheme> {
        let value: Option<String> =
            self.db.query_row("SELECT value FROM settings WHERE key = 'revision_scheme'", [], |r| r.get(0)).optional()?;
        Ok(if value.as_deref() == Some("numbers") { RevisionScheme::Numbers } else { RevisionScheme::Letters })
    }

    pub fn set_revision_scheme(&mut self, scheme: RevisionScheme) -> Result<()> {
        let value = match scheme {
            RevisionScheme::Letters => "letters",
            RevisionScheme::Numbers => "numbers",
        };
        self.db.execute("INSERT OR REPLACE INTO settings (key, value) VALUES ('revision_scheme', ?1)", [value])?;
        Ok(())
    }

    /// Changes the design's name and design properties in its file (`None` or empty text
    /// removes a property). Not checked in: the file then has changes like any edit.
    pub fn edit_properties(&mut self, path: &str, name: Option<&str>, properties: &[(&str, Option<&str>)]) -> Result<()> {
        for (key, value) in properties {
            if value.is_some_and(|v| v.len() > MAX_PROPERTY_BYTES) {
                return Err(Error::TooLong((*key).to_owned()));
            }
        }
        let file = self.absolute(path);
        let bytes = fs::read(&file).map_err(|e| match e.kind() {
            std::io::ErrorKind::NotFound => Error::Missing(path.to_owned()),
            _ => e.into(),
        })?;
        let edited = scformat::edit_document(&bytes, |doc| {
            if let Some(name) = name.map(str::trim).filter(|n| !n.is_empty()) {
                doc["name"] = name.into();
            }
            let Some(object) = doc.as_object_mut() else { return };
            let metadata = object.entry("metadata").or_insert_with(|| serde_json::json!({}));
            if let Some(metadata) = metadata.as_object_mut() {
                for (key, value) in properties {
                    match value.map(str::trim).filter(|v| !v.is_empty()) {
                        Some(v) => {
                            metadata.insert((*key).to_owned(), v.into());
                        }
                        None => {
                            metadata.shift_remove(*key);
                        }
                    }
                }
            }
            // A design without properties has no `metadata` at all, as SolidCraft saves it.
            if object.get("metadata").and_then(|m| m.as_object()).is_some_and(|m| m.is_empty()) {
                object.shift_remove("metadata");
            }
        })
        .map_err(|e| Error::Design(format!("{path}: {e}")))?;
        if edited != bytes {
            let perms = fs::metadata(&file).ok().map(|m| m.permissions());
            write_atomic(&file, &edited, perms.as_ref())?;
        }
        Ok(())
    }

    /// The release a version is, if any.
    pub fn release_of(&self, path: &str, version: &Version) -> Result<Option<Release>> {
        Ok(self.lifecycle(path)?.releases.into_iter().find(|r| r.version == version.number))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn letter_revisions_skip_letters_that_look_like_digits() {
        let mut rev = None;
        let mut seen = Vec::new();
        for _ in 0..22 {
            let next = next_revision(RevisionScheme::Letters, rev.as_deref());
            seen.push(next.clone());
            rev = Some(next);
        }
        assert_eq!(seen.join(" "), "A B C D E F G H J K L M N P R T U V W Y AA AB");
        assert_eq!(next_revision(RevisionScheme::Letters, Some("AY")), "BA");
        assert_eq!(next_revision(RevisionScheme::Letters, Some("YY")), "AAA");
        assert_eq!(next_revision(RevisionScheme::Letters, Some("h")), "J", "lower case is read too");
        assert_eq!(next_revision(RevisionScheme::Letters, Some("03")), "A", "another scheme starts over");
    }

    #[test]
    fn number_revisions_count_up() {
        assert_eq!(next_revision(RevisionScheme::Numbers, None), "01");
        assert_eq!(next_revision(RevisionScheme::Numbers, Some("09")), "10");
        assert_eq!(next_revision(RevisionScheme::Numbers, Some("C")), "01");
    }
}
