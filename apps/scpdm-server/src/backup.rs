//! Backups of the whole server (users and every vault) into another folder: a second disk, or
//! an SMB or NFS share mounted on the server.
//!
//! Each backup is a folder `pdm-backup-<UTC time>` holding a data folder the server can run
//! from as it is (`server.db`, `vaults/<name>/`), and `backup.json` describing it:
//!
//! - **Consistent:** the databases are copied with SQLite's `VACUUM INTO`, into the data
//!   folder first (SQLite and network file systems do not mix), then onto the target. A vault
//!   is held while its database and working files are copied, so no check-in lands half-way;
//!   its version files, which never change once written, are copied before that.
//! - **Small:** a version file already in the previous backup is hard-linked to it, where the
//!   target can (SMB often cannot: then it is copied).
//! - **Careful with the target:** a backup is written as `.partial-…` and renamed when complete;
//!   the target folder itself is never created (an unmounted share then fails, instead of
//!   filling the server's own disk); only folders this server made are ever removed.

use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use rusqlite::{OptionalExtension, params};
use serde::{Deserialize, Serialize};

use crate::Server;

/// Marks a folder as holding this server's backups.
const MARKER: &str = ".solidcraft-pdm-backups";
const PREFIX: &str = "pdm-backup-";
const PARTIAL: &str = ".partial-";
const MANIFEST: &str = "backup.json";
/// After a failed backup, the next try comes this soon (or after the interval, if shorter).
const RETRY_SECONDS: i64 = 3600;

/// How backups run (kept in `server.db`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub enabled: bool,
    /// Every this many hours (1 to 168).
    pub interval_hours: u32,
    /// The newest this many backups are kept (1 to 1000) …
    pub keep_count: u32,
    /// … and none older than this many days (0: no age limit). The newest is always kept.
    pub keep_days: u32,
    /// Where to, unless the server was started with one (`--backup-dir`).
    pub target: Option<String>,
}

impl Default for Settings {
    fn default() -> Self {
        Settings { enabled: false, interval_hours: 24, keep_count: 14, keep_days: 0, target: None }
    }
}

impl Settings {
    pub fn check(&self) -> Result<(), String> {
        if !(1..=168).contains(&self.interval_hours) {
            return Err("back up every 1 to 168 hours".into());
        }
        if !(1..=1000).contains(&self.keep_count) {
            return Err("keep 1 to 1000 backups".into());
        }
        if self.keep_days > 3650 {
            return Err("keep backups for at most 3650 days".into());
        }
        Ok(())
    }
}

/// One backup run, as recorded.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Run {
    pub id: i64,
    pub started_at: i64,
    pub finished_at: i64,
    pub ok: bool,
    /// "schedule", "page" or "command line".
    pub trigger: String,
    pub name: Option<String>,
    /// Files in the backup, their bytes, and the bytes actually written (not linked).
    pub files: u64,
    pub bytes: u64,
    pub copied: u64,
    pub message: String,
}

/// A backup in the target folder.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Stored {
    pub name: String,
    pub created_at: i64,
    pub files: u64,
    pub bytes: u64,
    pub vaults: Vec<String>,
}

fn now() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64)
}

impl Server {
    pub fn backup_settings(&self) -> anyhow::Result<Settings> {
        Ok(match self.accounts.setting("backup")? {
            Some(json) => serde_json::from_str(&json)?,
            // Started with a backup folder (as in Docker): backups are on until switched off.
            None => Settings { enabled: self.backup_dir.is_some(), ..Settings::default() },
        })
    }

    pub fn set_backup_settings(&self, settings: &Settings) -> anyhow::Result<()> {
        settings.check().map_err(anyhow::Error::msg)?;
        if self.backup_dir.is_none() {
            if let Some(target) = settings.target.as_deref().filter(|t| !t.trim().is_empty()) {
                self.check_target(Path::new(target.trim()))?;
            }
        }
        self.accounts.set_setting("backup", &serde_json::to_string(settings)?)?;
        self.backup_wake.notify_one();
        Ok(())
    }

