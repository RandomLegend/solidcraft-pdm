//! The administration page (`/admin`): a dashboard, users and their access, vaults. For
//! administrators only.
//!
//! The page (`admin/` next to this file, built into the program) talks to `/api/admin/…`.
//! Its login is a cookie (HttpOnly, SameSite=Strict); the app's Bearer tokens work too.
//! Requests that change something must carry `X-PDM-Admin: 1`, which another site's page
//! cannot send, so a link or form elsewhere cannot act with an administrator's cookie.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::Ordering;

use axum::Router;
use axum::extract::{FromRequestParts, Path, State};
use axum::http::{HeaderMap, HeaderValue, Method, StatusCode, header, request::Parts};
use axum::response::{IntoResponse, Redirect, Response};
use axum::routing::{get, post, put};
use protocol::{ErrorCode, Login};
use serde::{Deserialize, Serialize};

use crate::Server;
use crate::accounts::{self, User, UserInfo};
use crate::api::{Fail, Reply, fail, with_vault};

type Shared = Arc<Server>;

const COOKIE: &str = "pdm_admin";
/// The header a change must carry (see the module documentation).
const CHANGE_HEADER: &str = "x-pdm-admin";
/// Users seen this recently count as online.
const ONLINE_SECONDS: i64 = 5 * 60;

pub fn router() -> Router<Shared> {
    Router::new()
        .route("/", get(|| async { Redirect::to("/admin/") }))
        .route("/admin", get(|| async { Redirect::to("/admin/") }))
        .route("/admin/", get(|| async { page("text/html; charset=utf-8", include_str!("admin/index.html")) }))
        .route("/admin/app.js", get(|| async { page("text/javascript; charset=utf-8", include_str!("admin/app.js")) }))
        .route("/admin/app.css", get(|| async { page("text/css; charset=utf-8", include_str!("admin/app.css")) }))
        .route("/admin/icon.svg", get(|| async { page("image/svg+xml", include_str!("../../../assets/solidcraft-pdm.svg")) }))
        .route("/api/admin/login", post(login))
        .route("/api/admin/logout", post(logout))
        .route("/api/admin/me", get(me))
        .route("/api/admin/overview", get(overview))
        .route("/api/admin/users", get(users).post(add_user))
        .route("/api/admin/users/{name}", post(change_user).delete(remove_user))
        .route("/api/admin/users/{name}/access/{vault}", put(set_access))
        .route("/api/admin/vaults", get(vaults).post(add_vault))
        .route("/api/admin/vaults/{vault}/unlock", post(unlock))
        .route("/api/admin/vaults/{vault}/checkout", put(set_checkout))
        .route("/api/admin/backups", get(backups))
        .route("/api/admin/backups/settings", put(backup_settings))
        .route("/api/admin/backups/run", post(back_up_now))
}

/// A built-in file, with headers that keep it to itself (no framing, scripts only from here).
fn page(content_type: &'static str, body: &'static str) -> Response {
    let mut response = body.into_response();
    let headers = response.headers_mut();
    headers.insert(header::CONTENT_TYPE, HeaderValue::from_static(content_type));
    headers.insert(header::CONTENT_SECURITY_POLICY, HeaderValue::from_static("default-src 'self'; img-src 'self' data:; frame-ancestors 'none'"));
    headers.insert(header::X_CONTENT_TYPE_OPTIONS, HeaderValue::from_static("nosniff"));
    headers.insert(header::REFERRER_POLICY, HeaderValue::from_static("no-referrer"));
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-cache"));
    response
}

fn cookie_token(headers: &HeaderMap) -> Option<String> {
    headers.get_all(header::COOKIE).iter().filter_map(|v| v.to_str().ok()).flat_map(|v| v.split(';')).find_map(|pair| {
        let (name, value) = pair.trim().split_once('=')?;
        (name == COOKIE && !value.is_empty()).then(|| value.to_owned())
    })
}

/// An administrator's request (a change: with the change header).
struct Admin(User, String);

impl FromRequestParts<Shared> for Admin {
    type Rejection = Fail;

