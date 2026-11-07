use serde::{Deserialize, Serialize};

use crate::{Run, Sandbox, SandboxCommand, SandboxId, SandboxSpec};

/// A sandbox's execution, `GET /api/executions/{id}`: what it ran, one log
/// from its start to its end, under its sandbox's id. Its spec, its seed
/// among it; its run, how and when it ended, and its log's size; each
/// command it was sent, as it ran; and its replays.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[cfg_attr(feature = "typescript", ts(export_to = "controller-api.ts"))]
pub struct Execution {
    pub id: SandboxId,
    /// Its owner's GitHub login; none for one made before users signed in,
    /// which only admins see.
    pub owner: Option<String>,
    pub spec: SandboxSpec,
    pub run: Run,
    /// Oldest first.
    pub commands: Vec<SandboxCommand>,
    /// Oldest first.
    pub replays: Vec<Run>,
}

impl Execution {
    /// The execution of `sandbox`, which ran `commands`.
    pub fn of(sandbox: Sandbox, commands: Vec<SandboxCommand>) -> Self {
        Self {
            id: sandbox.id,
            owner: sandbox.owner,
            spec: sandbox.spec,
            run: sandbox.run,
            commands,
            replays: sandbox.replays,
        }
    }
}
