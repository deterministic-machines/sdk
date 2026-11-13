use std::future::{Future, IntoFuture};
use std::pin::Pin;
use std::sync::Arc;

use dvm_api::{
    EnvironmentVariable, Run, RunOutput, RunState, SandboxDraft, SandboxId, SandboxNetwork,
};
use reqwest::Method;

use crate::client::Client;
use crate::command::{CommandLine, RunBuilder, StartBuilder, WriteFileBuilder, no_commands};
use crate::error::{Error, Result};
use crate::sandbox_session::Session;

/// A sandbox, from [`Client::create_sandbox`] or [`Client::connect_sandbox`].
/// With neither command nor entrypoint it takes commands. Otherwise it is
/// one run of the supplied program. Dropping it does not end or stop it.
#[derive(Clone, Debug)]
pub struct Sandbox {
    calls: SandboxCalls,
    name: String,
    session: Option<Arc<Session>>,
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

    /// Where its connections out may go: [`SandboxNetwork::Public`] unless given.
    pub fn network(mut self, network: SandboxNetwork) -> Self {
        self.draft.network = network;
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
        let sandbox = Sandbox::new(self.client, created.clone(), Some(0));
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
    pub(crate) fn new(
        client: Client,
        sandbox: dvm_api::Sandbox,
        input_offset: Option<u64>,
    ) -> Self {
        let calls = SandboxCalls {
            path: format!("/sandboxes/{}", sandbox.id),
            client,
            id: sandbox.id,
        };
        let session = sandbox
            .spec
            .takes_commands()
            .then(|| Session::new(calls.clone(), input_offset));
        Self {
            calls,
            name: sandbox.spec.name().to_owned(),
            session,
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

    /// Where its connections out may go from now on; those open stay.
    pub async fn set_network(&self, network: &SandboxNetwork) -> Result<()> {
        let calls = &self.calls;
        let path = format!("{}/network", calls.path);
        calls
            .while_live(calls.client.json::<dvm_api::Sandbox>(
                Method::PUT,
                &path,
                &[],
                Some(network),
            ))
            .await
            .map(drop)
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
        self.calls.wait().await
    }

    /// Runs argv or a script, with stdin sent then closed, to its end.
    pub fn run(&self, command: impl Into<CommandLine>) -> RunBuilder {
        RunBuilder(self.start(command))
    }

    /// Returns once started. Omitted stdin stays open for the handle.
    pub fn start(&self, command: impl Into<CommandLine>) -> StartBuilder {
        StartBuilder {
            session: self.session.clone(),
            stdin: None,
            run: dvm_api::SandboxRun {
                program: command.into().0,
                env: Default::default(),
                cwd: None,
                stdin: true,
                user: None,
            },
        }
    }

    /// Writes in pieces. Parent directories must exist. A failed write can
    /// leave a partial file, with no protocol operation to roll it back.
    pub fn write_file(&self, path: impl Into<String>, data: impl AsRef<[u8]>) -> WriteFileBuilder {
        WriteFileBuilder {
            session: self.session.clone(),
            path: path.into(),
            data: data.as_ref().to_vec(),
            mode: None,
        }
    }

    pub async fn read_file(&self, path: impl Into<String>) -> Result<Vec<u8>> {
        self.session
            .as_ref()
            .ok_or_else(no_commands)?
            .read_file(path.into())
            .await
    }

    /// Ends a command sandbox's input and waits for exit. Resume it first
    /// if paused; EOF kills commands still running.
    pub async fn end(&self) -> Result<i32> {
        self.session.as_ref().ok_or_else(no_commands)?.end().await
    }

    /// Raw process output. A command sandbox's stdout is server answer lines.
    pub async fn output(&self) -> Result<RunOutput> {
        let path = format!("/executions/{}/output", self.id());
        self.calls
            .client
            .json(Method::GET, &path, &[], None::<&()>)
            .await
    }

    /// Ends input and waits at scope end, including cancellation. A paused
    /// sandbox or a supplied program is stopped instead.
    pub async fn scope<T>(&self, operation: impl AsyncFnOnce(&Self) -> Result<T>) -> Result<T> {
        let mut cleanup = FinishScopeOnDrop(Some(self.clone()));
        let result = operation(self).await;
        let stopped = self.finish_scope().await;
        cleanup.0 = None;
        let value = result?;
        stopped?;
        Ok(value)
    }

    async fn finish_scope(&self) -> Result<()> {
        let state = self.state().await?;
        if !state.is_active() {
            return Ok(());
        }
        if self.session.is_some() && state != RunState::Paused {
            self.end().await.map(drop)
        } else {
            self.stop().await
        }
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
    pub(crate) async fn wait(&self) -> Result<i32> {
        let mut run = self.sandbox().await?.run;
        loop {
            match run.state {
                RunState::Exited { code } => return Ok(code),
                state if !state.is_active() => return Err(Error::ended(self.id, state)),
                _ => run = self.newer_than(&run).await?,
            }
        }
    }

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

struct FinishScopeOnDrop(Option<Sandbox>);
impl Drop for FinishScopeOnDrop {
    fn drop(&mut self) {
        if let (Some(sandbox), Ok(runtime)) = (self.0.take(), tokio::runtime::Handle::try_current())
        {
            runtime.spawn(async move {
                let _ = sandbox.finish_scope().await;
            });
        }
    }
}
