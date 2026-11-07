//! Files in a sandbox, written in and read out whole.

use reqwest::Method;

use crate::error::{Error, Result};
use crate::sandbox::SandboxCalls;

/// A sandbox's files: [`Sandbox::files`](crate::Sandbox::files).
pub struct Files {
    calls: SandboxCalls,
}

impl Files {
    pub(crate) fn new(calls: SandboxCalls) -> Self {
        Self { calls }
    }

    /// Writes `data` to the file at `path`, in place of what it held; a new
    /// file gets mode `0o644`.
    pub async fn write(&self, path: &str, data: impl Into<Vec<u8>>) -> Result<()> {
        self.put(path, data.into(), None).await
    }

    /// Writes `data` to the file at `path`, and a new file gets `mode`
    /// (`0o755`).
    pub async fn write_with_mode(
        &self,
        path: &str,
        data: impl Into<Vec<u8>>,
        mode: u32,
    ) -> Result<()> {
        self.put(path, data.into(), Some(mode)).await
    }

    /// The file at `path`, as it is.
    pub async fn read(&self, path: &str) -> Result<Vec<u8>> {
        let calls = &self.calls;
        let query = [("path", path.to_owned())];
        let files = format!("{}/files", calls.path);
        calls
            .while_live(calls.client.bytes(Method::GET, &files, &query, None))
            .await
    }

    /// The file at `path`, as UTF-8 text.
    pub async fn read_text(&self, path: &str) -> Result<String> {
        String::from_utf8(self.read(path).await?)
            .map_err(|_| Error::Unexpected(format!("{path} isn't UTF-8")))
    }

    async fn put(&self, path: &str, data: Vec<u8>, mode: Option<u32>) -> Result<()> {
        let calls = &self.calls;
        let mut query = vec![("path", path.to_owned())];
        if let Some(mode) = mode {
            query.push(("mode", format!("{mode:o}")));
        }
        let files = format!("{}/files", calls.path);
        calls
            .while_live(calls.client.bytes(Method::PUT, &files, &query, Some(data)))
            .await
            .map(drop)
    }
}
