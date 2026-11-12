use std::fmt;

use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::{SandboxId, Timestamp};

/// A run's id, the controller's.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[cfg_attr(feature = "typescript", ts(export_to = "controller-api.ts"))]
#[serde(transparent)]
pub struct RunId(i64);

impl RunId {
    pub fn new(id: i64) -> Self {
        Self(id)
    }

    pub fn get(self) -> i64 {
        self.0
    }
}

impl fmt::Display for RunId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

/// A sandbox's run, of its spec, recorded; or a replay of its recording.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[cfg_attr(feature = "typescript", ts(export_to = "controller-api.ts"))]
pub struct Run {
    pub id: RunId,
    /// The run whose recording this one replays, if it's a replay.
    pub replay_of: Option<RunId>,
    pub state: RunState,
    /// The runner it was sent to, once it was.
    pub runner: Option<String>,
    /// When it was queued.
    pub created_at: Timestamp,
    pub started_at: Option<Timestamp>,
    pub ended_at: Option<Timestamp>,
    /// The size of its log, which its runner keeps, once it ended; none
    /// for a replay, or a run that left no log.
    pub log_bytes: Option<u64>,
    /// Whether its log was made by another dvm than its runner's now: a log
    /// replays only on the dvm that made it.
    pub log_outdated: bool,
    /// A replay's, once it exited: whether it was the run it replayed.
    pub verdict: Option<ReplayVerdict>,
    /// How many times it changed: of two copies of a run, the one with the
    /// higher version is the newer.
    pub version: u64,
}

impl Run {
    /// Whether this copy is the run as it is in `shown`'s place, or newer:
    /// the same run, and changed no fewer times. An answer and an event may
    /// come out of order; the newer stays.
    pub fn supersedes(&self, shown: &Run) -> bool {
        self.id == shown.id && self.version >= shown.version
    }

    /// The sandbox it's of: its own, or the one it replays.
    pub fn sandbox(&self) -> SandboxId {
        SandboxId::new(self.replay_of.unwrap_or(self.id).get())
    }

    /// Whether it may be replayed, and why not: it's no replay itself, its
    /// log is `whole` (one cut short replays only up to its cut), its runner
    /// keeps it, and its runner's dvm made it. Its sandbox says
    /// whether it's whole (`Sandbox::replayable`).
    pub(crate) fn replayable_if_whole(&self, whole: bool) -> Result<(), String> {
        if let Some(recorded) = self.replay_of {
            return Err(format!("run {} is a replay of run {recorded}", self.id));
        }
        if !whole {
            return Err(format!(
                "run {} didn't exit, so its log is cut short",
                self.id
            ));
        }
        if self.log_bytes.is_none() {
            return Err(format!("run {} has no log", self.id));
        }
        if self.log_outdated {
            return Err(format!(
                "run {}'s log was made by another dvm than its runner's now: run it again",
                self.id
            ));
        }
        Ok(())
    }
}

/// Whether a replay was the run it replayed, as far as can be seen from
/// outside: its exit code, and its output, byte for byte.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[cfg_attr(feature = "typescript", ts(export_to = "controller-api.ts"))]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ReplayVerdict {
    /// The same exit code, and the same output.
    Same,
    OtherExit {
        recorded: i32,
        replayed: i32,
    },
    /// Another output: the stream it parted in first, and at which byte.
    OtherOutput {
        stream: OutputStream,
        at: u64,
    },
}

impl ReplayVerdict {
    pub fn is_same(&self) -> bool {
        matches!(self, Self::Same)
    }
}

/// One of a program's output streams.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[cfg_attr(feature = "typescript", ts(export_to = "controller-api.ts"))]
#[serde(rename_all = "snake_case")]
pub enum OutputStream {
    Stdout,
    Stderr,
}

impl OutputStream {
    /// Its name, as a file of a run's output is named.
    pub fn name(self) -> &'static str {
        match self {
            Self::Stdout => "stdout",
            Self::Stderr => "stderr",
        }
    }
}

/// Bytes as a program wrote them: base64 in JSON, since they needn't be
/// UTF-8.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[cfg_attr(
    feature = "typescript",
    ts(export_to = "controller-api.ts", type = "string")
)]
pub struct ByteChunk(Vec<u8>);

impl ByteChunk {
    pub fn new(bytes: Vec<u8>) -> Self {
        Self(bytes)
    }

    pub fn bytes(&self) -> &[u8] {
        &self.0
    }
}

impl Serialize for ByteChunk {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&BASE64.encode(&self.0))
    }
}

