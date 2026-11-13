use std::collections::HashMap;
use std::future::IntoFuture;
use std::sync::{Arc, Mutex};

use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json, Router};
use dvm_run::api::{
    ApiError, ByteChunk, OutputAfter, Run, RunId, RunOutput, RunState, SandboxDraft, SandboxId,
    SandboxInput, SandboxInputState, SandboxRequest, SandboxRequestAction, SandboxSpec, Timestamp,
};
use dvm_run::{Client, Command, Error, Sandbox};
use serde_json::{Value, json};
use tokio::sync::{Mutex as AsyncMutex, mpsc};

struct Input {
    length: u64,
    probes: usize,
    posts: usize,
    ends: usize,
    stops: usize,
    output_offset: u64,
    state: RunState,
    program: bool,
    refuse: bool,
}
#[derive(Clone)]
struct Streams {
    input: Arc<Mutex<Input>>,
    requests: mpsc::UnboundedSender<(u64, SandboxRequest)>,
    output: Arc<AsyncMutex<mpsc::UnboundedReceiver<OutputAfter>>>,
    send_output: mpsc::UnboundedSender<OutputAfter>,
}
struct Fixture {
    sandbox: Sandbox,
    streams: Streams,
    requests: mpsc::UnboundedReceiver<(u64, SandboxRequest)>,
    server: tokio::task::JoinHandle<()>,
}
impl Drop for Fixture {
    fn drop(&mut self) {
        self.server.abort();
    }
}

