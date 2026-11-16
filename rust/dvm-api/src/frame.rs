//! A frame: a JSON object on one line, then the raw bytes it counts in
//! `"bytes"`. The sandbox server's requests and answers, the controller's
//! output answers and its messages with runners are frames, so bytes cross
//! every hop as they are, and only their headers are JSON.

use std::io::Write;
use std::ops::Range;

use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::{Map, Value};

/// The header's field that counts the frame's bytes; absent for none.
const BYTES: &str = "bytes";

/// The longest header, its newline excluded.
pub const MAX_HEADER: usize = 64 * 1024;

/// The most bytes one frame carries: a decoder holds a whole frame.
pub const MAX_PAYLOAD: u64 = 16 * 1024 * 1024;

/// A header and its bytes, and where the frame lies in its stream.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Frame<H> {
    pub header: H,
    pub payload: Vec<u8>,
    pub span: Range<u64>,
}

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum FrameError {
    #[error("a frame's header is longer than {MAX_HEADER} bytes")]
    LongHeader,
    #[error("a frame counts {0} bytes, more than {MAX_PAYLOAD}")]
    LongPayload(u64),
    #[error("a frame's header: {0}")]
    BadHeader(String),
    #[error("the frame ends early")]
    Cut,
    #[error("{0} bytes follow the frame")]
    Trailing(usize),
}

/// The frame of `header`, a struct or map, and `payload`.
pub fn encode_frame<H: Serialize>(header: &H, payload: &[u8]) -> Vec<u8> {
    let mut frame = serde_json::to_vec(header).expect("a frame's header serializes");
    assert!(
        frame.first() == Some(&b'{') && frame.last() == Some(&b'}'),
        "a frame's header is a JSON object"
    );
    if !payload.is_empty() {
        frame.pop();
        let comma = if frame.len() > 1 { "," } else { "" };
        write!(frame, "{comma}\"{BYTES}\":{}}}", payload.len()).unwrap();
    }
    frame.push(b'\n');
    frame.extend_from_slice(payload);
    frame
}

/// `bytes`, whole, as one frame.
pub fn decode_frame<H: DeserializeOwned>(bytes: &[u8]) -> Result<Frame<H>, FrameError> {
    let mut decoder = FrameDecoder::default();
    decoder.push(bytes);
    let frame = decoder.next_frame().ok_or(FrameError::Cut)??;
    match decoder.buffered() {
        0 => Ok(frame),
        trailing => Err(FrameError::Trailing(trailing)),
    }
}

/// Splits a stream, given as it comes, into frames. A header that isn't one
/// is an error for its frame alone; one too long to hold ends the stream,
/// which can't be split past it.
#[derive(Debug, Default)]
pub struct FrameDecoder {
    pending: Vec<u8>,
    /// Where in `pending` the next frame starts.
    start: usize,
    /// The stream's offset of `pending`'s first byte.
    offset: u64,
    ended: Option<FrameError>,
}

impl FrameDecoder {
    pub fn push(&mut self, bytes: &[u8]) {
        if self.start > self.pending.len() / 2 {
            self.pending.drain(..self.start);
            self.offset += self.start as u64;
            self.start = 0;
        }
        self.pending.extend_from_slice(bytes);
    }

    /// Bytes pushed but not yet taken as frames.
    pub fn buffered(&self) -> usize {
        self.pending.len() - self.start
    }

    /// The next whole frame, if it has come.
    pub fn next_frame<H: DeserializeOwned>(&mut self) -> Option<Result<Frame<H>, FrameError>> {
        if let Some(ended) = &self.ended {
            return Some(Err(ended.clone()));
        }
        let rest = &self.pending[self.start..];
        let Some(newline) = rest.iter().take(MAX_HEADER + 1).position(|&b| b == b'\n') else {
            return (rest.len() > MAX_HEADER).then(|| Err(self.end(FrameError::LongHeader)));
        };
        let fields = serde_json::from_slice::<Map<String, Value>>(&rest[..newline]);
        let mut fields = match fields {
            Ok(fields) => fields,
            Err(error) => {
                self.take(newline + 1);
                return Some(Err(FrameError::BadHeader(error.to_string())));
            }
        };
        let length = match fields.remove(BYTES) {
            None => 0,
            Some(count) => match count.as_u64() {
                Some(length) if length <= MAX_PAYLOAD => length,
                Some(length) => return Some(Err(self.end(FrameError::LongPayload(length)))),
                None => {
                    self.take(newline + 1);
                    return Some(Err(FrameError::BadHeader(format!("bytes: {count}"))));
                }
            },
        };
        let end = newline + 1 + length as usize;
        if rest.len() < end {
            return None;
        }
        let payload = rest[newline + 1..end].to_vec();
        let header = serde_json::from_value(Value::Object(fields));
        let span = self.take(end);
        Some(
            header
                .map(|header| Frame {
                    header,
                    payload,
                    span,
                })
                .map_err(|error| FrameError::BadHeader(error.to_string())),
        )
    }

