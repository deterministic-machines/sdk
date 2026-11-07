//! Commands in a sandbox: run one and get its output, or spawn one and hold
//! a handle to it.

use std::future::{Future, IntoFuture};
use std::marker::PhantomData;
use std::pin::Pin;
use std::time::Duration;

use dvm_api::{
    CommandLine, CommandNumber, CommandOutputEvent, CommandSignal, CommandState, NewCommand,
    OutputStream, SandboxCommand,
};
use reqwest::Method;
use tokio::time::Instant;

use crate::error::{Error, Result};
use crate::sandbox::SandboxCalls;

const SIGKILL: i32 = 9;

/// A sandbox's commands: [`Sandbox::commands`](crate::Sandbox::commands).
pub struct Commands {
    calls: SandboxCalls,
}

/// A command that exited: its output, and its exit code. A signal's kill is
/// code 128 and the signal, as a shell says, with the signal given.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommandResult {
    pub number: CommandNumber,
    pub stdout: String,
    pub stderr: String,
    pub exit_code: i32,
    pub signal: Option<i32>,
}

/// Called with each piece of a stream's output, as text, as it's written.
pub type OutputCallback = Box<dyn FnMut(&str) + Send>;

/// A command to run: [`Commands::run`] or [`Commands::spawn`], then how it
/// starts; awaited, its result or a handle to it.
#[must_use = "a command starts once this is awaited"]
pub struct CommandBuilder<Mode> {
    calls: SandboxCalls,
    new: NewCommand,
    stdin: Option<Vec<u8>>,
    waiting: Waiting,
    mode: PhantomData<Mode>,
}

/// Run to its exit, awaited: [`Commands::run`].
pub struct Foreground;
/// Started, and a handle to it: [`Commands::spawn`].
pub struct Background;

/// How a command is waited for.
#[derive(Default)]
struct Waiting {
    timeout: Option<Duration>,
    on_stdout: Option<OutputCallback>,
    on_stderr: Option<OutputCallback>,
}

/// A command spawned in the background.
#[derive(Clone, Debug)]
pub struct CommandHandle {
    calls: SandboxCalls,
    number: CommandNumber,
    path: String,
}

/// Waiting for a command, from [`CommandHandle::wait`]; awaited, its result
/// once it exits. Dropped, it stops the waiting, not the command.
#[must_use = "a command is waited for once this is awaited"]
pub struct WaitForCommand {
    handle: CommandHandle,
    waiting: Waiting,
}

impl Commands {
    pub(crate) fn new(calls: SandboxCalls) -> Self {
        Self { calls }
    }

    /// Runs `command` to its exit: a script (`&str`), which the sandbox's
    /// shell runs (`/bin/bash -lc`, or `/bin/sh -c` in an image without
    /// bash), or an argv (`["ls", "-l"]`), which runs as it is. Dropped
    /// before it exits, it's killed.
    pub fn run(&self, command: impl Into<CommandLine>) -> CommandBuilder<Foreground> {
        CommandBuilder::new(self.calls.clone(), command.into())
    }

    /// Starts `command`, as [`Commands::run`] takes it, and gives a handle
    /// to it at once.
    pub fn spawn(&self, command: impl Into<CommandLine>) -> CommandBuilder<Background> {
        CommandBuilder::new(self.calls.clone(), command.into())
    }
}

impl<Mode> CommandBuilder<Mode> {
    fn new(calls: SandboxCalls, command: CommandLine) -> Self {
        Self {
            calls,
            new: NewCommand {
                command,
                cwd: None,
                env: Default::default(),
                stdin: false,
            },
            stdin: None,
            waiting: Waiting::default(),
            mode: PhantomData,
        }
    }

    pub fn cwd(mut self, cwd: impl Into<String>) -> Self {
        self.new.cwd = Some(cwd.into());
        self
    }

    /// A variable, beside the sandbox's.
    pub fn env(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        self.new.env.insert(name.into(), value.into());
        self
    }

    /// Sent to its stdin, which is closed after; else it reads `/dev/null`.
    pub fn stdin(mut self, data: impl Into<Vec<u8>>) -> Self {
        self.new.stdin = true;
        self.stdin = Some(data.into());
        self
    }

