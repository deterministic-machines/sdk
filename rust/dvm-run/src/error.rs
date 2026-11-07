use std::path::PathBuf;

use dvm_api::{CommandNumber, RunId, RunState, SandboxId};

pub type Result<T, E = Error> = std::result::Result<T, E>;

/// What the SDK returns when a call fails. A command's exit code is never
/// one of these.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// 400: the request isn't one dvm.run takes, such as an image that
    /// isn't one.
    #[error("400: {reason}")]
    InvalidRequest { reason: String },
    /// 401: the API key was revoked, or isn't one.
    #[error("401: {reason}")]
    Authentication { reason: String },
    /// 404: no such sandbox, command, execution or replay, or not the key's
    /// user's.
    #[error("404: {reason}")]
    NotFound { reason: String },
    /// 409: what was asked can't be done in the state it's in.
    #[error("409: {reason}")]
    Conflict { reason: String },
    /// Another refusal: its status, and dvm.run's reason.
    #[error("{status}: {reason}")]
    Refused { status: u16, reason: String },
    /// The sandbox ended (stopped, or its command exited) before the call
    /// could act on it.
    #[error("sandbox {sandbox} is {}", state.kind())]
    SandboxEnded { sandbox: SandboxId, state: RunState },
    /// dvm or the platform ended the sandbox, and why: a syscall dvm can't
    /// serve, a limit, its runner going away.
    #[error("sandbox {sandbox} failed: {reason}")]
    SandboxFailed { sandbox: SandboxId, reason: String },
    /// The sandbox didn't take the command, and why: a script in an image
    /// with no shell, a command too long.
    #[error("command {number} was refused: {reason}")]
    CommandRefused {
        number: CommandNumber,
        reason: String,
    },
    /// A replay ended without a verdict: dvm saw it part from its execution
    /// and failed it, with where, or it was stopped.
    #[error("replay {replay} {}{}", state.kind(), failure_of(state))]
    ReplayEnded { replay: RunId, state: RunState },
    /// No key was given, in DVM_API_KEY, or saved by `dvm-run login`.
    #[error("no API key for {api_url}: run `dvm-run login`, or set DVM_API_KEY")]
    NoApiKey { api_url: String },
    /// The saved keys couldn't be read or written.
    #[error("{}: {problem}", path.display())]
    Credentials { path: PathBuf, problem: String },
    /// dvm.run couldn't be reached, or the connection to it dropped.
    #[error("{context}")]
    Network {
        context: String,
        #[source]
        source: Option<reqwest::Error>,
    },
    /// dvm.run answered what the SDK can't read.
    #[error("{0}")]
    Unexpected(String),
}

impl Error {
    /// The refusal a response's status and reason are.
    pub(crate) fn refusal(status: u16, reason: String) -> Self {
        match status {
            400 => Self::InvalidRequest { reason },
            401 => Self::Authentication { reason },
            404 => Self::NotFound { reason },
            409 => Self::Conflict { reason },
            status => Self::Refused { status, reason },
        }
    }

    /// The error a call to a sandbox that ended in `state` is.
    pub(crate) fn ended(sandbox: SandboxId, state: RunState) -> Self {
        match state {
            RunState::Failed { reason } => Self::SandboxFailed { sandbox, reason },
            state => Self::SandboxEnded { sandbox, state },
        }
    }

    pub(crate) fn network(context: impl Into<String>, source: reqwest::Error) -> Self {
        Self::Network {
            context: context.into(),
            source: Some(source),
        }
    }
}

fn failure_of(state: &RunState) -> String {
    match state {
        RunState::Failed { reason } => format!(": {reason}"),
        _ => String::new(),
    }
}