    async fn from_request_parts(parts: &mut Parts, server: &Shared) -> Result<Self, Self::Rejection> {
        let bearer = parts.headers.get(header::AUTHORIZATION).and_then(|v| v.to_str().ok()).and_then(|v| v.strip_prefix("Bearer "));
        let token = bearer
            .map(str::to_owned)
            .or_else(|| cookie_token(&parts.headers))
            .ok_or_else(|| fail(StatusCode::UNAUTHORIZED, ErrorCode::Unauthorized, "log in first"))?;
        let user = server
            .accounts
            .session(&token)?
            .ok_or_else(|| fail(StatusCode::UNAUTHORIZED, ErrorCode::Unauthorized, "the login has expired; log in again"))?;
        if !user.admin {
            return Err(fail(StatusCode::FORBIDDEN, ErrorCode::Forbidden, "only administrators"));
        }
        if !matches!(parts.method, Method::GET | Method::HEAD) && bearer.is_none() && !parts.headers.contains_key(CHANGE_HEADER) {
            return Err(fail(StatusCode::FORBIDDEN, ErrorCode::Forbidden, "missing the X-PDM-Admin header"));
        }
        Ok(Admin(user, token))
    }
}

fn blocking_error(e: tokio::task::JoinError) -> Fail {
    fail(StatusCode::INTERNAL_SERVER_ERROR, ErrorCode::Internal, e.to_string())
}

/// Account work (SQLite, Argon2) on the blocking pool.
async fn accounts<T: Send + 'static>(server: &Shared, work: impl FnOnce(&Server) -> anyhow::Result<T> + Send + 'static) -> Reply<T> {
    let server = server.clone();
    tokio::task::spawn_blocking(move || work(&server)).await.map_err(blocking_error)?.map_err(|e| fail(StatusCode::BAD_REQUEST, ErrorCode::Invalid, e.to_string()))
}

#[derive(Serialize)]
struct Me {
    user: String,
    server: String,
    version: &'static str,
}

fn me_of(server: &Server, user: &str) -> axum::Json<Me> {
    axum::Json(Me { user: user.to_owned(), server: server.name.clone(), version: env!("CARGO_PKG_VERSION") })
}

async fn login(State(server): State<Shared>, headers: HeaderMap, axum::Json(login): axum::Json<Login>) -> Reply<Response> {
    if !headers.contains_key(CHANGE_HEADER) {
        return Err(fail(StatusCode::FORBIDDEN, ErrorCode::Forbidden, "missing the X-PDM-Admin header"));
    }
    let (user, password) = (login.user.clone(), login.password);
    let session = accounts(&server, move |s| s.accounts.login(&user, &password)).await?;
    let Some(session) = session else {
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        return Err(fail(StatusCode::UNAUTHORIZED, ErrorCode::Unauthorized, "wrong user name or password"));
    };
    if !session.admin {
        let token = session.token.clone();
        accounts(&server, move |s| s.accounts.logout(&token)).await?;
        return Err(fail(StatusCode::FORBIDDEN, ErrorCode::Forbidden, "only administrators can use this page"));
    }
    // Secure when the browser came over HTTPS (through a proxy such as Caddy).
    let https = headers.get("x-forwarded-proto").and_then(|v| v.to_str().ok()) == Some("https");
    let cookie = format!(
        "{COOKIE}={}; HttpOnly; SameSite=Strict; Path=/; Max-Age={}{}",
        session.token,
        30 * 24 * 3600,
        if https { "; Secure" } else { "" }
    );
    let mut response = me_of(&server, &session.user).into_response();
    response.headers_mut().insert(header::SET_COOKIE, HeaderValue::from_str(&cookie).map_err(|e| fail(StatusCode::INTERNAL_SERVER_ERROR, ErrorCode::Internal, e.to_string()))?);
    Ok(response)
}

async fn logout(State(server): State<Shared>, Admin(_, token): Admin) -> Reply<Response> {
    accounts(&server, move |s| s.accounts.logout(&token)).await?;
    let mut response = StatusCode::NO_CONTENT.into_response();
    response.headers_mut().insert(header::SET_COOKIE, HeaderValue::from_static("pdm_admin=; HttpOnly; SameSite=Strict; Path=/; Max-Age=0"));
    Ok(response)
}

async fn me(State(server): State<Shared>, Admin(user, _): Admin) -> axum::Json<Me> {
    me_of(&server, &user.name)
}

#[derive(Serialize, Default)]
struct Totals {
    users: u64,
    admins: u64,
    online: u64,
    vaults: u64,
    designs: u64,
    versions: u64,
    releases: u64,
    checked_out: u64,
    stored_bytes: u64,
}

