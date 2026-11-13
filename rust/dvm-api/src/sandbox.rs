use std::collections::HashSet;
use std::fmt;
use std::ops::RangeInclusive;

use serde::{Deserialize, Serialize};

use crate::{Capacity, ImageReference, Run, RunId, RunState};

/// A sandbox's id: its run's, as its runner and its output know it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[cfg_attr(feature = "typescript", ts(export_to = "controller-api.ts"))]
#[serde(transparent)]
pub struct SandboxId(i64);

impl SandboxId {
    pub fn new(id: i64) -> Self {
        Self(id)
    }

    pub fn get(self) -> i64 {
        self.0
    }

    /// Its run's id.
    pub fn run(self) -> RunId {
        RunId::new(self.0)
    }
}

impl fmt::Display for SandboxId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

/// A sandbox: one run of its spec, recorded, started as it's made, and
/// the replays of its recording. Its id is its run's.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[cfg_attr(feature = "typescript", ts(export_to = "controller-api.ts"))]
pub struct Sandbox {
    pub id: SandboxId,
    /// Its owner's GitHub login; none for one made before users signed in,
    /// which only admins see.
    pub owner: Option<String>,
    pub spec: SandboxSpec,
    /// Its run: queued as it's made, then started, paused, ended.
    pub run: Run,
    /// Its replays, oldest first.
    pub replays: Vec<Run>,
}

impl Sandbox {
    /// Whether `id` is its run or one of its replays.
    pub fn holds(&self, id: RunId) -> bool {
        self.member(id).is_some()
    }

    /// Its run, or its replay, of that id.
    pub fn member(&self, id: RunId) -> Option<&Run> {
        std::iter::once(&self.run)
            .chain(&self.replays)
            .find(|member| member.id == id)
    }

    /// Whether its exited run has a whole log its runner can replay.
    pub fn replayable(&self) -> Result<(), String> {
        self.run
            .replayable_if_whole(matches!(self.run.state, RunState::Exited { .. }))
    }

    /// Whether a run of it is going, its own or a replay.
    pub fn is_going(&self) -> bool {
        std::iter::once(&self.run)
            .chain(&self.replays)
            .any(|member| member.state.is_active())
    }

    /// Takes its run or a replay as it is now, if it's of this sandbox and
    /// no older than the copy it has: whether it's of this sandbox.
    pub fn take(&mut self, changed: &Run) -> bool {
        if changed.sandbox() != self.id {
            return false;
        }
        if changed.id == self.run.id {
            if changed.supersedes(&self.run) {
                self.run = changed.clone();
            }
            return true;
        }
        match self
            .replays
            .iter_mut()
            .find(|replay| replay.id == changed.id)
        {
            Some(replay) if changed.supersedes(replay) => *replay = changed.clone(),
            Some(_) => {}
            None => {
                self.replays.push(changed.clone());
                self.replays.sort_by_key(|replay| replay.id.get());
            }
        }
        true
    }
}

/// A page of the sandboxes a user sees, `GET /api/sandboxes?before=`: the
/// newest before the sandbox `before`, if given, newest first.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[cfg_attr(feature = "typescript", ts(export_to = "controller-api.ts"))]
pub struct SandboxesPage {
    pub sandboxes: Vec<Sandbox>,
    /// Whether there are older ones than these.
    pub older: bool,
}

impl SandboxesPage {
    /// The most sandboxes a page holds.
    pub const SANDBOXES: usize = 50;
}

/// An environment variable the program is given.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[cfg_attr(feature = "typescript", ts(export_to = "controller-api.ts"))]
pub struct EnvironmentVariable {
    pub name: String,
    pub value: String,
}

impl EnvironmentVariable {
    /// Whether `name` is one a shell takes: `[A-Za-z_][A-Za-z0-9_]*`.
    pub fn is_name(name: &str) -> bool {
        let mut chars = name.chars();
        chars
            .next()
            .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
            && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
    }
}

/// A sandbox's spec as a person writes it, unchecked: what the panel's form
/// holds, and what the controller takes to create a sandbox. Each field but
/// the image may be left out, for `dvm run`'s defaults.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[cfg_attr(feature = "typescript", ts(export_to = "controller-api.ts"))]
#[serde(default)]
pub struct SandboxDraft {
    /// The image's name, unless given (`ImageReference::sandbox_name`).
    pub name: String,
    pub image: String,
    /// In place of the image's entrypoint.
    pub entrypoint: Option<String>,
    /// The program to run. With neither command nor entrypoint, takes commands.
    pub command: Vec<String>,
    /// After the image's own.
    pub environment: Vec<EnvironmentVariable>,
    pub cpus: u32,
    pub memory_mib: u32,
    pub seed: u64,
}

