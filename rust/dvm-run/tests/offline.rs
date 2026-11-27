use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json, Router};
use dvm_run::api::{
    ApiKey, CreatedApiKey, DvmStop, FileName, FileStatus, FileTime, FileType, Me,
    PathDirectoryEntry, Run, RunId, RunState, Sandbox, SandboxDraft, SandboxId, SandboxSpec,
    SeekSandbox, SeekTarget, StartedCliLogin, StepPoint, StopEvent, Timestamp, WatchPoint,
};
use dvm_run::{Client, Error};
use std::collections::HashMap;
use std::future::IntoFuture;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
/// Serves `router` as dvm.run: a client of it.
async fn controller(router: Router) -> Client {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(axum::serve(listener, router).into_future());
    Client::new(format!("http://{address}"), "dvm_test")
}

fn epoch() -> Timestamp {
    Timestamp::from_unix_millis(0)
}

fn run(state: RunState) -> Run {
    Run {
        id: RunId::new(1),
        state,
        runner: Some("runner".to_owned()),
        created_at: epoch(),
        started_at: Some(epoch()),
        ended_at: None,
        has_timeline: false,
        inspectable: true,
        outdated: false,
        version: 1,
    }
}

fn sandbox(state: RunState) -> Json<Sandbox> {
    let draft = SandboxDraft {
        name: "alpine".to_owned(),
        image: "alpine:3.20".to_owned(),
        command: vec!["true".to_owned()],
        ..SandboxDraft::default()
    };
    Json(Sandbox {
        id: SandboxId::new(1),
        owner: None,
        spec: SandboxSpec::try_from(draft).unwrap(),
        run: run(state),
    })
}

#[tokio::test]
async fn an_already_exited_program_can_be_created_and_waited() {
    let client = controller(
        Router::new()
            .route(
                "/api/sandboxes",
                post(|| async { sandbox(RunState::Exited { code: 3 }) }),
            )
            .route(
                "/api/sandboxes/1",
                get(|| async { sandbox(RunState::Exited { code: 3 }) }),
            )
            .route(
                "/api/sandboxes/1/output",
                get(|| async {
                    dvm_run::api::encode_frame(
                        &serde_json::json!({ "stdout": 2, "stderr": 0, "more": false, "cut": false }),
                        &[255, 0],
                    )
                }),
            ),
    )
    .await;
    let sbx = client
        .create_sandbox("alpine")
        .command(["true"])
        .await
        .unwrap();
    assert_eq!(sbx.wait().await.unwrap(), 3);
    assert_eq!(sbx.output().await.unwrap().stdout.bytes(), [255, 0]);
}

#[tokio::test]
async fn wait_reports_stopped_and_failed() {
    for state in [
        RunState::Stopped,
        RunState::Failed {
            reason: "unserved syscall".into(),
        },
    ] {
        let expected = state.clone();
        let client = controller(Router::new().route(
            "/api/sandboxes/1",
            get(move || async move { sandbox(state) }),
        ))
        .await;
        let sbx = client.connect_sandbox(SandboxId::new(1)).await.unwrap();
        match (sbx.wait().await, expected) {
            (
                Err(Error::SandboxEnded {
                    state: RunState::Stopped,
                    ..
                }),
                RunState::Stopped,
            ) => {}
            (Err(Error::SandboxFailed { reason, .. }), RunState::Failed { reason: expected }) => {
                assert_eq!(reason, expected)
            }
            other => panic!("{other:?}"),
        }
    }
}

