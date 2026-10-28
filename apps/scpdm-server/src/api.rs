//! The HTTP API (see `crates/protocol` for requests and responses).

use std::sync::Arc;
use std::sync::atomic::Ordering;

use axum::Router;
use axum::body::Bytes;
use axum::extract::{DefaultBodyLimit, FromRequestParts, Path, Query, State};
use axum::http::{StatusCode, header, request::Parts};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use protocol::{
    Access, Action, ApiError, Design, Done, ErrorCode, Image, Info, LockInfo, Login, ReleaseFileInfo, ReleaseInfo, Tree, VaultInfo,
    VersionInfo,
};
use serde::Deserialize;
use vault::{Status, Vault};

use crate::accounts::User;
use crate::{Server, SharedVault};

type Shared = Arc<Server>;

pub fn router(server: Shared) -> Router {
    Router::new()
        .route("/api/info", get(info))
        .route("/api/login", post(login))
        .route("/api/logout", post(logout))
        .route("/api/vaults", get(vaults))
        .route("/api/vaults/{vault}/tree", get(tree))
        .route("/api/vaults/{vault}/history", get(history))
        .route("/api/vaults/{vault}/file", get(file))
        .route("/api/vaults/{vault}/objects/{hash}", get(get_object).head(head_object).put(put_object))
        .route("/api/vaults/{vault}/{action}", post(action))
        .merge(crate::admin::router())
        .layer(DefaultBodyLimit::max(protocol::MAX_UPLOAD_BYTES))
        .with_state(server)
}

/// An error response: status and [`ApiError`].
pub(crate) struct Fail(pub StatusCode, pub ApiError);

impl IntoResponse for Fail {
    fn into_response(self) -> Response {
        (self.0, axum::Json(self.1)).into_response()
    }
}

pub(crate) fn fail(status: StatusCode, code: ErrorCode, message: impl Into<String>) -> Fail {
    Fail(status, ApiError::new(code, message))
}

impl From<vault::Error> for Fail {
    fn from(e: vault::Error) -> Self {
        use vault::Error as E;
        let message = e.to_string();
        match e {
            E::NotTracked(_) | E::Missing(_) | E::NoSuchVersion(..) => fail(StatusCode::NOT_FOUND, ErrorCode::NotFound, message),
            E::Locked(_, by) => Fail(StatusCode::CONFLICT, ApiError { by: Some(by), ..ApiError::new(ErrorCode::Locked, message) }),
            E::Exists(_) | E::Unchanged(_) | E::NotCheckedIn(_) | E::Obsolete(_) | E::NotReleased(_) | E::AlreadyReleased(..)
            | E::RevisionTaken(..) | E::FolderNotEmpty(_) => fail(StatusCode::CONFLICT, ErrorCode::Conflict, message),
            E::InvalidPath(_) | E::TooLong(_) | E::Design(_) | E::OutsideVault(_) => {
                fail(StatusCode::BAD_REQUEST, ErrorCode::Invalid, message)
            }
            _ => fail(StatusCode::INTERNAL_SERVER_ERROR, ErrorCode::Internal, message),
        }
    }
}

impl From<anyhow::Error> for Fail {
    fn from(e: anyhow::Error) -> Self {
        fail(StatusCode::INTERNAL_SERVER_ERROR, ErrorCode::Internal, e.to_string())
    }
}

pub(crate) type Reply<T> = Result<T, Fail>;

/// The logged-in user (from `Authorization: Bearer <token>`).
struct Auth(User, String);

impl FromRequestParts<Shared> for Auth {
    type Rejection = Fail;

    async fn from_request_parts(parts: &mut Parts, server: &Shared) -> Result<Self, Self::Rejection> {
        let token = parts
            .headers
            .get(header::AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.strip_prefix("Bearer "))
            .map(str::to_owned)
            .ok_or_else(|| fail(StatusCode::UNAUTHORIZED, ErrorCode::Unauthorized, "log in first"))?;
        match server.accounts.session(&token)? {
            Some(user) => Ok(Auth(user, token)),
            None => Err(fail(StatusCode::UNAUTHORIZED, ErrorCode::Unauthorized, "the login has expired; log in again")),
        }
    }
}

