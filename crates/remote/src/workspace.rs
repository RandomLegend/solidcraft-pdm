//! A workspace: a local folder that mirrors a server vault, where SolidCraft opens and saves
//! designs. Same folder structure as on the server, so links between designs keep working.
//!
//! ```text
//! workspace/
//!   .pdm-workspace/workspace.json   the server, the vault, what was fetched (version, hash)
//!   .pdm-workspace/session          the login (readable by the owner only)
//!   Enclosures/pi4-case.solidcraft  as fetched, or as you changed it
//! ```
//!
//! "Get" fetches the latest version; "check out" takes the server's lock (and makes the file
//! writable); "check in" uploads and records a new version. Files not checked out by you are
//! read-only, as a hint: SolidCraft saves by renaming over a file, so a change without check-out
//! is still possible and shows as [`RemoteStatus::ChangedWithoutCheckout`]. A local change is
//! never overwritten.

use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use protocol::{Action, Design, ErrorCode, Tree, VersionInfo};
use serde::{Deserialize, Serialize};

use crate::client::{Client, Error, Result};

/// The folder that makes a folder a workspace.
pub const WORKSPACE_DIR: &str = ".pdm-workspace";

/// What was fetched of a design: the server's document, version and content.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Base {
    pub id: i64,
    pub version: u32,
    pub hash: String,
}

#[derive(Default, Serialize, Deserialize)]
struct State {
    server: String,
    user: String,
    vault: String,
    /// Designs as fetched, by path.
    files: BTreeMap<String, Base>,
    /// Images as fetched (their hash), by path.
    images: BTreeMap<String, String>,
    /// Hashes of local files, valid while size and modification time match.
    stat: BTreeMap<String, (u64, u128, String)>,
    /// The last tree from the server (for its counter, and to show something offline).
    tree: Tree,
}

/// How a design in the workspace relates to the server.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RemoteStatus {
    /// On the server, not in the workspace yet.
    NotDownloaded,
    UpToDate,
    /// The server has a newer version than the one here (unchanged here).
    NewerOnServer,
    /// Checked out by you; `modified`: changed since.
    CheckedOut { modified: bool },
    /// Checked out by someone else.
    CheckedOutByOther(String),
    /// Changed here, but not checked out by you.
    ChangedWithoutCheckout,
    /// Changed here, and changed on the server too: check out and compare, or keep a copy.
    Conflict,
    /// A design here that is not on the server yet: add it.
    LocalOnly,
    /// Fetched before, but no longer on the server.
    RemovedOnServer,
}

#[derive(Clone, Debug)]
pub struct RemoteEntry {
    pub path: String,
    pub status: RemoteStatus,
    pub server: Option<Design>,
    pub base: Option<Base>,
    /// SHA-256 of the local file, if there is one.
    pub local_hash: Option<String>,
    pub size: Option<u64>,
    pub modified: Option<std::time::SystemTime>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct RemoteImage {
    pub path: String,
    /// On the server (its hash), here (its hash).
    pub server: Option<String>,
    pub local: Option<String>,
    pub size: Option<u64>,
    pub modified: Option<std::time::SystemTime>,
}

/// What [`Workspace::status`] found.
pub struct Status {
    pub designs: Vec<RemoteEntry>,
    pub images: Vec<RemoteImage>,
    pub folders: Vec<String>,
    /// Moves by others that were applied here (from, to).
    pub followed_moves: Vec<(String, String)>,
    /// The server could not be reached: the rest is from the last time it could.
    pub offline: Option<String>,
}

pub struct Workspace {
    root: PathBuf,
    client: Client,
    state: State,
}

impl Workspace {
    /// Makes `root` a workspace of `vault` on the client's server (logged in).
    pub fn create(root: &Path, client: Client, user: &str, vault: &str) -> Result<Workspace> {
        fs::create_dir_all(root)?;
        let root = dunce_canonical(root)?;
        if root.join(vault::PDM_DIR).exists() {
            return Err(Error::Local(format!("{} is a local vault; choose another folder", root.display())));
        }
        if root.join(WORKSPACE_DIR).exists() {
            return Err(Error::Local(format!("{} is a workspace already", root.display())));
        }
        client.tree(vault, None)?; // the vault exists, and the login may use it
        fs::create_dir_all(root.join(WORKSPACE_DIR))?;
        let state = State { server: client.url().to_owned(), user: user.to_owned(), vault: vault.to_owned(), ..State::default() };
        let ws = Workspace { root, client, state };
        ws.save_session()?;
        ws.save()?;
        Ok(ws)
    }