impl Fixture {
    async fn new(length: u64, state: RunState, program: bool) -> Self {
        let (requests, receive_requests) = mpsc::unbounded_channel();
        let (send_output, output) = mpsc::unbounded_channel();
        let streams = Streams {
            input: Arc::new(Mutex::new(Input {
                length,
                state,
                program,
                probes: 0,
                posts: 0,
                ends: 0,
                stops: 0,
                output_offset: 0,
                refuse: false,
            })),
            requests,
            output: Arc::new(AsyncMutex::new(output)),
            send_output,
        };
        let router = Router::new()
            .route("/api/sandboxes/1", get(sandbox))
            .route("/api/sandboxes/1/input", post(input))
            .route("/api/sandboxes/1/stop", post(stop))
            .route("/api/executions/1/output", get(output_after))
            .with_state(streams.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let client = Client::new(format!("http://{}", listener.local_addr().unwrap()), "test");
        let server = tokio::spawn(async move {
            axum::serve(listener, router).await.unwrap();
        });
        let sandbox = client.connect_sandbox(SandboxId::new(1)).await.unwrap();
        Self {
            sandbox,
            streams,
            requests: receive_requests,
            server,
        }
    }
    fn emit(&self, answers: &[Value]) {
        self.bytes(lines(answers), true);
    }
    fn bytes(&self, bytes: Vec<u8>, more: bool) {
        self.streams.send_output.send(output(bytes, more)).unwrap();
    }
    async fn next(&mut self) -> (u64, SandboxRequest) {
        self.requests.recv().await.unwrap()
    }
    async fn start(&mut self, name: &str) -> (Command, u64) {
        let pending = tokio::spawn(self.sandbox.start(name).into_future());
        let (id, request) = self.next().await;
        assert_eq!(id, request.id);
        self.emit(&[json!({"id": id, "started": {"pid": 1, "argv": [name]}})]);
        (pending.await.unwrap().unwrap(), id)
    }
}

fn lines(answers: &[Value]) -> Vec<u8> {
    let mut bytes = Vec::new();
    for answer in answers {
        bytes.extend(serde_json::to_vec(answer).unwrap());
        bytes.push(b'\n');
    }
    bytes
}
fn output(bytes: Vec<u8>, more: bool) -> OutputAfter {
    OutputAfter {
        output: RunOutput {
            stdout: ByteChunk::new(bytes),
            stderr: ByteChunk::default(),
        },
        more,
    }
}
fn api_run(state: RunState) -> Run {
    Run {
        id: RunId::new(1),
        replay_of: None,
        state,
        runner: Some("runner".into()),
        created_at: Timestamp::from_unix_millis(0),
        started_at: None,
        ended_at: None,
        log_bytes: None,
        log_outdated: false,
        verdict: None,
        version: 1,
    }
}
async fn sandbox(State(streams): State<Streams>) -> Json<dvm_run::api::Sandbox> {
    let input = streams.input.lock().unwrap();
    let draft = SandboxDraft {
        image: "alpine:3.20".into(),
        name: "test".into(),
        command: if input.program {
            vec!["cat".into()]
        } else {
            Vec::new()
        },
        ..SandboxDraft::default()
    };
    Json(dvm_run::api::Sandbox {
        id: SandboxId::new(1),
        owner: None,
        spec: SandboxSpec::try_from(draft).unwrap(),
        run: api_run(input.state.clone()),
        replays: Vec::new(),
    })
}
async fn input(
    State(streams): State<Streams>,
    Json(chunk): Json<SandboxInput>,
) -> Result<Json<SandboxInputState>, (StatusCode, Json<ApiError>)> {
    let mut input = streams.input.lock().unwrap();
    let bytes = chunk.chunk.bytes();
    if !bytes.is_empty() {
        input.posts += 1;
        if input.refuse {
            return Err((
                StatusCode::CONFLICT,
                Json(ApiError {
                    message: "the program is not reading its input; input length 0".into(),
                }),
            ));
        }
        assert_eq!(chunk.offset, input.length);
        assert!(bytes.len() <= 65536 && bytes.last() == Some(&b'\n'));
        let request = serde_json::from_slice(bytes).unwrap();
        streams.requests.send((chunk.offset, request)).unwrap();
        input.length += bytes.len() as u64;
    } else if !chunk.end {
        assert_eq!(chunk.offset, 0);
        input.probes += 1;
    }
    if chunk.end {
        input.ends += 1;
        input.state = RunState::Exited { code: 0 };
        streams.send_output.send(output(Vec::new(), false)).unwrap();
    }
    Ok(Json(SandboxInputState {
        length: input.length,
        ended: chunk.end,
    }))
}
async fn stop(State(streams): State<Streams>) -> Json<Run> {
    let mut input = streams.input.lock().unwrap();
    input.stops += 1;
    input.state = RunState::Stopped;
    streams.send_output.send(output(Vec::new(), false)).unwrap();
    Json(api_run(input.state.clone()))
}
async fn output_after(
    State(streams): State<Streams>,
    Query(query): Query<HashMap<String, u64>>,
) -> Json<OutputAfter> {
    assert_eq!(query["offset"], streams.input.lock().unwrap().output_offset);
    let chunk = streams.output.lock().await.recv().await.unwrap();
    streams.input.lock().unwrap().output_offset += chunk.output.stdout.bytes().len() as u64;
    Json(chunk)
}

#[tokio::test]
async fn split_lines_and_interleaved_commands_use_input_offsets() {
    let mut fixture = Fixture::new(123, RunState::Exited { code: 0 }, false).await;
    let first = tokio::spawn(fixture.sandbox.start(["first"]).into_future());
    let (aid, a) = fixture.next().await;
    assert_eq!((aid, a.id), (123, 123));
    assert!(matches!(a.action, SandboxRequestAction::Run(run) if run.user.is_none()));
    let second = tokio::spawn(fixture.sandbox.start("second").user("agent").into_future());
    let (bid, b) = fixture.next().await;
    assert!(bid > aid);
    assert_eq!(b.id, bid);
    assert!(
        matches!(b.action, SandboxRequestAction::Run(run) if run.program == dvm_run::api::SandboxProgram::Script("second".into()) && run.user.as_deref() == Some("agent"))
    );
    let answers = lines(&[
        json!({"id": aid, "started": {"pid": 1, "argv": ["first"]}}),
        json!({"id": bid, "started": {"pid": 2, "argv": ["sh", "-c", "second"]}}),
        json!({"id": aid, "stdout": "YQ=="}),
    ]);
    fixture.bytes(answers[..answers.len() - 4].to_vec(), true);
    fixture.bytes(answers[answers.len() - 4..].to_vec(), true);
    let a = Arc::new(first.await.unwrap().unwrap());
    let b = second.await.unwrap().unwrap();
    let data: Vec<u8> = (0..102400).map(|i| (i % 256) as u8).collect();
    let write_a = a.clone();
    let write_data = data.clone();
    let writing = tokio::spawn(async move { write_a.write_stdin(&write_data).await });
    let mut received = Vec::new();
    while received.len() < data.len() {
        let (_, request) = fixture.next().await;
        assert_eq!(request.id, aid);
        let SandboxRequestAction::Stdin(piece) = request.action else {
            panic!("not stdin")
        };
        received.extend_from_slice(piece.data.bytes());
    }
    writing.await.unwrap().unwrap();
    assert_eq!(received, data);
    a.close_stdin().await.unwrap();
    assert!(
        matches!(fixture.next().await.1.action, SandboxRequestAction::Stdin(piece) if piece.close)
    );
    fixture.emit(&[
        json!({"id": bid, "stdout": "Yg=="}),
        json!({"id": aid, "stderr": "ZXJy"}),
        json!({"id": bid, "exited": {"code": 3}}),
        json!({"id": aid, "exited": {"code": 0}}),
    ]);
    let ra = a.wait().await.unwrap();
    let rb = b.wait().await.unwrap();
    assert_eq!(
        (ra.exit_code, ra.stdout, ra.stderr),
        (0, b"a".to_vec(), b"err".to_vec())
    );
    assert_eq!(
        (rb.exit_code, rb.stdout, rb.stderr),
        (3, b"b".to_vec(), Vec::new())
    );
    assert_eq!(fixture.streams.input.lock().unwrap().probes, 1);
}

#[tokio::test]
async fn errors_dispatch_to_one_or_all_operations() {
    for failure in ["id", "no-id", "exit", "failed"] {
        let state = if failure == "failed" {
            RunState::Failed {
                reason: "recording limit".into(),
            }
        } else {
            RunState::Exited { code: 7 }
        };
        let mut fixture = Fixture::new(0, state, false).await;
        let (a, aid) = fixture.start("first").await;
        let (b, bid) = fixture.start("second").await;
        if failure == "id" {
            fixture.emit(&[
                json!({"id": aid, "error": "refused command"}),
                json!({"id": bid, "exited": {"code": 0}}),
            ]);
            assert!(
                matches!(a.wait().await, Err(Error::SandboxRequest { reason }) if reason == "refused command")
            );
            assert_eq!(b.wait().await.unwrap().exit_code, 0);
        } else {
            if failure == "no-id" {
                fixture.emit(&[json!({"error": "bad line"})]);
            } else {
                fixture.bytes(Vec::new(), false);
            }
            for command in [&a, &b] {
                match (failure, command.wait().await) {
                    ("no-id", Err(Error::SandboxRequest { reason })) => {
                        assert_eq!(reason, "bad line")
                    }
                    ("failed", Err(Error::SandboxFailed { reason, .. })) => {
                        assert_eq!(reason, "recording limit")
                    }
                    (
                        "exit",
                        Err(Error::SandboxEnded {
                            state: RunState::Exited { code: 7 },
                            ..
                        }),
                    ) => {}
                    other => panic!("{other:?}"),
                }
            }
            if failure == "no-id" {
                let (following, id) = fixture.start("following").await;
                fixture.emit(&[json!({"id": id, "exited": {"code": 0}})]);
                assert_eq!(following.wait().await.unwrap().exit_code, 0);
            }
        }
    }
}

#[tokio::test]
async fn files_are_cut_into_bounded_pieces_and_read_back() {
    let mut fixture = Fixture::new(0, RunState::Exited { code: 0 }, false).await;
    let data: Vec<u8> = (0..153600).map(|i| (i % 256) as u8).collect();
    let writing = tokio::spawn(
        fixture
            .sandbox
            .write_file("/tmp/a", &data)
            .mode(0o600)
            .into_future(),
    );
    let mut received = Vec::new();
    let mut write_id = None;
    loop {
        let (_, request) = fixture.next().await;
        assert_eq!(*write_id.get_or_insert(request.id), request.id);
        let SandboxRequestAction::Write(piece) = request.action else {
            panic!("not write")
        };
        assert_eq!((piece.path.as_str(), piece.mode), ("/tmp/a", Some(0o600)));
        received.extend_from_slice(piece.data.bytes());
        if piece.last {
            break;
        }
    }
    assert_eq!(received, data);
    fixture.emit(&[json!({"id": write_id, "written": true})]);
    writing.await.unwrap().unwrap();
    let sandbox = fixture.sandbox.clone();
    let reading = tokio::spawn(async move { sandbox.read_file("/tmp/a").await });
    let (rid, request) = fixture.next().await;
    assert!(rid > write_id.unwrap());
    assert!(matches!(request.action, SandboxRequestAction::Read(read) if read.path == "/tmp/a"));
    for piece in data.chunks(8000) {
        fixture.emit(&[json!({"id": rid, "data": ByteChunk::new(piece.to_vec())})]);
    }
    fixture.emit(&[json!({"id": rid, "end": true})]);
    assert_eq!(reading.await.unwrap().unwrap(), data);
}

#[tokio::test]
async fn program_sandboxes_refuse_locally_and_backpressure_is_not_retried() {
    let fixture = Fixture::new(0, RunState::Running, true).await;
    assert!(matches!(
        fixture.sandbox.run("true").await,
        Err(Error::Conflict { .. })
    ));
    assert!(matches!(
        fixture.sandbox.start("true").await,
        Err(Error::Conflict { .. })
    ));
    assert!(matches!(
        fixture.sandbox.write_file("a", b"").await,
        Err(Error::Conflict { .. })
    ));
    assert!(matches!(
        fixture.sandbox.read_file("a").await,
        Err(Error::Conflict { .. })
    ));
    assert!(matches!(
        fixture.sandbox.end().await,
        Err(Error::Conflict { .. })
    ));
    assert_eq!(fixture.streams.input.lock().unwrap().posts, 0);
    assert_eq!(fixture.streams.input.lock().unwrap().probes, 0);
    let fixture = Fixture::new(0, RunState::Running, false).await;
    fixture.streams.input.lock().unwrap().refuse = true;
    assert!(
        matches!(fixture.sandbox.run("true").await, Err(Error::Conflict { reason }) if reason.contains("not reading its input"))
    );
    assert_eq!(fixture.streams.input.lock().unwrap().posts, 1);
}

#[tokio::test]
async fn scope_cleanup_ends_input_or_stops_as_appropriate() {
    for kind in ["session", "paused", "program", "explicit"] {
        let state = if kind == "paused" {
            RunState::Paused
        } else {
            RunState::Running
        };
        let fixture = Fixture::new(0, state, kind == "program").await;
        fixture
            .sandbox
            .scope(async |sandbox| {
                if kind == "explicit" {
                    assert_eq!(sandbox.end().await?, 0);
                }
                Ok(())
            })
            .await
            .unwrap();
        let input = fixture.streams.input.lock().unwrap();
        let stopped = matches!(kind, "paused" | "program");
        assert_eq!(
            (input.ends, input.stops),
            if stopped { (0, 1) } else { (1, 0) }
        );
    }
}