/// The vault, if `user` has at least `need` access to it.
fn vault_for(server: &Server, user: &User, name: &str, need: Access) -> Reply<Arc<SharedVault>> {
    let access = server.accounts.access(user, name)?;
    let shared = server.shared(name).ok_or_else(|| fail(StatusCode::NOT_FOUND, ErrorCode::NotFound, format!("no vault {name}")))?;
    match access {
        None => Err(fail(StatusCode::NOT_FOUND, ErrorCode::NotFound, format!("no vault {name}"))),
        Some(a) if a < need => Err(fail(StatusCode::FORBIDDEN, ErrorCode::Forbidden, format!("{} may not change {name}", user.name))),
        Some(_) => Ok(shared),
    }
}

/// Runs vault work on the blocking pool (SQLite and files), with the vault locked.
pub(crate) async fn with_vault<T: Send + 'static>(shared: Arc<SharedVault>, work: impl FnOnce(&mut Vault, &SharedVault) -> Reply<T> + Send + 'static) -> Reply<T> {
    tokio::task::spawn_blocking(move || {
        let mut vault = shared.vault.lock().unwrap_or_else(|e| e.into_inner());
        work(&mut vault, &shared)
    })
    .await
    .map_err(|e| fail(StatusCode::INTERNAL_SERVER_ERROR, ErrorCode::Internal, e.to_string()))?
}

async fn info(State(server): State<Shared>) -> axum::Json<Info> {
    axum::Json(Info { name: server.name.clone(), version: env!("CARGO_PKG_VERSION").into(), protocol: protocol::PROTOCOL_VERSION })
}

async fn login(State(server): State<Shared>, axum::Json(login): axum::Json<Login>) -> Reply<axum::Json<protocol::Session>> {
    let accounts = server.clone();
    let session = tokio::task::spawn_blocking(move || accounts.accounts.login(&login.user, &login.password))
        .await
        .map_err(|e| fail(StatusCode::INTERNAL_SERVER_ERROR, ErrorCode::Internal, e.to_string()))??;
    match session {
        Some(s) => Ok(axum::Json(s)),
        None => {
            // A pause after a wrong password slows guessing down.
            tokio::time::sleep(std::time::Duration::from_millis(500)).await;
            Err(fail(StatusCode::UNAUTHORIZED, ErrorCode::Unauthorized, "wrong user name or password"))
        }
    }
}

async fn logout(State(server): State<Shared>, Auth(_, token): Auth) -> Reply<StatusCode> {
    server.accounts.logout(&token)?;
    Ok(StatusCode::NO_CONTENT)
}

async fn vaults(State(server): State<Shared>, Auth(user, _): Auth) -> Reply<axum::Json<Vec<VaultInfo>>> {
    let mut out = Vec::new();
    for name in server.vault_names()? {
        if let Some(access) = server.accounts.access(&user, &name)? {
            out.push(VaultInfo { name, access });
        }
    }
    Ok(axum::Json(out))
}

#[derive(Deserialize)]
struct Since {
    since: Option<u64>,
}

async fn tree(State(server): State<Shared>, Auth(user, _): Auth, Path(name): Path<String>, Query(q): Query<Since>) -> Reply<axum::Json<Tree>> {
    let shared = vault_for(&server, &user, &name, Access::Read)?;
    let tree = with_vault(shared, move |vault, shared| {
        let counter = shared.counter.load(Ordering::SeqCst);
        if q.since == Some(counter) {
            return Ok(Tree { counter, unchanged: true, ..Tree::default() });
        }
        let ids = vault.document_ids()?;
        let locks = vault.locks()?;
        let designs = vault
            .scan()?
            .into_iter()
            .filter(|e| e.status != Status::Missing)
            .filter_map(|e| {
                let latest = e.latest?;
                Some(Design {
                    id: *ids.get(&e.path)?,
                    lock: locks.get(&e.path).map(|l| LockInfo { user: l.user.clone(), since: l.since }),
                    latest: version_info(latest),
                    state: state_info(e.state.unwrap_or(vault::State::Wip)),
                    release: e.release.map(release_info),
                    path: e.path,
                })
            })
            .collect();
        let mut cache = shared.image_hashes.lock().unwrap_or_else(|e| e.into_inner());
        let mut images = Vec::new();
        for file in vault.images()? {
            let hash = match cache.get(&file.path) {
                Some((size, modified, hash)) if *size == file.size && *modified == file.modified => hash.clone(),
                _ => {
                    let Ok(bytes) = std::fs::read(vault.absolute(&file.path)) else { continue };
                    let hash = vault::hash(&bytes);
                    cache.insert(file.path.clone(), (file.size, file.modified, hash.clone()));
                    hash
                }
            };
            images.push(Image { path: file.path, hash, size: file.size });
        }
        Ok(Tree { counter, unchanged: false, checkout_required: vault.checkout_required()?, designs, folders: vault.folders()?, images })
    })
    .await?;
    Ok(axum::Json(tree))
}

