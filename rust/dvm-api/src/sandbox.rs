use std::collections::HashSet;
use std::fmt;
use std::ops::RangeInclusive;

use serde::{Deserialize, Serialize};

use crate::{Capacity, ImageReference, Run, RunId, RunState, SandboxNetwork};

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

    pub fn run(self) -> RunId {
        RunId::new(self.0)
    }
}

impl fmt::Display for SandboxId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[cfg_attr(feature = "typescript", ts(export_to = "controller-api.ts"))]
pub struct Sandbox {
    pub id: SandboxId,
    pub owner: Option<String>,
    pub spec: SandboxSpec,
    pub run: Run,
}

impl Sandbox {
    pub fn check_inspectable(&self) -> Result<(), String> {
        if self.run.inspectable {
            return Ok(());
        }
        if self.run.state.is_active() {
            return Err(format!("sandbox {} is still running", self.id));
        }
        if matches!(self.run.state, RunState::Stopped) {
            return Err(format!("sandbox {} was stopped before exit", self.id));
        }
        if matches!(self.run.state, RunState::Failed { .. }) {
            return Err(format!("sandbox {} failed before exit", self.id));
        }
        if self.run.outdated {
            return Err(format!(
                "sandbox {} ran on an older dvm than its runner's now: run it again",
                self.id
            ));
        }
        Err(format!("sandbox {} has no inspectable state", self.id))
    }

    pub fn take(&mut self, changed: &Run) -> bool {
        if changed.id != self.run.id {
            return false;
        }
        if changed.supersedes(&self.run) {
            self.run = changed.clone();
        }
        true
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[cfg_attr(feature = "typescript", ts(export_to = "controller-api.ts"))]
pub struct SandboxesPage {
    pub sandboxes: Vec<Sandbox>,
    pub older: bool,
}

impl SandboxesPage {
    pub const SANDBOXES: usize = 50;
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[cfg_attr(feature = "typescript", ts(export_to = "controller-api.ts"))]
pub struct EnvironmentVariable {
    pub name: String,
    pub value: String,
}

impl EnvironmentVariable {
    pub fn is_name(name: &str) -> bool {
        let mut chars = name.chars();
        chars
            .next()
            .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
            && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[cfg_attr(feature = "typescript", ts(export_to = "controller-api.ts"))]
#[serde(default)]
pub struct SandboxDraft {
    pub name: String,
    pub image: String,
    pub entrypoint: Option<String>,
    pub command: Vec<String>,
    pub environment: Vec<EnvironmentVariable>,
    pub cpus: u32,
    pub memory_mib: u32,
    pub seed: u64,
    pub network: SandboxNetwork,
}

impl Default for SandboxDraft {
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
            network: SandboxNetwork::default(),
        }
    }
}

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
    network: SandboxNetwork,
}

impl SandboxSpec {
    pub const CPUS: RangeInclusive<u32> = 1..=8;
    pub const MEMORY_MIB: RangeInclusive<u32> = 128..=16384;
    /// Bounded below 2^53 so JSON numbers pass unmodified across JavaScript clients.
    pub const SEEDS: RangeInclusive<u64> = 0..=(1 << 53) - 1;
    const NAME_LENGTH: usize = 63;
    /// Extra host memory reserved for dvm itself per VM.
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

    pub fn network(&self) -> &SandboxNetwork {
        &self.network
    }

    pub fn with_network(self, network: SandboxNetwork) -> Result<Self, SpecError> {
        network
            .check()
            .map_err(|problem| SpecError::new(SpecField::Network, problem))?;
        Ok(Self { network, ..self })
    }

    pub fn footprint(&self) -> Capacity {
        Capacity {
            cpus: self.cpus,
            memory_mib: u64::from(self.memory_mib) + Self::DVM_OWN_MEMORY_MIB,
        }
    }

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
        draft
            .network
            .check()
            .map_err(|problem| SpecError::new(SpecField::Network, problem))?;
        Ok(Self {
            name,
            image,
            entrypoint: draft.entrypoint,
            command: draft.command,
            environment: draft.environment,
            cpus: draft.cpus,
            memory_mib: draft.memory_mib,
            seed: draft.seed,
            network: draft.network,
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
            network: spec.network,
        }
    }
}

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
    Network,
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
            Self::Network => "network",
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
        let network = |d: &mut SandboxDraft| {
            d.network = SandboxNetwork::Allowlist {
                allow: vec!["::1".into()],
            }
        };
        assert_eq!(refused_field(network), SpecField::Network);
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

    #[test]
    fn a_sandbox_keeps_the_newer_copy_of_its_run() {
        use crate::Timestamp;
        let run = |id: i64, state: RunState, version: u64| Run {
            id: RunId::new(id),
            state,
            runner: None,
            created_at: Timestamp::from_unix_millis(0),
            started_at: None,
            ended_at: None,
            has_timeline: false,
            inspectable: false,
            outdated: false,
            version,
        };
        let mut sandbox = Sandbox {
            id: SandboxId::new(1),
            owner: None,
            spec: SandboxSpec::try_from(draft()).unwrap(),
            run: run(1, RunState::Queued, 1),
        };
        assert!(sandbox.take(&run(1, RunState::Running, 2)));
        assert_eq!(sandbox.run.state, RunState::Running);
        assert!(sandbox.take(&run(1, RunState::Queued, 0)));
        assert_eq!(sandbox.run.state, RunState::Running);
        assert!(!sandbox.take(&run(2, RunState::Running, 3)));
    }

    #[test]
    fn inspectability_reports_why_not() {
        use crate::Timestamp;
        let sandbox = |state: RunState, inspectable: bool, outdated: bool| Sandbox {
            id: SandboxId::new(1),
            owner: None,
            spec: SandboxSpec::try_from(draft()).unwrap(),
            run: Run {
                id: RunId::new(1),
                state,
                runner: Some("test".to_owned()),
                created_at: Timestamp::from_unix_millis(0),
                started_at: None,
                ended_at: None,
                has_timeline: true,
                inspectable,
                outdated,
                version: 0,
            },
        };
        assert!(
            sandbox(RunState::Stopped, false, false)
                .check_inspectable()
                .unwrap_err()
                .contains("stopped")
        );
        assert!(
            sandbox(RunState::Exited { code: 0 }, false, true)
                .check_inspectable()
                .unwrap_err()
                .contains("older dvm")
        );
        assert_eq!(
            sandbox(RunState::Exited { code: 3 }, true, false).check_inspectable(),
            Ok(())
        );
    }
}