#[derive(Serialize)]
struct Activity {
    vault: String,
    path: String,
    number: u32,
    author: String,
    created_at: i64,
    comment: String,
}

#[derive(Serialize)]
struct CheckOut {
    vault: String,
    path: String,
    user: String,
    since: i64,
}

#[derive(Serialize)]
struct Overview {
    name: String,
    version: &'static str,
    started_at: i64,
    now: i64,
    data: String,
    disk_bytes: u64,
    totals: Totals,
    /// Versions per day for the last 30 days, oldest first (Unix day number, UTC).
    per_day: Vec<(i64, u64)>,
    recent: Vec<Activity>,
    checked_out: Vec<CheckOut>,
    backup: BackupSummary,
}

/// For the dashboard: whether backups run, and the last one.
#[derive(Serialize)]
struct BackupSummary {
    enabled: bool,
    last: Option<crate::backup::Run>,
    last_ok_at: Option<i64>,
}

fn now() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64)
}

/// The size of everything under `dir`.
fn disk_use(dir: &std::path::Path) -> u64 {
    let Ok(entries) = std::fs::read_dir(dir) else { return 0 };
    entries
        .filter_map(|e| e.ok())
        .map(|e| match e.file_type() {
            Ok(t) if t.is_dir() => disk_use(&e.path()),
            Ok(t) if t.is_file() => e.metadata().map_or(0, |m| m.len()),
            _ => 0,
        })
        .sum()
}

async fn overview(State(server): State<Shared>, Admin(..): Admin) -> Reply<axum::Json<Overview>> {
    let s = server.clone();
    let overview = tokio::task::spawn_blocking(move || -> Reply<Overview> {
        let now = now();
        let today = now / 86400;
        let users = s.accounts.user_infos()?;
        let mut totals = Totals {
            users: users.len() as u64,
            admins: users.iter().filter(|u| u.admin).count() as u64,
            online: users.iter().filter(|u| u.last_seen.is_some_and(|t| now - t <= ONLINE_SECONDS)).count() as u64,
            ..Totals::default()
        };
        let mut per_day: BTreeMap<i64, u64> = (today - 29..=today).map(|d| (d, 0)).collect();
        let (mut recent, mut checked_out) = (Vec::new(), Vec::new());
        for name in s.vault_names()? {
            let Some(shared) = s.shared(&name) else { continue };
            let vault = shared.vault.lock().unwrap_or_else(|e| e.into_inner());
            let stats = vault.stats()?;
            totals.vaults += 1;
            totals.designs += stats.designs;
            totals.versions += stats.versions;
            totals.releases += stats.releases;
            totals.checked_out += stats.checked_out;
            totals.stored_bytes += stats.stored_bytes;
            for (day, n) in vault.versions_per_day((today - 29) * 86400)? {
                *per_day.entry(day).or_default() += n;
            }
            recent.extend(vault.recent_versions(15)?.into_iter().map(|v| Activity {
                vault: name.clone(),
                path: v.path,
                number: v.number,
                author: v.author,
                created_at: v.created_at,
                comment: v.comment,
            }));
            checked_out.extend(vault.locks()?.into_iter().map(|(path, l)| CheckOut { vault: name.clone(), path, user: l.user, since: l.since }));
        }
        recent.sort_by(|a, b| b.created_at.cmp(&a.created_at));
        recent.truncate(15);
        checked_out.sort_by(|a, b| a.since.cmp(&b.since));
        let runs = s.backup_runs(200)?;
        let backup = BackupSummary {
            enabled: s.backup_settings()?.enabled && s.backup_target()?.is_some(),
            last_ok_at: runs.iter().find(|r| r.ok).map(|r| r.finished_at),
            last: runs.into_iter().next(),
        };
        Ok(Overview {
            name: s.name.clone(),
            version: env!("CARGO_PKG_VERSION"),
            started_at: s.started,
            now,
            data: s.data.display().to_string(),
            disk_bytes: disk_use(&s.data),
            totals,
            per_day: per_day.into_iter().collect(),
            recent,
            checked_out,
            backup,
        })
    })
    .await
    .map_err(blocking_error)??;
    Ok(axum::Json(overview))
}

async fn users(State(server): State<Shared>, Admin(..): Admin) -> Reply<axum::Json<Vec<UserInfo>>> {
    Ok(axum::Json(accounts(&server, |s| s.accounts.user_infos()).await?))
}