#[tokio::test]
async fn dvm_run_login_saves_its_key_where_the_sdks_find_it() {
    let asked = Arc::new(AtomicUsize::new(0));
    let secret = format!("dvm_{}", "a".repeat(64));
    let created = CreatedApiKey {
        key: ApiKey {
            id: 1,
            name: "Login on laptop".to_owned(),
            ending: secret[secret.len() - 4..].to_owned(),
            created_at: epoch(),
            last_used_at: None,
        },
        secret: secret.clone(),
    };
    let routes = Router::new()
        .route(
            "/api/cli-logins",
            post(|| async {
                let started = StartedCliLogin {
                    approve_url: "http://dvm.test/cli-login/page".to_owned(),
                    code: "BCDF-GHJK".to_owned(),
                    collect_token: "collect".to_owned(),
                    expires_at: epoch(),
                };
                (StatusCode::CREATED, Json(started))
            }),
        )
        .route(
            "/api/cli-logins/key",
            post(move || async move {
                let approved = asked.fetch_add(1, Ordering::SeqCst) >= 2;
                Json(approved.then_some(created))
            }),
        )
        .route(
            "/api/me",
            get(|| async {
                let me = Me {
                    login: "alice".to_owned(),
                    admin: false,
                    limits: None,
                };
                Json(me)
            }),
        );
    let client = controller(routes).await;
    let config = tempfile::tempdir().unwrap();
    let login = tokio::process::Command::new(env!("CARGO_BIN_EXE_dvm-run"))
        .args(["login", "--no-browser", "--api-url", client.api_url()])
        .env("XDG_CONFIG_HOME", config.path())
        .output()
        .await
        .unwrap();
    let said = String::from_utf8(login.stdout).unwrap();
    assert!(login.status.success(), "{said}");
    assert!(
        said.starts_with("Approve this login in your browser: http://dvm.test/cli-login/page\n")
    );
    assert!(
        said.contains("as alice, with the key Login on laptop."),
        "{said}"
    );
    let saved = std::fs::read_to_string(config.path().join("dvm/credentials.json")).unwrap();
    let saved: serde_json::Value = serde_json::from_str(&saved).unwrap();
    assert_eq!(saved["keys"][client.api_url()], secret.as_str());
}

