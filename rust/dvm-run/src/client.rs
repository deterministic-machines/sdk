//! The connection to dvm.run's API: its address, the key every request
//! bears, what a refusal means, and server-sent events.

use std::sync::Arc;
use std::time::Duration;

use dvm_api::{ApiError, SandboxId};
use reqwest::{Method, RequestBuilder, Response};
use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::credentials;
use crate::error::{Error, Result};
use crate::execution::Execution;
use crate::sandbox::{Sandbox, SandboxBuilder};

pub const DEFAULT_API_URL: &str = "https://sandbox.dvm.run";

/// The controller answers a `?newer_than=` read within 30 s, and keeps a
/// quiet event stream alive with comments more often than this.
const QUIET_LIMIT: Duration = Duration::from_secs(60);

/// dvm.run's API, at an address, as the user whose key it bears. Cheap to
/// clone: clones share their connections.
#[derive(Clone, Debug)]
pub struct Client {
    inner: Arc<Connection>,
}

#[derive(Debug)]
struct Connection {
    url: String,
    api: String,
    key: Option<String>,
    http: reqwest::Client,
}

pub(crate) type Query<'a> = &'a [(&'a str, String)];

impl Client {
    /// dvm.run at `DVM_API_URL`, else `https://sandbox.dvm.run`, with the key
    /// in `DVM_API_KEY`, else the one `dvm-run login` saved for it.
    pub fn from_env() -> Result<Self> {
        let api_url = api_url_of(None);
        let key = match std::env::var("DVM_API_KEY") {
            Ok(key) if !key.is_empty() => Some(key),
            _ => credentials::saved_key(&api_url)?,
        };
        let key = key.ok_or_else(|| Error::NoApiKey {
            api_url: api_url.clone(),
        })?;
        Ok(Self::new(api_url, key))
    }

    /// dvm.run at `api_url`, with `api_key`.
    pub fn new(api_url: impl Into<String>, api_key: impl Into<String>) -> Self {
        Self::with_key(api_url.into(), Some(api_key.into()))
    }

    /// As no one, for what needs no key: a login.
    pub(crate) fn with_key(api_url: String, key: Option<String>) -> Self {
        let url = api_url.trim_end_matches('/').to_owned();
        Self {
            inner: Arc::new(Connection {
                api: format!("{url}/api"),
                url,
                key,
                http: reqwest::Client::new(),
            }),
        }
    }

    /// The dvm.run it calls.
    pub fn api_url(&self) -> &str {
        &self.inner.url
    }

    /// A sandbox of `image`, as `docker run` takes it, once it's running.
    pub fn create_sandbox(&self, image: impl Into<String>) -> SandboxBuilder {
        SandboxBuilder::new(self.clone(), image.into())
    }

    /// A handle to a sandbox made elsewhere.
    pub async fn connect_sandbox(&self, id: SandboxId) -> Result<Sandbox> {
        let sandbox = self
            .json(Method::GET, &format!("/sandboxes/{id}"), &[], None::<&()>)
            .await?;
        Ok(Sandbox::new(self.clone(), sandbox))
    }

    /// A sandbox's execution: everything it ran, under its id.
    pub async fn execution(&self, id: SandboxId) -> Result<Execution> {
        let execution = self
            .json(Method::GET, &format!("/executions/{id}"), &[], None::<&()>)
            .await?;
        Ok(Execution::new(self.clone(), execution))
    }

    /// The answer's JSON; an empty answer is `null`.
    pub(crate) async fn json<T: DeserializeOwned>(
        &self,
        method: Method,
        path: &str,
        query: Query<'_>,
        body: Option<&impl Serialize>,
    ) -> Result<T> {
        let mut request = self.request(method, path, query).timeout(QUIET_LIMIT);
        if let Some(body) = body {
            request = request.json(body);
        }
        let bytes = self.send(path, request).await?.bytes().await;
        let bytes = bytes.map_err(|error| Error::network(format!("reading {path}"), error))?;
        let bytes = if bytes.is_empty() {
            &b"null"[..]
        } else {
            &bytes
        };
        serde_json::from_slice(bytes)
            .map_err(|error| Error::Unexpected(format!("{path}'s answer: {error}")))
    }

    /// The answer's body, as it is.
    pub(crate) async fn bytes(
        &self,
        method: Method,
        path: &str,
        query: Query<'_>,
        content: Option<Vec<u8>>,
    ) -> Result<Vec<u8>> {
        let mut request = self.request(method, path, query);
        if let Some(content) = content {
            request = request.body(content);
        }
        let bytes = self.send(path, request).await?.bytes().await;
        Ok(bytes
            .map_err(|error| Error::network(format!("reading {path}"), error))?
            .to_vec())
    }

    /// The server-sent events at `path`.
    pub(crate) async fn events(&self, path: &str, query: Query<'_>) -> Result<EventStream> {
        let request = self.request(Method::GET, path, query);
        let answered = tokio::time::timeout(QUIET_LIMIT, self.send(path, request)).await;
        let response = answered.map_err(|_| Error::Network {
            context: format!("{path} didn't answer"),
            source: None,
        })??;
        Ok(EventStream {
            path: path.to_owned(),
            response,
            pending: Vec::new(),
            data: Vec::new(),
        })
    }

    fn request(&self, method: Method, path: &str, query: Query<'_>) -> RequestBuilder {
        let request = self
            .inner
            .http
            .request(method, format!("{}{path}", self.inner.api));
        let request = if query.is_empty() {
            request
        } else {
            request.query(query)
        };
        match &self.inner.key {
            Some(key) => request.bearer_auth(key),
            None => request,
        }
    }

    /// The response, refused unless it's a success.
    async fn send(&self, path: &str, request: RequestBuilder) -> Result<Response> {
        let response = request.send().await.map_err(|error| {
            Error::network(format!("couldn't reach {}{path}", self.inner.api), error)
        })?;
        let status = response.status();
        if status.is_success() {
            return Ok(response);
        }
        let text = response.text().await.unwrap_or_default();
        let reason = match serde_json::from_str::<ApiError>(&text) {
            Ok(refusal) => refusal.message,
            Err(_) if text.is_empty() => status.canonical_reason().unwrap_or("").to_owned(),
            Err(_) => text,
        };
        Err(Error::refusal(status.as_u16(), reason))
    }
}

/// Each server-sent event's data, as JSON, until the stream ends. A stream
/// that goes quiet for longer than the controller lets one is dropped.
pub(crate) struct EventStream {
    path: String,
    response: Response,
    pending: Vec<u8>,
    data: Vec<u8>,
}

impl EventStream {
    pub(crate) async fn next<T: DeserializeOwned>(&mut self) -> Result<Option<T>> {
        loop {
            while let Some(end) = self.pending.iter().position(|&byte| byte == b'\n') {
                let mut line: Vec<u8> = self.pending.drain(..=end).collect();
                line.pop();
                if line.last() == Some(&b'\r') {
                    line.pop();
                }
                if let Some(data) = line.strip_prefix(b"data:") {
                    if !self.data.is_empty() {
                        self.data.push(b'\n');
                    }
                    self.data
                        .extend_from_slice(data.strip_prefix(b" ").unwrap_or(data));
                } else if line.is_empty() && !self.data.is_empty() {
                    let data = std::mem::take(&mut self.data);
                    let event = serde_json::from_slice(&data).map_err(|error| {
                        Error::Unexpected(format!("{}'s event: {error}", self.path))
                    })?;
                    return Ok(Some(event));
                }
            }
            let chunk = tokio::time::timeout(QUIET_LIMIT, self.response.chunk()).await;
            let chunk = chunk.map_err(|_| Error::Network {
                context: format!("{}'s stream went quiet", self.path),
                source: None,
            })?;
            match chunk.map_err(|error| Error::network(format!("{}'s stream", self.path), error))? {
                Some(chunk) => self.pending.extend_from_slice(&chunk),
                None => return Ok(None),
            }
        }
    }
}

/// dvm.run's address: `given`, else `DVM_API_URL`, else dvm.run's.
pub(crate) fn api_url_of(given: Option<&str>) -> String {
    let from_environment = std::env::var("DVM_API_URL").ok();
    let url = given
        .filter(|url| !url.is_empty())
        .or(from_environment.as_deref().filter(|url| !url.is_empty()))
        .unwrap_or(DEFAULT_API_URL);
    url.trim_end_matches('/').to_owned()
}
