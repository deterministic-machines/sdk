use std::future::{Future, IntoFuture};
use std::pin::Pin;
use std::sync::Arc;

use dvm_api::{SandboxProgram, SandboxRun};

use crate::sandbox_session::{Operation, Session};
use crate::{Error, Result};

/// Argv (an array or vector), or a shell script (a string).
#[derive(Clone, Debug)]
pub struct CommandLine(pub(crate) SandboxProgram);

impl From<&str> for CommandLine {
    fn from(script: &str) -> Self {
        Self(SandboxProgram::Script(script.to_owned()))
    }
}
impl From<String> for CommandLine {
    fn from(script: String) -> Self {
        Self(SandboxProgram::Script(script))
    }
}
impl<T: Into<String>> From<Vec<T>> for CommandLine {
    fn from(argv: Vec<T>) -> Self {
        Self(SandboxProgram::Argv(
            argv.into_iter().map(Into::into).collect(),
        ))
    }
}
impl<T: Into<String>, const N: usize> From<[T; N]> for CommandLine {
    fn from(argv: [T; N]) -> Self {
        Self::from(Vec::from(argv))
    }
}

/// A command's bytes and exit code, or 128 + its killing signal.
/// Nonzero is a result; the protocol cannot distinguish a signal from
/// an explicit exit with that code.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CommandResult {
    pub exit_code: i32,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

/// A started command. With omitted stdin the handle keeps it open.
#[derive(Debug)]
pub struct Command {
    pub(crate) session: Arc<Session>,
    pub(crate) operation: Operation,
    pub(crate) stdin_closed: tokio::sync::Mutex<bool>,
}

impl Command {
    pub async fn wait(&self) -> Result<CommandResult> {
        self.operation.wait(false).await?;
        Ok(self.operation.state.borrow().output.clone())
    }

    /// Sends to the process group. The protocol has no success acknowledgment.
    pub async fn signal(&self, number: i32) -> Result<()> {
        if !(1..=64).contains(&number) {
            return Err(Error::SandboxRequest {
                reason: "signal must be between 1 and 64".into(),
            });
        }
        self.session.signal(&self.operation, number).await
    }

    pub async fn write_stdin(&self, data: &[u8]) -> Result<()> {
        let closed = self.stdin_closed.lock().await;
        if *closed {
            return Err(Error::SandboxRequest {
                reason: "command stdin is closed".into(),
            });
        }
        self.session.stdin(&self.operation, data, false).await
    }

    pub async fn close_stdin(&self) -> Result<()> {
        let mut closed = self.stdin_closed.lock().await;
        if !*closed {
            self.session.stdin(&self.operation, &[], true).await?;
            *closed = true;
        }
        Ok(())
    }
}

/// Options for starting a command, applied before awaiting it.
#[must_use = "await to start the command"]
pub struct StartBuilder {
    pub(crate) session: Option<Arc<Session>>,
    pub(crate) run: SandboxRun,
    pub(crate) stdin: Option<Vec<u8>>,
}

impl StartBuilder {
    pub fn env(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        self.run.env.insert(name.into(), value.into());
        self
    }
    pub fn cwd(mut self, path: impl Into<String>) -> Self {
        self.run.cwd = Some(path.into());
        self
    }
    /// As `docker exec --user`: a name or uid, optionally `:group` or `:gid`.
    /// Omitted, the command runs as the sandbox server's user.
    pub fn user(mut self, user: impl Into<String>) -> Self {
        self.run.user = Some(user.into());
        self
    }
    /// Sends bytes, then closes stdin. Omitted stdin stays open on a handle.
    pub fn stdin(mut self, data: impl AsRef<[u8]>) -> Self {
        self.stdin = Some(data.as_ref().to_vec());
        self
    }

    async fn start(self) -> Result<Command> {
        let session = self.session.ok_or_else(no_commands)?;
        session.start(self.run, self.stdin).await
    }
}

impl IntoFuture for StartBuilder {
    type Output = Result<Command>;
    type IntoFuture = Pin<Box<dyn Future<Output = Self::Output> + Send>>;
    fn into_future(self) -> Self::IntoFuture {
        Box::pin(self.start())
    }
}

/// Options for a command run to its end. Omitted stdin is closed empty.
#[must_use = "await to run the command"]
pub struct RunBuilder(pub(crate) StartBuilder);
impl RunBuilder {
    pub fn env(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        self.0 = self.0.env(name, value);
        self
    }
    pub fn cwd(mut self, path: impl Into<String>) -> Self {
        self.0 = self.0.cwd(path);
        self
    }
    pub fn user(mut self, user: impl Into<String>) -> Self {
        self.0 = self.0.user(user);
        self
    }
    pub fn stdin(mut self, data: impl AsRef<[u8]>) -> Self {
        self.0 = self.0.stdin(data);
        self
    }
}
impl IntoFuture for RunBuilder {
    type Output = Result<CommandResult>;
    type IntoFuture = Pin<Box<dyn Future<Output = Self::Output> + Send>>;
    fn into_future(mut self) -> Self::IntoFuture {
        self.0.stdin.get_or_insert_default();
        Box::pin(async move { self.0.await?.wait().await })
    }
}

#[must_use = "await to write the file"]
pub struct WriteFileBuilder {
    pub(crate) session: Option<Arc<Session>>,
    pub(crate) path: String,
    pub(crate) data: Vec<u8>,
    pub(crate) mode: Option<u32>,
}
impl WriteFileBuilder {
    pub fn mode(mut self, mode: u32) -> Self {
        self.mode = Some(mode);
        self
    }
}
impl IntoFuture for WriteFileBuilder {
    type Output = Result<()>;
    type IntoFuture = Pin<Box<dyn Future<Output = Self::Output> + Send>>;
    fn into_future(self) -> Self::IntoFuture {
        Box::pin(async move {
            self.session
                .ok_or_else(no_commands)?
                .write_file(self.path, &self.data, self.mode)
                .await
        })
    }
}

pub(crate) fn no_commands() -> Error {
    Error::Conflict {
        reason: "this sandbox runs a supplied program and does not take commands".into(),
    }
}
