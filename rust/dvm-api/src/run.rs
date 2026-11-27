use std::fmt;

use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::{SandboxId, Timestamp};

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

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[cfg_attr(feature = "typescript", ts(export_to = "controller-api.ts"))]
pub struct Run {
    pub id: RunId,
    pub state: RunState,
    pub runner: Option<String>,
    pub created_at: Timestamp,
    pub started_at: Option<Timestamp>,
    pub ended_at: Option<Timestamp>,
    pub has_timeline: bool,
    pub inspectable: bool,
    pub outdated: bool,
    pub version: u64,
}

impl Run {
    pub fn supersedes(&self, shown: &Run) -> bool {
        self.id == shown.id && self.version >= shown.version
    }

    pub fn sandbox(&self) -> SandboxId {
        SandboxId::new(self.id.get())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[cfg_attr(feature = "typescript", ts(export_to = "controller-api.ts"))]
#[serde(rename_all = "snake_case")]
pub enum OutputStream {
    Stdout,
    Stderr,
}

impl OutputStream {
    pub fn name(self) -> &'static str {
        match self {
            Self::Stdout => "stdout",
            Self::Stderr => "stderr",
        }
    }
}

/// Raw bytes serialized as base64 in JSON payloads.
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

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[cfg_attr(feature = "typescript", ts(export_to = "controller-api.ts"))]
pub struct RunOutput {
    pub stdout: ByteChunk,
    pub stderr: ByteChunk,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[cfg_attr(feature = "typescript", ts(export_to = "controller-api.ts"))]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RunState {
    Queued,
    Running,
    Paused,
    Exited { code: i32 },
    Stopped,
    Failed { reason: String },
}

impl RunState {
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

    pub fn is_active(&self) -> bool {
        matches!(self, Self::Queued | Self::Running | Self::Paused)
    }

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
