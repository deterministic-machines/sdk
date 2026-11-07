use serde::{Deserialize, Serialize};

/// The user signed in: `GET /api/me`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[cfg_attr(feature = "typescript", ts(export_to = "controller-api.ts"))]
pub struct Me {
    /// Their GitHub login.
    pub login: String,
    /// Whether they see everyone's sandboxes, and the runners.
    pub admin: bool,
    /// What they may use; `None` for an admin, who has no limits.
    pub limits: Option<UserLimits>,
}

/// What a user who isn't an admin may use.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[cfg_attr(feature = "typescript", ts(export_to = "controller-api.ts"))]
pub struct UserLimits {
    /// Runs they may have going (queued, running or paused) at once.
    pub active_runs: u32,
    /// vCPUs a sandbox may have.
    pub cpus: u32,
    /// Memory a sandbox may have, in MiB.
    pub memory_mib: u32,
}
