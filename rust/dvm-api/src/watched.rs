//! What a replay watches, chosen before it plays (dvm's `replay --watch`,
//! its `docs/05-replay.md`, Events), and the events it saw, each marked on
//! the run's timeline at its time.

use std::collections::BTreeMap;
use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

use crate::replay::Moment;

/// A kind of event a replay may watch: syscalls of a kind, or page faults.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[cfg_attr(feature = "typescript", ts(export_to = "controller-api.ts"))]
#[serde(rename_all = "kebab-case")]
pub enum WatchKind {
    /// Files and descriptors: opening, reading, writing, and the tree.
    Files,
    /// Sockets.
    Network,
    /// Processes made, programs run, exits, and waits for them.
    Processes,
    /// Mappings of memory.
    Memory,
    /// Clocks, sleeps and timers.
    Time,
    /// Signals: their actions, masks, sending, and waits for them.
    Signals,
    /// Waits on futexes, polls and events.
    Waits,
    /// Any other syscall: ids, limits, the machine's name, random bytes.
    Other,
    /// Page faults in the program's code.
    PageFaults,
}

impl WatchKind {
    pub const ALL: [Self; 9] = [
        Self::Files,
        Self::Network,
        Self::Processes,
        Self::Memory,
        Self::Time,
        Self::Signals,
        Self::Waits,
        Self::Other,
        Self::PageFaults,
    ];

    /// Its name, as dvm's `--watch` and the API spell it.
    pub fn name(self) -> &'static str {
        match self {
            Self::Files => "files",
            Self::Network => "network",
            Self::Processes => "processes",
            Self::Memory => "memory",
            Self::Time => "time",
            Self::Signals => "signals",
            Self::Waits => "waits",
            Self::Other => "other",
            Self::PageFaults => "page-faults",
        }
    }

    /// Its name, as a person reads it.
    pub fn label(self) -> &'static str {
        match self {
            Self::Files => "Files",
            Self::Network => "Network",
            Self::Processes => "Processes",
            Self::Memory => "Memory",
            Self::Time => "Time",
            Self::Signals => "Signals",
            Self::Waits => "Waits",
            Self::Other => "Other syscalls",
            Self::PageFaults => "Page faults",
        }
    }

    fn bit(self) -> u16 {
        1 << self as u16
    }
}

impl FromStr for WatchKind {
    type Err = String;

    fn from_str(name: &str) -> Result<Self, String> {
        Self::ALL
            .into_iter()
            .find(|kind| kind.name() == name)
            .ok_or_else(|| format!("no kind of event {name:?}"))
    }
}

/// The kinds of events a replay watches. Read and written as a list of
/// kinds; as text, as dvm's `--watch` takes them, comma-separated, empty
/// for none.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[cfg_attr(feature = "typescript", ts(export_to = "controller-api.ts"))]
#[serde(from = "Vec<WatchKind>", into = "Vec<WatchKind>")]
#[cfg_attr(feature = "typescript", ts(as = "Vec<WatchKind>"))]
pub struct WatchKinds(u16);

impl WatchKinds {
    pub const NONE: Self = Self(0);

    pub fn with(self, kind: WatchKind) -> Self {
        Self(self.0 | kind.bit())
    }

    pub fn without(self, kind: WatchKind) -> Self {
        Self(self.0 & !kind.bit())
    }

    pub fn contains(self, kind: WatchKind) -> bool {
        self.0 & kind.bit() != 0
    }

    pub fn is_empty(self) -> bool {
        self.0 == 0
    }

    /// Its kinds, in `WatchKind::ALL`'s order.
    pub fn kinds(self) -> impl Iterator<Item = WatchKind> {
        WatchKind::ALL
            .into_iter()
            .filter(move |&kind| self.contains(kind))
    }
}

impl From<Vec<WatchKind>> for WatchKinds {
    fn from(kinds: Vec<WatchKind>) -> Self {
        kinds.into_iter().fold(Self::NONE, Self::with)
    }
}