    async fn start(self) -> Result<(CommandHandle, Waiting)> {
        let calls = self.calls;
        let path = format!("{}/commands", calls.path);
        let started: SandboxCommand = calls
            .while_live(calls.client.json(Method::POST, &path, &[], Some(&self.new)))
            .await?;
        let handle = CommandHandle::new(calls, started.number);
        if let Some(data) = self.stdin {
            handle.send(data, true).await?;
        }
        Ok((handle, self.waiting))
    }
}

impl CommandBuilder<Foreground> {
    /// Once it passes, the command is killed (SIGKILL), and its result says
    /// so.
    pub fn timeout(mut self, timeout: Duration) -> Self {
        self.waiting.timeout = Some(timeout);
        self
    }

    pub fn on_stdout(mut self, callback: impl FnMut(&str) + Send + 'static) -> Self {
        self.waiting.on_stdout = Some(Box::new(callback));
        self
    }

    pub fn on_stderr(mut self, callback: impl FnMut(&str) + Send + 'static) -> Self {
        self.waiting.on_stderr = Some(Box::new(callback));
        self
    }
}

impl CommandBuilder<Background> {
    /// Keeps its stdin open, for [`CommandHandle::send_stdin`].
    pub fn open_stdin(mut self) -> Self {
        self.new.stdin = true;
        self
    }
}

impl IntoFuture for CommandBuilder<Foreground> {
    type Output = Result<CommandResult>;
    type IntoFuture = Pin<Box<dyn Future<Output = Result<CommandResult>> + Send>>;

    fn into_future(self) -> Self::IntoFuture {
        Box::pin(async move {
            let (handle, waiting) = self.start().await?;
            let mut kill_on_drop = KillOnDrop(Some(handle.clone()));
            let result = wait_for(&handle, waiting).await;
            kill_on_drop.0 = None;
            result
        })
    }
}

impl IntoFuture for CommandBuilder<Background> {
    type Output = Result<CommandHandle>;
    type IntoFuture = Pin<Box<dyn Future<Output = Result<CommandHandle>> + Send>>;

    fn into_future(self) -> Self::IntoFuture {
        Box::pin(async move { Ok(self.start().await?.0) })
    }
}

impl CommandHandle {
    fn new(calls: SandboxCalls, number: CommandNumber) -> Self {
        let path = format!("{}/commands/{number}", calls.path);
        Self {
            calls,
            number,
            path,
        }
    }

    pub fn number(&self) -> CommandNumber {
        self.number
    }

    /// Its result once it exits, with all it wrote.
    pub fn wait(&self) -> WaitForCommand {
        WaitForCommand {
            handle: self.clone(),
            waiting: Waiting::default(),
        }
    }

    /// Sends `data` to its stdin, which it was spawned with open.
    pub async fn send_stdin(&self, data: impl Into<Vec<u8>>) -> Result<()> {
        self.send(data.into(), false).await
    }

    /// Closes its stdin: it reads no more than it was sent.
    pub async fn close_stdin(&self) -> Result<()> {
        self.send(Vec::new(), true).await
    }

    /// Kills its process (SIGKILL; not what that started).
    pub async fn kill(&self) -> Result<()> {
        self.signal(SIGKILL).await
    }

    /// Sends `signal` to its process.
    pub async fn signal(&self, signal: i32) -> Result<()> {
        let path = format!("{}/signal", self.path);
        let body = CommandSignal { signal };
        let calls = &self.calls;
        calls
            .while_live(
                calls
                    .client
                    .json::<()>(Method::POST, &path, &[], Some(&body)),
            )
            .await
    }

    /// The command as it is now.
    pub async fn info(&self) -> Result<SandboxCommand> {
        self.calls
            .client
            .json(Method::GET, &self.path, &[], None::<&()>)
            .await
    }

    async fn send(&self, data: Vec<u8>, close: bool) -> Result<()> {
        let path = format!("{}/stdin", self.path);
        let closing = [("close", "true".to_owned())];
        let query: &[(&str, String)] = if close { &closing } else { &[] };
        let calls = &self.calls;
        calls
            .while_live(calls.client.bytes(Method::POST, &path, query, Some(data)))
            .await
            .map(drop)
    }
}

impl WaitForCommand {
    /// Once it passes, the command is killed (SIGKILL), and its result says
    /// so.
    pub fn timeout(mut self, timeout: Duration) -> Self {
        self.waiting.timeout = Some(timeout);
        self
    }

