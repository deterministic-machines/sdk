//! A command's user, named as `docker exec --user` names it, and looked up in
//! the image's own `/etc/passwd` and `/etc/group`, without NSS.

use std::io;

#[derive(Debug, PartialEq, Eq)]
pub struct CommandUser {
    uid: libc::uid_t,
    gid: libc::gid_t,
    groups: Vec<libc::gid_t>,
    home: String,
    name: Option<String>,
}

struct PasswdEntry<'a> {
    name: &'a str,
    uid: libc::uid_t,
    gid: libc::gid_t,
    home: &'a str,
}

struct GroupEntry<'a> {
    name: &'a str,
    gid: libc::gid_t,
    members: &'a str,
}

impl CommandUser {
    pub fn of_image(spec: &str) -> Result<Self, String> {
        Self::resolve(
            spec,
            &read_if_present("/etc/passwd")?,
            &read_if_present("/etc/group")?,
        )
    }

    /// A uid not in passwd is still a user, with group 0 and home `/`, as runc has it.
    fn resolve(spec: &str, passwd: &str, group: &str) -> Result<Self, String> {
        let (user, group_spec) = match spec.split_once(':') {
            Some((user, group)) => (user, Some(group)),
            None => (spec, None),
        };
        if user.is_empty() {
            return Err(format!("user {spec:?} names no user"));
        }
        let numeric_uid = user.parse::<libc::uid_t>().ok();
        let entry = passwd_entries(passwd)
            .find(|entry| entry.name == user || Some(entry.uid) == numeric_uid);
        let (uid, primary_gid, home, name) = match (entry, numeric_uid) {
            (Some(entry), _) => (
                entry.uid,
                entry.gid,
                entry.home.to_owned(),
                Some(entry.name.to_owned()),
            ),
            (None, Some(uid)) => (uid, 0, "/".to_owned(), None),
            (None, None) => return Err(format!("unknown user {user:?}: not in /etc/passwd")),
        };
        let gid = match group_spec {
            None => primary_gid,
            Some(group_spec) => {
                let numeric_gid = group_spec.parse::<libc::gid_t>().ok();
                match group_entries(group)
                    .find(|entry| entry.name == group_spec || Some(entry.gid) == numeric_gid)
                {
                    Some(entry) => entry.gid,
                    None => numeric_gid.ok_or_else(|| {
                        format!("unknown group {group_spec:?}: not in /etc/group")
                    })?,
                }
            }
        };
        let mut groups = vec![gid];
        if let Some(name) = &name {
            for entry in group_entries(group) {
                if entry.members.split(',').any(|member| member == name)
                    && !groups.contains(&entry.gid)
                {
                    groups.push(entry.gid);
                }
            }
        }
        Ok(Self {
            uid,
            gid,
            groups,
            home,
            name,
        })
    }

    pub fn home(&self) -> &str {
        &self.home
    }

    pub fn name(&self) -> Option<&str> {
        self.name.as_deref()
    }

    /// In the forked child, before exec. A server that isn't root can still
    /// run commands as itself, so it skips setgroups, which only root may call.
    pub fn switch(&self) -> io::Result<()> {
        // SAFETY: plain syscalls, safe in a forked child; `groups` was
        // resolved before the fork and outlives the call.
        unsafe {
            if libc::geteuid() == 0 && libc::setgroups(self.groups.len(), self.groups.as_ptr()) < 0
            {
                return Err(io::Error::last_os_error());
            }
            if libc::setgid(self.gid) < 0 || libc::setuid(self.uid) < 0 {
                return Err(io::Error::last_os_error());
            }
        }
        Ok(())
    }
}

fn read_if_present(path: &str) -> Result<String, String> {
    match std::fs::read_to_string(path) {
        Ok(contents) => Ok(contents),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(String::new()),
        Err(error) => Err(format!("{path}: {error}")),
    }
}

fn passwd_entries(passwd: &str) -> impl Iterator<Item = PasswdEntry<'_>> {
    passwd.lines().filter_map(|line| {
        let fields: Vec<&str> = line.split(':').collect();
        let [name, _, uid, gid, _, home, ..] = fields[..] else {
            return None;
        };
        Some(PasswdEntry {
            name,
            uid: uid.parse().ok()?,
            gid: gid.parse().ok()?,
            home,
        })
    })
}

fn group_entries(group: &str) -> impl Iterator<Item = GroupEntry<'_>> {
    group.lines().filter_map(|line| {
        let fields: Vec<&str> = line.split(':').collect();
        let [name, _, gid, members, ..] = fields[..] else {
            return None;
        };
        Some(GroupEntry {
            name,
            gid: gid.parse().ok()?,
            members,
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const PASSWD: &str = "\
root:x:0:0:root:/root:/bin/sh
# a comment
agent:x:1000:1000::/home/agent:/bin/bash
broken line
nobody:x:65534:65534:nobody:/:/sbin/nologin
";
    const GROUP: &str = "\
root:x:0:
agent:x:1000:
wheel:x:10:root,agent
docker:x:999:agent
nogroup:x:65534:
";

    fn resolved(spec: &str) -> Result<CommandUser, String> {
        CommandUser::resolve(spec, PASSWD, GROUP)
    }

    #[test]
    fn a_name_takes_its_ids_home_and_member_groups() {
        assert_eq!(
            resolved("agent").unwrap(),
            CommandUser {
                uid: 1000,
                gid: 1000,
                groups: vec![1000, 10, 999],
                home: "/home/agent".into(),
                name: Some("agent".into()),
            }
        );
    }

    #[test]
    fn a_uid_finds_its_entry_or_stands_alone() {
        assert_eq!(resolved("65534").unwrap().name(), Some("nobody"));
        assert_eq!(
            resolved("4242").unwrap(),
            CommandUser {
                uid: 4242,
                gid: 0,
                groups: vec![0],
                home: "/".into(),
                name: None,
            }
        );
    }

    #[test]
    fn a_group_replaces_the_primary_one() {
        let user = resolved("agent:wheel").unwrap();
        assert_eq!((user.uid, user.gid), (1000, 10));
        assert_eq!(user.groups, vec![10, 999]);
        assert_eq!(resolved("root:4242").unwrap().gid, 4242);
        assert_eq!(resolved("4242:999").unwrap().gid, 999);
    }

    #[test]
    fn unknown_names_are_errors() {
        assert!(
            resolved("ghost")
                .unwrap_err()
                .contains("unknown user \"ghost\"")
        );
        assert!(
            resolved("agent:ghosts")
                .unwrap_err()
                .contains("unknown group \"ghosts\"")
        );
        assert!(resolved("").is_err());
        assert!(resolved(":wheel").is_err());
        assert!(CommandUser::resolve("root", "", "").is_err());
    }
}
