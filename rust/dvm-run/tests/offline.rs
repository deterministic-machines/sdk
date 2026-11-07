//! The SDK against a controller of the test's own, which answers as dvm.run
//! does: what the scenarios can't make happen.

use std::collections::HashMap;
use std::future::IntoFuture;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use axum::extract::Query;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::{Json, Router};
use dvm_run::api::{
    ApiKey, ByteChunk, CommandLine, CommandNumber, CommandOutputEvent, CommandSignal, CommandState,
    CommandStdin, CreatedApiKey, Me, OutputStream, Run, RunId, RunState, Sandbox, SandboxCommand,
    SandboxDraft, SandboxId, SandboxSpec, StartedCliLogin, Timestamp,
};
use dvm_run::{Client, Error};
use tokio::sync::mpsc;

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
        replay_of: None,
        state,
        runner: Some("runner".to_owned()),
        created_at: epoch(),
        started_at: Some(epoch()),
        ended_at: None,
        log_bytes: None,
        log_outdated: false,
        verdict: None,
        version: 1,
    }
}

fn sandbox(state: RunState) -> Json<Sandbox> {
    let draft = SandboxDraft {
        name: "alpine".to_owned(),
        image: "alpine:3.20".to_owned(),
        ..SandboxDraft::default()
    };
    Json(Sandbox {
        id: SandboxId::new(1),
        owner: None,
        spec: SandboxSpec::try_from(draft).unwrap(),
        run: run(state),
        replays: Vec::new(),
    })
}

fn command(state: CommandState) -> SandboxCommand {
    SandboxCommand {
        number: CommandNumber::new(1),
        command: CommandLine::from("sleep 30"),
        cwd: None,
        env: Default::default(),
        argv: None,
        pid: None,
        stdin: CommandStdin::None,
        state,
        created_at: epoch(),
        started_at: None,
        ended_at: None,
    }
}

/// Server-sent events, each its data.
fn events(events: &[CommandOutputEvent]) -> String {
    let data = events
        .iter()
        .map(|event| serde_json::to_string(event).unwrap());
    data.map(|data| format!("data: {data}\n\n")).collect()
}

fn stdout(offset: u64, text: &str) -> CommandOutputEvent {
    CommandOutputEvent::Output {
        stream: OutputStream::Stdout,
        offset,
        chunk: ByteChunk::new(text.as_bytes().to_vec()),
    }
}

/// A sandbox, running, whose command 1 is `sleep 30`, and whose output is
/// `output`'s.
fn sandbox_with_command(output: Router) -> Router {
    Router::new()
        .route(
            "/api/sandboxes/1",
            get(|| async { sandbox(RunState::Running) }),
        )
        .route(
            "/api/sandboxes/1/commands",
            post(|| async { (StatusCode::CREATED, Json(command(CommandState::Running))) }),
        )
        .merge(output)
}

#[tokio::test]
async fn a_dropped_output_stream_is_followed_again_from_where_it_got_to() {
    let follows = Arc::new(AtomicUsize::new(0));
    let output = get(
        move |Query(query): Query<HashMap<String, String>>| async move {
            match follows.fetch_add(1, Ordering::SeqCst) {
                0 => events(&[stdout(0, "hel")]),
                _ => {
                    assert_eq!(query["stdout_offset"], "3");
                    let exited = CommandState::Exited {
                        code: 0,
                        signal: None,
                    };
                    events(&[
                        stdout(1, "ello"),
                        CommandOutputEvent::Command {
                            command: command(exited),
                        },
                    ])
                }
            }
        },
    );
    let client = controller(sandbox_with_command(
        Router::new().route("/api/sandboxes/1/commands/1/output", output),
    ))
    .await;
    let sbx = client.connect_sandbox(SandboxId::new(1)).await.unwrap();
    let result = sbx.commands().run("sleep 30").await.unwrap();
    assert_eq!((result.stdout.as_str(), result.exit_code), ("hello", 0));
}

#[tokio::test]
async fn a_stream_that_never_ends_its_command_is_an_error() {
    let output = get(|| async { events(&[]) });
    let client = controller(sandbox_with_command(
        Router::new().route("/api/sandboxes/1/commands/1/output", output),
    ))
    .await;
    let sbx = client.connect_sandbox(SandboxId::new(1)).await.unwrap();
    let error = sbx.commands().run("sleep 30").await.unwrap_err();
    assert!(
        error.to_string().contains("ended before its command"),
        "{error}"
    );
}

#[tokio::test]
async fn a_run_dropped_before_its_command_exits_kills_it() {
    let (following, mut followed) = mpsc::unbounded_channel();
    let (signals, mut signalled) = mpsc::unbounded_channel();
    let routes = Router::new()
        .route(
            "/api/sandboxes/1/commands/1/output",
            get(move || async move {
                following.send(()).unwrap();
                std::future::pending::<String>().await
            }),
        )
        .route(
            "/api/sandboxes/1/commands/1/signal",
            post(move |Json(signal): Json<CommandSignal>| async move {
                signals.send(signal.signal).unwrap();
                StatusCode::NO_CONTENT
            }),
        );
    let client = controller(sandbox_with_command(routes)).await;
    let sbx = client.connect_sandbox(SandboxId::new(1)).await.unwrap();
    let running = tokio::spawn(sbx.commands().run("sleep 30").into_future());
    followed.recv().await.unwrap();
    running.abort();
    assert_eq!(signalled.recv().await, Some(9));
}

#[tokio::test]
async fn a_command_refused_as_its_sandbox_failed_says_why() {
    let failed = RunState::Failed {
        reason: "unserved syscall".to_owned(),
    };
    let failed_now = Arc::new(AtomicUsize::new(0));
    let routes = Router::new()
        .route(
            "/api/sandboxes/1",
            get(move || async move {
                match failed_now.fetch_add(1, Ordering::SeqCst) {
                    0 => sandbox(RunState::Running),
                    _ => sandbox(failed),
                }
            }),
        )
        .route(
            "/api/sandboxes/1/commands",
            post(|| async {
                let refusal = serde_json::json!({ "message": "the sandbox isn't running" });
                (StatusCode::CONFLICT, Json(refusal)).into_response()
            }),
        );
    let client = controller(routes).await;
    let sbx = client.connect_sandbox(SandboxId::new(1)).await.unwrap();
    match sbx.commands().run("true").await {
        Err(Error::SandboxFailed { reason, .. }) => assert_eq!(reason, "unserved syscall"),
        other => panic!("{other:?}"),
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