impl<'de> Deserialize<'de> for ByteChunk {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let encoded = String::deserialize(deserializer)?;
        BASE64
            .decode(encoded)
            .map(Self)
            .map_err(serde::de::Error::custom)
    }
}

/// What a run's program has written so far, as bytes.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[cfg_attr(feature = "typescript", ts(export_to = "controller-api.ts"))]
pub struct RunOutput {
    pub stdout: ByteChunk,
    pub stderr: ByteChunk,
}

/// Where a run is:
///
/// ```text
/// Queued → Running ⇄ Paused
/// Queued | Running | Paused → Exited | Stopped | Failed
/// ```
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[cfg_attr(feature = "typescript", ts(export_to = "controller-api.ts"))]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RunState {
    /// Waiting for a runner to take it, or for the one it was sent to to
    /// start it.
    Queued,
    Running,
    Paused,
    /// The program exited, with its code.
    Exited {
        code: i32,
    },
    /// Stopped by its owner.
    Stopped,
    /// Ended by dvm or the platform, not by the program, and why.
    Failed {
        reason: String,
    },
}

impl RunState {
    /// The state's name, as the API and the controller's database spell it.
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Running => "running",
            Self::Paused => "paused",
            Self::Exited { .. } => "exited",
            Self::Stopped => "stopped",
            Self::Failed { .. } => "failed",
        }
    }

    /// Whether the run hasn't ended.
    pub fn is_active(&self) -> bool {
        matches!(self, Self::Queued | Self::Running | Self::Paused)
    }

    /// The state an owner's action takes the run to, if the action may be
    /// taken: a queued run can only be stopped, before it starts.
    pub fn after(&self, action: RunAction) -> Result<Self, InvalidRunAction> {
        match (self, action) {
            (Self::Running, RunAction::Pause) => Ok(Self::Paused),
            (Self::Paused, RunAction::Resume) => Ok(Self::Running),
            (Self::Queued | Self::Running | Self::Paused, RunAction::Stop) => Ok(Self::Stopped),
            _ => Err(InvalidRunAction {
                action,
                state: self.kind(),
            }),
        }
    }

    pub fn allows(&self, action: RunAction) -> bool {
        self.after(action).is_ok()
    }
}

/// What an owner may do to a sandbox's run, besides starting it, as it's
/// made: the last segment of its path, `POST /api/sandboxes/{id}/{action}`;
/// a replay may be stopped, `POST /api/replays/{id}/stop`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[cfg_attr(feature = "typescript", ts(export_to = "controller-api.ts"))]
#[serde(rename_all = "snake_case")]
pub enum RunAction {
    Pause,
    Resume,
    Stop,
}

impl fmt::Display for RunAction {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Pause => "pause",
            Self::Resume => "resume",
            Self::Stop => "stop",
        })
    }
}

/// An action the run's state doesn't allow.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("can't {action} a run that is {state}")]
pub struct InvalidRunAction {
    action: RunAction,
    state: &'static str,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn actions_follow_the_state_machine() {
        use RunAction::*;
        let exited = RunState::Exited { code: 0 };
        let failed = RunState::Failed {
            reason: "unserved syscall".to_owned(),
        };
        let cases = [
            (RunState::Queued, Pause, None),
            (RunState::Queued, Resume, None),
            (RunState::Queued, Stop, Some(RunState::Stopped)),
            (RunState::Running, Pause, Some(RunState::Paused)),
            (RunState::Running, Resume, None),
            (RunState::Running, Stop, Some(RunState::Stopped)),
            (RunState::Paused, Pause, None),
            (RunState::Paused, Resume, Some(RunState::Running)),
            (RunState::Paused, Stop, Some(RunState::Stopped)),
            (exited.clone(), Stop, None),
            (RunState::Stopped, Stop, None),
            (failed.clone(), Resume, None),
        ];
        for (state, action, after) in cases {
            assert_eq!(state.after(action).ok(), after, "{action} from {state:?}");
        }
        assert!(!exited.is_active() && !failed.is_active());
    }

    #[test]
    fn a_refusal_says_why() {
        let error = RunState::Queued.after(RunAction::Pause).unwrap_err();
        assert_eq!(error.to_string(), "can't pause a run that is queued");
    }

    #[test]
    fn serializes_tagged() {
        let json = serde_json::to_string(&RunState::Exited { code: 3 }).unwrap();
        assert_eq!(json, r#"{"kind":"exited","code":3}"#);
        assert_eq!(
            serde_json::to_string(&RunAction::Resume).unwrap(),
            r#""resume""#
        );
    }
}
