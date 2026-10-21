//! Users, sessions and who may use which vault: `server.db` in the data folder.

use std::path::Path;
use std::sync::Mutex;

use argon2::password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString};
use argon2::Argon2;
use protocol::{Access, Session};
use rand::RngCore;
use rusqlite::{Connection, OptionalExtension, params};

/// How long a login lasts.
const SESSION_SECONDS: i64 = 30 * 24 * 3600;

#[derive(Clone, Debug, PartialEq)]
pub struct User {
    pub id: i64,
    pub name: String,
    pub admin: bool,
}

pub struct Accounts {
    db: Mutex<Connection>,
}

/// A user as the administration page lists them.
#[derive(Clone, Debug, PartialEq, serde::Serialize)]
pub struct UserInfo {
    pub name: String,
    pub admin: bool,
    /// The last request with one of their logins (Unix seconds), and how many logins are open.
    pub last_seen: Option<i64>,
    pub sessions: u64,
    /// Vault → "read", "write" or "admin".
    pub access: std::collections::BTreeMap<String, String>,
}

/// How often a session's `last_seen` is written at most (seconds).
const SEEN_EVERY: i64 = 60;

/// User and vault names: letters, digits, `.`, `_`, `-` (vault names are folder names).
pub fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && !name.starts_with('.')
        && name.bytes().all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
}

fn now() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64)
}

fn token_hash(token: &str) -> String {
    vault::hash(token.as_bytes())
}

