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

#[tokio::test(start_paused = true)]
async fn idempotent_calls_retry_transient_transport_body_and_gateway_errors() {
    use axum::body::Body;
    use axum::extract::Query;
    use axum::response::IntoResponse;

    let gets = Arc::new(AtomicUsize::new(0));
    let get_count = gets.clone();
    let seeks = Arc::new(AtomicUsize::new(0));
    let seek_count = seeks.clone();
    let inputs = Arc::new(AtomicUsize::new(0));
    let input_count = inputs.clone();

    let router = Router::new()
        .route(
            "/api/sandboxes/1",
            get(move || {
                let attempt = get_count.fetch_add(1, Ordering::SeqCst);
                async move {
                    match attempt {
                        0 => StatusCode::BAD_GATEWAY.into_response(),
                        1 => (
                            StatusCode::SERVICE_UNAVAILABLE,
                            [(axum::http::header::RETRY_AFTER, "2")],
                            "Service Unavailable",
                        )
                            .into_response(),
                        _ => sandbox(RunState::Exited { code: 0 }).into_response(),
                    }
                }
            }),
        )
        .route(
            "/api/sandboxes/1/points/seek",
            post(move || {
                let attempt = seek_count.fetch_add(1, Ordering::SeqCst);
                async move {
                    if attempt == 0 {
                        StatusCode::GATEWAY_TIMEOUT.into_response()
                    } else {
                        Json(sample_stop(7, 10, 0, "openat")).into_response()
                    }
                }
            }),
        )
        .route(
            "/api/sandboxes/2",
            get(|| async {
                let draft = SandboxDraft {
                    name: "session".to_owned(),
                    image: "alpine:3.20".to_owned(),
                    ..SandboxDraft::default()
                };
                Json(Sandbox {
                    id: SandboxId::new(2),
                    owner: None,
                    spec: SandboxSpec::try_from(draft).unwrap(),
                    run: run(RunState::Exited { code: 0 }),
                })
            }),
        )
        .route(
            "/api/sandboxes/2/input",
            post(
                move |Query(query): Query<HashMap<String, String>>, body: axum::body::Bytes| {
                    let attempt = input_count.fetch_add(1, Ordering::SeqCst);
                    async move {
                        if query.get("end").map(String::as_str) == Some("true") {
                            if attempt == 1 {
                                return StatusCode::BAD_GATEWAY.into_response();
                            }
                            let offset = query["offset"].parse::<u64>().unwrap();
                            return (
                                StatusCode::CONFLICT,
                                Json(dvm_run::api::ApiError {
                                    message: format!("input already ended at length {offset}"),
                                }),
                            )
                                .into_response();
                        }
                        assert_eq!(body.len(), 0);
                        Json(dvm_run::api::SandboxInputState {
                            length: 3,
                            ended: false,
                        })
                        .into_response()
                    }
                },
            ),
        )
        .route(
            "/api/sandboxes/2/output",
            get(|| async {
                Body::from(dvm_run::api::encode_frame(
                    &serde_json::json!({ "stdout": 0, "stderr": 0, "more": false, "cut": false }),
                    &[],
                ))
            }),
        );

    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let start = tokio::time::Instant::now();
    let client = controller(router).await;
    let sbx = client.connect_sandbox(SandboxId::new(1)).await.unwrap();
    let _point = sbx.seek_end().await.unwrap();

    let session = client.connect_sandbox(SandboxId::new(2)).await.unwrap();
    assert_eq!(session.end().await.unwrap(), 0);

    let cut_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let cut_addr = cut_listener.local_addr().unwrap();
    let reads = Arc::new(AtomicUsize::new(0));
    let read_count = reads.clone();
    tokio::spawn(async move {
        while let Ok((mut socket, _)) = cut_listener.accept().await {
            let mut buf = [0u8; 512];
            let n = socket.read(&mut buf).await.unwrap_or(0);
            let request = String::from_utf8_lossy(&buf[..n]);
            if request.starts_with("GET /api/sandboxes/1 ") {
                let body = serde_json::to_vec(&sandbox(RunState::Exited { code: 0 }).0).unwrap();
                let head = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n",
                    body.len()
                );
                socket.write_all(head.as_bytes()).await.unwrap();
                socket.write_all(&body).await.unwrap();
            } else if request.starts_with("POST /api/sandboxes/1/points/seek ") {
                let body = serde_json::to_vec(&sample_stop(7, 10, 0, "openat")).unwrap();
                let head = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n",
                    body.len()
                );
                socket.write_all(head.as_bytes()).await.unwrap();
                socket.write_all(&body).await.unwrap();
            } else {
                let attempt = read_count.fetch_add(1, Ordering::SeqCst);
                let payload: &[u8] = if attempt == 0 { b"he" } else { b"hello" };
                socket
                    .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\n\r\n")
                    .await
                    .unwrap();
                socket.write_all(payload).await.unwrap();
            }
            drop(socket);
        }
    });
    let cut_client = Client::new(format!("http://{cut_addr}"), "dvm_test");
    let cut_point = cut_client
        .connect_sandbox(SandboxId::new(1))
        .await
        .unwrap()
        .seek_end()
        .await
        .unwrap();
    assert_eq!(cut_point.read_file("/etc/hosts").await.unwrap(), b"hello");

    assert_eq!(gets.load(Ordering::SeqCst), 3);
    assert_eq!(seeks.load(Ordering::SeqCst), 2);
    assert_eq!(reads.load(Ordering::SeqCst), 2);
    assert_eq!(inputs.load(Ordering::SeqCst), 3);
    assert!(start.elapsed() >= std::time::Duration::from_millis(2_400));
}