#[derive(Deserialize)]
struct NewUser {
    name: String,
    password: String,
    #[serde(default)]
    admin: bool,
}

async fn add_user(State(server): State<Shared>, Admin(..): Admin, axum::Json(new): axum::Json<NewUser>) -> Reply<StatusCode> {
    accounts(&server, move |s| s.accounts.add_user(new.name.trim(), &new.password, new.admin)).await?;
    Ok(StatusCode::CREATED)
}

/// Any of: a new password, administrator or not, ending every login.
#[derive(Deserialize)]
struct UserChange {
    password: Option<String>,
    admin: Option<bool>,
    #[serde(default)]
    end_sessions: bool,
}

async fn change_user(State(server): State<Shared>, Admin(me, _): Admin, Path(name): Path<String>, axum::Json(change): axum::Json<UserChange>) -> Reply<StatusCode> {
    if change.admin == Some(false) && name == me.name {
        return Err(fail(StatusCode::CONFLICT, ErrorCode::Conflict, "you cannot take away your own administrator rights"));
    }
    accounts(&server, move |s| {
        if let Some(password) = &change.password {
            s.accounts.set_password(&name, password)?;
        }
        if let Some(admin) = change.admin {
            s.accounts.set_admin(&name, admin)?;
        }
        if change.end_sessions {
            s.accounts.end_sessions(&name)?;
        }
        Ok(())
    })
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn remove_user(State(server): State<Shared>, Admin(me, _): Admin, Path(name): Path<String>) -> Reply<StatusCode> {
    if name == me.name {
        return Err(fail(StatusCode::CONFLICT, ErrorCode::Conflict, "you cannot remove yourself"));
    }
    accounts(&server, move |s| s.accounts.remove_user(&name)).await?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
struct AccessChange {
    /// "read", "write", "admin", or none: no access.
    access: Option<String>,
}

async fn set_access(State(server): State<Shared>, Admin(..): Admin, Path((name, vault)): Path<(String, String)>, axum::Json(change): axum::Json<AccessChange>) -> Reply<StatusCode> {
    if !server.vault_names()?.contains(&vault) {
        return Err(fail(StatusCode::NOT_FOUND, ErrorCode::NotFound, format!("no vault {vault}")));
    }
    let access = match change.access.as_deref() {
        None | Some("") | Some("none") => None,
        Some(a) => Some(accounts::parse_access(a).ok_or_else(|| fail(StatusCode::BAD_REQUEST, ErrorCode::Invalid, format!("{a}: read, write or admin")))?),
    };
    accounts(&server, move |s| match access {
        Some(a) => s.accounts.grant(&name, &vault, a),
        None => s.accounts.revoke(&name, &vault),
    })
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Serialize)]
struct VaultRow {
    name: String,
    designs: u64,
    versions: u64,
    releases: u64,
    checked_out: u64,
    stored_bytes: u64,
    last_change: Option<i64>,
    /// Users with access of their own (administrators have it anyway).
    users: u64,
    /// Whether designs must be checked out to be checked in.
    checkout_required: bool,
}

async fn vaults(State(server): State<Shared>, Admin(..): Admin) -> Reply<axum::Json<Vec<VaultRow>>> {
    let s = server.clone();
    let rows = tokio::task::spawn_blocking(move || -> Reply<Vec<VaultRow>> {
        let users = s.accounts.user_infos()?;
        let mut rows = Vec::new();
        for name in s.vault_names()? {
            let Some(shared) = s.shared(&name) else { continue };
            let held = shared.vault.lock().unwrap_or_else(|e| e.into_inner());
            let (stats, checkout_required) = (held.stats()?, held.checkout_required()?);
            drop(held);
            rows.push(VaultRow {
                checkout_required,
                users: users.iter().filter(|u| u.access.contains_key(&name)).count() as u64,
                name,
                designs: stats.designs,
                versions: stats.versions,
                releases: stats.releases,
                checked_out: stats.checked_out,
                stored_bytes: stats.stored_bytes,
                last_change: stats.last_change,
            });
        }
        Ok(rows)
    })
    .await
    .map_err(blocking_error)??;
    Ok(axum::Json(rows))
}

#[derive(Deserialize)]
struct NewVault {
    name: String,
}

async fn add_vault(State(server): State<Shared>, Admin(..): Admin, axum::Json(new): axum::Json<NewVault>) -> Reply<StatusCode> {
    accounts(&server, move |s| s.add_vault(new.name.trim())).await?;
    Ok(StatusCode::CREATED)
}

#[derive(Deserialize)]
struct Unlock {
    path: String,
}

/// Breaks a check-out (its holder's changes stay in their workspace).
async fn unlock(State(server): State<Shared>, Admin(me, _): Admin, Path(vault): Path<String>, axum::Json(unlock): axum::Json<Unlock>) -> Reply<StatusCode> {
    let shared = server.shared(&vault).ok_or_else(|| fail(StatusCode::NOT_FOUND, ErrorCode::NotFound, format!("no vault {vault}")))?;
    with_vault(shared, move |vault, shared| {
        vault.unlock(&unlock.path, &me.name, true)?;
        shared.counter.fetch_add(1, Ordering::SeqCst);
        Ok(())
    })
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Serialize)]
struct Backups {
    settings: crate::backup::Settings,
    /// Where backups go, and whether that was set when starting the server (not here).
    target: Option<String>,
    target_fixed: bool,
    /// Whether that folder is there, and on the data folder's disk.
    target_exists: bool,
    same_disk: Option<bool>,
    running: bool,
    next_at: Option<i64>,
    runs_total: u64,
    runs_ok: u64,
    last_ok_at: Option<i64>,
    /// The latest runs, newest first, and the backups in the folder.
    runs: Vec<crate::backup::Run>,
    stored: Vec<crate::backup::Stored>,
    /// The data folder, which a backup replaces when restoring.
    data: String,
}

