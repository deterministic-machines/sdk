use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

use crate::watched::WatchKinds;

/// Where a replay stops, as dvm names it (its `docs/05-replay.md`, A
/// moment): before one thread's operation exactly, or at a time of the
/// recorded run.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[cfg_attr(feature = "typescript", ts(export_to = "controller-api.ts"))]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Moment {
    /// Before thread `thread`'s operation `operation`, its first being 1.
    Event { thread: u64, operation: u64 },
    /// `at_us` microseconds into the recorded run: each thread before its
    /// first operation stamped after it.
    Time { at_us: u64 },
}

impl Moment {
    /// Where a replay stops first: before its first thread's first
    /// operation.
    pub const START: Self = Self::Event {
        thread: 1,
        operation: 1,
    };
}

impl fmt::Display for Moment {
    /// As dvm reads it: `<thread>:<operation>`, or `@<milliseconds>`.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            Self::Event { thread, operation } => write!(f, "{thread}:{operation}"),
            Self::Time { at_us } => write!(f, "@{}.{:03}", at_us / 1_000, at_us % 1_000),
        }
    }
}

impl FromStr for Moment {
    type Err = String;

    /// As dvm writes it: `<thread>:<operation>`, or `@<milliseconds>`, a
    /// fraction to the microsecond.
    fn from_str(moment: &str) -> Result<Self, String> {
        let bad = || format!("a moment is <thread>:<operation> or @<milliseconds>: {moment}");
        if let Some(milliseconds) = moment.strip_prefix('@') {
            let (whole, fraction) = milliseconds.split_once('.').unwrap_or((milliseconds, ""));
            let whole: u64 = whole.parse().map_err(|_| bad())?;
            let digits = fraction.get(..fraction.len().min(3)).ok_or_else(bad)?;
            let micros = match digits {
                "" => 0,
                digits => {
                    digits.parse::<u64>().map_err(|_| bad())? * 10_u64.pow(3 - digits.len() as u32)
                }
            };
            let at_us = whole
                .checked_mul(1_000)
                .and_then(|us| us.checked_add(micros));
            return at_us.map(|at_us| Self::Time { at_us }).ok_or_else(bad);
        }
        let (thread, operation) = moment.split_once(':').ok_or_else(bad)?;
        let thread = thread.parse().map_err(|_| bad())?;
        let operation = operation.parse().map_err(|_| bad())?;
        Ok(Self::Event { thread, operation })
    }
}

/// How to start a replay, `POST /api/executions/{id}/replays`' body, which may be
/// left out: played to its end, watching the events `watch` asks for, if
/// any (`WatchedEvents`); or held at its start, waiting for a seek, which
/// watches nothing.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[cfg_attr(feature = "typescript", ts(export_to = "controller-api.ts"))]
pub struct NewReplay {
    #[serde(default)]
    pub held: bool,
    #[serde(default)]
    pub watch: WatchKinds,
}

/// Where to take a replay, `POST /api/replays/{id}/seek`'s body.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[cfg_attr(feature = "typescript", ts(export_to = "controller-api.ts"))]
pub struct Seek {
    pub to: Moment,
}

/// A replay held at a moment, and the machine there: what a seek answers.
/// Seeking back starts the replay again, and goes forward to the moment.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[cfg_attr(feature = "typescript", ts(export_to = "controller-api.ts"))]
pub struct ReplayStop {
    /// The moment it's at: the one asked for, or its start, for a time
    /// before its first stamped operation.
    pub moment: Moment,
    /// The threads at their stops, and what each does next.
    pub threads: Vec<StoppedThread>,
    pub machine: MachineState,
}

/// A thread at its stop: the operation it stops before, and what that does,
/// in dvm's words (`syscall 263`, `a wait's end, by time`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[cfg_attr(feature = "typescript", ts(export_to = "controller-api.ts"))]
pub struct StoppedThread {
    pub thread: u64,
    pub operation: u64,
    pub before: String,
}

/// What dvm's kernel holds at a replay's stop: its processes, as dvm
/// describes them.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[cfg_attr(feature = "typescript", ts(export_to = "controller-api.ts"))]
pub struct MachineState {
    pub processes: Vec<ProcessState>,
}