#[tokio::test(start_paused = true)]
async fn non_idempotent_posts_retry_only_before_execution_and_controller_503_is_not_retried() {
    use axum::response::IntoResponse;
    use tokio::io::AsyncReadExt;

    let dropped_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let closed_addr = dropped_listener.local_addr().unwrap();
    drop(dropped_listener);
    let closed_start = tokio::time::Instant::now();
    let closed_client = Client::new(format!("http://{closed_addr}"), "dvm_test");
    assert!(matches!(
        closed_client.create_sandbox("alpine").await,
        Err(Error::Network { .. })
    ));
    assert!(closed_start.elapsed() >= std::time::Duration::from_millis(1_850));

    let reset_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let reset_addr = reset_listener.local_addr().unwrap();
    let accepted = Arc::new(AtomicUsize::new(0));
    let accepted_count = accepted.clone();
    tokio::spawn(async move {
        while let Ok((mut socket, _)) = reset_listener.accept().await {
            accepted_count.fetch_add(1, Ordering::SeqCst);
            let mut buf = [0u8; 256];
            let _ = socket.read(&mut buf).await;
            drop(socket);
        }
    });
    let reset_client = Client::new(format!("http://{reset_addr}"), "dvm_test");
    assert!(matches!(
        reset_client.create_sandbox("alpine").await,
        Err(Error::Network { .. })
    ));
    assert_eq!(accepted.load(Ordering::SeqCst), 1);

    let pause_502 = Arc::new(AtomicUsize::new(0));
    let pause_502_count = pause_502.clone();
    let pause_429 = Arc::new(AtomicUsize::new(0));
    let pause_429_count = pause_429.clone();
    let returning = Arc::new(AtomicUsize::new(0));
    let returning_count = returning.clone();

    let router = Router::new()
        .route(
            "/api/sandboxes/1",
            get(|| async { sandbox(RunState::Running) }),
        )
        .route(
            "/api/sandboxes/1/pause",
            post(move || {
                let attempt = pause_429_count.fetch_add(1, Ordering::SeqCst);
                async move {
                    if attempt == 0 {
                        (
                            StatusCode::TOO_MANY_REQUESTS,
                            [(axum::http::header::RETRY_AFTER, "99")],
                            "rate limited",
                        )
                            .into_response()
                    } else {
                        Json(run(RunState::Paused)).into_response()
                    }
                }
            }),
        )
        .route(
            "/api/sandboxes/2",
            get(|| async {
                let mut value = sandbox(RunState::Running);
                value.0.id = SandboxId::new(2);
                value
            }),
        )
        .route(
            "/api/sandboxes/2/pause",
            post(move || {
                pause_502_count.fetch_add(1, Ordering::SeqCst);
                async { StatusCode::BAD_GATEWAY }
            }),
        )
        .route(
            "/api/sandboxes/1/points/seek",
            post(|| async { Json(sample_stop(1, 1, 0, "read")) }),
        )
        .route(
            "/api/sandboxes/1/fs/stat",
            get(move || {
                returning_count.fetch_add(1, Ordering::SeqCst);
                async {
                    (
                        StatusCode::SERVICE_UNAVAILABLE,
                        Json(serde_json::json!({
                            "error": "returning",
                            "message": "still seeking"
                        })),
                    )
                }
            }),
        );

    let client = controller(router).await;
    let s1 = client.connect_sandbox(SandboxId::new(1)).await.unwrap();
    s1.pause().await.unwrap();
    assert_eq!(pause_429.load(Ordering::SeqCst), 2);

    let s2 = client.connect_sandbox(SandboxId::new(2)).await.unwrap();
    assert!(matches!(
        s2.pause().await,
        Err(Error::Refused { status: 502, .. })
    ));
    assert_eq!(pause_502.load(Ordering::SeqCst), 1);

    let point = s1.seek_end().await.unwrap();
    assert!(matches!(
        point.stat("/etc/hosts").await,
        Err(Error::Refused { status: 503, reason }) if reason == "still seeking"
    ));
    assert_eq!(returning.load(Ordering::SeqCst), 1);
}