    /// Its output from its start, as it's written.
    pub fn on_stdout(mut self, callback: impl FnMut(&str) + Send + 'static) -> Self {
        self.waiting.on_stdout = Some(Box::new(callback));
        self
    }

    pub fn on_stderr(mut self, callback: impl FnMut(&str) + Send + 'static) -> Self {
        self.waiting.on_stderr = Some(Box::new(callback));
        self
    }
}

impl IntoFuture for WaitForCommand {
    type Output = Result<CommandResult>;
    type IntoFuture = Pin<Box<dyn Future<Output = Result<CommandResult>> + Send>>;

    fn into_future(self) -> Self::IntoFuture {
        Box::pin(async move { wait_for(&self.handle, self.waiting).await })
    }
}

/// The command's result once it exits; past its timeout, killed first.
async fn wait_for(handle: &CommandHandle, waiting: Waiting) -> Result<CommandResult> {
    let mut followed = FollowedOutput::new(handle, waiting.on_stdout, waiting.on_stderr);
    match waiting.timeout {
        None => followed.follow().await?,
        Some(limit) => {
            let within_limit = tokio::time::timeout(limit, followed.follow()).await;
            match within_limit {
                Ok(followed_to_its_end) => followed_to_its_end?,
                Err(_) => {
                    handle.kill().await?;
                    followed.follow().await?;
                }
            }
        }
    }
    followed.result().await
}

/// Kills the command if a foreground run is dropped before it exits.
struct KillOnDrop(Option<CommandHandle>);

impl Drop for KillOnDrop {
    fn drop(&mut self) {
        let (Some(handle), Ok(runtime)) = (self.0.take(), tokio::runtime::Handle::try_current())
        else {
            return;
        };
        runtime.spawn(async move {
            let _ = handle.kill().await;
        });
    }
}

/// A command's output, followed from its start to its end over server-sent
/// events; a stream that drops is followed again from where it got to.
struct FollowedOutput<'a> {
    handle: &'a CommandHandle,
    path: String,
    stdout: StreamOutput,
    stderr: StreamOutput,
    command: Option<SandboxCommand>,
}

impl<'a> FollowedOutput<'a> {
    /// Streams in a row that end before the command, at once and with
    /// nothing new: one that went on a while was cut, by the network or a
    /// proxy.
    const MOST_FRUITLESS_FOLLOWS: u32 = 3;
    const CUT_AFTER: Duration = Duration::from_secs(1);

    fn new(
        handle: &'a CommandHandle,
        on_stdout: Option<OutputCallback>,
        on_stderr: Option<OutputCallback>,
    ) -> Self {
        Self {
            handle,
            path: format!("{}/output", handle.path),
            stdout: StreamOutput::new(on_stdout),
            stderr: StreamOutput::new(on_stderr),
            command: None,
        }
    }

    /// Returns once the command ended.
    async fn follow(&mut self) -> Result<()> {
        let mut fruitless = 0;
        while !self
            .command
            .as_ref()
            .is_some_and(|command| !command.state.is_active())
        {
            let written = self.written();
            let started = Instant::now();
            let dropped = match self.follow_from_offsets().await {
                Ok(()) => None,
                Err(dropped @ Error::Network { .. }) => Some(dropped),
                Err(error) => return Err(error),
            };
            let cut = started.elapsed() > Self::CUT_AFTER;
            fruitless = if self.written() > written || cut {
                0
            } else {
                fruitless + 1
            };
            if fruitless > Self::MOST_FRUITLESS_FOLLOWS {
                let why = dropped
                    .map(|error| format!(": {error}"))
                    .unwrap_or_default();
                return Err(Error::Unexpected(format!(
                    "{}'s stream ended before its command{why}",
                    self.path
                )));
            }
        }
        Ok(())
    }

    async fn follow_from_offsets(&mut self) -> Result<()> {
        let query = [
            ("follow", "true".to_owned()),
            ("stdout_offset", self.stdout.bytes.len().to_string()),
            ("stderr_offset", self.stderr.bytes.len().to_string()),
        ];
        let client = &self.handle.calls.client;
        let mut events = client.events(&self.path, &query).await?;
        while let Some(event) = events.next::<CommandOutputEvent>().await? {
            match event {
                CommandOutputEvent::Output {
                    stream,
                    offset,
                    chunk,
                } => {
                    let output = match stream {
                        OutputStream::Stdout => &mut self.stdout,
                        OutputStream::Stderr => &mut self.stderr,
                    };
                    output.take(offset, chunk.bytes())?;
                }
                CommandOutputEvent::Command { command } => self.command = Some(command),
            }
        }
        Ok(())
    }

