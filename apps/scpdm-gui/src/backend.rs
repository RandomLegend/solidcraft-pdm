//! What the window works on: a local vault, or a workspace of a server vault. The operations the
//! window uses, for both; a workspace's go to the server.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};

use protocol::Action;
use remote::{RemoteStatus, Workspace};
use vault::{Lifecycle, MoveReport, Release, Vault, Version};

pub enum Backend {
    Local(Vault),
    /// Shared with the background scan.
    Remote(Arc<Mutex<Workspace>>),
}

/// An error, as text for the status line.
pub type Result<T> = std::result::Result<T, String>;

fn text(e: impl std::fmt::Display) -> String {
    e.to_string()
}

impl Backend {
    /// The local vault or workspace containing `dir`.
    pub fn open(dir: &Path) -> std::result::Result<Backend, OpenError> {
        if let Ok(ws) = Workspace::discover(dir) {
            return Ok(Backend::Remote(Arc::new(Mutex::new(ws))));
        }
        match Vault::discover(dir) {
            Ok(v) => Ok(Backend::Local(v)),
            Err(vault::Error::NotAVault(_)) => Err(OpenError::NotAVault),
            Err(e) => Err(OpenError::Other(e.to_string())),
        }
    }

    pub fn is_remote(&self) -> bool {
        matches!(self, Backend::Remote(_))
    }

    pub fn workspace(&self) -> Option<MutexGuard<'_, Workspace>> {
        match self {
            Backend::Remote(ws) => Some(ws.lock().unwrap_or_else(|e| e.into_inner())),
            Backend::Local(_) => None,
        }
    }

    pub fn shared_workspace(&self) -> Option<Arc<Mutex<Workspace>>> {
        match self {
            Backend::Remote(ws) => Some(ws.clone()),
            Backend::Local(_) => None,
        }
    }

    pub fn local(&mut self) -> Option<&mut Vault> {
        match self {
            Backend::Local(v) => Some(v),
            Backend::Remote(_) => None,
        }
    }

    pub fn root(&self) -> PathBuf {
        match self {
            Backend::Local(v) => v.root().to_path_buf(),
            Backend::Remote(_) => self.workspace().map(|w| w.root().to_path_buf()).unwrap_or_default(),
        }
    }

    pub fn absolute(&self, path: &str) -> PathBuf {
        self.root().join(path)
    }

    /// "Workshop on server.lan" for a workspace; the folder's name for a vault.
    pub fn title(&self) -> String {
        let root = self.root();
        let folder = root.file_name().map_or(root.display().to_string(), |n| n.to_string_lossy().into_owned());
        match self.workspace() {
            Some(ws) => format!("{} on {}", ws.vault(), host(ws.client().url())),
            None => folder,
        }
    }

    pub fn history(&self, path: &str) -> Result<Vec<Version>> {
        match self {
            Backend::Local(v) => v.history(path).map_err(text),
            Backend::Remote(_) => {
                let ws = self.workspace().expect("remote");
                Ok(ws.history(path).map_err(text)?.into_iter().map(version).collect())
            }
        }
    }

    pub fn version_bytes(&self, v: &Version) -> Result<Vec<u8>> {
        match self {
            Backend::Local(vault) => vault.version_bytes(v).map_err(text),
            Backend::Remote(_) => self.workspace().expect("remote").object(&v.hash).map_err(text),
        }
    }

    pub fn thumbnail(&self, v: &Version) -> Option<Vec<u8>> {
        let hash = v.thumbnail.as_ref()?;
        match self {
            Backend::Local(vault) => vault.thumbnail(v).ok().flatten(),
            Backend::Remote(_) => self.workspace()?.object(hash).ok(),
        }
    }

    /// The design's lifecycle, and the revision to propose for its next release.
    pub fn lifecycle(&self, path: &str, entry: Option<&vault::Entry>) -> Option<(Lifecycle, String)> {
        match self {
            Backend::Local(v) => v.lifecycle(path).ok().zip(v.suggested_revision(path).ok()),
            Backend::Remote(_) => {
                // The server's state and newest release, from the last scan.
                let entry = entry?;
                let lifecycle = Lifecycle { state: entry.state?, releases: entry.release.clone().into_iter().collect() };
                let last = lifecycle.releases.first().map(|r| r.revision.clone());
                let suggested = match &last {
                    Some(r) => vault::next_revision(vault::RevisionScheme::Letters, Some(r)),
                    None => entry.latest.as_ref().and_then(|v| v.revision.clone()).filter(|r| !r.trim().is_empty()).unwrap_or_else(|| "A".into()),
                };
                Some((lifecycle, suggested))
            }
        }
    }

    pub fn restore(&mut self, path: &str, number: u32) -> Result<Version> {
        match self {
            Backend::Local(v) => v.restore(path, number).map_err(text),
            Backend::Remote(ws) => Ok(version(lock(ws).restore(path, number).map_err(text)?)),
        }
    }

    pub fn rename(&mut self, from: &str, to: &str) -> Result<MoveReport> {
        match self {
            Backend::Local(v) => v.rename(from, to).map_err(text),
            Backend::Remote(ws) => {
                let updated = lock(ws).move_file(from, to).map_err(text)?;
                Ok(MoveReport { to: to.to_owned(), updated: updated.into_iter().map(|path| vault::LinkUpdate { path, version: None }).collect(), failed: Vec::new() })
            }
        }
    }

    pub fn move_to_folder(&mut self, path: &str, folder: &str) -> Result<MoveReport> {
        let name = path.rsplit_once('/').map_or(path, |(_, n)| n);
        let to = if folder.is_empty() { name.to_owned() } else { format!("{folder}/{name}") };
        if to == path {
            return Ok(MoveReport { to, updated: Vec::new(), failed: Vec::new() });
        }
        self.rename(path, &to)
    }

    pub fn create_folder(&mut self, folder: &str) -> Result<()> {
        match self {
            Backend::Local(v) => v.create_folder(folder).map_err(text),
            Backend::Remote(ws) => lock(ws).act(Action::CreateFolder { path: folder.to_owned() }).map(drop).map_err(text),
        }
    }

    pub fn remove_folder(&mut self, folder: &str) -> Result<()> {
        match self {
            Backend::Local(v) => v.remove_folder(folder).map_err(text),
            Backend::Remote(ws) => lock(ws).act(Action::DeleteFolder { path: folder.to_owned() }).map(drop).map_err(text),
        }
    }

    /// A new design as v1 (in a workspace: written there and added to the server).
    pub fn add_new(&mut self, path: &str, bytes: &[u8], comment: &str) -> Result<Version> {
        match self {
            Backend::Local(v) => v.add_new(path, bytes, comment).map_err(text),
            Backend::Remote(ws) => {
                let mut ws = lock(ws);
                let file = ws.absolute(path);
                if file.exists() {
                    return Err(format!("{path} exists already"));
                }
                if let Some(dir) = file.parent() {
                    std::fs::create_dir_all(dir).map_err(text)?;
                }
                std::fs::write(&file, bytes).map_err(text)?;
                Ok(version(ws.add(path, comment).map_err(text)?))
            }
        }
    }

    pub fn release_as(&mut self, path: &str, revision: &str, note: &str) -> Result<Release> {
        match self {
            Backend::Local(v) => v.release_as(path, revision, note).map_err(text),
            Backend::Remote(ws) => {
                let action = Action::Release { path: path.to_owned(), revision: revision.to_owned(), note: note.to_owned() };
                let done = lock(ws).act_on_design(path, action).map_err(text)?;
                let r = done.release.ok_or("the server recorded no release")?;
                Ok(Release { revision: r.revision, version: r.version, released_at: r.released_at, note: r.note })
            }
        }
    }

    pub fn revise(&mut self, path: &str) -> Result<()> {
        match self {
            Backend::Local(v) => v.revise(path).map_err(text),
            Backend::Remote(ws) => lock(ws).act(Action::Revise { path: path.to_owned() }).map(drop).map_err(text),
        }
    }

    pub fn set_obsolete(&mut self, path: &str, obsolete: bool) -> Result<()> {
        match self {
            Backend::Local(v) => v.set_obsolete(path, obsolete).map_err(text),
            Backend::Remote(ws) => lock(ws).act(Action::Obsolete { path: path.to_owned(), obsolete }).map(drop).map_err(text),
        }
    }

    /// Writes design properties; `check_in`: with this comment as a new version (on a server
    /// always: the server's copy is the one that changes). The version number, if one was made.
    pub fn save_properties(&mut self, path: &str, name: &str, properties: &[(&str, Option<&str>)], check_in: Option<&str>) -> Result<Option<u32>> {
        match self {
            Backend::Local(v) => {
                v.edit_properties(path, Some(name), properties).map_err(text)?;
                let Some(comment) = check_in else { return Ok(None) };
                match v.check_in(path, comment) {
                    Ok(version) => Ok(Some(version.number)),
                    Err(vault::Error::Unchanged(_)) => Ok(None),
                    Err(e) => Err(e.to_string()),
                }
            }
            Backend::Remote(ws) => {
                let action = Action::Properties {
                    path: path.to_owned(),
                    name: Some(name.to_owned()),
                    properties: properties.iter().map(|(k, v)| ((*k).to_owned(), v.map(str::to_owned))).collect(),
                    comment: check_in.unwrap_or("Edited properties").to_owned(),
                };
                let done = lock(ws).act_on_design(path, action).map_err(text)?;
                Ok(done.version.map(|v| v.number))
            }
        }
    }
}