impl Accounts {
    pub fn open(data: &Path) -> anyhow::Result<Self> {
        std::fs::create_dir_all(data)?;
        let db = Connection::open(data.join("server.db"))?;
        db.busy_timeout(std::time::Duration::from_secs(5))?;
        db.execute_batch(
            "CREATE TABLE IF NOT EXISTS users (
                 id       INTEGER PRIMARY KEY,
                 name     TEXT NOT NULL UNIQUE,
                 password TEXT NOT NULL,
                 admin    INTEGER NOT NULL
             );
             CREATE TABLE IF NOT EXISTS sessions (
                 token_hash TEXT PRIMARY KEY,
                 user_id    INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
                 expires_at INTEGER NOT NULL
             );
             CREATE TABLE IF NOT EXISTS grants (
                 user_id INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
                 vault   TEXT NOT NULL,
                 access  TEXT NOT NULL,
                 PRIMARY KEY (user_id, vault)
             );
             PRAGMA foreign_keys = ON;",
        )?;
        let columns: Vec<String> =
            db.prepare("SELECT name FROM pragma_table_info('sessions')")?.query_map([], |r| r.get(0))?.collect::<rusqlite::Result<_>>()?;
        if !columns.iter().any(|c| c == "last_seen") {
            db.execute_batch("ALTER TABLE sessions ADD COLUMN last_seen INTEGER NOT NULL DEFAULT 0;")?;
        }
        Ok(Accounts { db: Mutex::new(db) })
    }

    fn db(&self) -> std::sync::MutexGuard<'_, Connection> {
        self.db.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn add_user(&self, name: &str, password: &str, admin: bool) -> anyhow::Result<()> {
        anyhow::ensure!(valid_name(name), "{name:?}: use letters, digits, '.', '_' and '-'");
        anyhow::ensure!(password.chars().count() >= 8, "the password needs at least 8 characters");
        let hash = hash_password(password)?;
        self.db()
            .execute("INSERT INTO users (name, password, admin) VALUES (?1, ?2, ?3)", params![name, hash, admin])
            .map_err(|e| anyhow::anyhow!("{name}: {e}"))?;
        Ok(())
    }

    pub fn set_password(&self, name: &str, password: &str) -> anyhow::Result<()> {
        anyhow::ensure!(password.chars().count() >= 8, "the password needs at least 8 characters");
        let hash = hash_password(password)?;
        let db = self.db();
        let n = db.execute("UPDATE users SET password = ?2 WHERE name = ?1", params![name, hash])?;
        anyhow::ensure!(n == 1, "no user {name}");
        // A new password ends the old logins.
        db.execute("DELETE FROM sessions WHERE user_id = (SELECT id FROM users WHERE name = ?1)", [name])?;
        Ok(())
    }

    pub fn remove_user(&self, name: &str) -> anyhow::Result<()> {
        let n = self.db().execute("DELETE FROM users WHERE name = ?1", [name])?;
        anyhow::ensure!(n == 1, "no user {name}");
        Ok(())
    }

    /// Every user, with whether they are an administrator.
    pub fn users(&self) -> anyhow::Result<Vec<(String, bool)>> {
        let db = self.db();
        let mut stmt = db.prepare("SELECT name, admin FROM users ORDER BY name")?;
        let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Checks the password; a new session on success.
    pub fn login(&self, name: &str, password: &str) -> anyhow::Result<Option<Session>> {
        let row: Option<(i64, String, bool)> = self
            .db()
            .query_row("SELECT id, password, admin FROM users WHERE name = ?1", [name], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
            .optional()?;
        let Some((id, hash, admin)) = row else {
            // As long as a real check, so the time does not tell which names exist.
            let _ = verify_password(password, dummy_hash());
            return Ok(None);
        };
        if !verify_password(password, &hash) {
            return Ok(None);
        }
        let mut bytes = [0u8; 32];
        rand::rng().fill_bytes(&mut bytes);
        let token: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
        let db = self.db();
        db.execute("DELETE FROM sessions WHERE expires_at < ?1", [now()])?;
        db.execute(
            "INSERT INTO sessions (token_hash, user_id, expires_at, last_seen) VALUES (?1, ?2, ?3, ?4)",
            params![token_hash(&token), id, now() + SESSION_SECONDS, now()],
        )?;
        Ok(Some(Session { token, user: name.to_owned(), admin }))
    }

    /// The user of a valid session (which counts as seen now).
    pub fn session(&self, token: &str) -> anyhow::Result<Option<User>> {
        let db = self.db();
        let hash = token_hash(token);
        let user = db
            .query_row(
                "SELECT u.id, u.name, u.admin FROM sessions s JOIN users u ON u.id = s.user_id
                 WHERE s.token_hash = ?1 AND s.expires_at >= ?2",
                params![hash, now()],
                |r| Ok(User { id: r.get(0)?, name: r.get(1)?, admin: r.get(2)? }),
            )
            .optional()?;
        if user.is_some() {
            db.execute("UPDATE sessions SET last_seen = ?2 WHERE token_hash = ?1 AND last_seen < ?2 - ?3", params![hash, now(), SEEN_EVERY])?;
        }
        Ok(user)
    }

    /// Every user with their access, logins and when they were last seen.
    pub fn user_infos(&self) -> anyhow::Result<Vec<UserInfo>> {
        let db = self.db();
        let mut stmt = db.prepare(
            "SELECT u.name, u.admin, MAX(s.last_seen), COUNT(s.token_hash) FROM users u
             LEFT JOIN sessions s ON s.user_id = u.id AND s.expires_at >= ?1
             GROUP BY u.id ORDER BY u.name",
        )?;
        let mut users: Vec<UserInfo> = stmt
            .query_map([now()], |r| {
                Ok(UserInfo {
                    name: r.get(0)?,
                    admin: r.get(1)?,
                    last_seen: r.get::<_, Option<i64>>(2)?.filter(|t| *t > 0),
                    sessions: r.get::<_, i64>(3)? as u64,
                    access: Default::default(),
                })
            })?
            .collect::<rusqlite::Result<_>>()?;
        let mut stmt = db.prepare("SELECT u.name, g.vault, g.access FROM grants g JOIN users u ON u.id = g.user_id")?;
        for row in stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?)))? {
            let (user, vault, access) = row?;
            if let Some(u) = users.iter_mut().find(|u| u.name == user) {
                u.access.insert(vault, access);
            }
        }
        Ok(users)
    }

    pub fn set_admin(&self, name: &str, admin: bool) -> anyhow::Result<()> {
        let n = self.db().execute("UPDATE users SET admin = ?2 WHERE name = ?1", params![name, admin])?;
        anyhow::ensure!(n == 1, "no user {name}");
        Ok(())
    }

    /// Ends every login of `name` (the app asks for the password again).
    pub fn end_sessions(&self, name: &str) -> anyhow::Result<u64> {
        let n = self.db().execute("DELETE FROM sessions WHERE user_id = (SELECT id FROM users WHERE name = ?1)", [name])?;
        Ok(n as u64)
    }

    pub fn logout(&self, token: &str) -> anyhow::Result<()> {
        self.db().execute("DELETE FROM sessions WHERE token_hash = ?1", [token_hash(token)])?;
        Ok(())
    }

    pub fn grant(&self, user: &str, vault: &str, access: Access) -> anyhow::Result<()> {
        let value = access_name(access);
        let n = self.db().execute(
            "INSERT OR REPLACE INTO grants (user_id, vault, access) SELECT id, ?2, ?3 FROM users WHERE name = ?1",
            params![user, vault, value],
        )?;
        anyhow::ensure!(n == 1, "no user {user}");
        Ok(())
    }

    pub fn revoke(&self, user: &str, vault: &str) -> anyhow::Result<()> {
        self.db().execute("DELETE FROM grants WHERE vault = ?2 AND user_id = (SELECT id FROM users WHERE name = ?1)", params![user, vault])?;
        Ok(())
    }

    /// What `user` may do in `vault` (administrators: everything everywhere).
    pub fn access(&self, user: &User, vault: &str) -> anyhow::Result<Option<Access>> {
        if user.admin {
            return Ok(Some(Access::Admin));
        }
        let value: Option<String> = self
            .db()
            .query_row("SELECT access FROM grants WHERE user_id = ?1 AND vault = ?2", params![user.id, vault], |r| r.get(0))
            .optional()?;
        Ok(value.as_deref().and_then(parse_access))
    }
}

