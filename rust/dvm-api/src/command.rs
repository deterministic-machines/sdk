use std::collections::BTreeMap;
use std::fmt;

use serde::{Deserialize, Serialize};

use crate::{ByteChunk, EnvironmentVariable, OutputStream, Run, Timestamp};

/// A command's number in its sandbox's execution, the controller's: 1, 2,
/// … in the order the commands were sent. A replay's commands have their
/// recording's numbers.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[cfg_attr(feature = "typescript", ts(export_to = "controller-api.ts"))]
#[serde(transparent)]
pub struct CommandNumber(u64);

impl CommandNumber {
    pub fn new(number: u64) -> Self {
        Self(number)
    }

    pub fn get(self) -> u64 {
        self.0
    }
}

impl fmt::Display for CommandNumber {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

/// What a command runs: its argv, as it is, its program found in the
/// sandbox's `PATH`; or a script, which the sandbox's shell runs,
/// `/bin/bash -lc` if its image has bash, else `/bin/sh -c` (dvm chooses
/// once, as the sandbox starts). In JSON, an array of strings, or a string.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[cfg_attr(feature = "typescript", ts(export_to = "controller-api.ts"))]
#[serde(untagged)]
pub enum CommandLine {
    Argv(Vec<String>),
    Script(String),
}

/// A script, which the sandbox's shell runs.
impl From<&str> for CommandLine {
    fn from(script: &str) -> Self {
        Self::Script(script.to_owned())
    }
}

impl From<String> for CommandLine {
    fn from(script: String) -> Self {
        Self::Script(script)
    }
}

/// An argv, which runs as it is.
impl<const N: usize> From<[&str; N]> for CommandLine {
    fn from(argv: [&str; N]) -> Self {
        Self::Argv(argv.iter().map(|&argument| argument.to_owned()).collect())
    }
}

impl From<&[&str]> for CommandLine {
    fn from(argv: &[&str]) -> Self {
        Self::Argv(argv.iter().map(|&argument| argument.to_owned()).collect())
    }
}

impl From<Vec<String>> for CommandLine {
    fn from(argv: Vec<String>) -> Self {
        Self::Argv(argv)
    }
}

impl CommandLine {
    /// The command as a person reads it: a script as it is, an argv its
    /// arguments joined by spaces.
    pub fn text(&self) -> String {
        match self {
            Self::Argv(argv) => argv.join(" "),
            Self::Script(script) => script.clone(),
        }
    }
}

/// A command to run in a sandbox, `POST /api/sandboxes/{id}/commands`' body.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[cfg_attr(feature = "typescript", ts(export_to = "controller-api.ts"))]
pub struct NewCommand {
    pub command: CommandLine,
    /// The directory it starts in, from the image's working directory if
    /// relative; the image's, unless given.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    /// Variables it's given, after the sandbox's, or in place of theirs.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub env: BTreeMap<String, String>,
    /// Whether it takes stdin, sent to it (`POST …/commands/{n}/stdin`)
    /// until its close; else it reads `/dev/null`.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub stdin: bool,
}

impl NewCommand {
    /// Whether a sandbox may be asked to run it, and why not: an argv names
    /// its program, no string holds a NUL, and each variable's name is one a
    /// shell takes. A sandbox refuses what it can't run besides (a script in
    /// an image with no shell), as the command's state says.
    pub fn check(&self) -> Result<(), String> {
        let strings: Vec<&str> = match &self.command {
            CommandLine::Argv(argv) if argv.is_empty() => {
                return Err("a command's argv names its program first".to_owned());
            }
            CommandLine::Argv(argv) => argv.iter().map(String::as_str).collect(),
            CommandLine::Script(script) if script.trim().is_empty() => {
                return Err("a command's script is empty".to_owned());
            }
            CommandLine::Script(script) => vec![script],
        };
        if strings
            .iter()
            .chain(&self.cwd.as_deref())
            .any(|text| text.contains('\0'))
        {
            return Err("a command's strings can't hold a NUL character".to_owned());
        }
        for (name, value) in &self.env {
            if !EnvironmentVariable::is_name(name) {
                return Err(format!(
                    "{name:?} isn't a variable name: letters, digits and '_', not starting with a digit"
                ));
            }
            if value.contains('\0') {
                return Err(format!("{name}'s value can't hold a NUL character"));
            }
        }
        Ok(())
    }
}

/// A command sent to a sandbox, or one its replay ran again.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[cfg_attr(feature = "typescript", ts(export_to = "controller-api.ts"))]
pub struct SandboxCommand {
    pub number: CommandNumber,
    /// What was asked: an argv, or a script.
    pub command: CommandLine,
    pub cwd: Option<String>,
    pub env: BTreeMap<String, String>,
    /// The argv that ran, once it started: a script's, its shell's.
    pub argv: Option<Vec<String>>,
    /// Its process's id in the sandbox, once it started.
    pub pid: Option<u64>,
    pub stdin: CommandStdin,
    pub state: CommandState,
    /// When it was sent; a replay's, when it started.
    pub created_at: Timestamp,
    pub started_at: Option<Timestamp>,
    pub ended_at: Option<Timestamp>,
}

impl SandboxCommand {
    /// The command as it stands once its run, `run`, is as it is: one that
    /// hadn't started or exited as its run ended is unfinished.
    pub fn as_of(mut self, run: &Run) -> Self {
        if self.state.is_active() && !run.state.is_active() {
            self.state = CommandState::Unfinished;
        }
        self
    }

