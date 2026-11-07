use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

/// A Docker image reference, as `docker run` takes it: `alpine`,
/// `alpine:3.20`, `ghcr.io/owner/name:tag`, `name@sha256:<hex>`.
///
/// Parsed by the grammar of distribution's `reference` package (IPv6 hosts
/// aside), so what passes is a reference and never anything else: no
/// spaces, and no leading `-` that `dvm run` would take for an option.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[cfg_attr(feature = "typescript", ts(export_to = "controller-api.ts"))]
#[serde(try_from = "String", into = "String")]
#[cfg_attr(feature = "typescript", ts(type = "string"))]
pub struct ImageReference(String);

/// Why a text isn't an image reference.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct InvalidImageReference(&'static str);

impl ImageReference {
    /// distribution's limit on a name, its repository's path with its domain.
    const NAME_LENGTH: usize = 255;
    /// distribution's limit on a tag.
    const TAG_LENGTH: usize = 128;

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The name of a sandbox made from it and given none: its repository's
    /// last part, as a DNS label (`ghcr.io/owner/my_app:1` is `my-app`).
    pub fn sandbox_name(&self) -> String {
        let name_and_tag = self.0.split('@').next().unwrap_or_default();
        let name_and_tag = name_and_tag.rsplit('/').next().unwrap_or_default();
        let last_part = name_and_tag.split(':').next().unwrap_or_default();
        // A part is lowercase letters and digits joined by '.', '_' and '-',
        // so the label starts with one and has no other characters.
        let label: String = last_part
            .chars()
            .map(|c| if c == '.' || c == '_' { '-' } else { c })
            .take(63)
            .collect();
        label.trim_end_matches('-').to_owned()
    }

    /// `name[:tag][@digest]`, the tag after the name's last `/`.
    fn check(text: &str) -> Result<(), InvalidImageReference> {
        if text.is_empty() {
            return Err(InvalidImageReference("an image is required"));
        }
        let (name_and_tag, digest) = match text.split_once('@') {
            Some((name_and_tag, digest)) => (name_and_tag, Some(digest)),
            None => (text, None),
        };
        let last_slash = name_and_tag.rfind('/');
        let (name, tag) = match name_and_tag.rfind(':') {
            Some(colon) if last_slash.is_none_or(|slash| colon > slash) => {
                (&name_and_tag[..colon], Some(&name_and_tag[colon + 1..]))
            }
            _ => (name_and_tag, None),
        };
        Self::check_name(name)?;
        if let Some(tag) = tag {
            Self::check_tag(tag)?;
        }
        if let Some(digest) = digest {
            Self::check_digest(digest)?;
        }
        Ok(())
    }

    /// `[domain/]path`: the first component is a domain when there is more
    /// than one and it has a `.` or a `:`, is `localhost`, or has an
    /// uppercase letter, as Docker splits them.
    fn check_name(name: &str) -> Result<(), InvalidImageReference> {
        if name.is_empty() {
            return Err(InvalidImageReference("the image's name is missing"));
        }
        if name.len() > Self::NAME_LENGTH {
            return Err(InvalidImageReference(
                "the image's name is longer than 255 characters",
            ));
        }
        let path = match name.split_once('/') {
            Some((first, rest))
                if first.contains(['.', ':'])
                    || first == "localhost"
                    || first.chars().any(|c| c.is_ascii_uppercase()) =>
            {
                Self::check_domain(first)?;
                rest
            }
            _ => name,
        };
        path.split('/').try_for_each(Self::check_path_component)
    }

    /// Lowercase letters and digits, runs of them joined by one `.`, one or
    /// two `_`, or any number of `-`.
    fn check_path_component(component: &str) -> Result<(), InvalidImageReference> {
        const INVALID: InvalidImageReference = InvalidImageReference(
            "each part of the image's name is lowercase letters and digits, joined by '.', '_', '__' or '-'",
        );
        let is_alphanumeric = |c: char| c.is_ascii_lowercase() || c.is_ascii_digit();
        let mut rest = component;
        loop {
            let run = rest.find(|c| !is_alphanumeric(c)).unwrap_or(rest.len());
            if run == 0 {
                return Err(INVALID);
            }
            rest = &rest[run..];
            let separator = if rest.starts_with("__") {
                2
            } else if rest.starts_with(['.', '_']) {
                1
            } else if rest.starts_with('-') {
                rest.find(|c| c != '-').unwrap_or(rest.len())
            } else if rest.is_empty() {
                return Ok(());
            } else {
                return Err(INVALID);
            };
            rest = &rest[separator..];
        }
    }

