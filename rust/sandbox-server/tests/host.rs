mod support;

use std::os::unix::fs::PermissionsExt;

use serde_json::json;
use support::{Session, encoded, ordered, output};

#[test]
fn argv_and_script_keep_streams_and_exit_codes() {
    let mut server = Session::host();
    server.send(json!({"id":1,"run":{"argv":["/bin/sh","-c","printf argv; exit 0"]}}));
    let answers = server.through_exit(1);
    ordered(&answers, 0);
    assert_eq!(output(&answers, "stdout"), b"argv");
    assert!(output(&answers, "stderr").is_empty());
    server.send(json!({"id":2,"run":{"script":"printf script; printf error >&2; exit 23"}}));
    let answers = server.through_exit(2);
    ordered(&answers, 23);
    assert_eq!(
        answers[0]["started"]["argv"][0],
        if std::path::Path::new("/bin/bash").exists() {
            "/bin/bash"
        } else {
            "/bin/sh"
        }
    );
    assert_eq!(output(&answers, "stdout"), b"script");
    assert_eq!(output(&answers, "stderr"), b"error");
    assert!(server.finish().success());
    assert!(server.stderr.is_empty());
}

#[test]
fn binary_output_and_piecewise_stdin() {
    let mut server = Session::host();
    server.send(
        json!({"id":1,"run":{"argv":["/bin/sh","-c","printf '\\377\\000'; cat"],"stdin":true}}),
    );
    server.send(json!({"id":1,"stdin":{"data":encoded(&[0, 255, 128])}}));
    server.send(json!({"id":1,"stdin":{"data":encoded(b"tail"),"close":true}}));
    let answers = server.through_exit(1);
    ordered(&answers, 0);
    assert_eq!(
        output(&answers, "stdout"),
        [b"\xff\0\0\xff\x80".as_slice(), b"tail"].concat()
    );
}

#[test]
fn exec_failures_are_child_stderr_and_shell_codes() {
    let directory = tempfile::tempdir().unwrap();
    let denied = directory.path().join("denied");
    std::fs::write(&denied, "#!/bin/sh\nexit 0\n").unwrap();
    std::fs::set_permissions(&denied, std::fs::Permissions::from_mode(0o644)).unwrap();
    let mut server = Session::host();
    for (id, argv, code) in [
        (1, vec!["/no/such/program".to_owned()], 127),
        (2, vec![denied.display().to_string()], 126),
    ] {
        server.send(json!({"id":id,"run":{"argv":argv}}));
        let answers = server.through_exit(id);
        ordered(&answers, code);
        assert!(!output(&answers, "stderr").is_empty());
        assert!(output(&answers, "stdout").is_empty());
    }
}

#[test]
fn environment_directory_and_path_are_the_commands() {
    let directory = tempfile::tempdir().unwrap();
    let program = directory.path().join("show");
    std::fs::write(&program, "#!/bin/sh\nprintf '%s:%s' \"$VALUE\" \"$PWD\"\n").unwrap();
    std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).unwrap();
    let cwd = std::env::current_dir().unwrap();
    let mut server = Session::host();
    server.send(json!({"id":1,"run":{"argv":["show"],"env":{"PATH":directory.path(),"VALUE":"passed"},"cwd":directory.path()}}));
    let answers = server.through_exit(1);
    ordered(&answers, 0);
    assert_eq!(
        output(&answers, "stdout"),
        format!("passed:{}", directory.path().display()).as_bytes()
    );
    server.send(json!({"id":2,"run":{"argv":["/bin/sh","-c","printf %s \"$PWD\""],"cwd":".."}}));
    let answers = server.through_exit(2);
    ordered(&answers, 0);
    assert_eq!(
        output(&answers, "stdout"),
        cwd.parent().unwrap().as_os_str().as_encoded_bytes()
    );
}

