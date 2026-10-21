//! The SolidCraft PDM server: vaults shared by a small team, with check-out locks.
//!
//! Data folder:
//!
//! ```text
//! server.db            users, sessions, who may use which vault (`accounts`)
//! vaults/<name>/       each an ordinary vault (`crates/vault`) whose working files are the
//!                      latest versions; people work in workspaces of their own (`crates/remote`)
//! ```
//!
//! The API is described in `crates/protocol`; the administration page is in `admin`.

pub mod accounts;
mod admin;
mod api;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicU64;
use std::sync::{Arc, Mutex};

use vault::Vault;

pub use accounts::Accounts;

pub struct Server {
    pub name: String,
    data: PathBuf,
    pub accounts: Accounts,
    vaults: Mutex<HashMap<String, Arc<SharedVault>>>,
    /// When it started (Unix seconds), for the administration page.
    started: i64,
}

/// A vault in use: one change at a time; the counter tells clients whether to fetch it again.
struct SharedVault {
    vault: Mutex<Vault>,
    counter: AtomicU64,
    /// Reference images' hashes, by path, valid while size and modification time match.
    image_hashes: Mutex<HashMap<String, (u64, Option<std::time::SystemTime>, String)>>,
}

impl Server {
    pub fn open(data: &Path, name: &str) -> anyhow::Result<Arc<Server>> {
        std::fs::create_dir_all(data.join("vaults"))?;
        Ok(Arc::new(Server {
            name: name.to_owned(),
            data: data.to_path_buf(),
            accounts: Accounts::open(data)?,
            vaults: Mutex::new(HashMap::new()),
            started: std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64),
        }))
    }

    /// Creates a vault.
    pub fn add_vault(&self, name: &str) -> anyhow::Result<()> {
        anyhow::ensure!(accounts::valid_name(name), "{name:?}: use letters, digits, '.', '_' and '-'");
        let dir = self.data.join("vaults").join(name);
        anyhow::ensure!(!dir.exists(), "vault {name} exists already");
        std::fs::create_dir_all(&dir)?;
        Vault::init(&dir)?;
        Ok(())
    }

    /// Every vault's name, sorted.
    pub fn vault_names(&self) -> anyhow::Result<Vec<String>> {
        let mut names: Vec<String> = std::fs::read_dir(self.data.join("vaults"))?
            .filter_map(|e| e.ok())
            .filter(|e| e.path().join(vault::PDM_DIR).is_dir())
            .filter_map(|e| e.file_name().into_string().ok())
            .collect();
        names.sort();
        Ok(names)
    }

    fn shared(&self, name: &str) -> Option<Arc<SharedVault>> {
        if !accounts::valid_name(name) {
            return None;
        }
        let mut vaults = self.vaults.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(v) = vaults.get(name) {
            return Some(v.clone());
        }
        let vault = Vault::open(self.data.join("vaults").join(name)).ok()?;
        let start = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(1, |d| d.as_millis() as u64);
        let shared = Arc::new(SharedVault {
            vault: Mutex::new(vault),
            // Different after a restart, so clients fetch everything again.
            counter: AtomicU64::new(start),
            image_hashes: Mutex::new(HashMap::new()),
        });
        vaults.insert(name.to_owned(), shared.clone());
        Some(shared)
    }
}

/// Serves the API on `listener` until `shutdown` resolves.
pub async fn serve(
    server: Arc<Server>,
    listener: tokio::net::TcpListener,
    shutdown: impl std::future::Future<Output = ()> + Send + 'static,
) -> std::io::Result<()> {
    axum::serve(listener, api::router(server)).with_graceful_shutdown(shutdown).await
}

/// Starts a server on a free local port in a background thread; for tests. Returns its URL.
pub fn spawn_for_tests(server: Arc<Server>) -> String {
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let runtime = tokio::runtime::Builder::new_multi_thread().enable_all().build().expect("a tokio runtime");
        runtime.block_on(async move {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.expect("a local port");
            tx.send(listener.local_addr().expect("an address")).expect("the test waits");
            serve(server, listener, std::future::pending()).await.expect("the server runs");
        });
    });
    format!("http://{}", rx.recv().expect("the server started"))
}
