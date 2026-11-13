//! Where a sandbox's connections out may go: dvm's `--network` and
//! `--allow` as it starts, and its control socket's `network` as it runs.

use std::net::Ipv4Addr;

use serde::{Deserialize, Serialize};

/// As JSON: `{"mode": "public"}`, `{"mode": "none"}`, or
/// `{"mode": "allowlist", "allow": ["pypi.org", "*.github.com", "8.8.8.0/24"]}`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[cfg_attr(feature = "typescript", ts(export_to = "controller-api.ts"))]
#[serde(tag = "mode", rename_all = "lowercase")]
pub enum SandboxNetwork {
    /// The internet, and none of its runner's own, LAN, or link-local
    /// addresses.
    #[default]
    Public,
    /// Nothing outside the sandbox, not even a resolver.
    None,
    /// The resolver; the internet addresses it gives each allowed name;
    /// and each address or network allowed, whatever it is.
    Allowlist { allow: Vec<String> },
}

impl SandboxNetwork {
    /// The most entries an allowlist has.
    pub const MOST_ENTRIES: usize = 256;

    /// What a sandbox never reaches, as dvm's `public` keeps from it: its
    /// runner, the runner's LAN, its cloud's metadata service, and the
    /// like. dvm would let an allowlist name them; dvm.run doesn't.
    const FENCED: [([u8; 4], u8); 10] = [
        ([0, 0, 0, 0], 8),
        ([10, 0, 0, 0], 8),
        ([100, 64, 0, 0], 10),
        ([127, 0, 0, 0], 8),
        ([169, 254, 0, 0], 16),
        ([172, 16, 0, 0], 12),
        ([192, 0, 0, 0], 24),
        ([192, 168, 0, 0], 16),
        ([224, 0, 0, 0], 4),
        ([240, 0, 0, 0], 4),
    ];

    /// Each entry a name (`pypi.org`), its subdomains (`*.github.com`), an
    /// IPv4 address, or an IPv4 network (`8.8.8.0/24`) on the internet: a
    /// sandbox has no IPv6.
    pub fn check(&self) -> Result<(), String> {
        let Self::Allowlist { allow } = self else {
            return Ok(());
        };
        if allow.len() > Self::MOST_ENTRIES {
            return Err(format!("at most {} entries", Self::MOST_ENTRIES));
        }
        allow.iter().try_for_each(|entry| Self::check_entry(entry))
    }

    /// The words dvm takes after `--network` (each entry as an `--allow`)
    /// or its control socket's `network`: the mode, then any entries.
    pub fn dvm_words(&self) -> Vec<&str> {
        match self {
            Self::Public => vec!["public"],
            Self::None => vec!["none"],
            Self::Allowlist { allow } => std::iter::once("allowlist")
                .chain(allow.iter().map(String::as_str))
                .collect(),
        }
    }

    fn check_entry(entry: &str) -> Result<(), String> {
        let not_one = || format!("{entry:?} is not a name, an IPv4 address or network");
        if entry.contains(':') {
            return Err(format!("{entry:?}: a sandbox has no IPv6"));
        }
        if let Some((address, prefix)) = entry.split_once('/') {
            let prefix: u8 = prefix.parse().map_err(|_| not_one())?;
            return match address.parse::<Ipv4Addr>() {
                Ok(address) if prefix <= 32 => Self::check_on_internet(entry, address, prefix),
                _ => Err(not_one()),
            };
        }
        if let Ok(address) = entry.parse::<Ipv4Addr>() {
            return Self::check_on_internet(entry, address, 32);
        }
        let name = entry.strip_prefix("*.").unwrap_or(entry);
        let name = name.strip_suffix('.').unwrap_or(name);
        let label = |label: &str| {
            !label.is_empty()
                && label.len() <= 63
                && label
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
        };
        if name.len() <= 253 && name.split('.').all(label) {
            Ok(())
        } else {
            Err(not_one())
        }
    }

    /// Whether the network shares no address with the fenced ones.
    fn check_on_internet(entry: &str, address: Ipv4Addr, prefix: u8) -> Result<(), String> {
        let mask = |prefix: u8| u32::MAX.checked_shl(32 - u32::from(prefix)).unwrap_or(0);
        let overlaps = Self::FENCED.iter().any(|&(fenced, fenced_prefix)| {
            let shorter = mask(prefix.min(fenced_prefix));
            u32::from(address) & shorter == u32::from_be_bytes(fenced) & shorter
        });
        if overlaps {
            Err(format!(
                "{entry:?} reaches what a sandbox never does: a runner, its LAN, link-local addresses"
            ))
        } else {
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::SandboxNetwork;

    fn allowlist(entries: &[&str]) -> SandboxNetwork {
        SandboxNetwork::Allowlist {
            allow: entries.iter().map(|entry| entry.to_string()).collect(),
        }
    }

    #[test]
    fn json_names_the_mode_and_an_allowlists_entries() {
        let json = |network: &SandboxNetwork| serde_json::to_string(network).unwrap();
        assert_eq!(json(&SandboxNetwork::default()), r#"{"mode":"public"}"#);
        assert_eq!(json(&SandboxNetwork::None), r#"{"mode":"none"}"#);
        assert_eq!(
            json(&allowlist(&["pypi.org"])),
            r#"{"mode":"allowlist","allow":["pypi.org"]}"#
        );
    }

    #[test]
    fn entries_are_names_subdomains_and_ipv4_addresses_or_networks() {
        let good = [
            "pypi.org",
            "*.github.com",
            "files.pythonhosted.org.",
            "8.8.8.0/24",
            "1.2.3.4",
        ];
        assert_eq!(allowlist(&good).check(), Ok(()));
        for bad in [
            "::1",
            "10.0.0.0/33",
            "10.0.0/8",
            "two words",
            "a..b",
            "",
            "*.",
        ] {
            assert!(allowlist(&[bad]).check().is_err(), "{bad:?}");
        }
        for fenced in [
            "10.1.2.3",
            "192.168.0.0/16",
            "169.254.169.254",
            "0.0.0.0/0",
            "172.0.0.0/8",
        ] {
            let refused = allowlist(&[fenced]).check().unwrap_err();
            assert!(refused.contains("never"), "{fenced:?}: {refused}");
        }
        let too_many: Vec<String> = (0..=SandboxNetwork::MOST_ENTRIES)
            .map(|index| format!("host{index}.example"))
            .collect();
        assert!(
            SandboxNetwork::Allowlist { allow: too_many }
                .check()
                .is_err()
        );
    }

    #[test]
    fn dvm_words_are_the_mode_then_the_entries() {
        assert_eq!(SandboxNetwork::Public.dvm_words(), ["public"]);
        assert_eq!(SandboxNetwork::None.dvm_words(), ["none"]);
        assert_eq!(
            allowlist(&["pypi.org", "8.8.8.0/24"]).dvm_words(),
            ["allowlist", "pypi.org", "8.8.8.0/24"]
        );
    }
}