/// The test's own user: a server that isn't root can switch only to itself.
#[test]
fn a_user_runs_the_command_with_its_ids_and_home() {
    let uid = unsafe { libc::getuid() };
    let passwd = std::fs::read_to_string("/etc/passwd").unwrap();
    let [name, _, _, gid, _, home, ..] = passwd
        .lines()
        .map(|line| line.split(':').collect::<Vec<_>>())
        .find(|fields| fields.get(2) == Some(&uid.to_string().as_str()))
        .expect("the test's user is in /etc/passwd")[..]
    else {
        panic!("a passwd line has its fields")
    };
    let show = "printf '%s %s %s %s' \"$(id -u)\" \"$(id -g)\" \"$HOME\" \"$USER\"";
    let mut server = Session::host();
    server.send(json!({"id":1,"run":{"argv":["/bin/sh","-c",show],"user":name}}));
    let answers = server.through_exit(1);
    ordered(&answers, 0);
    assert_eq!(
        output(&answers, "stdout"),
        format!("{uid} {gid} {home} {name}").as_bytes()
    );
    server.send(json!({"id":2,"run":{"argv":["/bin/sh","-c",show],"user":format!("{uid}:{gid}"),"env":{"HOME":"/given"}}}));
    let answers = server.through_exit(2);
    ordered(&answers, 0);
    assert_eq!(
        output(&answers, "stdout"),
        format!("{uid} {gid} /given {name}").as_bytes()
    );
}

#[test]
fn an_unknown_user_exits_126_with_the_reason() {
    let mut server = Session::host();
    server.send(json!({"id":1,"run":{"argv":["/bin/true"],"user":"no-such-dvm-user"}}));
    let answers = server.through_exit(1);
    ordered(&answers, 126);
    let stderr = String::from_utf8(output(&answers, "stderr")).unwrap();
    assert!(
        stderr.contains("unknown user \"no-such-dvm-user\""),
        "{stderr}"
    );
    assert!(output(&answers, "stdout").is_empty());
}

#[test]
fn queued_stdin_does_not_hold_up_other_requests() {
    let directory = tempfile::tempdir().unwrap();
    let gate = directory.path().join("gate");
    let mut server = Session::host();
    server.send(json!({"id":1,"run":{"argv":["/bin/sh","-c","while [ ! -f \"$GATE\" ]; do :; done; cat"],"env":{"GATE":gate},"stdin":true}}));
    let bytes = (0..256 * 1024).map(|index| index as u8).collect::<Vec<_>>();
    for piece in bytes.chunks(24000) {
        server.send(json!({"id":1,"stdin":{"data":encoded(piece)}}));
    }
    server.send(json!({"id":1,"stdin":{"close":true}}));
    server.send(json!({"id":2,"write":{"path":gate,"data":"","last":true}}));
    let mut command = Vec::new();
    let mut written = false;
    let mut exited = false;
    while !written || !exited {
        let answer = server.next();
        if answer["id"] == 2 {
            assert_eq!(answer["written"], true);
            written = true;
        } else {
            exited |= answer.get("exited").is_some();
            command.push(answer);
        }
    }
    ordered(&command, 0);
    assert_eq!(output(&command, "stdout"), bytes);
}

#[test]
fn signal_targets_the_process_group() {
    let mut server = Session::host();
    server.send(json!({"id":1,"run":{"script":"/bin/sh -c 'trap \"printf caught; exit 0\" TERM; printf ready; while :; do :; done' & wait"}}));
    let mut answers = Vec::new();
    while output(&answers, "stdout") != b"ready" {
        let answer = server.next();
        answers.push(answer);
    }
    server.send(json!({"id":1,"signal":15}));
    let mut exited = false;
    while !exited || output(&answers, "stdout") != b"readycaught" {
        let answer = server.next();
        if let Some(exit) = answer.get("exited") {
            assert_eq!(exit["code"], 143);
            exited = true;
        }
        answers.push(answer);
    }
}

#[test]
fn two_commands_make_progress_independently() {
    let mut server = Session::host();
    for id in [1, 2] {
        server.send(json!({"id":id,"run":{"argv":["/bin/sh","-c","cat"],"stdin":true}}));
    }
    let mut commands = [Vec::new(), Vec::new()];
    for _ in 0..2 {
        let answer = server.next();
        assert!(answer.get("started").is_some());
        commands[answer["id"].as_u64().unwrap() as usize - 1].push(answer);
    }
    for id in [2, 1] {
        server.send(json!({"id":id,"stdin":{"data":encoded(&[id as u8]),"close":true}}));
    }
    let mut exited = 0;
    while exited < 2 {
        let answer = server.next();
        exited += usize::from(answer.get("exited").is_some());
        commands[answer["id"].as_u64().unwrap() as usize - 1].push(answer);
    }
    for (index, answers) in commands.iter().enumerate() {
        ordered(answers, 0);
        assert_eq!(output(answers, "stdout"), [index as u8 + 1]);
    }
}