pub enum OpenError {
    NotAVault,
    Other(String),
}

fn lock(ws: &Arc<Mutex<Workspace>>) -> MutexGuard<'_, Workspace> {
    ws.lock().unwrap_or_else(|e| e.into_inner())
}

fn host(url: &str) -> String {
    url.split("://").nth(1).unwrap_or(url).trim_end_matches('/').to_owned()
}

/// A server's version, as the vault's.
pub fn version(v: protocol::VersionInfo) -> Version {
    Version {
        number: v.number,
        hash: v.hash,
        size: v.size,
        comment: v.comment,
        author: v.author,
        created_at: v.created_at,
        path: v.path,
        name: v.name,
        part_number: v.part_number,
        revision: v.revision,
        format: v.format,
        thumbnail: v.thumbnail,
    }
}

pub fn state(s: protocol::State) -> vault::State {
    match s {
        protocol::State::Wip => vault::State::Wip,
        protocol::State::Released => vault::State::Released,
        protocol::State::Obsolete => vault::State::Obsolete,
    }
}

pub fn release(r: protocol::ReleaseInfo) -> Release {
    Release { revision: r.revision, version: r.version, released_at: r.released_at, note: r.note }
}

/// How a workspace design shows in the window: the vault status the filters use.
pub fn vault_status(s: &RemoteStatus) -> vault::Status {
    match s {
        RemoteStatus::UpToDate | RemoteStatus::NewerOnServer | RemoteStatus::CheckedOut { modified: false } | RemoteStatus::CheckedOutByOther(_) => {
            vault::Status::Current
        }
        RemoteStatus::CheckedOut { modified: true } | RemoteStatus::ChangedWithoutCheckout | RemoteStatus::Conflict => vault::Status::Modified,
        RemoteStatus::LocalOnly => vault::Status::Untracked,
        RemoteStatus::NotDownloaded | RemoteStatus::RemovedOnServer => vault::Status::Missing,
    }
}
