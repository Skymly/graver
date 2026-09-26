//! Process boundary between the in-process text service and `graver-service`.
//!
//! Frames are a little-endian `u32` byte length followed by UTF-8 JSON.
//! [`PIPE_NAME`] is the Win32 path. .NET NamedPipeClientStream uses [`PIPE_NAME_SHORT`].
//!

use serde::{Deserialize, Serialize};
use thiserror::Error;

pub const PROTOCOL_VERSION: u32 = 1;
pub const PIPE_NAME: &str = r"\\.\pipe\Graver";
pub const PIPE_NAME_SHORT: &str = "Graver";
pub const MAX_FRAME_LEN: usize = 1024 * 1024;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum ProtocolError {
    #[error("frame is empty")]
    Empty,
    #[error("frame length {0} exceeds {1}")]
    TooLarge(usize, usize),
    #[error("connection closed before the frame finished")]
    UnexpectedEof,
    #[error("invalid json: {0}")]
    Json(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Request {
    Ping { v: u32, id: u64 },
    Reset { v: u32, id: u64 },
    Deactivate { v: u32, id: u64 },
    Key { v: u32, id: u64, key: KeyMessage },
}

impl Request {
    pub fn version(&self) -> u32 {
        match self {
            Self::Ping { v, .. }
            | Self::Reset { v, .. }
            | Self::Deactivate { v, .. }
            | Self::Key { v, .. } => *v,
        }
    }

    pub fn id(&self) -> u64 {
        match self {
            Self::Ping { id, .. }
            | Self::Reset { id, .. }
            | Self::Deactivate { id, .. }
            | Self::Key { id, .. } => *id,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KeyMessage {
    #[serde(flatten)]
    pub kind: KeyKindMessage,
    #[serde(default)]
    pub shift: bool,
    #[serde(default)]
    pub ctrl: bool,
    #[serde(default)]
    pub alt: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum KeyKindMessage {
    Char { ch: char },
    Backspace,
    Escape,
    Space,
    Enter,
    Left,
    Right,
    Up,
    Down,
    Digit { n: u8 },
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CandidateMessage {
    pub text: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub comment: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Response {
    Pong {
        v: u32,
        id: u64,
    },
    Update {
        v: u32,
        id: u64,
        preedit: String,
        candidates: Vec<CandidateMessage>,
        commit: Option<String>,
        consumed: bool,
    },
    Error {
        v: u32,
        id: u64,
        message: String,
    },
}

#[derive(Debug, Default)]
pub struct FrameDecoder {
    buf: Vec<u8>,
}

impl FrameDecoder {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn pending(&self) -> usize {
        self.buf.len()
    }

    pub fn push(&mut self, bytes: &[u8]) -> Result<Vec<Vec<u8>>, ProtocolError> {
        self.buf.extend_from_slice(bytes);
        let mut frames = Vec::new();
        loop {
            if self.buf.len() < 4 {
                break;
            }
            let len = u32::from_le_bytes(self.buf[..4].try_into().expect("4 bytes")) as usize;
            if len == 0 {
                self.buf.clear();
                return Err(ProtocolError::Empty);
            }
            if len > MAX_FRAME_LEN {
                self.buf.clear();
                return Err(ProtocolError::TooLarge(len, MAX_FRAME_LEN));
            }
            if self.buf.len() < 4 + len {
                break;
            }
            let payload = self.buf[4..4 + len].to_vec();
            self.buf.drain(..4 + len);
            frames.push(payload);
        }
        Ok(frames)
    }
}

pub fn encode_frame(payload: &[u8]) -> Result<Vec<u8>, ProtocolError> {
    if payload.is_empty() {
        return Err(ProtocolError::Empty);
    }
    if payload.len() > MAX_FRAME_LEN {
        return Err(ProtocolError::TooLarge(payload.len(), MAX_FRAME_LEN));
    }
    let mut out = Vec::with_capacity(4 + payload.len());
    out.extend_from_slice(&(payload.len() as u32).to_le_bytes());
    out.extend_from_slice(payload);
    Ok(out)
}

pub fn encode_request(request: &Request) -> Result<Vec<u8>, ProtocolError> {
    let payload =
        serde_json::to_vec(request).map_err(|err| ProtocolError::Json(err.to_string()))?;
    encode_frame(&payload)
}

pub fn encode_response(response: &Response) -> Result<Vec<u8>, ProtocolError> {
    let payload =
        serde_json::to_vec(response).map_err(|err| ProtocolError::Json(err.to_string()))?;
    encode_frame(&payload)
}

pub fn decode_request(payload: &[u8]) -> Result<Request, ProtocolError> {
    serde_json::from_slice(payload).map_err(|err| ProtocolError::Json(err.to_string()))
}

pub fn decode_response(payload: &[u8]) -> Result<Response, ProtocolError> {
    serde_json::from_slice(payload).map_err(|err| ProtocolError::Json(err.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pipe_names_match() {
        assert!(PIPE_NAME.ends_with(PIPE_NAME_SHORT));
    }

    #[test]
    fn settings_ping_roundtrips() {
        let payload = br#"{"v":1,"id":1,"op":"ping"}"#;
        let request = decode_request(payload).unwrap();
        assert_eq!(request, Request::Ping { v: 1, id: 1 });

        let response = Response::Pong { v: 1, id: 1 };
        let json = serde_json::to_string(&response).unwrap();
        assert!(json.contains("\"op\":\"pong\""));
    }

    #[test]
    fn frames_can_arrive_in_pieces() {
        let frame = encode_request(&Request::Reset { v: 1, id: 7 }).unwrap();
        let mut decoder = FrameDecoder::new();
        assert!(decoder.push(&frame[..3]).unwrap().is_empty());
        let frames = decoder.push(&frame[3..]).unwrap();
        assert_eq!(frames.len(), 1);
        assert_eq!(decode_request(&frames[0]).unwrap().id(), 7);
        assert_eq!(decoder.pending(), 0);
    }

    #[test]
    fn oversized_frame_is_rejected() {
        let mut decoder = FrameDecoder::new();
        let len = (MAX_FRAME_LEN as u32).saturating_add(1).to_le_bytes();
        let err = decoder.push(&len).unwrap_err();
        assert_eq!(
            err,
            ProtocolError::TooLarge(MAX_FRAME_LEN + 1, MAX_FRAME_LEN)
        );
    }
}