#[test]
fn orphan_output_can_follow_the_commands_exit() {
    let directory = tempfile::tempdir().unwrap();
    let gate = directory.path().join("gate");
    let mut server = Session::host();
    server.send(json!({"id":1,"run":{"argv":["/bin/sh","-c","(while [ ! -f \"$GATE\" ]; do :; done; printf late) & exit 7"],"env":{"GATE":gate}}}));
    let answers = server.through_exit(1);
    ordered(&answers, 7);
    assert!(output(&answers, "stdout").is_empty());
    std::fs::write(&gate, "").unwrap();
    let mut late = Vec::new();
    while late.len() < 4 {
        let answer = server.next();
        assert_eq!(answer["id"], 1);
        late.extend(output(&[answer], "stdout"));
    }
    assert_eq!(late, b"late");
}

#[test]
fn files_are_chunked_keep_modes_and_report_errors() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("file");
    std::fs::write(&path, "old contents").unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
    let bytes = (0..256 * 1024).map(|index| index as u8).collect::<Vec<_>>();
    let mut server = Session::host();
    for (index, piece) in bytes.chunks(24000).enumerate() {
        server.send(json!({"id":1,"write":{"path":path,"mode":420,"data":encoded(piece),"last":(index + 1) * 24000 >= bytes.len()}}));
    }
    assert_eq!(server.next(), json!({"id":1,"written":true}));
    assert_eq!(
        std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
        0o600
    );
    server.send(json!({"id":2,"read":{"path":path}}));
    let mut answers = Vec::new();
    loop {
        let answer = server.next();
        if answer.get("end").is_some() {
            assert_eq!(answer["end"], true);
            break;
        }
        assert!(answer.get("data").is_some());
        answers.push(answer);
    }
    assert!(answers.len() > 1);
    assert_eq!(output(&answers, "data"), bytes);
    let socket = directory.path().join("socket");
    let _listener = std::os::unix::net::UnixListener::bind(&socket).unwrap();
    let link = directory.path().join("link");
    std::os::unix::fs::symlink(&socket, &link).unwrap();
    let fifo = directory.path().join("fifo");
    let fifo_name = std::ffi::CString::new(fifo.as_os_str().as_encoded_bytes()).unwrap();
    assert_eq!(unsafe { libc::mkfifo(fifo_name.as_ptr(), 0o600) }, 0);
    for (id, path, errno) in [
        (3, directory.path().to_path_buf(), libc::EISDIR),
        (4, directory.path().join("missing"), libc::ENOENT),
        (5, "/dev/null".into(), libc::EINVAL),
        (6, socket, libc::EINVAL),
        (7, link, libc::EINVAL),
        (8, fifo, libc::EINVAL),
    ] {
        server.send(json!({"id":id,"read":{"path":path}}));
        let answer = server.next();
        assert_eq!(answer["id"], id);
        assert!(
            answer["error"]
                .as_str()
                .unwrap()
                .contains(&format!("os error {errno}")),
            "{answer}"
        );
    }
    server.send(json!({"id":9,"write":{"path":directory.path().join("missing/child"),"data":"","last":true}}));
    assert!(server.next().get("error").is_some());
    let partial = directory.path().join("partial");
    server.send(json!({"id":10,"write":{"path":partial,"data":encoded(b"kept")}}));
    server.send(json!({"id":10,"write":{"path":partial,"data":"bad base64","last":true}}));
    assert!(server.next().get("error").is_some());
    server.send(json!({"id":10,"write":{"path":partial,"data":"","last":true}}));
    assert!(
        server.next()["error"]
            .as_str()
            .unwrap()
            .contains("id must be greater than 10")
    );
    assert_eq!(std::fs::read(&partial).unwrap(), b"kept");
}

#[test]
fn unidentified_errors_abort_every_open_write() {
    for bad_line in 0..3 {
        let directory = tempfile::tempdir().unwrap();
        let first = directory.path().join("first");
        let second = directory.path().join("second");
        let mut server = Session::host();
        for (id, path) in [(1, &first), (2, &second)] {
            server.send(json!({"id":id,"write":{"path":path,"data":"AAAA"}}));
        }
        match bad_line {
            0 => server.raw(b"not JSON\n"),
            1 => server.send(json!({"write":{"path":first,"data":"CCCC"}})),
            _ => server.send(json!({"id":1,"write":{"path":first,"data":"A".repeat(65536)}})),
        }
        let mut ids = Vec::new();
        for _ in 0..3 {
            let answer = server.next();
            assert!(answer.get("error").is_some(), "{answer}");
            ids.push(answer.get("id").and_then(serde_json::Value::as_u64));
        }
        ids.sort();
        assert_eq!(ids, [None, Some(1), Some(2)]);
        for (id, path) in [(1, &first), (2, &second)] {
            server.send(json!({"id":id,"write":{"path":path,"data":"CCCC","last":true}}));
            assert!(server.next().get("error").is_some());
            assert_eq!(std::fs::read(path).unwrap(), [0; 3]);
        }
        server.send(json!({"id":3,"write":{"path":first,"data":"CCCC","last":true}}));
        assert_eq!(server.next(), json!({"id":3,"written":true}));
    }
}