#[derive(Deserialize)]
struct PathQuery {
    path: String,
}

async fn history(State(server): State<Shared>, Auth(user, _): Auth, Path(name): Path<String>, Query(q): Query<PathQuery>) -> Reply<axum::Json<Vec<VersionInfo>>> {
    let shared = vault_for(&server, &user, &name, Access::Read)?;
    let history = with_vault(shared, move |vault, _| Ok(vault.history(&q.path)?)).await?;
    Ok(axum::Json(history.into_iter().map(version_info).collect()))
}

/// The latest content of a design, or a reference image.
async fn file(State(server): State<Shared>, Auth(user, _): Auth, Path(name): Path<String>, Query(q): Query<PathQuery>) -> Reply<Response> {
    let shared = vault_for(&server, &user, &name, Access::Read)?;
    let bytes = with_vault(shared, move |vault, _| {
        let path = vault::Vault::relative_path(&q.path)?;
        let tracked = vault.document_ids()?.contains_key(&path);
        if !tracked && !vault::is_image(std::path::Path::new(&path)) {
            return Err(fail(StatusCode::NOT_FOUND, ErrorCode::NotFound, format!("no design or image {path}")));
        }
        std::fs::read(vault.absolute(&path)).map_err(|_| fail(StatusCode::NOT_FOUND, ErrorCode::NotFound, format!("{path} is missing")))
    })
    .await?;
    Ok(([(header::CONTENT_TYPE, "application/octet-stream")], bytes).into_response())
}

async fn get_object(State(server): State<Shared>, Auth(user, _): Auth, Path((name, hash)): Path<(String, String)>) -> Reply<Response> {
    let shared = vault_for(&server, &user, &name, Access::Read)?;
    let bytes = with_vault(shared, move |vault, _| {
        if !vault.has_object(&hash) {
            return Err(fail(StatusCode::NOT_FOUND, ErrorCode::NotFound, format!("no object {hash}")));
        }
        Ok(vault.object(&hash)?)
    })
    .await?;
    Ok(([(header::CONTENT_TYPE, "application/octet-stream")], bytes).into_response())
}

async fn head_object(State(server): State<Shared>, Auth(user, _): Auth, Path((name, hash)): Path<(String, String)>) -> Reply<StatusCode> {
    let shared = vault_for(&server, &user, &name, Access::Read)?;
    let found = with_vault(shared, move |vault, _| Ok(vault.has_object(&hash))).await?;
    Ok(if found { StatusCode::OK } else { StatusCode::NOT_FOUND })
}

async fn put_object(State(server): State<Shared>, Auth(user, _): Auth, Path((name, hash)): Path<(String, String)>, body: Bytes) -> Reply<StatusCode> {
    let shared = vault_for(&server, &user, &name, Access::Write)?;
    if vault::hash(&body) != hash {
        return Err(fail(StatusCode::BAD_REQUEST, ErrorCode::Invalid, "the upload does not match its hash"));
    }
    with_vault(shared, move |vault, _| Ok(vault.put_object(&body)?)).await?;
    Ok(StatusCode::CREATED)
}

async fn action(
    State(server): State<Shared>,
    Auth(user, _): Auth,
    Path((name, segment)): Path<(String, String)>,
    axum::Json(action): axum::Json<Action>,
) -> Reply<axum::Json<Done>> {
    if action.name() != segment {
        return Err(fail(StatusCode::BAD_REQUEST, ErrorCode::Invalid, format!("{segment} does not match the request")));
    }
    let need = match &action {
        Action::UndoCheckout { force: true, .. } => Access::Admin,
        _ => Access::Write,
    };
    let shared = vault_for(&server, &user, &name, need)?;
    let done = with_vault(shared, move |vault, shared| {
        vault.set_author(Some(user.name.clone()));
        let result = apply(vault, &user, action);
        vault.set_author(None);
        if result.is_ok() {
            shared.counter.fetch_add(1, Ordering::SeqCst);
        }
        result
    })
    .await?;
    Ok(axum::Json(done))
}