#[tokio::test]
async fn a_scope_stops_its_sandbox_even_when_the_operation_fails() {
    let stops = Arc::new(AtomicUsize::new(0));
    let counted = stops.clone();
    let client = controller(
        Router::new()
            .route(
                "/api/sandboxes/1",
                get(|| async { sandbox(RunState::Running) }),
            )
            .route(
                "/api/sandboxes/1/stop",
                post(move || async move {
                    counted.fetch_add(1, Ordering::SeqCst);
                    Json(run(RunState::Stopped))
                }),
            ),
    )
    .await;
    let sbx = client.connect_sandbox(SandboxId::new(1)).await.unwrap();
    let result = sbx
        .scope(async |_| Err::<(), _>(Error::Unexpected("operation failed".into())))
        .await;
    assert!(matches!(result, Err(Error::Unexpected(reason)) if reason == "operation failed"));
    assert_eq!(stops.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn output_follows_pages_and_advances_both_offsets() {
    use axum::extract::Query;
    use std::collections::HashMap;

    let data = vec![255; 4 * 1024 * 1024];
    let page = data.clone();
    let asked = Arc::new(AtomicUsize::new(0));
    let counted = asked.clone();
    let client = controller(Router::new()
        .route("/api/sandboxes/1", get(|| async { sandbox(RunState::Exited { code: 0 }) }))
        .route("/api/sandboxes/1/output", get(move |Query(query): Query<HashMap<String, usize>>| {
            let data = page.clone();
            let page = counted.fetch_add(1, Ordering::SeqCst);
            async move {
                // A sandbox still going: `more` stays true, and its output
                // so far ends where a page isn't cut.
                let (stdout, stderr, cut) = match page {
                    0 => {
                        assert!(query.is_empty(), "{query:?}");
                        (data.clone(), data, true)
                    }
                    1 => {
                        assert_eq!((query["offset"], query["stderr_offset"]), (data.len(), data.len()));
                        (vec![0, 255], vec![128], false)
                    }
                    _ => panic!("unexpected output page"),
                };
                let header = serde_json::json!({"stdout": stdout.len(), "stderr": stderr.len(), "more": true, "cut": cut});
                ([("content-type", "application/octet-stream")], dvm_run::api::encode_frame(&header, &[stdout, stderr].concat()))
            }
        }))).await;
    let sandbox = client.connect_sandbox(SandboxId::new(1)).await.unwrap();
    let output = sandbox.output().await.unwrap();
    assert_eq!(output.stdout.bytes(), [data.as_slice(), &[0, 255]].concat());
    assert_eq!(output.stderr.bytes(), [data.as_slice(), &[128]].concat());
    assert_eq!(asked.load(Ordering::SeqCst), 2);
}

fn sample_stop(thread: u64, event: u64, ns: u64, name: &str) -> DvmStop {
    DvmStop {
        thread,
        process: 1,
        operations: event,
        event,
        recording_ns: Some(ns),
        at: StopEvent::Syscall {
            number: 59,
            name: name.to_owned(),
            args: Default::default(),
            decoded: Vec::new(),
        },
        stats: HashMap::from([("syscalls".to_owned(), event)]),
    }
}

#[tokio::test]
async fn seek_step_and_files_use_stateless_point_routes() {
    use axum::extract::Query;

    let step_calls = Arc::new(AtomicUsize::new(0));
    let step_count = step_calls.clone();

    let router = Router::new()
        .route(
            "/api/sandboxes/1",
            get(|| async { sandbox(RunState::Exited { code: 0 }) }),
        )
        .route(
            "/api/sandboxes/1/points/seek",
            post(|Json(body): Json<SeekSandbox>| async move {
                match body.target {
                    SeekTarget::AtNs { ns } => Json(sample_stop(7, 10, ns, "openat")),
                    SeekTarget::Event { thread, event } => {
                        Json(sample_stop(thread, event, 500, "openat"))
                    }
                    SeekTarget::End => Json(sample_stop(7, 99, 999, "exit_group")),
                }
            }),
        )
        .route(
            "/api/sandboxes/1/points/step",
            post(move |Json(body): Json<StepPoint>| {
                let n = step_count.fetch_add(1, Ordering::SeqCst);
                async move {
                    match n {
                        0 => {
                            assert_eq!((body.thread, body.event), (7, 10));
                            Json(Some(sample_stop(7, 20, 900, "read")))
                        }
                        _ => {
                            assert_eq!((body.thread, body.event), (7, 20));
                            Json(None::<DvmStop>)
                        }
                    }
                }
            }),
        )
        .route(
            "/api/sandboxes/1/fs/stat",
            get(|Query(query): Query<HashMap<String, String>>| async move {
                assert_eq!(query["thread"], "7");
                assert_eq!(query["event"], "10");
                assert_eq!(query["path"], "/etc/hosts");
                assert_eq!(query["follow_symlinks"], "false");
                Json(FileStatus {
                    kind: FileType::Regular,
                    mode: 0o644,
                    uid: 0,
                    gid: 0,
                    size: 5,
                    links: 1,
                    mtime: FileTime(0, 0),
                    ctime: FileTime(0, 0),
                    target: None,
                    mount: None,
                })
            }),
        )
        .route(
            "/api/sandboxes/1/fs/list",
            get(|Query(query): Query<HashMap<String, String>>| async move {
                assert_eq!(query["thread"], "7");
                assert_eq!(query["event"], "10");
                assert_eq!(query["path"], "/etc");
                Json(vec![PathDirectoryEntry {
                    name: "hosts".to_owned(),
                    raw_name: FileName::from("hosts"),
                    kind: FileType::Regular,
                }])
            }),
        )
        .route(
            "/api/sandboxes/1/fs/read",
            get(|Query(query): Query<HashMap<String, String>>| async move {
                assert_eq!(query["thread"], "7");
                assert_eq!(query["event"], "10");
                assert_eq!(query["path"], "/etc/hosts");
                b"hello".to_vec()
            }),
        );

    let client = controller(router).await;
    let sbx = client.connect_sandbox(SandboxId::new(1)).await.unwrap();
    assert_eq!(sbx.spec().name(), "alpine");

    let p1 = sbx
        .seek_at_ns(500)
        .watch(WatchPoint::FileSyscalls)
        .await
        .unwrap();
    assert_eq!((p1.thread(), p1.event(), p1.ns()), (7, 10, Some(500)));

    let p2 = p1.step().await.unwrap().expect("second stop");
    assert_eq!(p2.event(), 20);
    assert!(p2.step().await.unwrap().is_none());

    let stat = p1.stat("/etc/hosts").follow_symlinks(false).await.unwrap();
    assert_eq!(stat.size, 5);
    assert_eq!(p1.list_dir("/etc").await.unwrap()[0].name, "hosts");
    assert_eq!(p1.read_file("/etc/hosts").await.unwrap(), b"hello");
}