    fn take(&mut self, length: usize) -> Range<u64> {
        let from = self.offset + self.start as u64;
        self.start += length;
        from..from + length as u64
    }

    fn end(&mut self, error: FrameError) -> FrameError {
        self.ended = Some(error.clone());
        error
    }
}

#[cfg(test)]
mod tests {
    use serde::Deserialize;

    use super::*;

    #[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Note {
        id: u64,
    }

    fn frames(decoder: &mut FrameDecoder) -> Vec<Result<Frame<Note>, FrameError>> {
        std::iter::from_fn(|| decoder.next_frame()).collect()
    }

    #[test]
    fn frames_carry_raw_bytes_and_split_wherever_the_stream_is_cut() {
        let mut stream = encode_frame(&Note { id: 1 }, b"\n{\"id\":9}\n\xff");
        stream.extend(encode_frame(&Note { id: 2 }, b""));
        assert_eq!(&stream[..23], b"{\"id\":1,\"bytes\":11}\n\n{\"");
        for cut in 0..stream.len() {
            let mut decoder = FrameDecoder::default();
            decoder.push(&stream[..cut]);
            let mut taken = frames(&mut decoder);
            decoder.push(&stream[cut..]);
            taken.extend(frames(&mut decoder));
            assert_eq!(
                taken,
                [
                    Ok(Frame {
                        header: Note { id: 1 },
                        payload: b"\n{\"id\":9}\n\xff".to_vec(),
                        span: 0..31,
                    }),
                    Ok(Frame {
                        header: Note { id: 2 },
                        payload: Vec::new(),
                        span: 31..40,
                    }),
                ],
                "cut at {cut}"
            );
            assert_eq!(decoder.buffered(), 0);
        }
    }

    #[test]
    fn a_bad_header_costs_its_frame_and_a_long_one_ends_the_stream() {
        let mut decoder = FrameDecoder::default();
        decoder.push(b"not json\n{\"id\":3,\"typo\":1,\"bytes\":2}\nhi{\"id\":4}\n");
        let taken = frames(&mut decoder);
        assert!(matches!(taken[0], Err(FrameError::BadHeader(_))));
        assert!(matches!(taken[1], Err(FrameError::BadHeader(_))));
        assert_eq!(taken[2].as_ref().unwrap().header, Note { id: 4 });
        assert_eq!(taken.len(), 3);

        decoder.push(format!("{{\"id\":5,\"bytes\":{}}}\n", MAX_PAYLOAD + 1).as_bytes());
        let ended = Err(FrameError::LongPayload(MAX_PAYLOAD + 1));
        assert_eq!(decoder.next_frame::<Note>(), Some(ended.clone()));
        decoder.push(b"{\"id\":6}\n");
        assert_eq!(decoder.next_frame::<Note>(), Some(ended));

        let mut decoder = FrameDecoder::default();
        decoder.push(&vec![b' '; MAX_HEADER]);
        assert_eq!(decoder.next_frame::<Note>(), None);
        decoder.push(b" ");
        assert_eq!(
            decoder.next_frame::<Note>(),
            Some(Err(FrameError::LongHeader))
        );
    }

    #[test]
    fn a_whole_frame_is_one_frame_and_nothing_more() {
        let frame = encode_frame(&Note { id: 1 }, b"hi");
        assert_eq!(decode_frame::<Note>(&frame).unwrap().payload, b"hi");
        assert_eq!(
            decode_frame::<Note>(&frame[..frame.len() - 1]),
            Err(FrameError::Cut)
        );
        let mut two = frame.clone();
        two.extend(&frame);
        assert_eq!(
            decode_frame::<Note>(&two),
            Err(FrameError::Trailing(frame.len()))
        );
    }
}
