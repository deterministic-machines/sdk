use std::future::{Future, IntoFuture};
use std::pin::Pin;

use dvm_api::{EnvironmentVariable, Run, RunOutput, RunState, SandboxDraft, SandboxId};
use reqwest::Method;

use crate::client::Client;
use crate::error::{Error, Result};

/// A sandbox, from [`Client::create_sandbox`] or [`Client::connect_sandbox`].
/// It runs until [`Sandbox::stop`]: dropping it doesn't stop it.
#[derive(Clone, Debug)]
pub struct Sandbox {
    calls: SandboxCalls,
    name: String,
}

/// What a sandbox is made of: [`Client::create_sandbox`], then what differs
/// from 1 vCPU, 1024 MiB, seed 0, the image's variables, and a name after
/// the image; awaited, the sandbox, once it's running.
#[must_use = "a sandbox is made once this is awaited"]
pub struct SandboxBuilder {
    client: Client,
    draft: SandboxDraft,
}

/// A sandbox's calls: its client,
/// its path, and what a refusal means once it ended.
#[derive(Clone, Debug)]
pub(crate) struct SandboxCalls {
    pub(crate) client: Client,
    pub(crate) id: SandboxId,
    pub(crate) path: String,
}

impl SandboxBuilder {
    pub(crate) fn new(client: Client, image: String) -> Self {
        let draft = SandboxDraft {
            image,
            ..SandboxDraft::default()
        };
        Self { client, draft }
    }

    pub fn command(mut self, command: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.draft.command = command.into_iter().map(Into::into).collect();
        self
    }

    pub fn entrypoint(mut self, entrypoint: impl Into<String>) -> Self {
        self.draft.entrypoint = Some(entrypoint.into());
        self
    }

    pub fn cpus(mut self, cpus: u32) -> Self {
        self.draft.cpus = cpus;
        self
    }

    pub fn memory_mib(mut self, memory_mib: u32) -> Self {
        self.draft.memory_mib = memory_mib;
        self
    }

    /// The seed of everything random in it, so a run is the same each time.
    pub fn seed(mut self, seed: u64) -> Self {
        self.draft.seed = seed;
        self
    }

    /// A variable for its processes, beside the image's.
    pub fn env(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        let (name, value) = (name.into(), value.into());
        self.draft
            .environment
            .push(EnvironmentVariable { name, value });
        self
    }

    pub fn name(mut self, name: impl Into<String>) -> Self {
        self.draft.name = name.into();
        self
    }

    async fn create(self) -> Result<Sandbox> {
        let created: dvm_api::Sandbox = self
            .client
            .json(Method::POST, "/sandboxes", &[], Some(&self.draft))
            .await?;
        let sandbox = Sandbox::new(self.client, created.clone());
        // Dropped while it starts, it's stopped: no one holds it.
        let mut stop_on_drop = StopOnDrop(Some(sandbox.calls.clone()));
        sandbox.calls.wait_until_running(created.run).await?;
        stop_on_drop.0 = None;
        Ok(sandbox)
    }
}

impl IntoFuture for SandboxBuilder {
    type Output = Result<Sandbox>;
    type IntoFuture = Pin<Box<dyn Future<Output = Result<Sandbox>> + Send>>;

    fn into_future(self) -> Self::IntoFuture {
        Box::pin(self.create())
    }
}

impl Sandbox {
    pub(crate) fn new(client: Client, sandbox: dvm_api::Sandbox) -> Self {
        Self {
            calls: SandboxCalls {
                path: format!("/sandboxes/{}", sandbox.id),
                client,
                id: sandbox.id,
            },
            name: sandbox.spec.name().to_owned(),
        }
    }