impl Default for SandboxDraft {
    /// What `dvm run` gives unless told: one vCPU, 1 GiB, seed 0.
    fn default() -> Self {
        Self {
            name: String::new(),
            image: String::new(),
            entrypoint: None,
            command: Vec::new(),
            environment: Vec::new(),
            cpus: 1,
            memory_mib: 1024,
            seed: 0,
        }
    }
}

/// What a sandbox runs, checked: each value is one `dvm run` may be
/// given as it is. Only a [`SandboxDraft`] that passes becomes one, deserialized
/// too.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[cfg_attr(feature = "typescript", ts(export_to = "controller-api.ts"))]
#[serde(try_from = "SandboxDraft", into = "SandboxDraft")]
#[cfg_attr(feature = "typescript", ts(as = "SandboxDraft"))]
pub struct SandboxSpec {
    name: String,
    image: ImageReference,
    entrypoint: Option<String>,
    command: Vec<String>,
    environment: Vec<EnvironmentVariable>,
    cpus: u32,
    memory_mib: u32,
    seed: u64,
}

impl SandboxSpec {
    /// The vCPUs a sandbox may have.
    pub const CPUS: RangeInclusive<u32> = 1..=8;
    /// The memory a sandbox may have, in MiB.
    pub const MEMORY_MIB: RangeInclusive<u32> = 128..=16384;
    /// The seeds a sandbox may have: those a JSON number carries exactly to
    /// every client, a browser's too (2^53 − 1 at most), so a spec read
    /// back runs as it was made.
    pub const SEEDS: RangeInclusive<u64> = 0..=(1 << 53) - 1;
    /// A name is a DNS label, so that it may name a host later.
    const NAME_LENGTH: usize = 63;
    /// dvm's memory over the VM's, which it backs only as the program
    /// touches it: 14 MiB measured (`alpine:3.20 echo hi`, at 256 MiB to
    /// 4 GiB of `--memory`), counted with room to spare.
    const DVM_OWN_MEMORY_MIB: u64 = 32;

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn image(&self) -> &ImageReference {
        &self.image
    }

    pub fn entrypoint(&self) -> Option<&str> {
        self.entrypoint.as_deref()
    }

    pub fn command(&self) -> &[String] {
        &self.command
    }

    /// Whether the sandbox runs the sandbox server and accepts input bytes.
    pub fn takes_commands(&self) -> bool {
        self.command.is_empty() && self.entrypoint.is_none()
    }

    pub fn environment(&self) -> &[EnvironmentVariable] {
        &self.environment
    }

    pub fn cpus(&self) -> u32 {
        self.cpus
    }

    pub fn memory_mib(&self) -> u32 {
        self.memory_mib
    }

    pub fn seed(&self) -> u64 {
        self.seed
    }

    /// What a run of the spec takes from its runner: a host CPU for each
    /// vCPU, and the VM's memory with dvm's own.
    pub fn footprint(&self) -> Capacity {
        Capacity {
            cpus: self.cpus,
            memory_mib: u64::from(self.memory_mib) + Self::DVM_OWN_MEMORY_MIB,
        }
    }

    /// Lowercase letters, digits and inner `-`, as a DNS label.
    fn check_name(name: &str) -> Result<(), SpecError> {
        let valid = !name.is_empty()
            && name.len() <= Self::NAME_LENGTH
            && !name.starts_with('-')
            && !name.ends_with('-')
            && name
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-');
        if valid {
            Ok(())
        } else {
            Err(SpecError::new(
                SpecField::Name,
                "up to 63 lowercase letters, digits and '-', not starting or ending with '-'",
            ))
        }
    }

    /// Names a shell would take (`[A-Za-z_][A-Za-z0-9_]*`), each once.
    fn check_environment(environment: &[EnvironmentVariable]) -> Result<(), SpecError> {
        let mut names = HashSet::new();
        for variable in environment {
            if !EnvironmentVariable::is_name(&variable.name) {
                return Err(SpecError::new(
                    SpecField::Environment,
                    format!(
                        "{:?} isn't a variable name: letters, digits and '_', not starting with a digit",
                        variable.name
                    ),
                ));
            }
            if !names.insert(variable.name.as_str()) {
                return Err(SpecError::new(
                    SpecField::Environment,
                    format!("{} is given twice", variable.name),
                ));
            }
            Self::check_argument(SpecField::Environment, &variable.value)?;
        }
        Ok(())
    }

    /// A program's argument can't hold a NUL.
    fn check_argument(field: SpecField, argument: &str) -> Result<(), SpecError> {
        if argument.contains('\0') {
            Err(SpecError::new(field, "can't contain a NUL character"))
        } else {
            Ok(())
        }
    }