#[test]
fn regular_stdin_reports_unterminated_input_once_and_stops() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("partial");
    let input = directory.path().join("requests");
    let mut bytes =
        serde_json::to_vec(&json!({"id":1,"write":{"path":path,"data":"AAAA"}})).unwrap();
    bytes.extend_from_slice(b"\n{");
    std::fs::write(&input, bytes).unwrap();
    let mut server = Session::host_with_stdin(std::fs::File::open(input).unwrap());
    assert!(server.finish().success(), "{}", server.diagnostics());
    let answers: Vec<serde_json::Value> = server
        .stdout
        .split(|&byte| byte == b'\n')
        .filter(|line| !line.is_empty())
        .map(|line| serde_json::from_slice(line).unwrap())
        .collect();
    assert_eq!(answers.len(), 2, "{answers:?}");
    assert_eq!(answers[0], json!({"error":"unterminated request line"}));
    assert_eq!(answers[1]["id"], 1);
    assert!(answers[1].get("error").is_some());
    assert_eq!(std::fs::read(path).unwrap(), [0; 3]);
}

#[test]
fn signal_numbers_are_checked_before_the_commands_state() {
    let mut server = Session::host();
    let reject = |server: &mut Session, id| {
        for signal in [0, 65] {
            server.send(json!({"id":id,"signal":signal}));
            assert_eq!(
                server.next(),
                json!({"id":id,"error":"signal must be between 1 and 64"})
            );
        }
    };
    reject(&mut server, 99);
    server.send(json!({"id":1,"run":{"argv":["/bin/sh","-c","exec cat"],"stdin":true}}));
    let started = server.next();
    assert!(started.get("started").is_some());
    reject(&mut server, 1);
    server.send(json!({"id":1,"signal":9}));
    let mut answers = vec![started];
    answers.extend(server.through_exit(1));
    ordered(&answers, 137);
    reject(&mut server, 1);
    reject(&mut server, 0);
}

#[test]
fn a_tracing_stop_keeps_the_command_live_for_signals() {
    let directory = tempfile::tempdir().unwrap();
    let mut server = Session::host();
    server.send(json!({"id":1,"run":{"argv":["python3","-c",
        "import ctypes, os, signal; assert ctypes.CDLL(None).ptrace(0, 0, None, None) == 0; os.kill(os.getpid(), signal.SIGSTOP)"
    ]}}));
    let started = server.next();
    let pid = started["started"]["pid"].as_i64().unwrap();
    server.wait_until("waiting for the command's tracing stop", || {
        std::fs::read_to_string(format!("/proc/{pid}/status"))
            .is_ok_and(|status| status.contains("State:\tt"))
    });
    server.send(
        json!({"id":2,"write":{"path":directory.path().join("barrier"),"data":"","last":true}}),
    );
    assert_eq!(server.next(), json!({"id":2,"written":true}));
    server.send(json!({"id":1,"signal":9}));
    let mut answers = vec![started];
    answers.extend(server.through_exit(1));
    ordered(&answers, 137);
}

#[test]
fn bad_requests_are_answered_and_the_server_continues() {
    let mut server = Session::host();
    for line in [
        b"not json\n".as_slice(),
        b"{}\n",
        b"{\"id\":8,\"signal\":0}\n",
        b"{\"id\":9,\"stdin\":{}}\n",
        b"{\"id\":10,\"run\":{\"argv\":[],\"script\":\"x\"}}\n",
    ] {
        server.raw(line);
        assert!(server.next().get("error").is_some());
    }
    let mut long = vec![b'x'; 65537];
    long.push(b'\n');
    server.raw(&long);
    assert!(server.next()["error"].as_str().unwrap().contains("65536"));
    server.send(json!({"id":1,"run":{"argv":["/bin/sh","-c","exit 0"]}}));
    ordered(&server.through_exit(1), 0);
    server.send(json!({"id":1,"run":{"argv":["/bin/sh","-c","exit 0"]}}));
    assert!(server.next().get("error").is_some());
    server.send(json!({"id":1,"stdin":{"data":encoded(b"ignored"),"close":true}}));
    server.send(json!({"id":1,"signal":15}));
    server.send(json!({"id":2,"run":{"argv":["/bin/sh","-c","printf alive"]}}));
    let answers = server.through_exit(2);
    ordered(&answers, 0);
    assert_eq!(output(&answers, "stdout"), b"alive");
}

