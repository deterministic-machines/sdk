use serde::{Deserialize, Serialize};

use crate::{ByteChunk, FrameError, RunOutput, decode_frame, encode_frame};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[cfg_attr(feature = "typescript", ts(export_to = "controller-api.ts"))]
pub struct SandboxInput {
    pub offset: u64,
    #[serde(default)]
    pub end: bool,
}

impl SandboxInput {
    pub const MAX_CHUNK_BYTES: usize = 64 * 1024;
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[cfg_attr(feature = "typescript", ts(export_to = "controller-api.ts"))]
pub struct SandboxInputState {
    pub length: u64,
    pub ended: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OutputAfter {
    pub output: RunOutput,
    pub more: bool,
    pub cut: bool,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct OutputAfterHeader {
    stdout: u64,
    stderr: u64,
    more: bool,
    cut: bool,
}

impl OutputAfter {
    pub fn to_frame(&self) -> Vec<u8> {
        let (stdout, stderr) = (self.output.stdout.bytes(), self.output.stderr.bytes());
        let header = OutputAfterHeader {
            stdout: stdout.len() as u64,
            stderr: stderr.len() as u64,
            more: self.more,
            cut: self.cut,
        };
        encode_frame(&header, &[stdout, stderr].concat())
    }

    pub fn from_frame(frame: &[u8]) -> Result<Self, FrameError> {
        let frame = decode_frame::<OutputAfterHeader>(frame)?;
        let header = frame.header;
        let mut stdout = frame.payload;
        if header.stdout.checked_add(header.stderr) != Some(stdout.len() as u64) {
            return Err(FrameError::BadHeader(format!(
                "{} and {} bytes of output in a frame of {}",
                header.stdout,
                header.stderr,
                stdout.len()
            )));
        }
        let stderr = stdout.split_off(header.stdout as usize);
        Ok(Self {
            output: RunOutput {
                stdout: ByteChunk::new(stdout),
                stderr: ByteChunk::new(stderr),
            },
            more: header.more,
            cut: header.cut,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn input_is_a_query_and_output_a_frame_of_raw_bytes() {
        let input: SandboxInput = serde_json::from_str(r#"{"offset":7}"#).unwrap();
        assert_eq!(
            input,
            SandboxInput {
                offset: 7,
                end: false
            }
        );
        let output = OutputAfter {
            output: RunOutput {
                stdout: ByteChunk::new(vec![0, 255]),
                stderr: ByteChunk::new(b"err".to_vec()),
            },
            more: false,
            cut: true,
        };
        let frame = output.to_frame();
        assert_eq!(
            frame,
            b"{\"stdout\":2,\"stderr\":3,\"more\":false,\"cut\":true,\"bytes\":5}\n\x00\xfferr"
        );
        assert_eq!(OutputAfter::from_frame(&frame).unwrap(), output);
        let short = b"{\"stdout\":9,\"stderr\":0,\"more\":true,\"cut\":false,\"bytes\":2}\nhi";
        assert!(matches!(
            OutputAfter::from_frame(short),
            Err(FrameError::BadHeader(_))
        ));
    }
}
