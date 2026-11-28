//! The connection to dvm.run's API: its address, the key every request
//! bears, and what a refusal means.

use std::sync::Arc;
use std::time::Duration;

use dvm_api::{ApiError, SandboxId};
use reqwest::header::HeaderMap;
use reqwest::{Method, RequestBuilder, StatusCode};
use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::credentials;
use crate::error::{Error, Result};
use crate::sandbox::{Sandbox, SandboxBuilder};

pub const DEFAULT_API_URL: &str = "https://sandbox.dvm.run";

/// The controller answers a `?newer_than=` read within 30 s, and keeps a
/// quiet event stream alive with comments more often than this.
const QUIET_LIMIT: Duration = Duration::from_secs(60);
const RETRY_DELAYS: [Duration; 4] = [
    Duration::from_millis(100),
    Duration::from_millis(250),
    Duration::from_millis(500),
    Duration::from_millis(1000),
];
const MAX_RETRY_AFTER: Duration = Duration::from_secs(5);

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
        Ok(Sandbox::new(self.clone(), sandbox, None))
    }

    /// The answer's JSON; an empty answer is `null`.
    pub(crate) async fn json<T: DeserializeOwned>(
        &self,
        method: Method,
        path: &str,
        query: Query<'_>,
        body: Option<&impl Serialize>,
    ) -> Result<T> {
        let idempotent = matches!(method, Method::GET | Method::PUT | Method::DELETE);
        self.send_json(method, path, query, body, idempotent).await
    }

    pub(crate) async fn json_idempotent<T: DeserializeOwned>(
        &self,
        method: Method,
        path: &str,
        query: Query<'_>,
        body: Option<&impl Serialize>,
    ) -> Result<T> {
        self.send_json(method, path, query, body, true).await
    }

    async fn send_json<T: DeserializeOwned>(
        &self,
        method: Method,
        path: &str,
        query: Query<'_>,
        body: Option<&impl Serialize>,
        idempotent: bool,
    ) -> Result<T> {
        let encoded = body
            .map(serde_json::to_vec)
            .transpose()
            .map_err(|error| Error::Unexpected(format!("{path}'s body: {error}")))?;
        let payload = encoded.as_deref().map(|bytes| ("application/json", bytes));
        let bytes = self.send(method, path, query, payload, idempotent).await?;
        let slice = if bytes.is_empty() {
            &b"null"[..]
        } else {
            &bytes
        };
        serde_json::from_slice(slice)
            .map_err(|error| Error::Unexpected(format!("{path}'s answer: {error}")))
    }

    pub(crate) async fn get_bytes(&self, path: &str, query: Query<'_>) -> Result<Vec<u8>> {
        self.send(Method::GET, path, query, None, true).await
    }

    pub(crate) async fn input(
        &self,
        path: &str,
        input: &dvm_api::SandboxInput,
        body: &[u8],
    ) -> Result<dvm_api::SandboxInputState> {
        let query = [
            ("offset", input.offset.to_string()),
            ("end", input.end.to_string()),
        ];
        let (status, bytes, attempt) = self
            .exchange(
                Method::POST,
                path,
                &query,
                Some(("application/octet-stream", body)),
                true,
            )
            .await?;
        if !status.is_success() {
            let reason = refusal_reason(status, &bytes);
            if status == StatusCode::CONFLICT
                && attempt > 0
                && input.end
                && body.is_empty()
                && reason == format!("input already ended at length {}", input.offset)
            {
                return Ok(dvm_api::SandboxInputState {
                    length: input.offset,
                    ended: true,
                });
            }
            return Err(Error::refusal(status.as_u16(), reason));
        }
        serde_json::from_slice(&bytes)
            .map_err(|error| Error::Unexpected(format!("{path}'s answer: {error}")))
    }

    pub(crate) async fn output_after(
        &self,
        path: &str,
        offset: u64,
        stderr_offset: u64,
    ) -> Result<dvm_api::OutputAfter> {
        let query = [
            ("offset", offset.to_string()),
            ("stderr_offset", stderr_offset.to_string()),
        ];
        self.output_page(path, &query).await
    }

    /// What the run has written so far: the first page at once, then the
    /// rest of what each page stopped short of.
    pub(crate) async fn output(&self, path: &str) -> Result<dvm_api::RunOutput> {
        let mut page = self.output_page(path, &[]).await?;
        let (mut stdout, mut stderr) = (Vec::new(), Vec::new());
        loop {
            stdout.extend_from_slice(page.output.stdout.bytes());
            stderr.extend_from_slice(page.output.stderr.bytes());
            if !page.cut {
                return Ok(dvm_api::RunOutput {
                    stdout: dvm_api::ByteChunk::new(stdout),
                    stderr: dvm_api::ByteChunk::new(stderr),
                });
            }
            page = self
                .output_after(path, stdout.len() as u64, stderr.len() as u64)
                .await?;
        }
    }

    async fn output_page(&self, path: &str, query: Query<'_>) -> Result<dvm_api::OutputAfter> {
        let bytes = self.send(Method::GET, path, query, None, true).await?;
        dvm_api::OutputAfter::from_frame(&bytes)
            .map_err(|error| Error::Unexpected(format!("{path}'s answer: {error}")))
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

    async fn send(
        &self,
        method: Method,
        path: &str,
        query: Query<'_>,
        body: Option<(&'static str, &[u8])>,
        idempotent: bool,
    ) -> Result<Vec<u8>> {
        let (status, bytes, _) = self.exchange(method, path, query, body, idempotent).await?;
        if status.is_success() {
            return Ok(bytes);
        }
        Err(Error::refusal(
            status.as_u16(),
            refusal_reason(status, &bytes),
        ))
    }

    async fn exchange(
        &self,
        method: Method,
        path: &str,
        query: Query<'_>,
        body: Option<(&'static str, &[u8])>,
        idempotent: bool,
    ) -> Result<(StatusCode, Vec<u8>, usize)> {
        for (attempt, delay) in RETRY_DELAYS
            .into_iter()
            .map(Some)
            .chain(std::iter::once(None))
            .enumerate()
        {
            let mut request = self
                .request(method.clone(), path, query)
                .timeout(QUIET_LIMIT);
            if let Some((content_type, payload)) = body {
                request = request
                    .header(reqwest::header::CONTENT_TYPE, content_type)
                    .body(payload.to_vec());
            }
            let response = match request.send().await {
                Ok(response) => response,
                Err(error) => {
                    if let Some(delay) = delay
                        && retryable_error(&error, idempotent, true)
                    {
                        tokio::time::sleep(delay).await;
                        continue;
                    }
                    return Err(Error::network(
                        format!("couldn't reach {}{path}", self.inner.api),
                        error,
                    ));
                }
            };
            let status = response.status();
            let retry_after = retry_after_delay(response.headers());
            let bytes = match response.bytes().await {
                Ok(bytes) => bytes,
                Err(error) => {
                    if let Some(delay) = delay
                        && retryable_error(&error, idempotent, false)
                    {
                        tokio::time::sleep(delay).await;
                        continue;
                    }
                    return Err(Error::network(format!("reading {path}"), error));
                }
            };
            if !status.is_success()
                && let Some(delay) = delay
                && retryable_status(status.as_u16(), &bytes, idempotent)
            {
                let wait = retry_after.map_or(delay, |hint| hint.max(delay).min(MAX_RETRY_AFTER));
                tokio::time::sleep(wait).await;
                continue;
            }
            return Ok((status, bytes.to_vec(), attempt));
        }
        unreachable!()
    }
}

fn retryable_error(error: &reqwest::Error, idempotent: bool, sending: bool) -> bool {
    !error.is_builder() && (idempotent || (sending && error.is_connect()))
}

fn retryable_status(status: u16, bytes: &[u8], idempotent: bool) -> bool {
    if matches!(status, 408 | 429) {
        return true;
    }
    if !idempotent {
        return false;
    }
    matches!(status, 502 | 504) || (status == 503 && !is_controller_refusal(bytes))
}

fn is_controller_refusal(bytes: &[u8]) -> bool {
    serde_json::from_slice::<serde_json::Value>(bytes)
        .ok()
        .and_then(|body| {
            body.get("error")
                .and_then(serde_json::Value::as_str)
                .map(|_| ())
        })
        .is_some()
}

fn retry_after_delay(headers: &HeaderMap) -> Option<Duration> {
    headers
        .get(reqwest::header::RETRY_AFTER)?
        .to_str()
        .ok()?
        .trim()
        .parse::<u64>()
        .ok()
        .map(Duration::from_secs)
}

fn refusal_reason(status: StatusCode, bytes: &[u8]) -> String {
    if let Ok(refusal) = serde_json::from_slice::<ApiError>(bytes) {
        return refusal.message;
    }
    let text = String::from_utf8_lossy(bytes);
    if text.is_empty() {
        status.canonical_reason().unwrap_or("").to_owned()
    } else {
        text.into_owned()
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