    fn check_range<T: PartialOrd + fmt::Display>(
        field: SpecField,
        value: T,
        range: RangeInclusive<T>,
    ) -> Result<(), SpecError> {
        if range.contains(&value) {
            Ok(())
        } else {
            Err(SpecError::new(
                field,
                format!("{value} is outside {}..={}", range.start(), range.end()),
            ))
        }
    }
}

impl TryFrom<SandboxDraft> for SandboxSpec {
    type Error = SpecError;

    fn try_from(draft: SandboxDraft) -> Result<Self, Self::Error> {
        if !draft.name.is_empty() {
            Self::check_name(&draft.name)?;
        }
        let image = ImageReference::try_from(draft.image)
            .map_err(|error| SpecError::new(SpecField::Image, error.to_string()))?;
        let name = match draft.name {
            name if name.is_empty() => image.sandbox_name(),
            name => name,
        };
        if let Some(entrypoint) = &draft.entrypoint {
            if entrypoint.is_empty() {
                return Err(SpecError::new(SpecField::Entrypoint, "is empty"));
            }
            Self::check_argument(SpecField::Entrypoint, entrypoint)?;
        }
        for argument in &draft.command {
            Self::check_argument(SpecField::Command, argument)?;
        }
        Self::check_environment(&draft.environment)?;
        Self::check_range(SpecField::Cpus, draft.cpus, Self::CPUS)?;
        Self::check_range(SpecField::Memory, draft.memory_mib, Self::MEMORY_MIB)?;
        Self::check_range(SpecField::Seed, draft.seed, Self::SEEDS)?;
        Ok(Self {
            name,
            image,
            entrypoint: draft.entrypoint,
            command: draft.command,
            environment: draft.environment,
            cpus: draft.cpus,
            memory_mib: draft.memory_mib,
            seed: draft.seed,
        })
    }
}

impl From<SandboxSpec> for SandboxDraft {
    fn from(spec: SandboxSpec) -> Self {
        Self {
            name: spec.name,
            image: spec.image.into(),
            entrypoint: spec.entrypoint,
            command: spec.command,
            environment: spec.environment,
            cpus: spec.cpus,
            memory_mib: spec.memory_mib,
            seed: spec.seed,
        }
    }
}

/// Why a draft isn't a spec: the first field that's wrong, and how.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("{field}: {problem}")]
pub struct SpecError {
    field: SpecField,
    problem: String,
}

impl SpecError {
    fn new(field: SpecField, problem: impl Into<String>) -> Self {
        Self {
            field,
            problem: problem.into(),
        }
    }

    pub fn field(&self) -> SpecField {
        self.field
    }

    pub fn problem(&self) -> &str {
        &self.problem
    }
}

/// A field of a [`SandboxDraft`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SpecField {
    Name,
    Image,
    Entrypoint,
    Command,
    Environment,
    Cpus,
    Memory,
    Seed,
}

impl fmt::Display for SpecField {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Name => "name",
            Self::Image => "image",
            Self::Entrypoint => "entrypoint",
            Self::Command => "command",
            Self::Environment => "environment",
            Self::Cpus => "cpus",
            Self::Memory => "memory",
            Self::Seed => "seed",
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_spec_without_a_program_takes_commands() {
        for (command, entrypoint, takes_commands) in [
            (vec![], None, true),
            (vec!["echo".to_owned()], None, false),
            (vec![], Some("/bin/sh".to_owned()), false),
            (vec!["hi".to_owned()], Some("/bin/echo".to_owned()), false),
        ] {
            let spec = SandboxSpec::try_from(SandboxDraft {
                command,
                entrypoint,
                ..draft()
            })
            .unwrap();
            assert_eq!(spec.takes_commands(), takes_commands);
        }
    }

    fn draft() -> SandboxDraft {
        SandboxDraft {
            name: "hello".to_owned(),
            image: "alpine:3.20".to_owned(),
            command: vec!["echo".to_owned(), "hi".to_owned()],
            environment: vec![EnvironmentVariable {
                name: "RUST_LOG".to_owned(),
                value: "debug".to_owned(),
            }],
            ..SandboxDraft::default()
        }
    }

    fn refused_field(change: impl FnOnce(&mut SandboxDraft)) -> SpecField {
        let mut draft = draft();
        change(&mut draft);
        SandboxSpec::try_from(draft).unwrap_err().field()
    }

    #[test]
    fn a_draft_becomes_a_spec_and_back() {
        let spec = SandboxSpec::try_from(draft()).unwrap();
        assert_eq!(spec.image().as_str(), "alpine:3.20");
        assert_eq!(SandboxDraft::from(spec), draft());
    }