    /// Where backups go: the folder the server was started with, else the one set on the page.
    pub fn backup_target(&self) -> anyhow::Result<Option<PathBuf>> {
        if let Some(dir) = &self.backup_dir {
            return Ok(Some(dir.clone()));
        }
        Ok(self.backup_settings()?.target.map(|t| PathBuf::from(t.trim())).filter(|t| !t.as_os_str().is_empty()))
    }

    /// Whether the target is fixed by how the server was started.
    pub fn backup_target_fixed(&self) -> bool {
        self.backup_dir.is_some()
    }

    fn check_target(&self, target: &Path) -> anyhow::Result<()> {
        anyhow::ensure!(target.is_absolute(), "{}: give the full path", target.display());
        anyhow::ensure!(
            target.is_dir(),
            "{} does not exist (or is not a folder): create it, or mount the share",
            target.display()
        );
        let (target, data) = (fs::canonicalize(target)?, fs::canonicalize(&self.data)?);
        anyhow::ensure!(!target.starts_with(&data) && !data.starts_with(&target), "the backups need a folder outside the data folder ({})", data.display());
        Ok(())
    }

    /// Whether the target is on the same disk as the data folder (then a backup does not
    /// survive that disk, and an unmounted share's folder looks just like it).
    pub fn backup_target_same_disk(&self) -> Option<bool> {
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            let target = self.backup_target().ok()??;
            Some(fs::metadata(target).ok()?.dev() == fs::metadata(&self.data).ok()?.dev())
        }
        #[cfg(not(unix))]
        None
    }

    pub fn backup_running(&self) -> bool {
        self.backup_busy.load(Ordering::SeqCst)
    }

    /// The recorded runs, newest first.
    pub fn backup_runs(&self, limit: usize) -> anyhow::Result<Vec<Run>> {
        self.accounts.with_db(|db| {
            let mut stmt = db.prepare(
                "SELECT id, started_at, finished_at, ok, trigger, name, files, bytes, copied, message
                 FROM backup_runs ORDER BY id DESC LIMIT ?1",
            )?;
            let rows = stmt.query_map([limit as i64], |r| {
                Ok(Run {
                    id: r.get(0)?,
                    started_at: r.get(1)?,
                    finished_at: r.get(2)?,
                    ok: r.get(3)?,
                    trigger: r.get(4)?,
                    name: r.get(5)?,
                    files: r.get::<_, i64>(6)? as u64,
                    bytes: r.get::<_, i64>(7)? as u64,
                    copied: r.get::<_, i64>(8)? as u64,
                    message: r.get(9)?,
                })
            })?;
            rows.collect()
        })
    }

    /// How many runs there were, and how many succeeded.
    pub fn backup_counts(&self) -> anyhow::Result<(u64, u64)> {
        self.accounts.with_db(|db| {
            db.query_row("SELECT COUNT(*), COALESCE(SUM(ok), 0) FROM backup_runs", [], |r| Ok((r.get::<_, i64>(0)? as u64, r.get::<_, i64>(1)? as u64)))
        })
    }

    fn last_run(&self) -> anyhow::Result<Option<(i64, bool)>> {
        self.accounts.with_db(|db| db.query_row("SELECT started_at, ok FROM backup_runs ORDER BY id DESC LIMIT 1", [], |r| Ok((r.get(0)?, r.get(1)?))).optional())
    }

    /// When the schedule runs the next backup (Unix seconds); `None` while switched off.
    pub fn next_backup(&self) -> anyhow::Result<Option<i64>> {
        let settings = self.backup_settings()?;
        if !settings.enabled || self.backup_target()?.is_none() {
            return Ok(None);
        }
        let interval = settings.interval_hours as i64 * 3600;
        Ok(Some(match self.last_run()? {
            None => self.started + 60,
            Some((at, true)) => at + interval,
            Some((at, false)) => at + interval.min(RETRY_SECONDS),
        }))
    }

    /// The backups in the target folder, newest first.
    pub fn stored_backups(&self) -> anyhow::Result<Vec<Stored>> {
        let Some(target) = self.backup_target()? else { return Ok(Vec::new()) };
        Ok(stored(&target))
    }

    /// Backs everything up now (unless a backup is running already); the run as recorded.
    pub fn back_up(&self, trigger: &str) -> anyhow::Result<Run> {
        anyhow::ensure!(!self.backup_busy.swap(true, Ordering::SeqCst), "a backup is running already");
        // Cleared however the run ends.
        struct Busy<'a>(&'a std::sync::atomic::AtomicBool);
        impl Drop for Busy<'_> {
            fn drop(&mut self) {
                self.0.store(false, Ordering::SeqCst);
            }
        }
        let busy = Busy(&self.backup_busy);
        let started_at = now();
        let result = self.target_for_run().and_then(|target| {
            let done = self.write_backup(&target)?;
            let settings = self.backup_settings()?;
            prune(&target, &settings);
            Ok(done)
        });
        drop(busy);
        let (ok, name, count, message) = match &result {
            Ok(done) => (true, Some(done.name.clone()), done.count, String::new()),
            Err(e) => (false, None, Count::default(), format!("{e:#}")),
        };
        let finished_at = now();
        let id = self.accounts.with_db(|db| {
            db.execute(
                "INSERT INTO backup_runs (started_at, finished_at, ok, trigger, name, files, bytes, copied, message)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                params![started_at, finished_at, ok, trigger, name, count.files as i64, count.bytes as i64, count.copied as i64, message],
            )?;
            Ok(db.last_insert_rowid())
        })?;
        let run = Run { id, started_at, finished_at, ok, trigger: trigger.into(), name, files: count.files, bytes: count.bytes, copied: count.copied, message };
        match &result {
            Ok(_) => println!("backup {}: {} files, {} bytes ({} written)", run.name.as_deref().unwrap_or("?"), run.files, run.bytes, run.copied),
            Err(_) => eprintln!("backup failed: {}", run.message),
        }
        Ok(run)
    }

    fn target_for_run(&self) -> anyhow::Result<PathBuf> {
        let target = self.backup_target()?.ok_or_else(|| anyhow::anyhow!("no backup folder set"))?;
        self.check_target(&target)?;
        Ok(target)
    }

    fn write_backup(&self, target: &Path) -> anyhow::Result<Done> {
        let marker = target.join(MARKER);
        if !marker.exists() {
            fs::write(&marker, "Backups of a SolidCraft PDM server. Each pdm-backup-… folder is a complete data folder.\n")
                .map_err(|e| anyhow::anyhow!("cannot write into {}: {e}", target.display()))?;
        }
        // Left over from a run that broke off.
        for entry in fs::read_dir(target)?.flatten() {
            if entry.file_name().to_string_lossy().starts_with(PARTIAL) {
                let _ = fs::remove_dir_all(entry.path());
            }
        }
        let name = format!("{PREFIX}{}", chrono::Utc::now().format("%Y%m%dT%H%M%S%.3fZ"));
        let partial = target.join(format!("{PARTIAL}{name}"));
        let previous = stored(target).first().map(|b| target.join(&b.name));
        let mut copier = Copier { previous, count: Count::default(), links: true };
        fs::create_dir(&partial).map_err(|e| anyhow::anyhow!("cannot write into {}: {e}", target.display()))?;

        let result = (|| -> anyhow::Result<Vec<String>> {
            // Database copies are made in the data folder (a local disk), then copied over.
            let scratch = self.data.join(".backup-scratch");
            let _ = fs::remove_dir_all(&scratch);
            fs::create_dir_all(&scratch)?;
            let db_copy = scratch.join("server.db");
            self.accounts.backup_db(&db_copy)?;
            copier.file(&db_copy, &partial.join("server.db"), None)?;

            let names = self.vault_names()?;
            for vault in &names {
                let Some(shared) = self.shared(vault) else { continue };
                let from = self.data.join("vaults").join(vault);
                let to = partial.join("vaults").join(vault);
                let objects = Path::new(vault::PDM_DIR).join("objects");
                // Version files first, while people go on working: they never change.
                copier.tree(&from.join(&objects), &to.join(&objects), &Path::new("vaults").join(vault).join(&objects))?;
                let db_copy = scratch.join(format!("{vault}.db"));
                {
                    let held = shared.vault.lock().unwrap_or_else(|e| e.into_inner());
                    held.backup_db(&db_copy)?;
                    // The working files and anything else, and versions added meanwhile.
                    copier.tree_except(&from, &to, &Path::new("vaults").join(vault), &[Path::new(vault::PDM_DIR).join("vault.db")])?;
                }
                copier.file(&db_copy, &to.join(vault::PDM_DIR).join("vault.db"), None)?;
                fs::remove_file(&db_copy)?;
            }
            let _ = fs::remove_dir_all(&scratch);
            Ok(names)
        })();
        let vaults = match result {
            Ok(v) => v,
            Err(e) => {
                let _ = fs::remove_dir_all(&partial);
                return Err(e);
            }
        };
        let manifest = Stored { name: name.clone(), created_at: now(), files: copier.count.files, bytes: copier.count.bytes, vaults };
        {
            let mut file = fs::File::create(partial.join(MANIFEST))?;
            file.write_all(serde_json::to_string_pretty(&manifest)?.as_bytes())?;
            file.sync_all()?;
        }
        rename_when_closed(&partial, &target.join(&name)).map_err(|e| anyhow::anyhow!("cannot finish the backup {name}: {e}"))?;
        Ok(Done { name, count: copier.count })
    }
}