pub fn access_name(access: Access) -> &'static str {
    match access {
        Access::Read => "read",
        Access::Write => "write",
        Access::Admin => "admin",
    }
}

pub fn parse_access(s: &str) -> Option<Access> {
    match s {
        "read" => Some(Access::Read),
        "write" => Some(Access::Write),
        "admin" => Some(Access::Admin),
        _ => None,
    }
}

fn hash_password(password: &str) -> anyhow::Result<String> {
    let mut salt = [0u8; 16];
    rand::rng().fill_bytes(&mut salt);
    let salt = SaltString::encode_b64(&salt).map_err(|e| anyhow::anyhow!("{e}"))?;
    Ok(Argon2::default().hash_password(password.as_bytes(), &salt).map_err(|e| anyhow::anyhow!("{e}"))?.to_string())
}

fn verify_password(password: &str, hash: &str) -> bool {
    PasswordHash::new(hash).is_ok_and(|h| Argon2::default().verify_password(password.as_bytes(), &h).is_ok())
}

/// The Argon2 hash of a password nobody has, for timing-safe failures (made once).
fn dummy_hash() -> &'static str {
    static HASH: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    HASH.get_or_init(|| hash_password("not a password anyone has").unwrap_or_default())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn users_log_in_and_get_access() {
        let dir = tempfile::tempdir().unwrap();
        let a = Accounts::open(dir.path()).unwrap();
        a.add_user("anna", "correct horse", false).unwrap();
        a.add_user("root", "battery staple", true).unwrap();
        assert!(a.add_user("anna", "another one!", false).is_err(), "names are unique");
        assert!(a.add_user("../x", "long enough", false).is_err());
        assert!(a.add_user("ben", "short", false).is_err());

        assert_eq!(a.login("anna", "wrong password").unwrap(), None);
        assert_eq!(a.login("nobody", "correct horse").unwrap(), None);
        let s = a.login("anna", "correct horse").unwrap().unwrap();
        assert_eq!((s.user.as_str(), s.admin, s.token.len()), ("anna", false, 64));
        let anna = a.session(&s.token).unwrap().unwrap();
        assert_eq!(a.session("not a token").unwrap(), None);

        assert_eq!(a.access(&anna, "workshop").unwrap(), None);
        a.grant("anna", "workshop", Access::Write).unwrap();
        assert_eq!(a.access(&anna, "workshop").unwrap(), Some(Access::Write));
        let root = a.session(&a.login("root", "battery staple").unwrap().unwrap().token).unwrap().unwrap();
        assert_eq!(a.access(&root, "anything").unwrap(), Some(Access::Admin));

        a.set_password("anna", "new password").unwrap();
        assert_eq!(a.session(&s.token).unwrap(), None, "a new password ends old logins");
        a.logout(&s.token).unwrap();
        a.remove_user("anna").unwrap();
        assert_eq!(a.users().unwrap(), [("root".to_owned(), true)]);
    }

    #[test]
    fn the_dummy_hash_is_a_valid_hash() {
        assert!(PasswordHash::new(dummy_hash()).is_ok());
    }
}
