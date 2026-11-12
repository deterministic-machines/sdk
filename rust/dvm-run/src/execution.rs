use dvm_api::{ReplayVerdict, Run, RunId, RunOutput, RunState, SandboxSpec};
use reqwest::Method;

use crate::client::Client;
use crate::error::{Error, Result};

/// A sandbox's execution, from [`Client::execution`]: its spec, its state,
/// its replays, and its log's size once it ended.
#[derive(Clone, Debug)]
pub struct Execution {
    client: Client,
    recorded: dvm_api::Execution,
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

    pub fn replays(&self) -> Vec<Replay> {
        let replays = self.recorded.replays.iter().cloned();
        replays
            .map(|run| Replay::new(self.client.clone(), run))
            .collect()
    }

    /// Replays the exited program from its whole recording.
    pub async fn replay(&self) -> Result<Replay> {
        let path = format!("/executions/{}/replays", self.recorded.id);
        let run = self
            .client
            .json(Method::POST, &path, &[], None::<&()>)
            .await?;
        Ok(Replay::new(self.client.clone(), run))
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

    pub async fn stop(&mut self) -> Result<()> {
        let path = format!("{}/stop", self.path);
        self.run = self
            .client
            .json(Method::POST, &path, &[], None::<&()>)
            .await?;
        Ok(())
    }

    /// The program's stdout and stderr so far, as bytes.
    pub async fn output(&self) -> Result<RunOutput> {
        let path = format!("{}/output", self.path);
        self.client.json(Method::GET, &path, &[], None::<&()>).await
    }
}
