use std::fmt;

use serde::{Deserialize, Serialize};

/// Non-UTF-8 file name or symlink target bytes carried as lowercase hex.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[cfg_attr(
    feature = "typescript",
    ts(type = "string", export_to = "controller-api.ts")
)]
pub struct FileName(Vec<u8>);

impl AsRef<[u8]> for FileName {
    fn as_ref(&self) -> &[u8] {
        &self.0
    }
}

impl From<Vec<u8>> for FileName {
    fn from(bytes: Vec<u8>) -> Self {
        Self(bytes)
    }
}

impl From<&[u8]> for FileName {
    fn from(bytes: &[u8]) -> Self {
        Self(bytes.to_vec())
    }
}

impl<const N: usize> From<&[u8; N]> for FileName {
    fn from(bytes: &[u8; N]) -> Self {
        Self(bytes.to_vec())
    }
}

impl From<&str> for FileName {
    fn from(text: &str) -> Self {
        Self(text.as_bytes().to_vec())
    }
}

impl TryFrom<String> for FileName {
    type Error = &'static str;

    fn try_from(hex: String) -> Result<Self, Self::Error> {
        const REFUSAL: &str = "a file name is lowercase hex, two digits a byte";
        let digit = |byte: u8| match byte {
            b'0'..=b'9' => Ok(byte - b'0'),
            b'a'..=b'f' => Ok(byte - b'a' + 10),
            _ => Err(REFUSAL),
        };
        hex.as_bytes()
            .chunks(2)
            .map(|pair| match pair {
                [high, low] => Ok(digit(*high)? << 4 | digit(*low)?),
                _ => Err(REFUSAL),
            })
            .collect::<Result<_, _>>()
            .map(Self)
    }
}

impl From<FileName> for String {
    fn from(name: FileName) -> Self {
        name.0.iter().map(|byte| format!("{byte:02x}")).collect()
    }
}

impl fmt::Debug for FileName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?}", self.0.escape_ascii().to_string())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[cfg_attr(feature = "typescript", ts(export_to = "controller-api.ts"))]
pub enum FileType {
    Regular,
    Directory,
    Symlink,
    Fifo,
    Socket,
    CharDevice,
    BlockDevice,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[cfg_attr(feature = "typescript", ts(export_to = "controller-api.ts"))]
pub struct PathDirectoryEntry {
    pub name: String,
    pub raw_name: FileName,
    #[serde(rename = "type")]
    pub kind: FileType,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[cfg_attr(feature = "typescript", ts(export_to = "controller-api.ts"))]
pub struct FileTime(pub i64, pub u32);

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[cfg_attr(feature = "typescript", ts(export_to = "controller-api.ts"))]
pub struct FileStatus {
    #[serde(rename = "type")]
    pub kind: FileType,
    pub mode: u32,
    pub uid: u32,
    pub gid: u32,
    pub size: u64,
    pub links: u64,
    pub mtime: FileTime,
    pub ctime: FileTime,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "typescript", ts(optional))]
    pub target: Option<FileName>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "typescript", ts(optional))]
    pub mount: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_name_is_any_bytes_in_hex() {
        let name = FileName::from(b"caf\xe9");
        let written = serde_json::to_string(&name).unwrap();
        assert_eq!(written, r#""636166e9""#);
        assert_eq!(serde_json::from_str::<FileName>(&written).unwrap(), name);
        for wrong in [r#""6""#, r#""6G""#, r#""4A""#] {
            assert!(serde_json::from_str::<FileName>(wrong).is_err(), "{wrong}");
        }
    }

    #[test]
    fn file_status_and_path_entries_round_trip() {
        let status: FileStatus = serde_json::from_str(
            r#"{"type":"symlink","mode":511,"uid":0,"gid":0,"size":6,"links":1,"mtime":[1732400000,123456789],"ctime":[1732400000,123456789],"target":"2e2e2f6c6962"}"#,
        )
        .unwrap();
        assert_eq!(status.kind, FileType::Symlink);
        assert_eq!(status.target, Some(FileName::from("../lib")));
        assert_eq!(status.mtime, FileTime(1_732_400_000, 123_456_789));
        let mount: FileStatus = serde_json::from_str(
            r#"{"type":"directory","mode":365,"uid":0,"gid":0,"size":0,"links":2,"mtime":[0,0],"ctime":[0,0],"mount":"proc"}"#,
        )
        .unwrap();
        assert_eq!(mount.mount.as_deref(), Some("proc"));
        let entry: PathDirectoryEntry =
            serde_json::from_str(r#"{"name":"hosts","raw_name":"686f737473","type":"regular"}"#)
                .unwrap();
        assert_eq!(entry.raw_name, FileName::from("hosts"));
    }
}