    /// Its execution's id too.
    pub fn id(&self) -> SandboxId {
        self.calls.id
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    /// Where it is: queued, running, paused, exited, stopped, failed.
    pub async fn state(&self) -> Result<RunState> {
        self.calls.state().await
    }

    pub async fn pause(&self) -> Result<()> {
        self.act("pause").await
    }

    pub async fn resume(&self) -> Result<()> {
        self.act("resume").await
    }

    /// Stops an active run. Its cut recording cannot replay.
    pub async fn stop(&self) -> Result<()> {
        let calls = &self.calls;
        let stop = format!("{}/stop", calls.path);
        match calls
            .client
            .json::<Run>(Method::POST, &stop, &[], None::<&()>)
            .await
        {
            Ok(_) => {}
            Err(refused @ Error::Conflict { .. }) => {
                let run = calls.sandbox().await?.run;
                if run.state.is_active() {
                    return Err(refused);
                }
            }
            Err(error) => return Err(error),
        };
        Ok(())
    }

    /// The program's exit code; stopped and failed runs return their error.
    pub async fn wait(&self) -> Result<i32> {
        let calls = &self.calls;
        let mut run = calls.sandbox().await?.run;
        loop {
            match run.state {
                RunState::Exited { code } => return Ok(code),
                state if !state.is_active() => return Err(Error::ended(self.id(), state)),
                _ => run = calls.newer_than(&run).await?,
            }
        }
    }

    /// The program's stdout and stderr so far, as bytes.
    pub async fn output(&self) -> Result<RunOutput> {
        let path = format!("/executions/{}/output", self.id());
        self.calls
            .client
            .json(Method::GET, &path, &[], None::<&()>)
            .await
    }

    /// Runs a scope, stopping this sandbox when it returns or is cancelled.
    pub async fn scope<T>(&self, operation: impl AsyncFnOnce(&Self) -> Result<T>) -> Result<T> {
        let mut stop_on_drop = StopOnDrop(Some(self.calls.clone()));
        let result = operation(self).await;
        let stopped = self.stop().await;
        stop_on_drop.0 = None;
        let value = result?;
        stopped?;
        Ok(value)
    }

    async fn act(&self, action: &str) -> Result<()> {
        let calls = &self.calls;
        let path = format!("{}/{action}", calls.path);
        calls
            .while_live(
                calls
                    .client
                    .json::<Run>(Method::POST, &path, &[], None::<&()>),
            )
            .await
            .map(drop)
    }
}

impl SandboxCalls {
    pub(crate) async fn state(&self) -> Result<RunState> {
        Ok(self.sandbox().await?.run.state)
    }

    /// What `call` answers; if it's refused because the sandbox ended, that
    /// it ended, and how.
    pub(crate) async fn while_live<T>(&self, call: impl Future<Output = Result<T>>) -> Result<T> {
        match call.await {
            Err(refused @ Error::Conflict { .. }) => {
                Err(self.ended_error().await?.unwrap_or(refused))
            }
            answer => answer,
        }
    }

    /// What a call to it returns if it ended; `None` if it hasn't.
    pub(crate) async fn ended_error(&self) -> Result<Option<Error>> {
        let state = self.state().await?;
        Ok((!state.is_active()).then(|| Error::ended(self.id, state)))
    }

    async fn sandbox(&self) -> Result<dvm_api::Sandbox> {
        self.client
            .json(Method::GET, &self.path, &[], None::<&()>)
            .await
    }

    async fn wait_until_running(&self, mut run: Run) -> Result<()> {
        loop {
            match run.state {
                RunState::Running | RunState::Paused | RunState::Exited { .. } => return Ok(()),
                state if !state.is_active() => return Err(Error::ended(self.id, state)),
                _ => run = self.newer_than(&run).await?,
            }
        }
    }

    async fn newer_than(&self, run: &Run) -> Result<Run> {
        let query = [("newer_than", run.version.to_string())];
        let sandbox: dvm_api::Sandbox = self
            .client
            .json(Method::GET, &self.path, &query, None::<&()>)
            .await?;
        Ok(sandbox.run)
    }
}

/// Stops the sandbox, if it's still held when dropped: a creation given up
/// on leaves nothing running.
struct StopOnDrop(Option<SandboxCalls>);

impl Drop for StopOnDrop {
    fn drop(&mut self) {
        let (Some(calls), Ok(runtime)) = (self.0.take(), tokio::runtime::Handle::try_current())
        else {
            return;
        };
        runtime.spawn(async move {
            let stop = format!("{}/stop", calls.path);
            let _ = calls
                .client
                .json::<Run>(Method::POST, &stop, &[], None::<&()>)
                .await;
        });
    }
}
