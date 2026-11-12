mod support;

use std::path::Path;
use std::process::Command;
use std::time::Duration;

use serde_json::json;
use support::{Session, encoded, ordered, output};

fn session(dvm: &std::ffi::OsStr, server: &Path, log: Option<&Path>) -> Session {
    let mut command = Command::new(dvm);
    command.arg("run");
    if let Some(log) = log {
        command.arg("--log").arg(log);
    } else {
        command.arg("--no-recording");
    }
    command
        .args(["--cpus", "1", "--time-limit", "45", "--volume"])
        .arg(format!("{}:/.dvm:ro", server.parent().unwrap().display()))
        .args(["--entrypoint", "/.dvm/sandbox-server", "alpine:3.20"]);
    Session::start(command, Duration::from_secs(60))
}

fn exercise(server: &mut Session) {
    for id in [1, 2] {
        server.send(json!({"id":id,"run":{"argv":["/bin/sh","-c","printf sequential"]}}));
        let answers = server.through_exit(id);
        ordered(&answers, 0);
        assert_eq!(output(&answers, "stdout"), b"sequential");
    }
    for id in [3, 4] {
        server.send(json!({"id":id,"run":{"argv":["/bin/cat"],"stdin":true}}));
    }
    let mut commands = [Vec::new(), Vec::new()];
    for _ in 0..2 {
        let answer = server.next();
        assert!(answer.get("started").is_some(), "{answer}");
        commands[answer["id"].as_u64().unwrap() as usize - 3].push(answer);
    }
    for id in [4, 3] {
        server.send(json!({"id":id,"stdin":{"data":encoded(&[0, 255, id as u8]),"close":true}}));
    }
    let mut exited = 0;
    while exited < 2 {
        let answer = server.next();
        exited += usize::from(answer.get("exited").is_some());
        commands[answer["id"].as_u64().unwrap() as usize - 3].push(answer);
    }
    for (index, answers) in commands.iter().enumerate() {
        ordered(answers, 0);
        assert_eq!(output(answers, "stdout"), [0, 255, index as u8 + 3]);
    }
    server.send(json!({"id":5,"run":{"argv":["/bin/cat"],"stdin":true}}));
    let started = server.next();
    assert!(started.get("started").is_some(), "{started}");
    server.send(json!({"id":5,"signal":15}));
    let mut answers = vec![started];
    answers.extend(server.through_exit(5));
    ordered(&answers, 143);
    server.send(json!({"id":6,"write":{"path":"/tmp/session","data":encoded(b"first")}}));
    server.send(
        json!({"id":6,"write":{"path":"/tmp/session","data":encoded(&[0, 255]),"last":true}}),
    );
    assert_eq!(server.next(), json!({"id":6,"written":true}));
    server.send(json!({"id":7,"read":{"path":"/tmp/session"}}));
    let mut answers = Vec::new();
    loop {
        let answer = server.next();
        assert_eq!(answer["id"], 7);
        if answer.get("end").is_some() {
            break;
        }
        assert!(answer.get("data").is_some(), "{answer}");
        answers.push(answer);
    }
    assert_eq!(output(&answers, "data"), b"first\0\xff");
    server.send(json!({"id":8,"run":{"script":"/bin/sh -c 'trap \"printf caught; exit 0\" TERM; printf ready; sleep 1000 & wait' & wait"}}));
    let mut answers = Vec::new();
    while output(&answers, "stdout") != b"ready" {
        let answer = server.next();
        assert_eq!(answer["id"], 8);
        answers.push(answer);
    }
    assert!(answers[0].get("started").is_some());
    server.send(json!({"id":8,"signal":15}));
    let mut exited = false;
    while !exited || output(&answers, "stdout") != b"readycaught" {
        let answer = server.next();
        assert_eq!(answer["id"], 8);
        if let Some(exit) = answer.get("exited") {
            assert_eq!(exit["code"], 143);
            exited = true;
        }
        answers.push(answer);
    }
    server.send(json!({"id":9,"run":{
        "argv":["/bin/sh","-c","setsid /bin/sh -c 'printf \"%s\\n\" \"$$\" > \"$GATE\"; exec sleep 31337' </dev/null >/dev/null 2>&1 & while [ ! -s \"$GATE\" ]; do :; done; exit 0"],
        "env":{"GATE":"/tmp/daemon"}
    }}));
    ordered(&server.through_exit(9), 0);
}

fn run(record: bool) {
    let dvm = std::env::var_os("DVM").expect("set DVM to the dvm executable");
    let server = std::fs::canonicalize(
        std::env::var_os("SANDBOX_SERVER")
            .expect("set SANDBOX_SERVER to the static sandbox-server"),
    )
    .unwrap();
    let directory = tempfile::tempdir().unwrap();
    let volume = directory.path().join("volume");
    std::fs::create_dir(&volume).unwrap();
    let mounted_server = volume.join("sandbox-server");
    std::fs::copy(server, &mounted_server).unwrap();
    let log = directory.path().join("session.dvm");
    let mut recorded = session(&dvm, &mounted_server, record.then_some(log.as_path()));
    exercise(&mut recorded);
    let status = recorded.finish();
    assert_eq!(status.code(), Some(0), "{}", recorded.diagnostics());
    eprintln!(
        "dvm {}: {} answer bytes, exit {status}; stderr:\n{}",
        if record { "record" } else { "no-recording" },
        recorded.stdout.len(),
        String::from_utf8_lossy(&recorded.stderr)
    );
    if record {
        let mut command = Command::new(dvm);
        command.arg("replay").arg(log).args(["--time-limit", "45"]);
        let mut replay = Session::start(command, Duration::from_secs(60));
        let replay_status = replay.finish();
        assert_eq!(replay_status, status, "{}", replay.diagnostics());
        assert_eq!(replay.stdout, recorded.stdout, "{}", replay.diagnostics());
        eprintln!(
            "dvm replay: {} identical answer bytes, exit {replay_status}; stderr:\n{}",
            replay.stdout.len(),
            String::from_utf8_lossy(&replay.stderr)
        );
    }
}

#[test]
#[ignore = "needs KVM, DVM and SANDBOX_SERVER; records and replays one vCPU"]
fn recorded_session_replays_identical_answers() {
    run(true);
}

#[test]
#[ignore = "needs KVM, DVM and SANDBOX_SERVER; isolates kernel gaps"]
fn session_without_recording() {
    run(false);
}
