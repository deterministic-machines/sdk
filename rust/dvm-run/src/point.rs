use std::collections::HashMap;
use std::future::{Future, IntoFuture};
use std::pin::Pin;

use dvm_api::{
    DvmStop, FileStatus, InspectionWatches, PathDirectoryEntry, SandboxId, SeekSandbox, SeekTarget,
    StepPoint, StopEvent, WatchPoint, WatchPoints,
};
use reqwest::Method;

use crate::client::Client;
use crate::error::{Error, Result};

#[derive(Clone, Debug)]
pub struct SandboxPoint {
    client: Client,
    sandbox: SandboxId,
    stop: DvmStop,
}

#[must_use = "a seek happens once awaited"]
pub struct SeekBuilder {
    client: Client,
    sandbox: SandboxId,
    target: SeekTarget,
    watches: Vec<WatchPoint>,
}

#[must_use = "a step happens once awaited"]
pub struct StepBuilder {
    point: SandboxPoint,
    watches: Vec<WatchPoint>,
}

#[must_use = "stat runs once awaited"]
pub struct StatBuilder {
    point: SandboxPoint,
    path: String,
    follow_symlinks: bool,
}

impl SeekBuilder {
    pub(crate) fn new(client: Client, sandbox: SandboxId, target: SeekTarget) -> Self {
        Self {
            client,
            sandbox,
            target,
            watches: Vec::new(),
        }
    }

    pub fn watch(mut self, watch: WatchPoint) -> Self {
        self.watches.push(watch);
        self
    }

    pub fn watches(mut self, watches: impl IntoIterator<Item = WatchPoint>) -> Self {
        self.watches.extend(watches);
        self
    }

    async fn run(self) -> Result<SandboxPoint> {
        let body = SeekSandbox {
            target: self.target,
            watches: encode_watches(self.watches)?,
        };
        let stop: DvmStop = self
            .client
            .json(
                Method::POST,
                &format!("/sandboxes/{}/points/seek", self.sandbox),
                &[],
                Some(&body),
            )
            .await?;
        Ok(SandboxPoint::new(self.client, self.sandbox, stop))
    }
}

impl IntoFuture for SeekBuilder {
    type Output = Result<SandboxPoint>;
    type IntoFuture = Pin<Box<dyn Future<Output = Result<SandboxPoint>> + Send>>;

    fn into_future(self) -> Self::IntoFuture {
        Box::pin(self.run())
    }
}

impl StepBuilder {
    pub fn watch(mut self, watch: WatchPoint) -> Self {
        self.watches.push(watch);
        self
    }

    pub fn watches(mut self, watches: impl IntoIterator<Item = WatchPoint>) -> Self {
        self.watches.extend(watches);
        self
    }

    async fn run(self) -> Result<Option<SandboxPoint>> {
        let body = StepPoint {
            thread: self.point.stop.thread,
            event: self.point.stop.event,
            watches: encode_watches(self.watches)?,
        };
        let stop: Option<DvmStop> = self
            .point
            .client
            .json(
                Method::POST,
                &format!("/sandboxes/{}/points/step", self.point.sandbox),
                &[],
                Some(&body),
            )
            .await?;
        Ok(stop.map(|stop| SandboxPoint::new(self.point.client, self.point.sandbox, stop)))
    }
}

impl IntoFuture for StepBuilder {
    type Output = Result<Option<SandboxPoint>>;
    type IntoFuture = Pin<Box<dyn Future<Output = Result<Option<SandboxPoint>>> + Send>>;

    fn into_future(self) -> Self::IntoFuture {
        Box::pin(self.run())
    }
}

impl StatBuilder {
    pub fn follow_symlinks(mut self, follow: bool) -> Self {
        self.follow_symlinks = follow;
        self
    }

    async fn run(self) -> Result<FileStatus> {
        let mut query = self.point.fs_query(self.path);
        query.push(("follow_symlinks", self.follow_symlinks.to_string()));
        self.point
            .client
            .json(
                Method::GET,
                &format!("/sandboxes/{}/fs/stat", self.point.sandbox),
                &query,
                None::<&()>,
            )
            .await
    }
}

impl IntoFuture for StatBuilder {
    type Output = Result<FileStatus>;
    type IntoFuture = Pin<Box<dyn Future<Output = Result<FileStatus>> + Send>>;

    fn into_future(self) -> Self::IntoFuture {
        Box::pin(self.run())
    }
}

impl SandboxPoint {
    fn new(client: Client, sandbox: SandboxId, stop: DvmStop) -> Self {
        Self {
            client,
            sandbox,
            stop,
        }
    }

    pub fn sandbox_id(&self) -> SandboxId {
        self.sandbox
    }

    pub fn ns(&self) -> Option<u64> {
        self.stop.recording_ns
    }

    pub fn thread(&self) -> u64 {
        self.stop.thread
    }

    pub fn process(&self) -> u64 {
        self.stop.process
    }

    pub fn operations(&self) -> u64 {
        self.stop.operations
    }

    pub fn event(&self) -> u64 {
        self.stop.event
    }

    pub fn at(&self) -> &StopEvent {
        &self.stop.at
    }

    pub fn stats(&self) -> &HashMap<String, u64> {
        &self.stop.stats
    }

    pub fn step(&self) -> StepBuilder {
        StepBuilder {
            point: self.clone(),
            watches: Vec::new(),
        }
    }

    pub fn stat(&self, path: impl Into<String>) -> StatBuilder {
        StatBuilder {
            point: self.clone(),
            path: path.into(),
            follow_symlinks: true,
        }
    }

    pub async fn list_dir(&self, path: impl AsRef<str>) -> Result<Vec<PathDirectoryEntry>> {
        let query = self.fs_query(path.as_ref().to_owned());
        self.client
            .json(
                Method::GET,
                &format!("/sandboxes/{}/fs/list", self.sandbox),
                &query,
                None::<&()>,
            )
            .await
    }

    pub async fn read_file(&self, path: impl AsRef<str>) -> Result<Vec<u8>> {
        let query = self.fs_query(path.as_ref().to_owned());
        self.client
            .get_bytes(&format!("/sandboxes/{}/fs/read", self.sandbox), &query)
            .await
    }

    fn fs_query(&self, path: String) -> Vec<(&'static str, String)> {
        vec![
            ("thread", self.stop.thread.to_string()),
            ("event", self.stop.event.to_string()),
            ("path", path),
        ]
    }
}

fn encode_watches(watches: Vec<WatchPoint>) -> Result<InspectionWatches> {
    if watches.is_empty() {
        Ok(InspectionWatches::default())
    } else {
        WatchPoints::try_from(watches)
            .map(InspectionWatches::from)
            .map_err(|reason| Error::InvalidRequest { reason })
    }
}
