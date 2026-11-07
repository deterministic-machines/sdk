//! The API keys `dvm-run login` saved, by the address of the dvm.run each is
//! for: `dvm/credentials.json` in `$XDG_CONFIG_HOME`, else in `~/.config`,
//! readable by its user alone. The Python and JS SDKs read and write the
//! same file.

use std::collections::BTreeMap;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};

#[derive(Default, Serialize, Deserialize)]
struct SavedKeys {
    keys: BTreeMap<String, String>,
}

/// Where the keys are saved.
pub fn credentials_path() -> PathBuf {
    let config = match std::env::var_os("XDG_CONFIG_HOME") {
        Some(config) if !config.is_empty() => PathBuf::from(config),
        _ => home().join(".config"),
    };
    config.join("dvm").join("credentials.json")
}

/// The key saved for the dvm.run at `api_url`, if there's one.
pub(crate) fn saved_key(api_url: &str) -> Result<Option<String>> {
    saved_key_in(&credentials_path(), api_url)
}

/// Saves `key` for the dvm.run at `api_url`, in place of the one saved for
/// it before; the file it's in.
pub fn save_key(api_url: &str, key: &str) -> Result<PathBuf> {
    save_key_in(credentials_path(), api_url, key)
}

fn saved_key_in(path: &Path, api_url: &str) -> Result<Option<String>> {
    Ok(saved_keys(path)?.keys.remove(address_of(api_url)))
}

fn save_key_in(path: PathBuf, api_url: &str, key: &str) -> Result<PathBuf> {
    let mut saved = saved_keys(&path)?;
    saved
        .keys
        .insert(address_of(api_url).to_owned(), key.to_owned());
    let mut text = serde_json::to_string_pretty(&saved).expect("keys are JSON");
    text.push('\n');
    write_privately(&path, text.as_bytes()).map_err(|error| Error::Credentials {
        path: path.clone(),
        problem: error.to_string(),
    })?;
    Ok(path)
}

fn saved_keys(path: &Path) -> Result<SavedKeys> {
    let text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(SavedKeys::default()),
        Err(error) => {
            return Err(Error::Credentials {
                path: path.to_owned(),
                problem: error.to_string(),
            });
        }
    };
    serde_json::from_str(&text).map_err(|_| Error::Credentials {
        path: path.to_owned(),
        problem: "isn't what `dvm-run login` writes: delete it, and log in again".to_owned(),
    })
}

/// Written beside it, then moved over it, so a reader never sees half of
/// it; made for its user alone before the key is in it.
fn write_privately(path: &Path, contents: &[u8]) -> io::Result<()> {
    let directory = path.parent().expect("the file is in a directory");
    let written = path.with_extension("json.new");
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
        fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(directory)?;
        options.mode(0o600);
    }
    #[cfg(not(unix))]
    fs::create_dir_all(directory)?;
    match fs::remove_file(&written) {
        Err(error) if error.kind() != io::ErrorKind::NotFound => return Err(error),
        _ => {}
    }
    let mut file = options.open(&written)?;
    file.write_all(contents)?;
    file.sync_all()?;
    fs::rename(&written, path)
}

fn home() -> PathBuf {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .unwrap_or_default()
}

fn address_of(api_url: &str) -> &str {
    api_url.trim_end_matches('/')
}

#[cfg(test)]
mod tests {
    use super::*;

    const URL: &str = "http://dvm.test";

    #[test]
    fn a_key_is_saved_for_its_user_alone() {
        let config = tempfile::tempdir().unwrap();
        let path = config.path().join("dvm/credentials.json");
        save_key_in(path.clone(), &format!("{URL}/"), "dvm_a").unwrap();
        assert_eq!(saved_key_in(&path, URL).unwrap().as_deref(), Some("dvm_a"));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = |path: &Path| fs::metadata(path).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode(&path), 0o600);
            assert_eq!(mode(path.parent().unwrap()), 0o700);
        }
    }

    #[test]
    fn each_dvm_run_keeps_its_own_key() {
        let config = tempfile::tempdir().unwrap();
        let path = config.path().join("dvm/credentials.json");
        save_key_in(path.clone(), URL, "dvm_a").unwrap();
        save_key_in(path.clone(), "http://other.test", "dvm_b").unwrap();
        save_key_in(path.clone(), URL, "dvm_c").unwrap();
        assert_eq!(saved_key_in(&path, URL).unwrap().as_deref(), Some("dvm_c"));
        let other = saved_key_in(&path, "http://other.test").unwrap();
        assert_eq!(other.as_deref(), Some("dvm_b"));
        assert_eq!(saved_key_in(&path, "http://third.test").unwrap(), None);
    }

    #[test]
    fn the_python_sdks_file_is_the_same() {
        let config = tempfile::tempdir().unwrap();
        let path = config.path().join("credentials.json");
        // As Python's json.dump(indent=2) writes it.
        fs::write(
            &path,
            format!("{{\n  \"keys\": {{\n    \"{URL}\": \"dvm_python\"\n  }}\n}}\n"),
        )
        .unwrap();
        assert_eq!(
            saved_key_in(&path, URL).unwrap().as_deref(),
            Some("dvm_python")
        );
    }

    #[test]
    fn a_file_it_didnt_write_is_refused() {
        let config = tempfile::tempdir().unwrap();
        let path = config.path().join("credentials.json");
        fs::write(&path, "not json").unwrap();
        let refused = saved_key_in(&path, URL).unwrap_err();
        assert!(
            refused.to_string().contains("delete it, and log in again"),
            "{refused}"
        );
    }
}
