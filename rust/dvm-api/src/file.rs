use std::fmt;

use serde::{Deserialize, Serialize};

/// A file's path in a sandbox, checked: absolute, or from its image's
/// working directory. `PUT` and `GET /api/sandboxes/{id}/files?path=` name
/// a file by it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[cfg_attr(feature = "typescript", ts(export_to = "controller-api.ts"))]
#[serde(try_from = "String", into = "String")]
#[cfg_attr(feature = "typescript", ts(type = "string"))]
pub struct SandboxPath(String);

impl SandboxPath {
    /// The longest a path may be, as Linux's `PATH_MAX` has it.
    const MOST_BYTES: usize = 4096;

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for SandboxPath {
    type Error = String;

    /// Not empty, no NUL, and no longer than `PATH_MAX`.
    fn try_from(path: String) -> Result<Self, String> {
        if path.is_empty() {
            return Err("a file's path is empty".to_owned());
        }
        if path.contains('\0') {
            return Err("a file's path can't hold a NUL character".to_owned());
        }
        if path.len() > Self::MOST_BYTES {
            return Err(format!(
                "a file's path is {} bytes, past {}",
                path.len(),
                Self::MOST_BYTES
            ));
        }
        Ok(Self(path))
    }
}

impl From<SandboxPath> for String {
    fn from(path: SandboxPath) -> Self {
        path.0
    }
}

impl fmt::Display for SandboxPath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// A file's mode, as a file written into a sandbox is made with: its
/// permission bits, at most `0o7777`; `0o644` unless given.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[cfg_attr(feature = "typescript", ts(export_to = "controller-api.ts"))]
#[serde(try_from = "u32", into = "u32")]
#[cfg_attr(feature = "typescript", ts(type = "number"))]
pub struct FileMode(u32);

impl FileMode {
    const MOST: u32 = 0o7777;

    pub fn bits(self) -> u32 {
        self.0
    }

    /// The mode as `chmod` reads it: octal digits, `644` or `0755`.
    pub fn from_octal(digits: &str) -> Result<Self, String> {
        let bits = u32::from_str_radix(digits, 8)
            .map_err(|_| format!("a mode is octal digits, as chmod takes them: {digits:?}"))?;
        Self::try_from(bits)
    }
}

impl Default for FileMode {
    /// `rw-r--r--`.
    fn default() -> Self {
        Self(0o644)
    }
}

impl TryFrom<u32> for FileMode {
    type Error = String;

    fn try_from(bits: u32) -> Result<Self, String> {
        if bits > Self::MOST {
            return Err(format!("a mode is at most 7777, not {bits:o}"));
        }
        Ok(Self(bits))
    }
}

impl From<FileMode> for u32 {
    fn from(mode: FileMode) -> Self {
        mode.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_path_is_checked() {
        assert_eq!(
            SandboxPath::try_from("/repo/fix.diff".to_owned())
                .unwrap()
                .as_str(),
            "/repo/fix.diff"
        );
        assert!(SandboxPath::try_from("out.txt".to_owned()).is_ok());
        for wrong in [String::new(), "a\0b".to_owned(), "a".repeat(4097)] {
            assert!(SandboxPath::try_from(wrong).is_err());
        }
        assert!(serde_json::from_str::<SandboxPath>(r#""""#).is_err());
    }

    #[test]
    fn a_mode_is_octal_as_chmod_reads_it() {
        assert_eq!(FileMode::from_octal("755").unwrap().bits(), 0o755);
        assert_eq!(FileMode::from_octal("0600").unwrap().bits(), 0o600);
        assert_eq!(FileMode::default().bits(), 0o644);
        for wrong in ["", "9", "17777", "rw"] {
            assert!(FileMode::from_octal(wrong).is_err(), "{wrong}");
        }
    }
}