    /// The command as a person reads it: the argv it ran, once it started,
    /// else what was asked.
    pub fn text(&self) -> String {
        match &self.argv {
            Some(argv) => argv.join(" "),
            None => self.command.text(),
        }
    }
}

/// A command's stdin.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[cfg_attr(feature = "typescript", ts(export_to = "controller-api.ts"))]
#[serde(rename_all = "snake_case")]
pub enum CommandStdin {
    /// It reads `/dev/null`.
    None,
    /// It takes what's sent to it, until its close.
    Open,
    /// Closed: it reads no more than it was sent.
    Closed,
}

/// A signal for a command's process, `POST …/commands/{n}/signal`'s body:
/// 15 for SIGTERM, 9 for SIGKILL. It goes to the command's process alone,
/// not what that started, and to none once it exited.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[cfg_attr(feature = "typescript", ts(export_to = "controller-api.ts"))]
pub struct CommandSignal {
    pub signal: i32,
}

impl CommandSignal {
    /// Linux's signals, real-time ones too.
    const SIGNALS: std::ops::RangeInclusive<i32> = 1..=64;

    /// Whether it's a signal, and why not.
    pub fn check(self) -> Result<(), String> {
        if Self::SIGNALS.contains(&self.signal) {
            Ok(())
        } else {
            Err(format!("a signal is 1 to 64, not {}", self.signal))
        }
    }
}

/// Where a command is:
///
/// ```text
/// Starting → Running → Exited
/// Starting → Refused
/// Starting | Running → Unfinished, as its sandbox ends
/// ```
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[cfg_attr(feature = "typescript", ts(export_to = "controller-api.ts"))]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum CommandState {
    /// Sent to its sandbox, and not started yet.
    Starting,
    Running,
    /// It exited with its code; or a signal killed it: its code is 128 and
    /// the signal, as a shell says, and the signal is given.
    Exited {
        code: i32,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        signal: Option<i32>,
    },
    /// Its sandbox didn't take it, and why: a script in an image with no
    /// shell, a command too long.
    Refused {
        reason: String,
    },
    /// Its sandbox ended before it did.
    Unfinished,
}

impl CommandState {
    /// Whether it may still start, or exit.
    pub fn is_active(&self) -> bool {
        matches!(self, Self::Starting | Self::Running)
    }
}

/// What `GET /api/sandboxes/{id}/commands/{n}/output?follow=true` sends, each
/// event's data: a command's output as it's written, from the offsets asked
/// for, and the command each time it changes. The stream ends once the
/// command ended, after all it wrote.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[cfg_attr(feature = "typescript", ts(export_to = "controller-api.ts"))]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum CommandOutputEvent {
    /// What it wrote to `stream`, from `offset`. A stream's chunks come in
    /// order, with no gap and no overlap.
    Output {
        stream: OutputStream,
        offset: u64,
        chunk: ByteChunk,
    },
    /// The command as it is: after its output so far, and after each change.
    /// The last event is the command, ended.
    Command { command: SandboxCommand },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_command_is_an_argv_or_a_script() {
        let script: NewCommand = serde_json::from_str(r#"{"command":"echo hi > /f"}"#).unwrap();
        assert_eq!(
            script.command,
            CommandLine::Script("echo hi > /f".to_owned())
        );
        assert_eq!(script.cwd, None);
        let argv: NewCommand =
            serde_json::from_str(r#"{"command":["cat","/f"],"cwd":"/tmp","env":{"A":"1"}}"#)
                .unwrap();
        assert_eq!(
            argv.command,
            CommandLine::Argv(vec!["cat".to_owned(), "/f".to_owned()])
        );
        assert_eq!(argv.env["A"], "1");
        assert_eq!(
            serde_json::to_string(&argv).unwrap(),
            r#"{"command":["cat","/f"],"cwd":"/tmp","env":{"A":"1"}}"#
        );
        assert!(script.check().is_ok() && argv.check().is_ok());
    }

    #[test]
    fn a_command_is_checked() {
        let refused = |json: &str| {
            let command: NewCommand = serde_json::from_str(json).unwrap();
            command.check().unwrap_err()
        };
        assert!(refused(r#"{"command":[]}"#).contains("program"));
        assert!(refused(r#"{"command":"  "}"#).contains("empty"));
        assert!(refused(r#"{"command":["a\u0000b"]}"#).contains("NUL"));
        assert!(refused(r#"{"command":"ls","cwd":"/a\u0000"}"#).contains("NUL"));
        assert!(refused(r#"{"command":"ls","env":{"1A":"x"}}"#).contains("variable name"));
        assert!(refused(r#"{"command":"ls","env":{"A":"\u0000"}}"#).contains("NUL"));
    }

    #[test]
    fn a_command_takes_stdin_only_if_it_says() {
        let reads: NewCommand =
            serde_json::from_str(r#"{"command":["cat"],"stdin":true}"#).unwrap();
        assert!(reads.stdin);
        let quiet: NewCommand = serde_json::from_str(r#"{"command":["cat"]}"#).unwrap();
        assert!(!quiet.stdin);
        assert_eq!(
            serde_json::to_string(&quiet).unwrap(),
            r#"{"command":["cat"]}"#
        );
        assert!(CommandSignal { signal: 15 }.check().is_ok());
        assert!(CommandSignal { signal: 0 }.check().is_err());
        assert!(CommandSignal { signal: 65 }.check().is_err());
    }

    #[test]
    fn a_state_serializes_tagged() {
        let killed = CommandState::Exited {
            code: 143,
            signal: Some(15),
        };
        assert_eq!(
            serde_json::to_string(&killed).unwrap(),
            r#"{"kind":"exited","code":143,"signal":15}"#
        );
        let exited: CommandState = serde_json::from_str(r#"{"kind":"exited","code":0}"#).unwrap();
        assert_eq!(
            exited,
            CommandState::Exited {
                code: 0,
                signal: None
            }
        );
        assert!(CommandState::Starting.is_active() && !exited.is_active());
    }
}
