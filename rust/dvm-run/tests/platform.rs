//! The scenarios (`scenarios/mod.rs`) against dvm.run, or a controller and a
//! runner, at DVM_API_URL with DVM_API_KEY: `cargo test -- --ignored`.
//! dvm-platform's tests run them against a controller and a runner of
//! their own.

mod scenarios;

use dvm_run::Client;

async fn dvm_run_from_environment() -> ((), Client) {
    ((), Client::from_env().expect("DVM_API_URL and DVM_API_KEY"))
}

scenarios::scenarios!(
    dvm_run_from_environment,
    true,
    #[ignore = "needs dvm.run, or a controller and a runner: DVM_API_URL and DVM_API_KEY"]
);
