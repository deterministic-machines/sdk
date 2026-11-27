use std::collections::HashMap;

use serde::{Deserialize, Deserializer, Serialize};

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[cfg_attr(feature = "typescript", ts(export_to = "controller-api.ts"))]
pub struct InspectionWatches {
    #[serde(
        default,
        deserialize_with = "InspectionWatches::read_watch_points",
        skip_serializing_if = "Option::is_none"
    )]
    #[cfg_attr(feature = "typescript", ts(optional))]
    watch_points: Option<WatchPoints>,
}

impl InspectionWatches {
    pub fn into_requested_points(self) -> Result<Option<WatchPoints>, String> {
        if let Some(points) = &self.watch_points
            && points.is_empty()
        {
            return Err("watch_points must contain at least one watch point".into());
        }
        Ok(self.watch_points)
    }

    fn read_watch_points<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Option<WatchPoints>, D::Error> {
        WatchPoints::deserialize(deserializer).map(Some)
    }
}

impl From<InspectionWatches> for WatchPoints {
    fn from(watches: InspectionWatches) -> Self {
        watches.watch_points.unwrap_or_default()
    }
}

impl From<WatchPoints> for InspectionWatches {
    fn from(watch_points: WatchPoints) -> Self {
        Self {
            watch_points: Some(watch_points),
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "Vec<WatchPoint>")]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[cfg_attr(feature = "typescript", ts(export_to = "controller-api.ts"))]
pub struct WatchPoints(Vec<WatchPoint>);

impl WatchPoints {
    pub const MAX_WATCH_POINTS: usize = 32;

    pub fn default_step() -> Self {
        Self(vec![
            WatchPoint::FileSyscalls,
            WatchPoint::Processes,
            WatchPoint::SocketSyscalls,
        ])
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn as_slice(&self) -> &[WatchPoint] {
        &self.0
    }
}

impl TryFrom<Vec<WatchPoint>> for WatchPoints {
    type Error = String;

    fn try_from(watch_points: Vec<WatchPoint>) -> Result<Self, Self::Error> {
        if watch_points.len() > Self::MAX_WATCH_POINTS {
            return Err(format!(
                "at most {} watch points are allowed",
                Self::MAX_WATCH_POINTS
            ));
        }
        Ok(Self(watch_points))
    }
}

impl From<WatchPoints> for Vec<WatchPoint> {
    fn from(points: WatchPoints) -> Self {
        points.0
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[cfg_attr(feature = "typescript", ts(export_to = "controller-api.ts"))]
pub enum WatchPoint {
    Syscall,
    NamedSyscall { name: SyscallName },
    PageFault,
    Processes,
    FileSyscalls,
    SocketSyscalls,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[cfg_attr(
    feature = "typescript",
    ts(type = "string", export_to = "controller-api.ts")
)]
pub struct SyscallName(String);

impl SyscallName {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for SyscallName {
    type Error = &'static str;

    fn try_from(name: String) -> Result<Self, Self::Error> {
        const MAX_NAME_BYTES: usize = 64;
        if !name.is_empty()
            && name.len() <= MAX_NAME_BYTES
            && name
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
        {
            Ok(Self(name))
        } else {
            Err("a syscall name must be 1 to 64 letters, digits or underscores")
        }
    }
}

impl From<SyscallName> for String {
    fn from(name: SyscallName) -> Self {
        name.0
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[cfg_attr(feature = "typescript", ts(export_to = "controller-api.ts"))]
pub enum SeekTarget {
    #[default]
    End,
    AtNs {
        ns: u64,
    },
    Event {
        thread: u64,
        event: u64,
    },
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[cfg_attr(feature = "typescript", ts(export_to = "controller-api.ts"))]
pub struct SeekSandbox {
    #[serde(default)]
    pub target: SeekTarget,
    #[serde(flatten)]
    pub watches: InspectionWatches,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct StepPoint {
    pub thread: u64,
    pub event: u64,
    #[serde(flatten)]
    pub watches: InspectionWatches,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[cfg_attr(feature = "typescript", ts(export_to = "controller-api.ts"))]
pub struct DvmStop {
    pub thread: u64,
    pub process: u64,
    pub operations: u64,
    pub event: u64,
    pub recording_ns: Option<u64>,
    pub at: StopEvent,
    pub stats: HashMap<String, u64>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[cfg_attr(feature = "typescript", ts(export_to = "controller-api.ts"))]
pub enum StopEvent {
    Syscall {
        number: u64,
        name: String,
        args: [String; 6],
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        decoded: Vec<DecodedArgument>,
    },
    PageFault {
        address: String,
        write: bool,
        present: bool,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[cfg_attr(feature = "typescript", ts(export_to = "controller-api.ts"))]
pub struct DecodedArgument {
    pub arg: u8,
    #[serde(flatten)]
    pub value: DecodedValue,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub truncated: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[cfg_attr(feature = "typescript", ts(export_to = "controller-api.ts"))]
pub enum DecodedValue {
    Path(String),
    Strings(Vec<String>),
    Address(String),
    Bytes(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn watch_points_validate_names_and_limits() {
        let points = serde_json::from_str::<WatchPoints>(
            r#"[{"kind":"syscall"},{"kind":"named_syscall","name":"openat"},{"kind":"page_fault"},{"kind":"processes"},{"kind":"file_syscalls"},{"kind":"socket_syscalls"}]"#,
        )
        .unwrap();
        assert_eq!(points.as_slice().len(), 6);
        assert!(
            serde_json::from_str::<WatchPoint>(
                r#"{"kind":"named_syscall","name":"openat\nresume"}"#
            )
            .is_err()
        );
        assert_eq!(
            serde_json::from_str::<SeekSandbox>("{}").unwrap().target,
            SeekTarget::End
        );
    }

    #[test]
    fn a_stop_reads_what_dvm_decoded() {
        let stop: DvmStop = serde_json::from_str(
            r#"{"thread":7,"process":5,"operations":120,"event":3456,"recording_ns":null,"at":{"syscall":{"number":59,"name":"execve","args":["0x20057d","0x7fffffffeba8","0x0","0x0","0x0","0x0"],"decoded":[{"arg":0,"path":"/bin/cat"},{"arg":1,"strings":["cat","/etc/hosts"],"truncated":true}]}},"stats":{"syscalls":3456}}"#,
        )
        .unwrap();
        let StopEvent::Syscall { decoded, .. } = &stop.at else {
            panic!("expected a syscall");
        };
        assert_eq!(
            decoded,
            &[
                DecodedArgument {
                    arg: 0,
                    value: DecodedValue::Path("/bin/cat".to_owned()),
                    truncated: false,
                },
                DecodedArgument {
                    arg: 1,
                    value: DecodedValue::Strings(vec!["cat".to_owned(), "/etc/hosts".to_owned()]),
                    truncated: true,
                },
            ]
        );
    }

    #[test]
    fn requests_distinguish_omitted_watches_from_an_empty_list() {
        let step: StepPoint = serde_json::from_str(r#"{"thread":1,"event":2}"#).unwrap();
        assert_eq!(step.watches.into_requested_points().unwrap(), None);
        let step: StepPoint =
            serde_json::from_str(r#"{"thread":1,"event":2,"watch_points":[]}"#).unwrap();
        assert!(step.watches.into_requested_points().is_err());
        assert!(
            serde_json::from_str::<StepPoint>(r#"{"thread":1,"event":2,"watch_points":null}"#)
                .is_err()
        );
        let stored: InspectionWatches = serde_json::from_str(r#"{"watch_points":[]}"#).unwrap();
        assert!(WatchPoints::from(stored).is_empty());
    }
}
