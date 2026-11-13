//! The sandbox server's JSON lines, independent of the controller's API.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::ByteChunk;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[cfg_attr(feature = "typescript", ts(export_to = "controller-api.ts"))]
#[serde(deny_unknown_fields)]
pub struct SandboxRequest {
    /// Run, read, and first-write ids increase across the session; input,
    /// signals, and open-write pieces refer to earlier ids.
    #[cfg_attr(feature = "typescript", ts(type = "number"))]
    pub id: u64,
    #[serde(flatten)]
    pub action: SandboxRequestAction,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[cfg_attr(feature = "typescript", ts(export_to = "controller-api.ts"))]
#[serde(rename_all = "snake_case")]
pub enum SandboxRequestAction {
    Run(SandboxRun),
    Stdin(SandboxStdin),
    Signal(i32),
    Write(SandboxWrite),
    Read(SandboxRead),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[cfg_attr(feature = "typescript", ts(export_to = "controller-api.ts"))]
#[serde(deny_unknown_fields)]
pub struct SandboxRun {
    #[serde(flatten)]
    pub program: SandboxProgram,
    #[serde(default)]
    #[cfg_attr(feature = "typescript", ts(optional = nullable))]
    pub env: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    #[serde(default)]
    #[cfg_attr(feature = "typescript", ts(optional = nullable))]
    pub stdin: bool,
    /// As `docker exec --user`: a name or uid, optionally `:group` or `:gid`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[cfg_attr(feature = "typescript", ts(export_to = "controller-api.ts"))]
#[serde(rename_all = "snake_case")]
pub enum SandboxProgram {
    Argv(Vec<String>),
    Script(String),
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[cfg_attr(feature = "typescript", ts(export_to = "controller-api.ts"))]
#[serde(deny_unknown_fields)]
pub struct SandboxStdin {
    #[serde(default)]
    #[cfg_attr(feature = "typescript", ts(optional = nullable))]
    pub data: ByteChunk,
    #[serde(default)]
    #[cfg_attr(feature = "typescript", ts(optional = nullable))]
    pub close: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[cfg_attr(feature = "typescript", ts(export_to = "controller-api.ts"))]
#[serde(deny_unknown_fields)]
pub struct SandboxWrite {
    pub path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<u32>,
    pub data: ByteChunk,
    #[serde(default)]
    #[cfg_attr(feature = "typescript", ts(optional = nullable))]
    pub last: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[cfg_attr(feature = "typescript", ts(export_to = "controller-api.ts"))]
#[serde(deny_unknown_fields)]
pub struct SandboxRead {
    pub path: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[cfg_attr(feature = "typescript", ts(export_to = "controller-api.ts"))]
#[serde(deny_unknown_fields)]
pub struct SandboxAnswer {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "typescript", ts(type = "number | null"))]
    pub id: Option<u64>,
    #[serde(flatten)]
    pub answer: SandboxAnswerKind,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[cfg_attr(feature = "typescript", ts(export_to = "controller-api.ts"))]
#[serde(rename_all = "snake_case")]
pub enum SandboxAnswerKind {
    Started(SandboxStarted),
    Stdout(ByteChunk),
    Stderr(ByteChunk),
    Exited(SandboxExited),
    Written(bool),
    Data(ByteChunk),
    End(bool),
    Error(String),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[cfg_attr(feature = "typescript", ts(export_to = "controller-api.ts"))]
#[serde(deny_unknown_fields)]
pub struct SandboxStarted {
    pub pid: i32,
    pub argv: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[cfg_attr(feature = "typescript", ts(export_to = "controller-api.ts"))]
#[serde(deny_unknown_fields)]
pub struct SandboxExited {
    pub code: i32,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_lines_round_trip_and_reject_ambiguous_requests() {
        for line in [
            r#"{"id":0,"run":{"argv":["ls","-l"],"env":{"A":"1"},"cwd":"/repo","stdin":true}}"#,
            r#"{"id":2,"run":{"script":"ls -l | wc -l"}}"#,
            r#"{"id":0,"stdin":{"data":"aGk=","close":true}}"#,
            r#"{"id":0,"signal":15}"#,
            r#"{"id":3,"write":{"path":"/repo/a","mode":420,"data":"aGk="}}"#,
            r#"{"id":4,"read":{"path":"/repo/a"}}"#,
            r#"{"id":3,"write":{"path":"/repo/a","data":"","last":true}}"#,
        ] {
            let request: SandboxRequest = serde_json::from_str(line).unwrap();
            let encoded = serde_json::to_string(&request).unwrap();
            assert_eq!(request, serde_json::from_str(&encoded).unwrap());
        }
        for line in [
            r#"{"id":1,"signal":15,"read":{"path":"a"}}"#,
            r#"{"id":1,"run":{"argv":["ls"],"script":"ls"}}"#,
            r#"{"id":1,"run":{"argv":["ls"],"typo":true}}"#,
            r#"{"id":1,"stdin":{"data":"!"}}"#,
        ] {
            assert!(
                serde_json::from_str::<SandboxRequest>(line).is_err(),
                "{line}"
            );
        }
        let answer = SandboxAnswer {
            id: None,
            answer: SandboxAnswerKind::Error("bad line".into()),
        };
        assert_eq!(
            serde_json::to_string(&answer).unwrap(),
            r#"{"error":"bad line"}"#
        );
        for line in [
            r#"{"id":1,"started":{"pid":7,"argv":["ls","-l"]}}"#,
            r#"{"id":1,"stdout":"/wA="}"#,
            r#"{"id":1,"stderr":"ZXJy"}"#,
            r#"{"id":1,"exited":{"code":143}}"#,
            r#"{"id":3,"written":true}"#,
            r#"{"id":4,"data":"aGk="}"#,
            r#"{"id":4,"end":true}"#,
            r#"{"id":3,"error":"failed"}"#,
            r#"{"error":"bad line"}"#,
        ] {
            let answer: SandboxAnswer = serde_json::from_str(line).unwrap();
            assert_eq!(serde_json::to_string(&answer).unwrap(), line);
        }
    }
}