async fn backups(State(server): State<Shared>, Admin(..): Admin) -> Reply<axum::Json<Backups>> {
    let b = accounts(&server, |s| {
        let target = s.backup_target()?;
        let runs = s.backup_runs(50)?;
        let (runs_total, runs_ok) = s.backup_counts()?;
        let last_ok_at = s.accounts.with_db(|db| {
            use rusqlite::OptionalExtension;
            db.query_row("SELECT MAX(finished_at) FROM backup_runs WHERE ok = 1", [], |r| r.get::<_, Option<i64>>(0)).optional().map(Option::flatten)
        })?;
        Ok(Backups {
            settings: s.backup_settings()?,
            target_exists: target.as_ref().is_some_and(|t| t.is_dir()),
            target: target.map(|t| t.display().to_string()),
            target_fixed: s.backup_target_fixed(),
            same_disk: s.backup_target_same_disk(),
            running: s.backup_running(),
            next_at: s.next_backup()?,
            runs_total,
            runs_ok,
            last_ok_at,
            runs,
            stored: s.stored_backups()?,
            data: s.data.display().to_string(),
        })
    })
    .await?;
    Ok(axum::Json(b))
}

async fn backup_settings(State(server): State<Shared>, Admin(..): Admin, axum::Json(settings): axum::Json<crate::backup::Settings>) -> Reply<StatusCode> {
    accounts(&server, move |s| s.set_backup_settings(&settings)).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// Starts a backup; the page follows it on `/api/admin/backups`.
async fn back_up_now(State(server): State<Shared>, Admin(..): Admin) -> Reply<StatusCode> {
    if server.backup_running() {
        return Err(fail(StatusCode::CONFLICT, ErrorCode::Conflict, "a backup is running already"));
    }
    if server.backup_target()?.is_none() {
        return Err(fail(StatusCode::BAD_REQUEST, ErrorCode::Invalid, "set the backup folder first"));
    }
    let s = server.clone();
    tokio::task::spawn_blocking(move || s.back_up("page"));
    Ok(StatusCode::ACCEPTED)
}

#[derive(Deserialize)]
struct CheckoutRule {
    required: bool,
}

/// Whether the vault's designs must be checked out to be checked in (off: one person's vault).
async fn set_checkout(State(server): State<Shared>, Admin(..): Admin, Path(vault): Path<String>, axum::Json(rule): axum::Json<CheckoutRule>) -> Reply<StatusCode> {
    let shared = server.shared(&vault).ok_or_else(|| fail(StatusCode::NOT_FOUND, ErrorCode::NotFound, format!("no vault {vault}")))?;
    with_vault(shared, move |vault, shared| {
        vault.set_checkout_required(rule.required)?;
        // Clients fetch the tree again and see the rule.
        shared.counter.fetch_add(1, Ordering::SeqCst);
        Ok(())
    })
    .await?;
    Ok(StatusCode::NO_CONTENT)
}
