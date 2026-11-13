use serde::{Deserialize, Serialize};

use crate::{ByteChunk, RunOutput};

/// Appends bytes to `POST /api/sandboxes/{id}/input`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[cfg_attr(feature = "typescript", ts(export_to = "controller-api.ts"))]
pub struct SandboxInput {
    pub offset: u64,
    pub chunk: ByteChunk,
    /// Closes stdin after the chunk; further input is refused.
    pub end: bool,
}

impl SandboxInput {
    /// Maximum decoded bytes in one input request.
    pub const MAX_CHUNK_BYTES: usize = 64 * 1024;
}

/// The input accepted into the runner connection, which may still be in flight.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[cfg_attr(feature = "typescript", ts(export_to = "controller-api.ts"))]
pub struct SandboxInputState {
    pub length: u64,
    pub ended: bool,
}

/// Output after the stdout cursor `offset` and optional `stderr_offset`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[cfg_attr(feature = "typescript", ts(export_to = "controller-api.ts"))]
pub struct OutputAfter {
    #[serde(flatten)]
    pub output: RunOutput,
    /// Whether the run can still write output.
    pub more: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn input_and_incremental_output_round_trip_as_bytes() {
        let input = SandboxInput {
            offset: u64::MAX,
            chunk: ByteChunk::new(vec![0, 255]),
            end: true,
        };
        let encoded = serde_json::to_string(&input).unwrap();
        assert!(encoded.contains("AP8="));
        assert_eq!(
            serde_json::from_str::<SandboxInput>(&encoded).unwrap(),
            input
        );
        let state = SandboxInputState {
            length: 2,
            ended: true,
        };
        assert_eq!(
            serde_json::from_str::<SandboxInputState>(&serde_json::to_string(&state).unwrap())
                .unwrap(),
            state
        );
        let output = OutputAfter {
            output: RunOutput {
                stdout: input.chunk,
                stderr: ByteChunk::default(),
            },
            more: false,
        };
        let encoded = serde_json::to_string(&output).unwrap();
        assert_eq!(encoded, r#"{"stdout":"AP8=","stderr":"","more":false}"#);
        assert_eq!(
            serde_json::from_str::<OutputAfter>(&encoded).unwrap(),
            output
        );
    }
}
