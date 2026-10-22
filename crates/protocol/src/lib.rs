//! The SolidCraft PDM server's API: JSON over HTTP, shared by the server (`apps/scpdm-server`)
//! and its clients (`crates/remote`).
//!
//! Every request but `GET /api/info` and `POST /api/login` carries `Authorization: Bearer
//! <token>`. Errors are an HTTP status with an [`ApiError`] body.
//!
//! | Request | Body → response |
//! |---|---|
//! | `GET /api/info` | → [`Info`] |
//! | `POST /api/login` | [`Login`] → [`Session`] |
//! | `POST /api/logout` | |
//! | `GET /api/vaults` | → `Vec<`[`VaultInfo`]`>` |
//! | `GET /api/vaults/{v}/tree?since=N` | → [`Tree`] (`unchanged` when the counter is still `N`) |
//! | `GET /api/vaults/{v}/history?path=P` | → `Vec<`[`VersionInfo`]`>` |
//! | `GET /api/vaults/{v}/file?path=P` | → the latest content of a design or image |
//! | `GET`/`HEAD`/`PUT /api/vaults/{v}/objects/{sha256}` | an object's bytes (the server checks the hash) |
//! | `POST /api/vaults/{v}/{action}` | an [`Action`] → [`Done`] |

use serde::{Deserialize, Serialize};

/// Changes whenever a change to the API would break an older client or server.
pub const PROTOCOL_VERSION: u32 = 1;