struct Done {
    name: String,
    count: Count,
}

#[derive(Clone, Copy, Debug, Default)]
struct Count {
    files: u64,
    bytes: u64,
    copied: u64,
}

/// Copies files into a backup, linking unchanged version files to the previous backup.
struct Copier {
    previous: Option<PathBuf>,
    count: Count,
    /// Cleared after the first hard link the target refuses.
    links: bool,
}

impl Copier {
    /// Copies `from` to `to`; `relative` (its path in a backup) lets it be linked to the
    /// previous backup's file of that path and size.
    fn file(&mut self, from: &Path, to: &Path, relative: Option<&Path>) -> anyhow::Result<()> {
        let size = fs::metadata(from)?.len();
        if let Some(dir) = to.parent() {
            fs::create_dir_all(dir).map_err(|e| anyhow::anyhow!("{}: {e}", dir.display()))?;
        }
        self.count.files += 1;
        self.count.bytes += size;
        if let (true, Some(previous), Some(relative)) = (self.links, &self.previous, relative) {
            let old = previous.join(relative);
            if fs::metadata(&old).is_ok_and(|m| m.len() == size) {
                match fs::hard_link(&old, to) {
                    Ok(()) => return Ok(()),
                    // Shares without hard links: copy from now on.
                    Err(_) => self.links = false,
                }
            }
        }
        // A plain copy: no permissions or times carried over, which shares may refuse.
        let mut source = fs::File::open(from)?;
        let mut target = io::BufWriter::with_capacity(1 << 20, fs::File::create(to).map_err(|e| anyhow::anyhow!("{}: {e}", to.display()))?);
        io::copy(&mut source, &mut target)?;
        target.into_inner().map_err(|e| e.into_error())?.sync_data()?;
        self.count.copied += size;
        Ok(())
    }

