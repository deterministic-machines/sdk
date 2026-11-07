use serde::{Deserialize, Serialize};

/// CPUs and memory: what a runner hands out, and what a run takes of it
/// ([`SandboxSpec::footprint`](crate::SandboxSpec::footprint)).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[cfg_attr(feature = "typescript", ts(export_to = "controller-api.ts"))]
pub struct Capacity {
    pub cpus: u32,
    pub memory_mib: u64,
}

impl Capacity {
    /// What's left once `footprint` is taken out, if it fits.
    pub fn without(self, footprint: Capacity) -> Option<Capacity> {
        Some(Capacity {
            cpus: self.cpus.checked_sub(footprint.cpus)?,
            memory_mib: self.memory_mib.checked_sub(footprint.memory_mib)?,
        })
    }

    /// This, with `footprint` given back.
    pub fn with(self, footprint: Capacity) -> Capacity {
        Capacity {
            cpus: self.cpus.saturating_add(footprint.cpus),
            memory_mib: self.memory_mib.saturating_add(footprint.memory_mib),
        }
    }
}