/// The largest file or object the server takes (SolidCraft's own limit for a design part).
pub const MAX_UPLOAD_BYTES: usize = 256 * 1024 * 1024;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Info {
    /// The server's name, as its administrator set it.
    pub name: String,
    /// `CARGO_PKG_VERSION` of the server.
    pub version: String,
    pub protocol: u32,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Login {
    pub user: String,
    pub password: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Session {
    pub token: String,
    pub user: String,
    pub admin: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Access {
    Read,
    Write,
    Admin,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct VaultInfo {
    pub name: String,
    pub access: Access,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum State {
    Wip,
    Released,
    Obsolete,
}

/// A version, as `vault::Version`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct VersionInfo {
    pub number: u32,
    pub hash: String,
    pub size: u64,
    pub comment: String,
    pub author: String,
    pub created_at: i64,
    pub path: String,
    pub name: Option<String>,
    pub part_number: Option<String>,
    pub revision: Option<String>,
    pub format: Option<String>,
    pub thumbnail: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ReleaseInfo {
    pub revision: String,
    pub version: u32,
    pub released_at: i64,
    pub note: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LockInfo {
    pub user: String,
    pub since: i64,
}

/// A design on the server.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Design {
    /// Stays the same when the design moves: workspaces follow moves by it.
    pub id: i64,
    pub path: String,
    pub latest: VersionInfo,
    pub state: State,
    pub release: Option<ReleaseInfo>,
    pub lock: Option<LockInfo>,
}

/// A reference image on the server (shared, not versioned).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Image {
    pub path: String,
    pub hash: String,
    pub size: u64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Tree {
    /// Changes with every change to the vault.
    pub counter: u64,
    /// The rest is empty: nothing changed since the counter the client sent.
    pub unchanged: bool,
    /// Whether a design must be checked out to be checked in (servers before 0.2.3: always).
    #[serde(default = "yes")]
    pub checkout_required: bool,
    pub designs: Vec<Design>,
    pub folders: Vec<String>,
    pub images: Vec<Image>,
}

/// A change to a server vault: `POST /api/vaults/{v}/{action}` with the body below.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "kebab-case")]
pub enum Action {
    Checkout { path: String },
    /// `force`: an administrator releasing someone else's check-out.
    UndoCheckout { path: String, force: bool },
    /// The design must be checked out by the caller, and `base` its latest version (else 409).
    /// The object `hash` must be uploaded before.
    Checkin { path: String, base: u32, hash: String, comment: String, keep_checked_out: bool },
    /// A new design (uploaded as `hash`), v1.
    Add { path: String, hash: String, comment: String },
    /// Moves or renames, updating links in other designs (refused while any of them is checked
    /// out by someone else).
    Move { from: String, to: String },
    Release { path: String, revision: String, note: String },
    Revise { path: String },
    Obsolete { path: String, obsolete: bool },
    /// Changes design properties (as `vault::Vault::edit_properties`) and checks that in.
    Properties { path: String, name: Option<String>, properties: Vec<(String, Option<String>)>, comment: String },
    CreateFolder { path: String },
    DeleteFolder { path: String },
    /// A reference image (uploaded as `hash`), new or replaced.
    PutImage { path: String, hash: String },
    DeleteImage { path: String },
    /// Deletes a design: it is gone for everyone, and kept on the server for a while (an
    /// administrator can restore it). Refused while someone else has it checked out.
    Delete { path: String },
}

impl Action {
    /// The URL segment of the action.
    pub fn name(&self) -> &'static str {
        match self {
            Action::Checkout { .. } => "checkout",
            Action::UndoCheckout { .. } => "undo-checkout",
            Action::Checkin { .. } => "checkin",
            Action::Add { .. } => "add",
            Action::Move { .. } => "move",
            Action::Release { .. } => "release",
            Action::Revise { .. } => "revise",
            Action::Obsolete { .. } => "obsolete",
            Action::Properties { .. } => "properties",
            Action::CreateFolder { .. } => "create-folder",
            Action::DeleteFolder { .. } => "delete-folder",
            Action::PutImage { .. } => "put-image",
            Action::DeleteImage { .. } => "delete-image",
            Action::Delete { .. } => "delete",
        }
    }
}

/// The outcome of an [`Action`].
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Done {
    /// The version the action recorded, if any.
    pub version: Option<VersionInfo>,
    pub release: Option<ReleaseInfo>,
    pub lock: Option<LockInfo>,
    /// After a move: designs whose links were updated.
    pub updated: Vec<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ErrorCode {
    /// No or an expired session: log in again.
    Unauthorized,
    /// Not allowed for this user.
    Forbidden,
    NotFound,
    /// Checked out by someone else (`by`).
    Locked,
    /// Checking in without having it checked out.
    NotCheckedOut,
    /// The design changed on the server since the version the change was made on.
    Stale,
    /// Exists already, or another conflict.
    Conflict,
    /// A malformed request.
    Invalid,
    /// A different protocol version.
    Protocol,
    Internal,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ApiError {
    pub code: ErrorCode,
    pub message: String,
    /// For `Locked`: who has it checked out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub by: Option<String>,
    /// For `Stale`: the latest version on the server.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub latest: Option<u32>,
}

impl ApiError {
    pub fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        ApiError { code, message: message.into(), by: None, latest: None }
    }
}

impl std::fmt::Display for ApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for ApiError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn actions_are_tagged_json() {
        let a = Action::Checkin { path: "a.solidcraft".into(), base: 3, hash: "ab".into(), comment: "c".into(), keep_checked_out: false };
        let json = serde_json::to_value(&a).unwrap();
        assert_eq!(json["action"], "checkin");
        assert_eq!(json["base"], 3);
        assert_eq!(serde_json::from_value::<Action>(json).unwrap(), a);
        assert_eq!(a.name(), "checkin");
        assert_eq!(serde_json::to_value(Access::Write).unwrap(), "write");
        assert!(Access::Admin > Access::Write && Access::Write > Access::Read);
    }

    #[test]
    fn errors_carry_their_details() {
        let e = ApiError { by: Some("anna".into()), ..ApiError::new(ErrorCode::Locked, "checked out by anna") };
        let json = serde_json::to_string(&e).unwrap();
        assert!(json.contains(r#""code":"locked""#) && json.contains(r#""by":"anna""#) && !json.contains("latest"));
    }
}

fn yes() -> bool {
    true
}

impl Default for Tree {
    /// Empty, with check-out required (the safe assumption).
    fn default() -> Self {
        Tree { counter: 0, unchanged: false, checkout_required: true, designs: Vec::new(), folders: Vec::new(), images: Vec::new() }
    }
}