    /// Copies a folder's files (those not there already); version files may be linked.
    fn tree(&mut self, from: &Path, to: &Path, relative: &Path) -> anyhow::Result<()> {
        self.tree_except(from, to, relative, &[])
    }

    fn tree_except(&mut self, from: &Path, to: &Path, relative: &Path, except: &[PathBuf]) -> anyhow::Result<()> {
        self.walk(from, to, relative, Path::new(""), except)
    }

    fn walk(&mut self, from: &Path, to: &Path, relative: &Path, inner: &Path, except: &[PathBuf]) -> anyhow::Result<()> {
        let entries = match fs::read_dir(from.join(inner)) {
            Ok(e) => e,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(()),
            Err(e) => return Err(e.into()),
        };
        for entry in entries {
            let entry = entry?;
            let name = entry.file_name();
            let inner = inner.join(&name);
            // The live database's journal: the copy comes from VACUUM INTO.
            if except.iter().any(|e| inner == *e) || name.to_string_lossy().starts_with("vault.db-") {
                continue;
            }
            let kind = entry.file_type()?;
            if kind.is_dir() {
                self.walk(from, to, relative, &inner, except)?;
            } else if kind.is_file() && !to.join(&inner).exists() {
                let linkable = inner.starts_with(Path::new(vault::PDM_DIR).join("objects")) || relative.ends_with("objects");
                self.file(&entry.path(), &to.join(&inner), linkable.then(|| relative.join(&inner)).as_deref())?;
            }
        }
        Ok(())
    }
}