impl From<WatchKinds> for Vec<WatchKind> {
    fn from(kinds: WatchKinds) -> Self {
        kinds.kinds().collect()
    }
}

impl fmt::Display for WatchKinds {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let names: Vec<&str> = self.kinds().map(WatchKind::name).collect();
        f.write_str(&names.join(","))
    }
}

impl FromStr for WatchKinds {
    type Err = String;

    fn from_str(list: &str) -> Result<Self, String> {
        list.split(',')
            .filter(|name| !name.is_empty())
            .try_fold(Self::NONE, |kinds, name| Ok(kinds.with(name.parse()?)))
    }
}

/// Where an event is among its thread's operations (dvm's positions).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[cfg_attr(feature = "typescript", ts(export_to = "controller-api.ts"))]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum EventPosition {
    /// It is the operation numbered so: a replay stops just before it.
    At { operation: u64 },
    /// It is between two operations, after so many: a page fault.
    After { operations: u64 },
}

impl EventPosition {
    /// The moment a replay goes to for it, by thread `thread`: before it,
    /// or before the thread's next operation.
    pub fn moment(self, thread: u64) -> Moment {
        let operation = match self {
            Self::At { operation } => operation,
            Self::After { operations } => operations + 1,
        };
        Moment::Event { thread, operation }
    }
}

/// An event a replay watched.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[cfg_attr(feature = "typescript", ts(export_to = "controller-api.ts"))]
pub struct WatchedEvent {
    pub thread: u64,
    pub position: EventPosition,
    /// When it was in the recorded run, from its start, as the log's stamps
    /// say: at most a millisecond early. None if the log has no stamp then.
    pub at_us: Option<u64>,
    pub kind: WatchKind,
    /// What it was, a line: `openat /etc/os-release → 3`.
    pub what: String,
}

/// What a replay watched, and what it saw: its events, in the order the
/// replay made them, of a kind with more than `MOST_OF_A_KIND` a sample
/// spread over the run (its first in each thousandth of the run, and every
/// so many beside, about the most); and how many of each kind there were,
/// all of them.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[cfg_attr(feature = "typescript", ts(export_to = "controller-api.ts"))]
pub struct WatchedEvents {
    pub watched: WatchKinds,
    pub events: Vec<WatchedEvent>,
    pub counts: BTreeMap<WatchKind, u64>,
}

impl WatchedEvents {
    /// About the most events of a kind kept, beside its first in each
    /// thousandth of the run: enough to step through where they're dense.
    pub const MOST_OF_A_KIND: usize = 2_000;

    /// How many events of `kind` there were, kept or not.
    pub fn count(&self, kind: WatchKind) -> u64 {
        self.counts.get(&kind).copied().unwrap_or(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kinds_are_read_as_written() {
        let kinds: WatchKinds = "files,page-faults".parse().unwrap();
        assert_eq!(kinds.to_string(), "files,page-faults");
        assert!(kinds.contains(WatchKind::PageFaults));
        assert!(!kinds.contains(WatchKind::Network));
        assert_eq!("".parse::<WatchKinds>(), Ok(WatchKinds::NONE));
        assert!("files,sockets".parse::<WatchKinds>().is_err());
        let json = serde_json::to_string(&kinds).unwrap();
        assert_eq!(json, r#"["files","page-faults"]"#);
        assert_eq!(serde_json::from_str::<WatchKinds>(&json).unwrap(), kinds);
    }

    #[test]
    fn a_page_fault_is_gone_to_at_its_threads_next_operation() {
        let after = EventPosition::After { operations: 41 };
        assert_eq!(
            after.moment(3),
            Moment::Event {
                thread: 3,
                operation: 42
            }
        );
        let at = EventPosition::At { operation: 22 };
        assert_eq!(at.moment(5).to_string(), "5:22");
    }
}
