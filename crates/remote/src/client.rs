//! The server's API, from a client (blocking; use it from worker threads).

use std::time::Duration;

use protocol::{Action, ApiError, Done, ErrorCode, Info, Login, Session, Tree, VaultInfo, VersionInfo};
use serde::de::DeserializeOwned;
use ureq::Agent;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// The server refused (see the code: locked, stale, …).
    #[error("{0}")]
    Api(ApiError),
    #[error("cannot reach {url}: {message}")]
    Transport { url: String, message: String },
    #[error("the server speaks protocol {server}, this app {client}: update the one that is older")]
    Protocol { server: u32, client: u32 },
    #[error("{0}")]
    Io(#[from] std::io::Error),
    #[error("{0}")]
    Vault(#[from] vault::Error),
    /// A problem in the workspace (local changes in the way, …).
    #[error("{0}")]
    Local(String),
}

impl Error {
    /// The server's error code, if it was the server that refused.
    pub fn code(&self) -> Option<ErrorCode> {
        match self {
            Error::Api(e) => Some(e.code),
            _ => None,
        }
    }
}

pub type Result<T> = std::result::Result<T, Error>;

/// A connection to one server, logged in or not.
#[derive(Clone)]
pub struct Client {
    url: String,
    token: Option<String>,
    agent: Agent,
}

impl Client {
    /// A client for the server at `url` (`http://host:7380`); does not connect yet.
    pub fn new(url: &str, token: Option<String>) -> Self {
        let agent: Agent = Agent::config_builder()
            .http_status_as_error(false)
            .timeout_connect(Some(Duration::from_secs(10)))
            .timeout_global(Some(Duration::from_secs(600)))
            .build()
            .into();
        Client { url: url.trim_end_matches('/').to_owned(), token, agent }
    }

    pub fn url(&self) -> &str {
        &self.url
    }

    pub fn token(&self) -> Option<&str> {
        self.token.as_deref()
    }

    /// The server's name and versions; fails for another protocol version.
    pub fn info(&self) -> Result<Info> {
        let info: Info = self.json(self.agent.get(self.endpoint("/api/info")).call())?;
        if info.protocol != protocol::PROTOCOL_VERSION {
            return Err(Error::Protocol { server: info.protocol, client: protocol::PROTOCOL_VERSION });
        }
        Ok(info)
    }

    /// Logs in; the client keeps the session.
    pub fn login(&mut self, user: &str, password: &str) -> Result<Session> {
        self.info()?;
        let login = Login { user: user.to_owned(), password: password.to_owned() };
        let session: Session = self.json(self.agent.post(self.endpoint("/api/login")).send_json(&login))?;
        self.token = Some(session.token.clone());
        Ok(session)
    }

    pub fn logout(&mut self) -> Result<()> {
        self.empty(self.authed(self.agent.post(self.endpoint("/api/logout"))).send_empty())?;
        self.token = None;
        Ok(())
    }

    pub fn vaults(&self) -> Result<Vec<VaultInfo>> {
        self.json(self.authed(self.agent.get(self.endpoint("/api/vaults"))).call())
    }

    /// The vault's designs, folders and images; `unchanged` (and nothing else) when its counter
    /// is still `since`.
    pub fn tree(&self, vault: &str, since: Option<u64>) -> Result<Tree> {
        let mut request = self.authed(self.agent.get(self.vault_endpoint(vault, "tree")));
        if let Some(since) = since {
            request = request.query("since", since.to_string());
        }
        self.json(request.call())
    }

    pub fn history(&self, vault: &str, path: &str) -> Result<Vec<VersionInfo>> {
        self.json(self.authed(self.agent.get(self.vault_endpoint(vault, "history"))).query("path", path).call())
    }

    /// The latest content of a design or an image.
    pub fn file(&self, vault: &str, path: &str) -> Result<Vec<u8>> {
        self.bytes(self.authed(self.agent.get(self.vault_endpoint(vault, "file"))).query("path", path).call())
    }

    /// An object (a version, a thumbnail) by its SHA-256, checked.
    pub fn object(&self, vault: &str, hash: &str) -> Result<Vec<u8>> {
        let bytes = self.bytes(self.authed(self.agent.get(self.object_endpoint(vault, hash))).call())?;
        if vault::hash(&bytes) != hash {
            return Err(Error::Local(format!("the download of {hash} is damaged")));
        }
        Ok(bytes)
    }

    pub fn has_object(&self, vault: &str, hash: &str) -> Result<bool> {
        let response = self.send(self.authed(self.agent.head(self.object_endpoint(vault, hash))).call())?;
        match response.status().as_u16() {
            200 => Ok(true),
            404 => Ok(false),
            _ => Err(self.refusal(response)),
        }
    }

    /// Uploads `bytes` unless the server has them already; returns their hash.
    pub fn upload(&self, vault: &str, bytes: &[u8]) -> Result<String> {
        let hash = vault::hash(bytes);
        if !self.has_object(vault, &hash)? {
            self.empty(self.authed(self.agent.put(self.object_endpoint(vault, &hash))).send(bytes))?;
        }
        Ok(hash)
    }

    pub fn act(&self, vault: &str, action: &Action) -> Result<Done> {
        self.json(self.authed(self.agent.post(self.vault_endpoint(vault, action.name()))).send_json(action))
    }

    fn endpoint(&self, path: &str) -> String {
        format!("{}{path}", self.url)
    }

    fn vault_endpoint(&self, vault: &str, what: &str) -> String {
        format!("{}/api/vaults/{vault}/{what}", self.url)
    }

    fn object_endpoint(&self, vault: &str, hash: &str) -> String {
        format!("{}/api/vaults/{vault}/objects/{hash}", self.url)
    }

    fn authed<B>(&self, request: ureq::RequestBuilder<B>) -> ureq::RequestBuilder<B> {
        match &self.token {
            Some(t) => request.header("Authorization", format!("Bearer {t}")),
            None => request,
        }
    }

    fn send(&self, result: std::result::Result<ureq::http::Response<ureq::Body>, ureq::Error>) -> Result<ureq::http::Response<ureq::Body>> {
        result.map_err(|e| Error::Transport { url: self.url.clone(), message: e.to_string() })
    }

    /// The server's error from a refused request.
    fn refusal(&self, mut response: ureq::http::Response<ureq::Body>) -> Error {
        let status = response.status();
        match response.body_mut().read_json::<ApiError>() {
            Ok(e) => Error::Api(e),
            Err(_) => Error::Api(ApiError::new(ErrorCode::Internal, format!("the server answered {status}"))),
        }
    }

    fn json<T: DeserializeOwned>(&self, result: std::result::Result<ureq::http::Response<ureq::Body>, ureq::Error>) -> Result<T> {
        let mut response = self.send(result)?;
        if !response.status().is_success() {
            return Err(self.refusal(response));
        }
        response.body_mut().read_json().map_err(|e| Error::Transport { url: self.url.clone(), message: e.to_string() })
    }

    fn bytes(&self, result: std::result::Result<ureq::http::Response<ureq::Body>, ureq::Error>) -> Result<Vec<u8>> {
        let mut response = self.send(result)?;
        if !response.status().is_success() {
            return Err(self.refusal(response));
        }
        response
            .body_mut()
            .with_config()
            .limit(protocol::MAX_UPLOAD_BYTES as u64)
            .read_to_vec()
            .map_err(|e| Error::Transport { url: self.url.clone(), message: e.to_string() })
    }

    fn empty(&self, result: std::result::Result<ureq::http::Response<ureq::Body>, ureq::Error>) -> Result<()> {
        let response = self.send(result)?;
        if !response.status().is_success() {
            return Err(self.refusal(response));
        }
        Ok(())
    }
}
