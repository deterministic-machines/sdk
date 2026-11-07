//! An execution: everything a sandbox ran, one log from its start to its
//! end, under the sandbox's id; and its replays, each judged the same
//! execution or not.

use std::future::{Future, IntoFuture};
use std::pin::Pin;

use dvm_api::{
    Moment, NewReplay, ReplayStop, ReplayVerdict, Run, RunId, RunState, SandboxCommand,
    SandboxSpec, Seek, WatchKind, WatchedEvents,
};
use reqwest::Method;

use crate::client::Client;
use crate::error::{Error, Result};

/// A sandbox's execution, from [`Client::execution`]: its spec, its state,
/// each command it was sent, as it ran, and its log's size once it ended.
#[derive(Clone, Debug)]
pub struct Execution {
    client: Client,
    recorded: dvm_api::Execution,
}

/// A replay to start: [`Execution::replay`]; awaited, the replay.
#[must_use = "a replay starts once this is awaited"]
pub struct ReplayBuilder {
    client: Client,
    path: String,
    new: NewReplay,
}

/// A replay of an execution.
#[derive(Clone, Debug)]
pub struct Replay {
    client: Client,
    path: String,
    run: Run,
}

impl Execution {
    pub(crate) fn new(client: Client, recorded: dvm_api::Execution) -> Self {
        Self { client, recorded }
    }

    pub fn spec(&self) -> &SandboxSpec {
        &self.recorded.spec
    }

    pub fn state(&self) -> &RunState {
        &self.recorded.run.state
    }

    /// Its log's size, once it ended whole.
    pub fn log_bytes(&self) -> Option<u64> {
        self.recorded.run.log_bytes
    }

    /// Each command it was sent, as it ran, oldest first.
    pub fn commands(&self) -> &[SandboxCommand] {
        &self.recorded.commands
    }

    pub fn replays(&self) -> Vec<Replay> {
        let replays = self.recorded.replays.iter().cloned();
        replays
            .map(|run| Replay::new(self.client.clone(), run))
            .collect()
    }

    /// Replays it, once its sandbox ended: played to its end, or held at
    /// its start for [`Replay::seek`].
    pub fn replay(&self) -> ReplayBuilder {
        ReplayBuilder {
            client: self.client.clone(),
            path: format!("/executions/{}/replays", self.recorded.id),
            new: NewReplay::default(),
        }
    }

    /// The machine at `at`: each process, its threads, files and memory.
    pub async fn machine(&self, at: Moment) -> Result<ReplayStop> {
        let path = format!("/executions/{}/machine", self.recorded.id);
        let query = [("at", at.to_string())];
        self.client
            .json(Method::GET, &path, &query, None::<&()>)
            .await
    }
}

impl ReplayBuilder {
    /// Held at its start, for [`Replay::seek`], until stopped.
    pub fn held(mut self) -> Self {
        self.new.held = true;
        self
    }

    /// Watches `kind`'s events as it plays: [`Replay::events`].
    pub fn watch(mut self, kind: WatchKind) -> Self {
        self.new.watch = self.new.watch.with(kind);
        self
    }
}

impl IntoFuture for ReplayBuilder {
    type Output = Result<Replay>;
    type IntoFuture = Pin<Box<dyn Future<Output = Result<Replay>> + Send>>;

    fn into_future(self) -> Self::IntoFuture {
        Box::pin(async move {
            let run = self
                .client
                .json(Method::POST, &self.path, &[], Some(&self.new))
                .await?;
            Ok(Replay::new(self.client, run))
        })
    }
}

impl Replay {
    fn new(client: Client, run: Run) -> Self {
        let path = format!("/replays/{}", run.id);
        Self { client, path, run }
    }

    pub fn id(&self) -> RunId {
        self.run.id
    }

    /// Its state when last read.
    pub fn state(&self) -> &RunState {
        &self.run.state
    }

    /// Its verdict, once it ended.
    pub fn verdict(&self) -> Option<&ReplayVerdict> {
        self.run.verdict.as_ref()
    }

    /// Its verdict, once it ended: the same execution, or where it parted.
    /// A replay dvm failed, as it saw it part, is an error that says where.
    pub async fn wait(&mut self) -> Result<ReplayVerdict> {
        while self.run.state.is_active() {
            let query = [("newer_than", self.run.version.to_string())];
            self.run = self
                .client
                .json(Method::GET, &self.path, &query, None::<&()>)
                .await?;
        }
        self.run.verdict.clone().ok_or_else(|| Error::ReplayEnded {
            replay: self.run.id,
            state: self.run.state.clone(),
        })
    }

    /// Takes a held replay to `to`, and holds it there: the moment, each
    /// thread's next operation, and the machine there.
    pub async fn seek(&self, to: Moment) -> Result<ReplayStop> {
        let path = format!("{}/seek", self.path);
        self.client
            .json(Method::POST, &path, &[], Some(&Seek { to }))
            .await
    }

    pub async fn stop(&mut self) -> Result<()> {
        let path = format!("{}/stop", self.path);
        self.run = self
            .client
            .json(Method::POST, &path, &[], None::<&()>)
            .await?;
        Ok(())
    }

    /// The commands it ran again, oldest first.
    pub async fn commands(&self) -> Result<Vec<SandboxCommand>> {
        let path = format!("{}/commands", self.path);
        self.client.json(Method::GET, &path, &[], None::<&()>).await
    }

    /// What it watched, and the events it saw, once it ended.
    pub async fn events(&self) -> Result<WatchedEvents> {
        let path = format!("{}/events", self.path);
        self.client.json(Method::GET, &path, &[], None::<&()>).await
    }
}