/// Refuses a change to a design someone else has checked out.
fn not_checked_out_by_others(vault: &Vault, path: &str, user: &User, why: &str) -> Reply<()> {
    match vault.lock_of(path)? {
        Some(lock) if lock.user != user.name => Err(Fail(
            StatusCode::CONFLICT,
            ApiError { by: Some(lock.user.clone()), ..ApiError::new(ErrorCode::Locked, format!("{path} is checked out by {}{why}", lock.user)) },
        )),
        _ => Ok(()),
    }
}

/// Refuses a change to a design anyone has checked out (it would make their copy stale).
fn not_checked_out(vault: &Vault, path: &str) -> Reply<()> {
    match vault.lock_of(path)? {
        Some(lock) => Err(Fail(
            StatusCode::CONFLICT,
            ApiError { by: Some(lock.user.clone()), ..ApiError::new(ErrorCode::Locked, format!("{path} is checked out by {}: check it in first", lock.user)) },
        )),
        None => Ok(()),
    }
}

fn uploaded(vault: &Vault, hash: &str) -> Reply<Vec<u8>> {
    if !vault.has_object(hash) {
        return Err(fail(StatusCode::BAD_REQUEST, ErrorCode::Invalid, format!("upload {hash} first")));
    }
    Ok(vault.object(hash)?)
}