#[test]
fn operation_ids_increase_and_old_input_is_ignored() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("file");
    let mut server = Session::host();
    server.send(json!({"id":0,"run":{"argv":["/bin/sh","-c","exit 0"]}}));
    ordered(&server.through_exit(0), 0);
    server.send(json!({"id":10,"write":{"path":path,"data":encoded(b"first")}}));
    server.send(json!({"id":20,"run":{"argv":["/bin/sh","-c","exit 0"]}}));
    ordered(&server.through_exit(20), 0);
    server.send(json!({"id":10,"write":{"path":path,"data":encoded(b"last"),"last":true}}));
    assert_eq!(server.next(), json!({"id":10,"written":true}));
    assert_eq!(std::fs::read(&path).unwrap(), b"firstlast");
    for request in [
        json!({"id":19,"run":{"argv":["/bin/sh","-c","exit 0"]}}),
        json!({"id":20,"read":{"path":path}}),
        json!({"id":10,"write":{"path":path,"data":"","last":true}}),
    ] {
        let id = request["id"].clone();
        server.send(request);
        assert_eq!(
            server.next(),
            json!({"id":id,"error":"id must be greater than 20"})
        );
    }
    for id in [0, 1, 10, 19, 20] {
        server.send(json!({"id":id,"stdin":{"data":encoded(b"ignored"),"close":true}}));
        server.send(json!({"id":id,"signal":15}));
    }
    for request in [json!({"id":21,"stdin":{}}), json!({"id":21,"signal":15})] {
        server.send(request);
        assert!(server.next().get("error").is_some());
    }
    server.send(json!({"id":21,"run":{"argv":["/bin/sh","-c","printf alive"]}}));
    let answers = server.through_exit(21);
    ordered(&answers, 0);
    assert_eq!(output(&answers, "stdout"), b"alive");
}

#[test]
fn shutdown_does_not_wait_for_a_daemon_outside_the_group() {
    struct Daemon(i32);
    impl Drop for Daemon {
        fn drop(&mut self) {
            unsafe { libc::kill(self.0, libc::SIGKILL) };
        }
    }

    let directory = tempfile::tempdir().unwrap();
    let gate = directory.path().join("daemon");
    let mut server = Session::host();
    server.send(json!({"id":1,"run":{
        "argv":["/bin/sh","-c","setsid /bin/sh -c 'printf \"%s\\n\" \"$$\" > \"$GATE\"; exec sleep 31337' </dev/null >/dev/null 2>&1 & while [ ! -s \"$GATE\" ]; do :; done; exit 0"],
        "env":{"GATE":gate}
    }}));
    ordered(&server.through_exit(1), 0);
    let daemon = Daemon(
        std::fs::read_to_string(gate)
            .unwrap()
            .trim()
            .parse()
            .unwrap(),
    );
    assert!(server.finish().success(), "{}", server.diagnostics());
    assert_eq!(unsafe { libc::kill(daemon.0, 0) }, 0);
}

#[test]
fn eof_and_termination_kill_commands_and_exit_zero() {
    for signal in [None, Some(libc::SIGTERM), Some(libc::SIGINT)] {
        let mut server = Session::host();
        server.send(json!({"id":1,"run":{"argv":["/bin/sh","-c","exec cat"],"stdin":true}}));
        let started = server.next();
        let pid = started["started"]["pid"].as_i64().unwrap() as i32;
        if let Some(signal) = signal {
            assert_eq!(unsafe { libc::kill(server.child.id() as i32, signal) }, 0);
        }
        assert!(server.finish().success(), "{}", server.diagnostics());
        server.wait_until("waiting for the killed command to be reaped", || {
            (unsafe { libc::kill(pid, 0) }) == -1
                && std::io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH)
        });
        assert!(server.stderr.is_empty());
    }
}
