//! The SDK against dvm.run, or a controller and a runner: the Python and JS
//! SDKs' scenarios. `scenarios!` makes a test of each, given an async
//! function that gives a client and what keeps its dvm.run up: this crate's
//! `platform.rs` calls dvm.run at DVM_API_URL with DVM_API_KEY, and
//! dvm-platform's `rust_sdk.rs` includes this file and calls a controller
//! and a runner of its own.
//!
//! dvm-platform runs these against a stand-in sandbox, which runs commands
//! on the host from a directory of its own, so paths in these are relative
//! to it, as they are to alpine's working directory, `/`.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use dvm_run::api::{CommandState, RunState};
use dvm_run::{Client, Error, Sandbox};

const IMAGE: &str = "alpine:3.20";

/// A test of each scenario, against the client `$client().await` gives
/// (with what keeps its dvm.run up), with `$attribute`s.
macro_rules! scenarios {
    ($client:path $(, #[$attribute:meta])*) => {
        crate::scenarios::scenarios!(@each $client [$(#[$attribute])*]
            a_command_returns_its_output_and_exit_code,
            output_comes_to_callbacks_as_it_is_written,
            a_spawned_command_takes_stdin_until_its_close,
            a_killed_command_says_its_signal,
            a_file_goes_in_and_out,
            a_paused_sandbox_takes_no_commands_until_resumed,
            another_process_connects_by_id,
            a_stopped_sandbox_says_it_ended,
            an_execution_replays_the_same,
            refusals_are_typed
        );
    };
    (@each $client:path [$($attribute:tt)*] $scenario:ident $(, $rest:ident)*) => {
        #[tokio::test]
        $($attribute)*
        async fn $scenario() {
            let (_dvm_run, client) = $client().await;
            crate::scenarios::$scenario(&client).await;
        }
        crate::scenarios::scenarios!(@each $client [$($attribute)*] $($rest),*);
    };
    (@each $client:path [$($attribute:tt)*]) => {};
}
pub(crate) use scenarios;

async fn sandbox(client: &Client) -> Sandbox {
    client.create_sandbox(IMAGE).await.unwrap()
}

pub async fn a_command_returns_its_output_and_exit_code(client: &Client) {
    let sbx = sandbox(client).await;
    let script = sbx
        .commands()
        .run("echo out; echo err >&2; exit 3")
        .await
        .unwrap();
    assert_eq!(
        (
            script.stdout.as_str(),
            script.stderr.as_str(),
            script.exit_code,
            script.signal
        ),
        ("out\n", "err\n", 3, None)
    );
    let argv = sbx.commands().run(["echo", "a  b"]).await.unwrap();
    assert_eq!(argv.stdout, "a  b\n");
    let variable = sbx.commands().run("echo $A").env("A", "1").await.unwrap();
    assert_eq!(variable.stdout, "1\n");
    sbx.stop().await.unwrap();
}

pub async fn output_comes_to_callbacks_as_it_is_written(client: &Client) {
    let sbx = sandbox(client).await;
    let pieces = Arc::new(Mutex::new(String::new()));
    let taken = Arc::clone(&pieces);
    let counted = sbx
        .commands()
        .run("for i in 1 2 3; do echo $i; sleep 0.02; done")
        .on_stdout(move |text| taken.lock().unwrap().push_str(text))
        .await
        .unwrap();
    assert_eq!(*pieces.lock().unwrap(), "1\n2\n3\n");
    assert_eq!(counted.stdout, "1\n2\n3\n");
    sbx.stop().await.unwrap();
}

pub async fn a_spawned_command_takes_stdin_until_its_close(client: &Client) {
    let sbx = sandbox(client).await;
    let cat = sbx.commands().spawn(["cat"]).open_stdin().await.unwrap();
    cat.send_stdin(b"one\n".to_vec()).await.unwrap();
    cat.send_stdin("two\n").await.unwrap();
    cat.close_stdin().await.unwrap();
    assert_eq!(cat.wait().await.unwrap().stdout, "one\ntwo\n");
    let exited = cat.info().await.unwrap().state;
    assert!(matches!(exited, CommandState::Exited { .. }), "{exited:?}");
    let piped = sbx.commands().run(["cat"]).stdin("piped").await.unwrap();
    assert_eq!(piped.stdout, "piped");
    sbx.stop().await.unwrap();
}

pub async fn a_killed_command_says_its_signal(client: &Client) {
    let sbx = sandbox(client).await;
    let sleeper = sbx.commands().spawn(["sleep", "30"]).await.unwrap();
    sleeper.kill().await.unwrap();
    let killed = sleeper.wait().await.unwrap();
    assert_eq!((killed.exit_code, killed.signal), (137, Some(9)));
    let timed_out = sbx
        .commands()
        .run(["sleep", "30"])
        .timeout(Duration::from_millis(200))
        .await
        .unwrap();
    assert_eq!((timed_out.exit_code, timed_out.signal), (137, Some(9)));
    sbx.stop().await.unwrap();
}

pub async fn a_file_goes_in_and_out(client: &Client) {
    let sbx = sandbox(client).await;
    let data: Vec<u8> = (0..=255).collect();
    sbx.files().write("/data.bin", data.clone()).await.unwrap();
    assert_eq!(sbx.files().read("/data.bin").await.unwrap(), data);
    let script = "#!/bin/sh\necho ran\n";
    sbx.files()
        .write_with_mode("/run.sh", script, 0o755)
        .await
        .unwrap();
    assert_eq!(
        sbx.commands().run("./run.sh").await.unwrap().stdout,
        "ran\n"
    );
    assert_eq!(sbx.files().read_text("/run.sh").await.unwrap(), script);
    let missing = sbx.files().read("/missing").await.unwrap_err();
    assert!(matches!(missing, Error::NotFound { .. }), "{missing}");
    sbx.stop().await.unwrap();
}

pub async fn a_paused_sandbox_takes_no_commands_until_resumed(client: &Client) {
    let sbx = sandbox(client).await;
    sbx.pause().await.unwrap();
    assert_eq!(sbx.state().await.unwrap(), RunState::Paused);
    let refused = sbx.commands().run("true").await.unwrap_err();
    assert!(matches!(refused, Error::Conflict { .. }), "{refused}");
    sbx.resume().await.unwrap();
    assert_eq!(
        sbx.commands().run("echo back").await.unwrap().stdout,
        "back\n"
    );
    sbx.stop().await.unwrap();
}

pub async fn another_process_connects_by_id(client: &Client) {
    let sbx = sandbox(client).await;
    let connected = client.connect_sandbox(sbx.id()).await.unwrap();
    assert_eq!(
        connected.commands().run("echo here").await.unwrap().stdout,
        "here\n"
    );
    sbx.stop().await.unwrap();
}

pub async fn a_stopped_sandbox_says_it_ended(client: &Client) {
    let sbx = client.create_sandbox(IMAGE).name("stopped").await.unwrap();
    assert_eq!(sbx.name(), "stopped");
    sbx.stop().await.unwrap();
    sbx.stop().await.unwrap();
    match sbx.commands().run("true").await {
        Err(Error::SandboxEnded { state, .. }) => assert_eq!(state, RunState::Stopped),
        other => panic!("{other:?}"),
    }
}

pub async fn an_execution_replays_the_same(client: &Client) {
    let sbx = client.create_sandbox(IMAGE).seed(7).await.unwrap();
    sbx.files().write("/note.txt", "hello\n").await.unwrap();
    assert_eq!(
        sbx.commands().run("cat note.txt").await.unwrap().stdout,
        "hello\n"
    );
    assert_eq!(sbx.commands().run("exit 2").await.unwrap().exit_code, 2);
    sbx.stop().await.unwrap();

    let execution = client.execution(sbx.id()).await.unwrap();
    assert_eq!(*execution.state(), RunState::Stopped);
    assert_eq!(
        (execution.spec().seed(), execution.spec().name()),
        (7, "alpine")
    );
    assert!(execution.log_bytes().is_some());
    let commands: Vec<_> = execution
        .commands()
        .iter()
        .map(|command| (command.number.get(), command.state.clone()))
        .collect();
    let exited = |code| CommandState::Exited { code, signal: None };
    assert_eq!(commands, [(1, exited(0)), (2, exited(2))]);
    let mut replay = execution.replay().await.unwrap();
    assert!(replay.wait().await.unwrap().is_same());
}

pub async fn refusals_are_typed(client: &Client) {
    let image = client.create_sandbox("Not An Image").await.unwrap_err();
    assert!(matches!(image, Error::InvalidRequest { .. }), "{image}");
    let stranger = Client::new(client.api_url(), "dvm_not-a-key");
    let key = stranger.create_sandbox(IMAGE).await.unwrap_err();
    assert!(matches!(key, Error::Authentication { .. }), "{key}");
}
