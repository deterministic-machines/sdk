//! The same program lifecycle exercised by all three SDKs.
use dvm_run::api::RunState;
use dvm_run::{Client, Error};
const IMAGE: &str = "alpine:3.20";

macro_rules! scenarios {
    ($client:path $(, #[$attribute:meta])*) => {
        crate::scenarios::scenarios!(@each $client [$(#[$attribute])*]
            a_program_returns_its_output_and_exit_code,
            entrypoint_and_environment,
            an_image_runs_its_own_command,
            pause_resume_connect_and_stop,
            refusals_are_typed
        );
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

pub async fn an_image_runs_its_own_command(client: &Client) {
    let sandbox = client.create_sandbox(IMAGE).await.unwrap();
    assert_eq!(sandbox.wait().await.unwrap(), 0);
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