    fn written(&self) -> usize {
        self.stdout.bytes.len() + self.stderr.bytes.len()
    }

    async fn result(self) -> Result<CommandResult> {
        let Some(command) = self.command else {
            return Err(Error::Unexpected(format!("{} wasn't followed", self.path)));
        };
        match command.state {
            CommandState::Exited { code, signal } => Ok(CommandResult {
                number: command.number,
                stdout: self.stdout.finish(),
                stderr: self.stderr.finish(),
                exit_code: code,
                signal,
            }),
            CommandState::Refused { reason } => Err(Error::CommandRefused {
                number: command.number,
                reason,
            }),
            CommandState::Unfinished => {
                let ended = self.handle.calls.ended_error().await?;
                Err(ended.unwrap_or_else(|| {
                    Error::Unexpected(format!("command {} is unfinished", command.number))
                }))
            }
            CommandState::Starting | CommandState::Running => Err(Error::Unexpected(format!(
                "{} wasn't followed to its end",
                self.path
            ))),
        }
    }
}

/// One stream's bytes so far, and its text to a callback as it comes: a
/// character split across chunks waits for its end.
struct StreamOutput {
    bytes: Vec<u8>,
    callback: Option<OutputCallback>,
    undecoded: Vec<u8>,
}

impl StreamOutput {
    fn new(callback: Option<OutputCallback>) -> Self {
        Self {
            bytes: Vec::new(),
            callback,
            undecoded: Vec::new(),
        }
    }

    fn take(&mut self, offset: u64, chunk: &[u8]) -> Result<()> {
        let length = self.bytes.len() as u64;
        if offset > length {
            return Err(Error::Unexpected(format!(
                "output from byte {offset}, after only {length}"
            )));
        }
        let seen = usize::try_from(length - offset).unwrap_or(usize::MAX);
        let fresh = chunk.get(seen..).unwrap_or_default();
        self.bytes.extend_from_slice(fresh);
        if let Some(callback) = &mut self.callback {
            self.undecoded.extend_from_slice(fresh);
            let text = decode_whole_characters(&mut self.undecoded);
            if !text.is_empty() {
                callback(&text);
            }
        }
        Ok(())
    }

    fn finish(mut self) -> String {
        if let Some(callback) = &mut self.callback
            && !self.undecoded.is_empty()
        {
            callback(&String::from_utf8_lossy(&self.undecoded));
        }
        String::from_utf8_lossy(&self.bytes).into_owned()
    }
}

/// The text of `bytes`' whole characters, which leave it; a character not
/// yet whole stays. Bytes that are no character are U+FFFD, as in a lossy
/// decoding of the whole.
fn decode_whole_characters(bytes: &mut Vec<u8>) -> String {
    let mut text = String::new();
    loop {
        match std::str::from_utf8(bytes) {
            Ok(whole) => {
                text.push_str(whole);
                bytes.clear();
                return text;
            }
            Err(error) => {
                let valid = error.valid_up_to();
                text.push_str(std::str::from_utf8(&bytes[..valid]).expect("valid up to here"));
                match error.error_len() {
                    Some(invalid) => {
                        text.push(char::REPLACEMENT_CHARACTER);
                        bytes.drain(..valid + invalid);
                    }
                    None => {
                        bytes.drain(..valid);
                        return text;
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_character_split_across_chunks_waits_for_its_end() {
        let mut pending = "é".as_bytes()[..1].to_vec();
        assert_eq!(decode_whole_characters(&mut pending), "");
        pending.extend_from_slice(&"é!".as_bytes()[1..]);
        assert_eq!(decode_whole_characters(&mut pending), "é!");
        assert!(pending.is_empty());
        let mut invalid = vec![b'a', 0xff, b'b'];
        assert_eq!(decode_whole_characters(&mut invalid), "a\u{fffd}b");
    }

    #[test]
    fn output_seen_before_isnt_taken_again() {
        let mut output = StreamOutput::new(None);
        output.take(0, b"hel").unwrap();
        output.take(1, b"ello").unwrap();
        assert!(output.take(9, b"x").is_err());
        assert_eq!(output.finish(), "hello");
    }
}