/// A process at a stop: its id and its parent's (0 for the first's), its
/// name, the program it runs, its directory, its threads, its open files,
/// and its memory's regions.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[cfg_attr(feature = "typescript", ts(export_to = "controller-api.ts"))]
pub struct ProcessState {
    pub id: u64,
    pub parent: u64,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub program: String,
    #[serde(default)]
    pub directory: String,
    pub threads: Vec<ThreadState>,
    #[serde(default)]
    pub files: Vec<OpenFile>,
    #[serde(default)]
    pub memory: Vec<MemoryRegion>,
}

/// A thread at a stop: its id, its name, its state (`Running`, `Ready`,
/// `Waiting`, as dvm's kernel names them), and the syscall it waits in, or
/// last waited in, which its state tells apart.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[cfg_attr(feature = "typescript", ts(export_to = "controller-api.ts"))]
pub struct ThreadState {
    pub id: u64,
    pub name: String,
    pub state: String,
    pub waits_in: Option<ThreadWait>,
    /// For a wait on a futex, what its word holds now, in hex.
    pub futex_holds: Option<String>,
}

/// A syscall a thread waits in: its name and number, and its first
/// argument (a futex's address, a file's descriptor), in hex.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[cfg_attr(feature = "typescript", ts(export_to = "controller-api.ts"))]
pub struct ThreadWait {
    pub syscall: String,
    pub number: u64,
    pub first: String,
}

impl ThreadState {
    /// The syscall it waits in now, if it waits.
    pub fn waiting_in(&self) -> Option<&ThreadWait> {
        self.waits_in.as_ref().filter(|_| self.state == "Waiting")
    }
}

/// An open file of a process: its descriptor, where it leads, as
/// `/proc/<pid>/fd/<n>` says (a path, `pipe:[n]`, `socket:[n]`), and its
/// offset.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[cfg_attr(feature = "typescript", ts(export_to = "controller-api.ts"))]
pub struct OpenFile {
    pub fd: u64,
    pub to: Option<String>,
    pub offset: u64,
}

/// A region of a process's memory, as `/proc/<pid>/maps` lists it: its
/// addresses in hex, its protection (`r-x`), and what it maps, if named.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[cfg_attr(feature = "typescript", ts(export_to = "controller-api.ts"))]
pub struct MemoryRegion {
    pub start: String,
    pub end: String,
    pub protection: String,
    pub name: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_moment_is_spelled_as_dvm_reads_it() {
        let event = Moment::Event {
            thread: 3,
            operation: 20,
        };
        assert_eq!(event.to_string(), "3:20");
        assert_eq!(Moment::Time { at_us: 1_500_250 }.to_string(), "@1500.250");
        assert_eq!(Moment::Time { at_us: 7 }.to_string(), "@0.007");
        for moment in [event, Moment::Time { at_us: 1_500_250 }, Moment::START] {
            assert_eq!(moment.to_string().parse(), Ok(moment));
        }
        // dvm writes a time it was given as a float of milliseconds.
        assert_eq!("@1500".parse(), Ok(Moment::Time { at_us: 1_500_000 }));
        assert_eq!("@2.5".parse(), Ok(Moment::Time { at_us: 2_500 }));
        assert!("@x".parse::<Moment>().is_err() && "3".parse::<Moment>().is_err());
    }

    #[test]
    fn a_description_reads_as_dvm_writes_it() {
        let described = r#"{"processes":[{"directory":"/","files":[{"fd":0,"offset":0,"to":"pipe:[1]"},{"fd":9,"offset":4,"to":null}],"id":3,"memory":[{"end":"0x7ffffffff000","name":"[stack]","protection":"rw-","start":"0x7fffff7ff000"}],"name":"sleep","parent":1,"program":"/bin/busybox","threads":[{"futex_holds":null,"id":3,"name":"sleep","state":"Waiting","waits_in":{"first":"0x7fffffffece8","number":35,"syscall":"nanosleep"}}]},{"id":4,"parent":1,"threads":[]}]}"#;
        let machine: MachineState = serde_json::from_str(described).unwrap();
        let sleep = &machine.processes[0];
        assert_eq!(
            (sleep.id, sleep.parent, sleep.name.as_str()),
            (3, 1, "sleep")
        );
        assert_eq!(sleep.files[1].to, None);
        assert_eq!(
            sleep.threads[0]
                .waiting_in()
                .map(|wait| wait.syscall.as_str()),
            Some("nanosleep")
        );
        // A process whose files are gone says only its id, parent and threads.
        assert_eq!(machine.processes[1].name, "");
    }
}
