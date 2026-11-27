//! dvm.run's REST API types and checked request drafts.

mod api_key;
mod capacity;
mod cli_login;
mod frame;
mod guest_files;
mod image_reference;
mod inspection;
mod run;
mod sandbox;
mod sandbox_input;
mod sandbox_network;
mod sandbox_protocol;
mod timestamp;
mod user;

pub use api_key::{ApiKey, CreatedApiKey, NewApiKey};
pub use capacity::Capacity;
pub use cli_login::{CliLogin, CollectCliLogin, NewCliLogin, StartedCliLogin};
pub use frame::{
    Frame, FrameDecoder, FrameError, MAX_HEADER, MAX_PAYLOAD, decode_frame, encode_frame,
};
pub use guest_files::{FileName, FileStatus, FileTime, FileType, PathDirectoryEntry};
pub use image_reference::{ImageReference, InvalidImageReference};
pub use inspection::{
    DecodedArgument, DecodedValue, DvmStop, InspectionWatches, SeekSandbox, SeekTarget, StepPoint,
    StopEvent, SyscallName, WatchPoint, WatchPoints,
};
pub use run::{
    ByteChunk, InvalidRunAction, OutputStream, Run, RunAction, RunId, RunOutput, RunState,
};
pub use sandbox::{
    EnvironmentVariable, Sandbox, SandboxDraft, SandboxId, SandboxSpec, SandboxesPage, SpecError,
    SpecField,
};
pub use sandbox_input::{OutputAfter, SandboxInput, SandboxInputState};
pub use sandbox_network::SandboxNetwork;
pub use sandbox_protocol::{
    SandboxAnswer, SandboxAnswerKind, SandboxExited, SandboxProgram, SandboxRead, SandboxRequest,
    SandboxRequestAction, SandboxRun, SandboxStarted, SandboxStdin, SandboxWrite,
};
pub use timestamp::Timestamp;
pub use user::{Me, UserLimits};

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[cfg_attr(feature = "typescript", ts(export_to = "controller-api.ts"))]
pub struct ApiError {
    pub message: String,
}

#[cfg(feature = "typescript")]
pub fn export_typescript(config: &ts_rs::Config) -> Result<(), ts_rs::ExportError> {
    use ts_rs::TS;
    ApiError::export_all(config)?;
    ApiKey::export_all(config)?;
    ByteChunk::export_all(config)?;
    Capacity::export_all(config)?;
    CliLogin::export_all(config)?;
    CreatedApiKey::export_all(config)?;
    DecodedArgument::export_all(config)?;
    DecodedValue::export_all(config)?;
    DvmStop::export_all(config)?;
    EnvironmentVariable::export_all(config)?;
    FileName::export_all(config)?;
    FileStatus::export_all(config)?;
    FileTime::export_all(config)?;
    FileType::export_all(config)?;
    ImageReference::export_all(config)?;
    InspectionWatches::export_all(config)?;
    Me::export_all(config)?;
    NewApiKey::export_all(config)?;
    OutputStream::export_all(config)?;
    PathDirectoryEntry::export_all(config)?;
    Run::export_all(config)?;
    RunAction::export_all(config)?;
    RunId::export_all(config)?;
    RunOutput::export_all(config)?;
    RunState::export_all(config)?;
    Sandbox::export_all(config)?;
    SandboxAnswer::export_all(config)?;
    SandboxDraft::export_all(config)?;
    SandboxId::export_all(config)?;
    SandboxInput::export_all(config)?;
    SandboxInputState::export_all(config)?;
    SandboxNetwork::export_all(config)?;
    SandboxRequest::export_all(config)?;
    SandboxSpec::export_all(config)?;
    SandboxesPage::export_all(config)?;
    SeekSandbox::export_all(config)?;
    SeekTarget::export_all(config)?;
    StopEvent::export_all(config)?;
    SyscallName::export_all(config)?;
    Timestamp::export_all(config)?;
    UserLimits::export_all(config)?;
    WatchPoint::export_all(config)?;
    WatchPoints::export_all(config)?;
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
            "export type DvmStop = ",
            "export type FileStatus = ",
            "export type ApiError = ",
            "export type Me = ",
            "export type SandboxRequest = ",
            "export type SandboxAnswer = ",
        ] {
            assert!(written.contains(declaration), "{declaration}");
        }
        for (name, fields) in [
            ("SandboxRun", &["env", "cwd", "stdin", "user"][..]),
            ("SandboxStdin", &["close"][..]),
            ("SandboxWrite", &["mode", "last"][..]),
            ("SandboxAnswer", &["id"][..]),
        ] {
            let declaration = written
                .split_once(&format!("export type {name} = "))
                .unwrap()
                .1
                .split(';')
                .next()
                .unwrap();
            for field in fields {
                assert!(
                    declaration.contains(&format!("{field}?:")),
                    "{name}.{field}: {declaration}"
                );
            }
        }
    }
}