fn apply(vault: &mut Vault, user: &User, action: Action) -> Reply<Done> {
    match action {
        Action::Checkout { path } => {
            let lock = vault.lock(&path, &user.name)?;
            Ok(Done { lock: Some(LockInfo { user: lock.user, since: lock.since }), ..Done::default() })
        }
        Action::UndoCheckout { path, force } => {
            vault.unlock(&path, &user.name, force)?;
            Ok(Done::default())
        }
        Action::Checkin { path, base, hash, comment, keep_checked_out } => {
            match vault.lock_of(&path)? {
                Some(lock) if lock.user == user.name => {}
                Some(lock) => {
                    return Err(Fail(
                        StatusCode::CONFLICT,
                        ApiError { by: Some(lock.user.clone()), ..ApiError::new(ErrorCode::Locked, format!("{path} is checked out by {}", lock.user)) },
                    ));
                }
                // A vault one person uses may do without check-outs.
                None if !vault.checkout_required()? => {}
                None => return Err(fail(StatusCode::CONFLICT, ErrorCode::NotCheckedOut, format!("check {path} out before checking it in"))),
            }
            let latest = vault.history(&path)?.into_iter().next().ok_or_else(|| fail(StatusCode::NOT_FOUND, ErrorCode::NotFound, path.clone()))?;
            if latest.number != base {
                return Err(Fail(
                    StatusCode::CONFLICT,
                    ApiError {
                        latest: Some(latest.number),
                        ..ApiError::new(ErrorCode::Stale, format!("{path} is at v{} on the server, the change was made on v{base}", latest.number))
                    },
                ));
            }
            let bytes = uploaded(vault, &hash)?;
            let version = if hash == latest.hash { latest } else { vault.check_in_bytes(&path, &bytes, &comment)? };
            if !keep_checked_out {
                vault.unlock(&path, &user.name, false)?;
            }
            Ok(Done { version: Some(version_info(version)), ..Done::default() })
        }
        Action::Add { path, hash, comment } => {
            let path = Vault::relative_path(&path)?;
            if !vault::is_design(std::path::Path::new(&path)) {
                return Err(fail(StatusCode::BAD_REQUEST, ErrorCode::Invalid, format!("{path} is not a SolidCraft design (.sc, .solidcraft)")));
            }
            if vault.absolute(&path).exists() {
                return Err(fail(StatusCode::CONFLICT, ErrorCode::Conflict, format!("{path} exists already")));
            }
            let bytes = uploaded(vault, &hash)?;
            let version = vault.check_in_bytes(&path, &bytes, &comment)?;
            Ok(Done { version: Some(version_info(version)), ..Done::default() })
        }
        Action::Move { from, to } => {
            not_checked_out_by_others(vault, &from, user, "")?;
            // Designs whose links the move rewrites must be checked in (by anyone, also the
            // caller): otherwise their checked-out copies would be stale.
            if vault::is_design(std::path::Path::new(&from)) {
                for design in vault.move_affects(&from, &to)? {
                    not_checked_out(vault, &design)?;
                }
            }
            let report = vault.rename(&from, &to)?;
            Ok(Done { updated: report.updated.into_iter().map(|u| u.path).collect(), ..Done::default() })
        }
        Action::Release { path, revision, note } => {
            not_checked_out(vault, &path)?;
            let release = vault.release_as(&path, &revision, &note)?;
            let version = vault.history(&path)?.into_iter().find(|v| v.number == release.version).map(version_info);
            Ok(Done { version, release: Some(release_info(release)), ..Done::default() })
        }
        Action::Revise { path } => {
            vault.revise(&path)?;
            Ok(Done::default())
        }
        Action::Obsolete { path, obsolete } => {
            vault.set_obsolete(&path, obsolete)?;
            Ok(Done::default())
        }
        Action::Properties { path, name, properties, comment } => {
            not_checked_out(vault, &path)?;
            let values: Vec<(&str, Option<&str>)> = properties.iter().map(|(k, v)| (k.as_str(), v.as_deref())).collect();
            vault.edit_properties(&path, name.as_deref(), &values)?;
            let version = match vault.check_in(&path, &comment) {
                Ok(v) => Some(version_info(v)),
                Err(vault::Error::Unchanged(_)) => None,
                Err(e) => return Err(e.into()),
            };
            Ok(Done { version, ..Done::default() })
        }
        Action::CreateFolder { path } => {
            vault.create_folder(&path)?;
            Ok(Done::default())
        }
        Action::DeleteFolder { path } => {
            vault.remove_folder(&path)?;
            Ok(Done::default())
        }
        Action::PutImage { path, hash } => {
            let path = Vault::relative_path(&path)?;
            if !vault::is_image(std::path::Path::new(&path)) {
                return Err(fail(StatusCode::BAD_REQUEST, ErrorCode::Invalid, format!("{path} is not a PNG, JPEG or WebP image")));
            }
            let bytes = uploaded(vault, &hash)?;
            vault.put_file(&path, &bytes)?;
            Ok(Done::default())
        }
        Action::Delete { path } => {
            not_checked_out_by_others(vault, &path, user, "")?;
            vault.delete(&path, Some(&user.name))?;
            Ok(Done::default())
        }
        Action::ReleaseFiles { path, version, files } => {
            for file in &files {
                if !vault.has_object(&file.hash) {
                    return Err(fail(StatusCode::BAD_REQUEST, ErrorCode::Invalid, format!("upload {} first", file.hash)));
                }
                vault.attach_release_file(&path, version, &file.name, &file.hash)?;
            }
            let release = vault.lifecycle(&path)?.releases.into_iter().find(|r| r.version == version).map(release_info);
            Ok(Done { release, ..Done::default() })
        }
        Action::DeleteImage { path } => {
            if !vault::is_image(std::path::Path::new(&path)) {
                return Err(fail(StatusCode::BAD_REQUEST, ErrorCode::Invalid, format!("{path} is not an image")));
            }
            vault.remove_file(&path)?;
            Ok(Done::default())
        }
    }
}

pub fn version_info(v: vault::Version) -> VersionInfo {
    VersionInfo {
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

fn release_info(r: vault::Release) -> ReleaseInfo {
    let files = r.files.into_iter().map(|f| ReleaseFileInfo { name: f.name, hash: f.hash, size: f.size }).collect();
    ReleaseInfo { revision: r.revision, version: r.version, released_at: r.released_at, note: r.note, files }
}

fn state_info(s: vault::State) -> protocol::State {
    match s {
        vault::State::Wip => protocol::State::Wip,
        vault::State::Released => protocol::State::Released,
        vault::State::Obsolete => protocol::State::Obsolete,
    }
}