/// Renames a finished backup. An SMB share refuses that while a file in it is open, and the
/// client keeps closed files open for a moment (`closetimeo`), so it is tried for a while.
fn rename_when_closed(from: &Path, to: &Path) -> io::Result<()> {
    let mut tries = 0;
    loop {
        match fs::rename(from, to) {
            Err(e) if e.kind() == io::ErrorKind::PermissionDenied && tries < 40 => {
                tries += 1;
                std::thread::sleep(Duration::from_millis(250));
            }
            result => return result,
        }
    }
}

/// The complete backups in `target`, newest first.
fn stored(target: &Path) -> Vec<Stored> {
    let Ok(entries) = fs::read_dir(target) else { return Vec::new() };
    let mut backups: Vec<Stored> = entries
        .flatten()
        .filter(|e| e.file_name().to_string_lossy().starts_with(PREFIX))
        .filter_map(|e| serde_json::from_slice::<Stored>(&fs::read(e.path().join(MANIFEST)).ok()?).ok().filter(|m| m.name == e.file_name().to_string_lossy()))
        .collect();
    backups.sort_by(|a, b| b.name.cmp(&a.name));
    backups
}

/// Removes backups beyond what `settings` keep; never the newest.
fn prune(target: &Path, settings: &Settings) {
    let cutoff = (settings.keep_days > 0).then(|| now() - settings.keep_days as i64 * 86400);
    for (i, backup) in stored(target).into_iter().enumerate() {
        let too_many = i >= settings.keep_count as usize;
        let too_old = cutoff.is_some_and(|c| backup.created_at < c);
        if i > 0 && (too_many || too_old) {
            if let Err(e) = fs::remove_dir_all(target.join(&backup.name)) {
                eprintln!("could not remove the old backup {}: {e}", backup.name);
            }
        }
    }
}

/// Runs the schedule until the server stops: a backup whenever one is due.
pub(crate) async fn schedule(server: Arc<Server>) {
    loop {
        let next = server.next_backup().unwrap_or(None);
        let wait = match next {
            Some(at) => Duration::from_secs((at - now()).clamp(0, 3600) as u64),
            None => Duration::from_secs(3600),
        };
        let due = next.is_some_and(|at| at <= now());
        if !due {
            // Woken early when the settings change; checked again at least hourly.
            let _ = tokio::time::timeout(wait, server.backup_wake.notified()).await;
            continue;
        }
        let s = server.clone();
        let _ = tokio::task::spawn_blocking(move || s.back_up("schedule")).await;
    }
}
