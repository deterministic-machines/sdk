//! dvm.run's REST API: what it takes and returns. dvm.run's controller
//! serves it, and the SDKs and its panel call it; the controller, the panel
//! and the Rust SDK check a sandbox's spec with the same code.
//!
//! A type that holds an invariant (a checked spec, an image reference) keeps
//! its fields private; a plain record of what the controller answers has
//! them public.

mod api_key;
mod capacity;
mod cli_login;
mod execution;
mod image_reference;
mod run;
mod sandbox;
mod timestamp;
mod user;

pub use api_key::{ApiKey, CreatedApiKey, NewApiKey};
pub use capacity::Capacity;
pub use cli_login::{CliLogin, CollectCliLogin, NewCliLogin, StartedCliLogin};
pub use execution::Execution;
pub use image_reference::{ImageReference, InvalidImageReference};
pub use run::{
    ByteChunk, InvalidRunAction, OutputStream, ReplayVerdict, Run, RunAction, RunId, RunOutput,
    RunState,
};
pub use sandbox::{
    EnvironmentVariable, Sandbox, SandboxDraft, SandboxId, SandboxSpec, SandboxesPage, SpecError,
    SpecField,
};
pub use timestamp::Timestamp;
pub use user::{Me, UserLimits};

use serde::{Deserialize, Serialize};

/// What the controller answers instead, when it refuses a request: the body
/// of every response that isn't a success.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[cfg_attr(feature = "typescript", ts(export_to = "controller-api.ts"))]
pub struct ApiError {
    pub message: String,
}

/// Writes every type's TypeScript, with the types it names, into
/// `controller-api.ts` in `config`'s directory: the panel's API.
#[cfg(feature = "typescript")]
pub fn export_typescript(config: &ts_rs::Config) -> Result<(), ts_rs::ExportError> {
    use ts_rs::TS;
    ApiError::export_all(config)?;
    ApiKey::export_all(config)?;
    ByteChunk::export_all(config)?;
    Capacity::export_all(config)?;
    CliLogin::export_all(config)?;
    CreatedApiKey::export_all(config)?;
    EnvironmentVariable::export_all(config)?;
    Execution::export_all(config)?;
    ImageReference::export_all(config)?;
    Me::export_all(config)?;
    NewApiKey::export_all(config)?;
    OutputStream::export_all(config)?;
    ReplayVerdict::export_all(config)?;
    Run::export_all(config)?;
    RunAction::export_all(config)?;
    RunId::export_all(config)?;
    RunOutput::export_all(config)?;
    RunState::export_all(config)?;
    Sandbox::export_all(config)?;
    SandboxDraft::export_all(config)?;
    SandboxId::export_all(config)?;
    SandboxSpec::export_all(config)?;
    SandboxesPage::export_all(config)?;
    Timestamp::export_all(config)?;
    UserLimits::export_all(config)?;
    Ok(())
}

#[cfg(all(test, feature = "typescript"))]
mod typescript_tests {
    #[test]
    fn every_type_is_written_to_one_file() {
        let directory = std::env::temp_dir().join(format!("dvm-api-{}", std::process::id()));
        let config = ts_rs::Config::from_env().with_out_dir(&directory);
        super::export_typescript(&config).unwrap();
        let written = std::fs::read_to_string(directory.join("controller-api.ts")).unwrap();
        std::fs::remove_dir_all(&directory).unwrap();
        for declaration in [
            "export type Sandbox = ",
            "export type ApiError = ",
            "export type Me = ",
        ] {
            assert!(written.contains(declaration), "{declaration}");
        }
    }
}
