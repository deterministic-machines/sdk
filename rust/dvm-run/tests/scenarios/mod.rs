//! The same program lifecycle exercised by all three SDKs.
use dvm_run::api::RunState;
use dvm_run::{Client, Error};
const IMAGE: &str = "alpine:3.20";

macro_rules! scenarios {
    ($client:path, $replay:expr $(, #[$attribute:meta])*) => {
        crate::scenarios::scenarios!(@each $client [$(#[$attribute])*]
            a_program_returns_its_output_and_exit_code,
            entrypoint_and_environment,
            pause_resume_connect_and_stop,
            refusals_are_typed,
            new_methods_refuse_a_sandbox_with_a_command
        );
        #[tokio::test]
        $(#[$attribute])*
        async fn a_command_session_ends_with_a_whole_recording() {
            let (_platform, client) = $client().await;
            crate::scenarios::a_command_session_ends_with_a_whole_recording(&client, $replay).await;
        }
    };
    (@each $client:path [$($attribute:tt)*] $scenario:ident $(, $rest:ident)*) => {
        #[tokio::test]
        $($attribute)*
        async fn $scenario() {
            let (_platform, client) = $client().await;
            crate::scenarios::$scenario(&client).await;
        }
        crate::scenarios::scenarios!(@each $client [$($attribute)*] $($rest),*);
    };
    (@each $client:path [$($attribute:tt)*]) => {};
}
pub(crate) use scenarios;

pub async fn a_program_returns_its_output_and_exit_code(client: &Client) {
    let sandbox = client
        .create_sandbox(IMAGE)
        .command(["sh", "-c", "printf 'out\\377'; echo err >&2; exit 3"])
        .seed(7)
        .await
        .unwrap();
    assert_eq!(sandbox.wait().await.unwrap(), 3);
    let output = sandbox.output().await.unwrap();
    assert_eq!(output.stdout.bytes(), b"out\xfferr\n");
    assert_eq!(output.stderr.bytes(), b"");
    let execution = client.execution(sandbox.id()).await.unwrap();
    assert_eq!(execution.state(), &RunState::Exited { code: 3 });
    assert_eq!(
        (execution.spec().seed(), execution.spec().name()),
        (7, "alpine")
    );
    assert!(execution.log_bytes().is_some());
    let mut replay = execution.replay().await.unwrap();
    assert!(replay.wait().await.unwrap().is_same());
    assert_eq!(replay.output().await.unwrap(), output);
    sandbox.stop().await.unwrap();
}

pub async fn entrypoint_and_environment(client: &Client) {
    let sandbox = client
        .create_sandbox(IMAGE)
        .entrypoint("sh")
        .command(["-c", "printf '%s' \"$A\""])
        .env("A", "a  b")
        .await
        .unwrap();
    assert_eq!(sandbox.wait().await.unwrap(), 0);
    assert_eq!(sandbox.output().await.unwrap().stdout.bytes(), b"a  b");
}

pub async fn pause_resume_connect_and_stop(client: &Client) {
    let sandbox = client
        .create_sandbox(IMAGE)
        .command(["sh", "-c", "while :; do :; done"])
        .name("stopped")
        .await
        .unwrap();
    let connected = client.connect_sandbox(sandbox.id()).await.unwrap();
    connected.pause().await.unwrap();
    assert_eq!(sandbox.state().await.unwrap(), RunState::Paused);
    connected.resume().await.unwrap();
    assert_eq!(sandbox.state().await.unwrap(), RunState::Running);
    connected.stop().await.unwrap();
    connected.stop().await.unwrap();
    assert!(matches!(
        sandbox.wait().await,
        Err(Error::SandboxEnded {
            state: RunState::Stopped,
            ..
        })
    ));
    assert!(matches!(
        client.execution(sandbox.id()).await.unwrap().replay().await,
        Err(Error::Conflict { .. })
    ));
}

pub async fn refusals_are_typed(client: &Client) {
    assert!(matches!(
        client.create_sandbox("Not An Image").await,
        Err(Error::InvalidRequest { .. })
    ));
    let stranger = Client::new(client.api_url(), "dvm_not-a-key");
    assert!(matches!(
        stranger.create_sandbox(IMAGE).await,
        Err(Error::Authentication { .. })
    ));
}

pub async fn a_command_session_ends_with_a_whole_recording(client: &Client, replay: bool) {
    let sandbox = client
        .create_sandbox(IMAGE)
        .env("A", "inherited")
        .await
        .unwrap();
    let id = sandbox.id();
    let path = format!("/tmp/dvm-rust-{id}");
    sandbox
        .scope(async |sandbox| {
            let result = sandbox
                .run(["sh", "-c", "printf '%s ' \"$A\"; pwd; printf err >&2"])
                .env("A", "command")
                .cwd("/tmp")
                .await?;
            assert_eq!(
                (result.exit_code, result.stdout, result.stderr),
                (0, b"command /tmp\n".to_vec(), b"err".to_vec())
            );
            assert_eq!(sandbox.run("exit 3").await?.exit_code, 3);
            assert_eq!(
                sandbox.run("printf '%s' \"$A\"").await?.stdout,
                b"inherited"
            );
            assert_eq!(
                sandbox.run(["cat"]).stdin(b"hi\x00\xff\n").await?.stdout,
                b"hi\x00\xff\n"
            );
            let data: Vec<u8> = (0..102400).map(|i| (i % 256) as u8).collect();
            sandbox.write_file(path.clone(), &data).mode(0o600).await?;
            assert_eq!(sandbox.read_file(path.clone()).await?, data);
            let server = sandbox.start(["sh", "-c", "while :; do :; done"]).await?;
            assert_eq!(
                sandbox.run("printf concurrent").await?.stdout,
                b"concurrent"
            );
            server.signal(15).await?;
            assert_eq!(server.wait().await?.exit_code, 143);
            let cat = sandbox.start(["cat"]).await?;
            cat.write_stdin(b"from the handle").await?;
            cat.close_stdin().await?;
            assert_eq!(cat.wait().await?.stdout, b"from the handle");
            sandbox.run(["rm", &path]).await?;
            assert!(
                String::from_utf8_lossy(sandbox.output().await?.stdout.bytes())
                    .contains("\"started\"")
            );
            let connected = client.connect_sandbox(id).await?;
            assert_eq!(
                connected.run("printf connected").await?.stdout,
                b"connected"
            );
            Ok(())
        })
        .await
        .unwrap();
    let execution = client.execution(id).await.unwrap();
    assert_eq!(execution.state(), &RunState::Exited { code: 0 });
    if replay {
        assert!(
            execution
                .replay()
                .await
                .unwrap()
                .wait()
                .await
                .unwrap()
                .is_same()
        );
    }
}

pub async fn new_methods_refuse_a_sandbox_with_a_command(client: &Client) {
    let sandbox = client
        .create_sandbox(IMAGE)
        .command(["true"])
        .await
        .unwrap();
    sandbox
        .scope(async |sandbox| {
            assert!(matches!(
                sandbox.run("true").await,
                Err(Error::Conflict { .. })
            ));
            assert!(matches!(
                sandbox.start("true").await,
                Err(Error::Conflict { .. })
            ));
            assert!(matches!(
                sandbox.write_file("/tmp/a", b"hi").await,
                Err(Error::Conflict { .. })
            ));
            assert!(matches!(
                sandbox.read_file("/tmp/a").await,
                Err(Error::Conflict { .. })
            ));
            assert!(matches!(sandbox.end().await, Err(Error::Conflict { .. })));
            Ok(())
        })
        .await
        .unwrap();
}