    /// Opens the workspace at `root`, with its saved login.
    pub fn open(root: &Path) -> Result<Workspace> {
        let root = dunce_canonical(root)?;
        let dir = root.join(WORKSPACE_DIR);
        let text = fs::read_to_string(dir.join("workspace.json")).map_err(|_| Error::Local(format!("{} is not a workspace", root.display())))?;
        let state: State = serde_json::from_str(&text).map_err(|e| Error::Local(format!("workspace.json: {e}")))?;
        let token = fs::read_to_string(dir.join("session")).ok().map(|t| t.trim().to_owned()).filter(|t| !t.is_empty());
        let client = Client::new(&state.server, token);
        Ok(Workspace { root, client, state })
    }

    /// The workspace containing `path` (itself or the nearest folder above).
    pub fn discover(path: &Path) -> Result<Workspace> {
        let start = dunce_canonical(path)?;
        let mut dir = Some(start.as_path());
        while let Some(d) = dir {
            if d.join(WORKSPACE_DIR).is_dir() {
                return Workspace::open(d);
            }
            dir = d.parent();
        }
        Err(Error::Local(format!("{} is not inside a workspace", start.display())))
    }

    pub fn is_workspace(path: &Path) -> bool {
        path.join(WORKSPACE_DIR).join("workspace.json").is_file()
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn vault(&self) -> &str {
        &self.state.vault
    }

    pub fn user(&self) -> &str {
        &self.state.user
    }

    pub fn client(&self) -> &Client {
        &self.client
    }

    /// Uses a new login (after it expired).
    pub fn set_client(&mut self, client: Client, user: &str) -> Result<()> {
        self.state.server = client.url().to_owned();
        self.state.user = user.to_owned();
        self.client = client;
        self.save_session()?;
        self.save()
    }

    pub fn absolute(&self, path: &str) -> PathBuf {
        self.root.join(path)
    }

    /// The workspace path of a file given absolute or relative to the current folder.
    pub fn relative(&self, path: &Path) -> Result<String> {
        let abs = if path.is_absolute() { path.to_path_buf() } else { std::env::current_dir()?.join(path) };
        let rel = abs.strip_prefix(&self.root).map_err(|_| Error::Local(format!("{} is outside the workspace", abs.display())))?;
        let rel = rel.components().map(|c| c.as_os_str().to_string_lossy()).collect::<Vec<_>>().join("/");
        Ok(vault::Vault::relative_path(&rel)?)
    }

    /// Fetches what changed on the server, follows moves by others, and compares with the
    /// local files.
    pub fn status(&mut self) -> Result<Status> {
        let mut offline = None;
        match self.client.tree(&self.state.vault, Some(self.state.tree.counter).filter(|c| *c != 0)) {
            Ok(tree) if tree.unchanged => {}
            Ok(tree) => self.state.tree = tree,
            Err(e @ Error::Api(_)) => return Err(e),
            Err(e) => offline = Some(e.to_string()),
        }
        let followed_moves = if offline.is_none() { self.follow_moves()? } else { Vec::new() };
        let designs = self.designs()?;
        let images = self.image_status()?;
        self.save()?;
        Ok(Status { designs, images, folders: self.state.tree.folders.clone(), followed_moves, offline })
    }

    /// The last known server state of a design.
    pub fn server_design(&self, path: &str) -> Option<&Design> {
        self.state.tree.designs.iter().find(|d| d.path == path)
    }

    /// Renames local files that others moved on the server (by document id), when unchanged here.
    fn follow_moves(&mut self) -> Result<Vec<(String, String)>> {
        let by_id: HashMap<i64, String> = self.state.tree.designs.iter().map(|d| (d.id, d.path.clone())).collect();
        let mut moves = Vec::new();
        for (path, base) in self.state.files.clone() {
            let Some(new_path) = by_id.get(&base.id) else { continue };
            if *new_path == path {
                continue;
            }
            let (from, to) = (self.absolute(&path), self.absolute(new_path));
            let unchanged = self.local_hash(&path)?.is_none_or(|h| h == base.hash);
            if !unchanged || to.exists() {
                continue; // shown as removed / not downloaded: the user decides
            }
            if from.exists() {
                if let Some(dir) = to.parent() {
                    fs::create_dir_all(dir)?;
                }
                fs::rename(&from, &to)?;
            }
            self.state.files.remove(&path);
            self.state.stat.remove(&path);
            self.state.files.insert(new_path.clone(), base);
            moves.push((path, new_path.clone()));
        }
        Ok(moves)
    }

    fn designs(&mut self) -> Result<Vec<RemoteEntry>> {
        let mut local = Vec::new();
        vault::find_designs(&self.root, &mut local)?;
        let mut paths: BTreeMap<String, ()> = BTreeMap::new();
        for file in &local {
            if let Some(p) = self.vault_path(file) {
                paths.insert(p, ());
            }
        }
        for d in &self.state.tree.designs {
            paths.insert(d.path.clone(), ());
        }
        for p in self.state.files.keys() {
            paths.insert(p.clone(), ());
        }
        let mut out = Vec::new();
        for path in paths.into_keys() {
            let meta = fs::metadata(self.absolute(&path)).ok();
            let local_hash = if meta.is_some() { self.local_hash(&path)? } else { None };
            let server = self.server_design(&path).cloned();
            let mut base = self.state.files.get(&path).cloned();
            // A file here with exactly the server's content, never fetched: as if fetched.
            if let (None, Some(s), Some(h)) = (&base, &server, &local_hash) {
                if *h == s.latest.hash {
                    let adopted = Base { id: s.id, version: s.latest.number, hash: h.clone() };
                    self.state.files.insert(path.clone(), adopted.clone());
                    base = Some(adopted);
                }
            }
            let status = self.status_of(server.as_ref(), base.as_ref(), local_hash.as_deref());
            out.push(RemoteEntry {
                path,
                status,
                server,
                base,
                local_hash,
                size: meta.as_ref().map(|m| m.len()),
                modified: meta.and_then(|m| m.modified().ok()),
            });
        }
        Ok(out)
    }

    fn status_of(&self, server: Option<&Design>, base: Option<&Base>, local: Option<&str>) -> RemoteStatus {
        let Some(s) = server else {
            return if base.is_some() { RemoteStatus::RemovedOnServer } else { RemoteStatus::LocalOnly };
        };
        let Some(local) = local else { return RemoteStatus::NotDownloaded };
        let modified = base.is_none_or(|b| b.hash != local);
        let stale = base.is_none_or(|b| b.version != s.latest.number);
        let mine = s.lock.as_ref().is_some_and(|l| l.user == self.state.user);
        match (mine, modified, stale) {
            (_, true, true) => RemoteStatus::Conflict,
            (true, modified, _) => RemoteStatus::CheckedOut { modified },
            (false, true, false) => RemoteStatus::ChangedWithoutCheckout,
            (false, false, _) if s.lock.is_some() => RemoteStatus::CheckedOutByOther(s.lock.as_ref().map(|l| l.user.clone()).unwrap_or_default()),
            (false, false, true) => RemoteStatus::NewerOnServer,
            (false, false, false) => RemoteStatus::UpToDate,
        }
    }

    fn image_status(&mut self) -> Result<Vec<RemoteImage>> {
        let mut local = Vec::new();
        vault::find_images(&self.root, &mut local)?;
        let mut out: BTreeMap<String, RemoteImage> = BTreeMap::new();
        for file in local {
            let Some(path) = self.vault_path(&file) else { continue };
            let meta = fs::metadata(&file).ok();
            let hash = self.local_hash(&path)?;
            out.insert(path.clone(), RemoteImage {
                path,
                server: None,
                local: hash,
                size: meta.as_ref().map(|m| m.len()),
                modified: meta.and_then(|m| m.modified().ok()),
            });
        }
        for image in &self.state.tree.images {
            out.entry(image.path.clone())
                .or_insert_with(|| RemoteImage { path: image.path.clone(), server: None, local: None, size: Some(image.size), modified: None })
                .server = Some(image.hash.clone());
        }
        Ok(out.into_values().collect())
    }

    /// Fetches the latest version of a design (or an image) into the workspace. Refuses to
    /// overwrite local changes.
    pub fn get(&mut self, path: &str) -> Result<()> {
        if vault::is_image(Path::new(path)) {
            let bytes = self.client.file(&self.state.vault, path)?;
            let known = self.state.images.get(path).cloned();
            if let Some(local) = self.local_hash(path)? {
                if known.as_deref() != Some(local.as_str()) && local != vault::hash(&bytes) {
                    return Err(Error::Local(format!("{path} was changed here; upload it or move it away first")));
                }
            }
            self.write(path, &bytes, true)?;
            self.state.images.insert(path.to_owned(), vault::hash(&bytes));
            return self.save();
        }
        let design = self.design_now(path)?;
        if let Some(local) = self.local_hash(path)? {
            let base = self.state.files.get(path);
            let unchanged = base.is_some_and(|b| b.hash == local) || local == design.latest.hash;
            if !unchanged {
                return Err(Error::Local(format!("{path} has changes here that are not checked in; check them in or undo them first")));
            }
        }
        let bytes = self.client.object(&self.state.vault, &design.latest.hash)?;
        let mine = design.lock.as_ref().is_some_and(|l| l.user == self.state.user);
        self.write(path, &bytes, !mine)?;
        self.state.files.insert(path.to_owned(), Base { id: design.id, version: design.latest.number, hash: design.latest.hash.clone() });
        self.save()
    }

    /// The server's design at `path`, fetching the tree again when the last one does not know it.
    fn design_now(&mut self, path: &str) -> Result<Design> {
        if self.server_design(path).is_none() {
            self.refresh_tree()?;
        }
        self.server_design(path).cloned().ok_or_else(|| Error::Local(format!("{path} is not on the server")))
    }

    /// Checks a design out: takes the lock and makes the file writable (fetching the latest
    /// version first when the local copy is old and unchanged).
    pub fn checkout(&mut self, path: &str) -> Result<()> {
        let done = self.client.act(&self.state.vault, &Action::Checkout { path: path.to_owned() })?;
        self.design_now(path)?;
        if let Some(lock) = done.lock {
            if let Some(d) = self.state.tree.designs.iter_mut().find(|d| d.path == path) {
                d.lock = Some(lock);
            }
        }
        let status = self.single(path)?;
        if matches!(status, RemoteStatus::NotDownloaded) || self.is_old_and_unchanged(path)? {
            self.get(path)?;
        }
        set_readonly(&self.absolute(path), false);
        self.save()
    }

    /// Gives the lock back; `discard`: also puts the fetched version back over local changes.
    pub fn undo_checkout(&mut self, path: &str, discard: bool) -> Result<()> {
        self.client.act(&self.state.vault, &Action::UndoCheckout { path: path.to_owned(), force: false })?;
        if let Some(d) = self.state.tree.designs.iter_mut().find(|d| d.path == path) {
            d.lock = None;
        }
        if discard {
            if let Some(base) = self.state.files.get(path).cloned() {
                let bytes = self.client.object(&self.state.vault, &base.hash)?;
                self.write(path, &bytes, true)?;
            }
        } else {
            set_readonly(&self.absolute(path), true);
        }
        self.save()
    }

    /// Uploads the local file and records it as the next version (the design must be checked
    /// out by you). `keep_checked_out`: keep the lock to go on working.
    pub fn checkin(&mut self, path: &str, comment: &str, keep_checked_out: bool) -> Result<VersionInfo> {
        let base = self.state.files.get(path).cloned().ok_or_else(|| Error::Local(format!("{path} was never fetched; get it first")))?;
        let bytes = fs::read(self.absolute(path))?;
        let hash = self.client.upload(&self.state.vault, &bytes)?;
        let action = Action::Checkin { path: path.to_owned(), base: base.version, hash: hash.clone(), comment: comment.to_owned(), keep_checked_out };
        let done = self.client.act(&self.state.vault, &action)?;
        let version = done.version.ok_or_else(|| Error::Local("the server recorded no version".into()))?;
        self.state.files.insert(path.to_owned(), Base { id: base.id, version: version.number, hash: version.hash.clone() });
        set_readonly(&self.absolute(path), !keep_checked_out);
        self.refresh_tree()?;
        Ok(version)
    }

    /// Puts a new design from the workspace onto the server (v1).
    pub fn add(&mut self, path: &str, comment: &str) -> Result<VersionInfo> {
        let bytes = fs::read(self.absolute(path))?;
        let hash = self.client.upload(&self.state.vault, &bytes)?;
        let done = self.client.act(&self.state.vault, &Action::Add { path: path.to_owned(), hash, comment: comment.to_owned() })?;
        let version = done.version.ok_or_else(|| Error::Local("the server recorded no version".into()))?;
        self.refresh_tree()?;
        let id = self.server_design(path).map_or(0, |d| d.id);
        self.state.files.insert(path.to_owned(), Base { id, version: version.number, hash: version.hash.clone() });
        set_readonly(&self.absolute(path), true);
        self.save()?;
        Ok(version)
    }

    /// Puts version `number` back, as a new version (checking the design out and in).
    pub fn restore(&mut self, path: &str, number: u32) -> Result<VersionInfo> {
        let old = self
            .client
            .history(&self.state.vault, path)?
            .into_iter()
            .find(|v| v.number == number)
            .ok_or_else(|| Error::Local(format!("{path} has no version {number}")))?;
        let checked_out = matches!(self.single(path)?, RemoteStatus::CheckedOut { .. });
        if !checked_out {
            self.checkout(path)?;
        }
        if matches!(self.single(path)?, RemoteStatus::CheckedOut { modified: true }) {
            return Err(Error::Local(format!("{path} has changes that are not checked in; check them in or undo them first")));
        }
        let bytes = self.client.object(&self.state.vault, &old.hash)?;
        self.write(path, &bytes, false)?;
        self.checkin(path, &format!("Restored v{number}"), false)
    }

    /// Moves or renames on the server (with link updates), then here; designs whose links the
    /// server updated are fetched again when unchanged here.
    pub fn move_file(&mut self, from: &str, to: &str) -> Result<Vec<String>> {
        let done = self.client.act(&self.state.vault, &Action::Move { from: from.to_owned(), to: to.to_owned() })?;
        let (src, dst) = (self.absolute(from), self.absolute(to));
        if src.exists() && !dst.exists() {
            if let Some(dir) = dst.parent() {
                fs::create_dir_all(dir)?;
            }
            fs::rename(&src, &dst)?;
        }
        self.state.stat.remove(from);
        if let Some(base) = self.state.files.remove(from) {
            self.state.files.insert(to.to_owned(), base);
        }
        if let Some(hash) = self.state.images.remove(from) {
            self.state.images.insert(to.to_owned(), hash);
        }
        self.refresh_tree()?;
        for path in &done.updated {
            if self.is_old_and_unchanged(path)? {
                self.get(path)?;
            }
        }
        self.save()?;
        Ok(done.updated)
    }

    /// An action on the server that changes a design there (release, properties…); the local
    /// copy is fetched again when unchanged here.
    pub fn act_on_design(&mut self, path: &str, action: Action) -> Result<protocol::Done> {
        let done = self.client.act(&self.state.vault, &action)?;
        self.refresh_tree()?;
        if self.is_old_and_unchanged(path)? {
            self.get(path)?;
        }
        Ok(done)
    }

    /// An action that only changes the server's bookkeeping (revise, obsolete, folders).
    pub fn act(&mut self, action: Action) -> Result<protocol::Done> {
        let done = self.client.act(&self.state.vault, &action)?;
        if let Action::CreateFolder { path } = &action {
            fs::create_dir_all(self.absolute(path))?;
        }
        if let Action::DeleteFolder { path } = &action {
            let _ = fs::remove_dir(self.absolute(path));
        }
        self.refresh_tree()?;
        Ok(done)
    }

    /// Uploads a local reference image (new or replaced).
    pub fn put_image(&mut self, path: &str) -> Result<()> {
        let bytes = fs::read(self.absolute(path))?;
        let hash = self.client.upload(&self.state.vault, &bytes)?;
        self.client.act(&self.state.vault, &Action::PutImage { path: path.to_owned(), hash: hash.clone() })?;
        self.state.images.insert(path.to_owned(), hash);
        self.refresh_tree()
    }

    pub fn history(&self, path: &str) -> Result<Vec<VersionInfo>> {
        self.client.history(&self.state.vault, path)
    }

    /// A version's bytes or a thumbnail, by hash.
    pub fn object(&self, hash: &str) -> Result<Vec<u8>> {
        self.client.object(&self.state.vault, hash)
    }

    /// The status of one design, from the last known server state.
    pub fn single(&mut self, path: &str) -> Result<RemoteStatus> {
        let local = self.local_hash(path)?;
        let server = self.server_design(path).cloned();
        let base = self.state.files.get(path).cloned();
        Ok(self.status_of(server.as_ref(), base.as_ref(), local.as_deref()))
    }

    fn is_old_and_unchanged(&mut self, path: &str) -> Result<bool> {
        let Some(base) = self.state.files.get(path).cloned() else { return Ok(false) };
        let Some(server) = self.server_design(path).cloned() else { return Ok(false) };
        Ok(base.version != server.latest.number && self.local_hash(path)?.is_none_or(|h| h == base.hash))
    }

    fn refresh_tree(&mut self) -> Result<()> {
        self.state.tree = self.client.tree(&self.state.vault, None)?;
        self.save()
    }

    /// The local file's hash, from the cache while size and modification time match.
    fn local_hash(&mut self, path: &str) -> Result<Option<String>> {
        let file = self.absolute(path);
        let Ok(meta) = fs::metadata(&file) else { return Ok(None) };
        let mtime = meta.modified().ok().and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map_or(0, |d| d.as_nanos());
        if let Some((size, m, hash)) = self.state.stat.get(path) {
            if *size == meta.len() && *m == mtime {
                return Ok(Some(hash.clone()));
            }
        }
        let bytes = match fs::read(&file) {
            Ok(b) => b,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(e.into()),
        };
        let hash = vault::hash(&bytes);
        self.state.stat.insert(path.to_owned(), (meta.len(), mtime, hash.clone()));
        Ok(Some(hash))
    }

    /// Writes a file in the workspace atomically; `readonly`: as a hint that it is not checked out.
    fn write(&mut self, path: &str, bytes: &[u8], readonly: bool) -> Result<()> {
        let file = self.absolute(path);
        if let Some(dir) = file.parent() {
            fs::create_dir_all(dir)?;
        }
        // Windows does not rename over a read-only file.
        set_readonly(&file, false);
        let tmp = file.with_file_name(format!(".{}.pdm-get", file.file_name().map(|n| n.to_string_lossy()).unwrap_or_default()));
        fs::write(&tmp, bytes)?;
        fs::rename(&tmp, &file)?;
        set_readonly(&file, readonly);
        self.state.stat.remove(path);
        Ok(())
    }

    fn vault_path(&self, file: &Path) -> Option<String> {
        let rel = file.strip_prefix(&self.root).ok()?;
        Some(rel.components().map(|c| c.as_os_str().to_string_lossy()).collect::<Vec<_>>().join("/"))
    }

    fn save(&self) -> Result<()> {
        let dir = self.root.join(WORKSPACE_DIR);
        let tmp = dir.join("workspace.json.tmp");
        fs::write(&tmp, serde_json::to_vec_pretty(&self.state).map_err(|e| Error::Local(e.to_string()))?)?;
        fs::rename(tmp, dir.join("workspace.json"))?;
        Ok(())
    }

    fn save_session(&self) -> Result<()> {
        let file = self.root.join(WORKSPACE_DIR).join("session");
        fs::write(&file, self.client.token().unwrap_or(""))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&file, fs::Permissions::from_mode(0o600))?;
        }
        Ok(())
    }
}

impl Error {
    /// Whether logging in again would help.
    pub fn needs_login(&self) -> bool {
        self.code() == Some(ErrorCode::Unauthorized)
    }
}

fn set_readonly(file: &Path, readonly: bool) {
    if let Ok(meta) = fs::metadata(file) {
        let mut perms = meta.permissions();
        if perms.readonly() != readonly {
            #[allow(clippy::permissions_set_readonly_false)]
            perms.set_readonly(readonly);
            let _ = fs::set_permissions(file, perms);
        }
    }
}

fn dunce_canonical(path: &Path) -> Result<PathBuf> {
    let abs = path.canonicalize()?;
    #[cfg(windows)]
    {
        if let Some(s) = abs.to_str().and_then(|s| s.strip_prefix(r"\\?\")) {
            if !s.starts_with("UNC") {
                return Ok(PathBuf::from(s));
            }
        }
    }
    Ok(abs)
}