    /// `host[:port]`, the host's labels letters, digits and inner `-`.
    fn check_domain(domain: &str) -> Result<(), InvalidImageReference> {
        let (host, port) = match domain.split_once(':') {
            Some((host, port)) => (host, Some(port)),
            None => (domain, None),
        };
        let label_is_valid = |label: &str| {
            !label.is_empty()
                && !label.starts_with('-')
                && !label.ends_with('-')
                && label.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
        };
        if !host.split('.').all(label_is_valid) {
            return Err(InvalidImageReference(
                "the image's registry isn't a host name",
            ));
        }
        if let Some(port) = port
            && (port.is_empty() || port.len() > 5 || !port.chars().all(|c| c.is_ascii_digit()))
        {
            return Err(InvalidImageReference(
                "the image's registry port isn't a number",
            ));
        }
        Ok(())
    }

    /// A letter, digit or `_`, then up to 127 of those, `.` and `-`.
    fn check_tag(tag: &str) -> Result<(), InvalidImageReference> {
        let is_word = |c: char| c.is_ascii_alphanumeric() || c == '_';
        let mut chars = tag.chars();
        let valid = tag.len() <= Self::TAG_LENGTH
            && chars.next().is_some_and(is_word)
            && chars.all(|c| is_word(c) || c == '.' || c == '-');
        if valid {
            Ok(())
        } else {
            Err(InvalidImageReference(
                "the tag is up to 128 letters, digits, '_', '.' and '-', not starting with '.' or '-'",
            ))
        }
    }

    /// `sha256:` and 64 hex digits, or `sha512:` and 128.
    fn check_digest(digest: &str) -> Result<(), InvalidImageReference> {
        let (hex, length) = match digest.split_once(':') {
            Some(("sha256", hex)) => (hex, 64),
            Some(("sha512", hex)) => (hex, 128),
            _ => {
                return Err(InvalidImageReference(
                    "the digest isn't sha256:<hex> or sha512:<hex>",
                ));
            }
        };
        if hex.len() == length && hex.chars().all(|c| matches!(c, '0'..='9' | 'a'..='f')) {
            Ok(())
        } else {
            Err(InvalidImageReference(
                "the digest's hex is the wrong length, or not lowercase hex",
            ))
        }
    }
}

impl FromStr for ImageReference {
    type Err = InvalidImageReference;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        Self::check(text)?;
        Ok(Self(text.to_owned()))
    }
}

impl TryFrom<String> for ImageReference {
    type Error = InvalidImageReference;

    fn try_from(text: String) -> Result<Self, Self::Error> {
        Self::check(&text)?;
        Ok(Self(text))
    }
}

impl From<ImageReference> for String {
    fn from(reference: ImageReference) -> Self {
        reference.0
    }
}

impl fmt::Display for ImageReference {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_references_docker_accepts() {
        let sha256 = format!("sha256:{}", "a1".repeat(32));
        for reference in [
            "alpine".to_owned(),
            "alpine:3.20".to_owned(),
            "library/alpine".to_owned(),
            "docker.io/library/alpine:3.20".to_owned(),
            "ghcr.io/owner/name".to_owned(),
            "localhost/name".to_owned(),
            "localhost:5000/name:latest".to_owned(),
            "registry.example.com:443/a/b_c__d-e.f---g:v1.2-rc_3".to_owned(),
            format!("alpine@{sha256}"),
            format!("alpine:3.20@{sha256}"),
        ] {
            assert!(reference.parse::<ImageReference>().is_ok(), "{reference}");
        }
    }

    #[test]
    fn a_sandbox_is_named_after_its_repository() {
        let sha256 = format!("sha256:{}", "a1".repeat(32));
        for (reference, name) in [
            ("alpine".to_owned(), "alpine"),
            ("python:3.12".to_owned(), "python"),
            ("localhost:5000/name:latest".to_owned(), "name"),
            (
                "registry.example.com:443/a/b_c__d-e.f---g:v1".to_owned(),
                "b-c--d-e-f---g",
            ),
            (format!("alpine:3.20@{sha256}"), "alpine"),
            (format!("{}-b", "a".repeat(62)), &"a".repeat(62)),
        ] {
            let reference: ImageReference = reference.parse().unwrap();
            assert_eq!(reference.sandbox_name(), name, "{reference}");
        }
    }

    #[test]
    fn refuses_what_isnt_a_reference() {
        for reference in [
            "",
            "--volume=/:/host",
            "-alpine",
            "Alpine",
            "alpine:",
            "alpine:-rc",
            ":3.20",
            "a//b",
            "a/",
            "/a",
            "a b",
            "a___b",
            "a.-b",
            "alpine@sha256:abc",
            "alpine@md5:0123456789abcdef0123456789abcdef",
            "-registry.io/a",
            "registry.io:port/a",
        ] {
            assert!(
                reference.parse::<ImageReference>().is_err(),
                "{reference:?}"
            );
        }
    }

    #[test]
    fn deserializing_checks() {
        assert!(serde_json::from_str::<ImageReference>(r#""alpine:3.20""#).is_ok());
        assert!(serde_json::from_str::<ImageReference>(r#""--dir=/""#).is_err());
    }
}