    #[test]
    fn a_draft_needs_only_its_image() {
        let draft: SandboxDraft =
            serde_json::from_str(r#"{"image":"ghcr.io/owner/my_app:1"}"#).unwrap();
        let spec = SandboxSpec::try_from(draft).unwrap();
        assert_eq!(
            SandboxDraft::from(spec.clone()),
            SandboxDraft {
                name: "my-app".to_owned(),
                image: "ghcr.io/owner/my_app:1".to_owned(),
                ..SandboxDraft::default()
            }
        );
        assert!(spec.command().is_empty() && spec.entrypoint().is_none());
    }

    #[test]
    fn each_field_is_checked() {
        assert_eq!(refused_field(|d| d.name = "Hello".into()), SpecField::Name);
        assert_eq!(refused_field(|d| d.name = "-a".into()), SpecField::Name);
        assert_eq!(refused_field(|d| d.name = "a".repeat(64)), SpecField::Name);
        assert_eq!(
            refused_field(|d| d.image = "--volume=/:/h".into()),
            SpecField::Image
        );
        assert_eq!(
            refused_field(|d| d.entrypoint = Some(String::new())),
            SpecField::Entrypoint
        );
        assert_eq!(
            refused_field(|d| d.command.push("a\0b".into())),
            SpecField::Command
        );
        assert_eq!(
            refused_field(|d| d.environment[0].name = "1A".into()),
            SpecField::Environment
        );
        assert_eq!(
            refused_field(|d| d.environment.push(d.environment[0].clone())),
            SpecField::Environment
        );
        assert_eq!(refused_field(|d| d.cpus = 0), SpecField::Cpus);
        assert_eq!(refused_field(|d| d.memory_mib = 64), SpecField::Memory);
        assert_eq!(refused_field(|d| d.seed = 1 << 53), SpecField::Seed);
    }

    #[test]
    fn deserializing_checks() {
        let json = serde_json::to_string(&draft()).unwrap();
        let spec: SandboxSpec = serde_json::from_str(&json).unwrap();
        assert_eq!(spec, SandboxSpec::try_from(draft()).unwrap());

        let invalid = serde_json::to_string(&SandboxDraft {
            cpus: 64,
            ..draft()
        })
        .unwrap();
        assert!(serde_json::from_str::<SandboxSpec>(&invalid).is_err());
    }

    /// A sandbox keeps the newer copy of its run and each replay, and
    /// takes a new replay of its own, but no other sandbox's run.
    #[test]
    fn a_sandbox_keeps_the_newer_copy_of_each_of_its_runs() {
        use crate::Timestamp;
        let run = |id: i64, replay_of: Option<i64>, state: RunState, version: u64| Run {
            id: RunId::new(id),
            replay_of: replay_of.map(RunId::new),
            state,
            runner: None,
            created_at: Timestamp::from_unix_millis(0),
            started_at: None,
            ended_at: None,
            log_bytes: None,
            log_outdated: false,
            verdict: None,
            version,
        };
        let exited = RunState::Exited { code: 0 };
        let mut sandbox = Sandbox {
            id: SandboxId::new(1),
            owner: None,
            spec: SandboxSpec::try_from(draft()).unwrap(),
            run: run(1, None, exited.clone(), 3),
            replays: vec![],
        };
        let running = run(2, Some(1), RunState::Running, 2);
        assert!(sandbox.take(&run(2, Some(1), RunState::Queued, 0)));
        assert!(sandbox.take(&running));
        // The queued copy, answered late, is older than the running one.
        assert!(sandbox.take(&run(2, Some(1), RunState::Queued, 1)));
        assert_eq!(sandbox.replays, [running]);
        assert!(sandbox.is_going());
        assert!(!sandbox.take(&run(3, None, RunState::Queued, 0)));
        assert!(sandbox.holds(RunId::new(2)) && !sandbox.holds(RunId::new(3)));
        assert_eq!(run(2, Some(1), exited, 0).sandbox(), sandbox.id);
    }

    /// A stopped run's recording is cut short: only an exited one replays.
    #[test]
    fn only_an_exited_run_replays() {
        use crate::Timestamp;
        let sandbox = |state: RunState| Sandbox {
            id: SandboxId::new(1),
            owner: None,
            spec: SandboxSpec::try_from(draft()).unwrap(),
            run: Run {
                id: RunId::new(1),
                replay_of: None,
                state,
                runner: Some("test".to_owned()),
                created_at: Timestamp::from_unix_millis(0),
                started_at: None,
                ended_at: None,
                log_bytes: Some(1),
                log_outdated: false,
                verdict: None,
                version: 0,
            },
            replays: vec![],
        };
        let stopped = sandbox(RunState::Stopped).replayable().unwrap_err();
        assert!(stopped.contains("cut short"), "{stopped}");
        assert_eq!(sandbox(RunState::Exited { code: 3 }).replayable(), Ok(()));
    }
}
