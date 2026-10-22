//! Deleted designs on the server: gone for everyone at once, kept for a number of days (7 by
//! default; on the administration page) in which an administrator can restore them, then removed
//! for good by an hourly clean-up.

use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use serde::Serialize;

use crate::Server;

const DEFAULT_DAYS: u32 = 7;
pub const MAX_DAYS: u32 = 365;

/// A deleted design, for the administration page.
#[derive(Clone, Debug, Serialize)]
pub struct DeletedDesign {
    pub vault: String,
    pub id: i64,
    pub path: String,
    pub deleted_by: String,
    pub deleted_at: i64,
    /// When the clean-up removes it for good.
    pub purge_at: i64,
    pub versions: u32,
    pub size: u64,
}

impl Server {
    /// How many days deleted designs are kept.
    pub fn trash_days(&self) -> anyhow::Result<u32> {
        Ok(self.accounts.setting("trash_days")?.and_then(|v| v.parse().ok()).unwrap_or(DEFAULT_DAYS))
    }

    pub fn set_trash_days(&self, days: u32) -> anyhow::Result<()> {
        anyhow::ensure!((1..=MAX_DAYS).contains(&days), "keep deleted designs 1 to {MAX_DAYS} days");
        self.accounts.set_setting("trash_days", &days.to_string())
    }

    /// Every vault's deleted designs, most recently deleted first.
    pub fn deleted_designs(&self) -> anyhow::Result<Vec<DeletedDesign>> {
        let keep = i64::from(self.trash_days()?) * 86400;
        let mut out = Vec::new();
        for name in self.vault_names()? {
            let Some(shared) = self.shared(&name) else { continue };
            let vault = shared.vault.lock().unwrap_or_else(|e| e.into_inner());
            for d in vault.deleted()? {
                out.push(DeletedDesign {
                    vault: name.clone(),
                    id: d.id,
                    path: d.path,
                    deleted_by: d.deleted_by,
                    deleted_at: d.deleted_at,
                    purge_at: d.deleted_at + keep,
                    versions: d.latest.as_ref().map_or(0, |v| v.number),
                    size: d.latest.as_ref().map_or(0, |v| v.size),
                });
            }
        }
        out.sort_by(|a, b| b.deleted_at.cmp(&a.deleted_at));
        Ok(out)
    }

    /// Brings a deleted design back; the path it got (beside a design that took its name).
    pub fn restore_deleted(&self, vault: &str, id: i64) -> anyhow::Result<String> {
        let shared = self.shared(vault).ok_or_else(|| anyhow::anyhow!("no vault {vault}"))?;
        let path = shared.vault.lock().unwrap_or_else(|e| e.into_inner()).restore_deleted(id)?;
        shared.counter.fetch_add(1, Ordering::SeqCst);
        Ok(path)
    }

    /// Removes a deleted design for good, now.
    pub fn purge_deleted(&self, vault: &str, id: i64) -> anyhow::Result<()> {
        let shared = self.shared(vault).ok_or_else(|| anyhow::anyhow!("no vault {vault}"))?;
        shared.vault.lock().unwrap_or_else(|e| e.into_inner()).purge_one(id)?;
        Ok(())
    }

    /// Removes what was deleted longer ago than the days kept; how many designs.
    pub fn purge_old_deleted(&self) -> anyhow::Result<usize> {
        let before = crate::now() - i64::from(self.trash_days()?) * 86400;
        let mut purged = 0;
        for name in self.vault_names()? {
            let Some(shared) = self.shared(&name) else { continue };
            purged += shared.vault.lock().unwrap_or_else(|e| e.into_inner()).purge_deleted(before)?;
        }
        Ok(purged)
    }
}

/// The hourly clean-up, until the server stops.
pub(crate) async fn schedule(server: Arc<Server>) {
    loop {
        let s = server.clone();
        match tokio::task::spawn_blocking(move || s.purge_old_deleted()).await {
            Ok(Ok(n)) if n > 0 => println!("removed {n} deleted design(s) for good"),
            Ok(Err(e)) => eprintln!("cleaning up deleted designs: {e:#}"),
            _ => {}
        }
        tokio::time::sleep(Duration::from_secs(3600)).await;
    }
}
