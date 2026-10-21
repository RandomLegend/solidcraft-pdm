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
            "INSERT INTO sessions (token_hash, user_id, expires_at) VALUES (?1, ?2, ?3)",
            params![token_hash(&token), id, now() + SESSION_SECONDS],
        )?;
        Ok(Some(Session { token, user: name.to_owned(), admin }))
    }

    /// The user of a valid session.
    pub fn session(&self, token: &str) -> anyhow::Result<Option<User>> {
        Ok(self
            .db()
            .query_row(
                "SELECT u.id, u.name, u.admin FROM sessions s JOIN users u ON u.id = s.user_id
                 WHERE s.token_hash = ?1 AND s.expires_at >= ?2",
                params![token_hash(token), now()],
                |r| Ok(User { id: r.get(0)?, name: r.get(1)?, admin: r.get(2)? }),
            )
            .optional()?)
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
