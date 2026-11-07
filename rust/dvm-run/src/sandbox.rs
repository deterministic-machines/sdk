//! A sandbox: a VM made from an image, which takes commands and files until
//! it's stopped. Everything it ran is its execution, under its id.

use std::future::{Future, IntoFuture};
use std::pin::Pin;
use std::time::Duration;

use dvm_api::{EnvironmentVariable, Run, RunState, SandboxDraft, SandboxId};
use reqwest::Method;
use tokio::time::Instant;

use crate::client::Client;
use crate::commands::Commands;
use crate::error::{Error, Result};
use crate::files::Files;

/// A stopped sandbox's runner reports its log within this, or went away.
const LOG_WAIT: Duration = Duration::from_secs(60);

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

/// A sandbox's calls, which its commands and files make too: its client,
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

    pub fn commands(&self) -> Commands {
        Commands::new(self.calls.clone())
    }

    pub fn files(&self) -> Files {
        Files::new(self.calls.clone())
    }

    /// Where it is: queued, running, paused, stopped, failed.
    pub async fn state(&self) -> Result<RunState> {
        self.calls.state().await
    }

    pub async fn pause(&self) -> Result<()> {
        self.act("pause").await
    }

    pub async fn resume(&self) -> Result<()> {
        self.act("resume").await
    }

    /// Stops it, which ends its execution, and returns once its log is in,
    /// so that the execution replays. A sandbox that already ended stays as
    /// it is.
    pub async fn stop(&self) -> Result<()> {
        let calls = &self.calls;
        let stop = format!("{}/stop", calls.path);
        let run = match calls
            .client
            .json(Method::POST, &stop, &[], None::<&()>)
            .await
        {
            Ok(run) => run,
            Err(refused @ Error::Conflict { .. }) => {
                let run = calls.sandbox().await?.run;
                if run.state.is_active() {
                    return Err(refused);
                }
                run
            }
            Err(error) => return Err(error),
        };
        calls.wait_for_log(run).await
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
                RunState::Running => return Ok(()),
                state if !state.is_active() => return Err(Error::ended(self.id, state)),
                _ => run = self.newer_than(&run).await?,
            }
        }
    }

    /// A stopped sandbox's runner ends dvm, then reports its log's size as
    /// the run's next version; one that never reached a runner has none.
    async fn wait_for_log(&self, mut run: Run) -> Result<()> {
        let deadline = Instant::now() + LOG_WAIT;
        while run.state == RunState::Stopped
            && run.runner.is_some()
            && run.log_bytes.is_none()
            && Instant::now() < deadline
        {
            run = self.newer_than(&run).await?;
        }
        Ok(())
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
