use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json, Router};
use dvm_run::api::{
    ApiKey, CreatedApiKey, Me, Run, RunId, RunState, Sandbox, SandboxDraft, SandboxId, SandboxSpec,
    StartedCliLogin, Timestamp,
};
use dvm_run::{Client, Error};
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
        command: vec!["true".to_owned()],
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
                "/api/executions/1/output",
                get(|| async { Json(serde_json::json!({ "stdout": "/wA=", "stderr": "" })) }),
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
