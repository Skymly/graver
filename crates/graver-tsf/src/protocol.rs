//! Handwritten v1 frames for the in-process text service.
//!
//! This module must not depend on serde, graver-ipc, or Win32. The bytes it
//! emits are the golden request frames `graver-ipc` decodes. Responses are
//! parsed here so a protocol error can be turned into "return the key"
//! without panicking.

use std::fmt::Write as _;

pub const PROTOCOL_VERSION: u32 = 1;
pub const PIPE_NAME: &str = r"\\.\pipe\Graver";
pub const MAX_FRAME_LEN: usize = 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProtocolError {
    Empty,
    TooLarge,
    Invalid,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyKind {
    Char(char),
    Backspace,
    Escape,
    Space,
    Enter,
    Left,
    Right,
    Up,
    Down,
    Digit(u8),
    Other,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KeyRequest {
    pub kind: KeyKind,
    pub shift: bool,
    pub ctrl: bool,
    pub alt: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClientRequest {
    Ping { id: u64 },
    Reset { id: u64 },
    Deactivate { id: u64 },
    Key { id: u64, key: KeyRequest },
}

/// Composition fields the text service acts on. Candidates stay on the wire
/// and are accepted, but this stage does not display them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompositionUpdate {
    pub id: u64,
    pub preedit: String,
    pub commit: Option<String>,
    pub consumed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClientResponse {
    Pong { id: u64 },
    Update(CompositionUpdate),
    Error { id: u64, message: String },
}

pub fn encode_frame(payload: &[u8]) -> Result<Vec<u8>, ProtocolError> {
    if payload.is_empty() {
        return Err(ProtocolError::Empty);
    }
    if payload.len() > MAX_FRAME_LEN {
        return Err(ProtocolError::TooLarge);
    }
    let mut out = Vec::with_capacity(4 + payload.len());
    out.extend_from_slice(&(payload.len() as u32).to_le_bytes());
    out.extend_from_slice(payload);
    Ok(out)
}

pub fn encode_request_json(request: &ClientRequest) -> Vec<u8> {
    let json = match *request {
        ClientRequest::Ping { id } => format!(r#"{{"op":"ping","v":1,"id":{id}}}"#),
        ClientRequest::Reset { id } => format!(r#"{{"op":"reset","v":1,"id":{id}}}"#),
        ClientRequest::Deactivate { id } => format!(r#"{{"op":"deactivate","v":1,"id":{id}}}"#),
        ClientRequest::Key { id, key } => encode_key_json(id, &key),
    };
    json.into_bytes()
}

pub fn encode_request_frame(request: &ClientRequest) -> Result<Vec<u8>, ProtocolError> {
    encode_frame(&encode_request_json(request))
}

pub fn parse_response(payload: &[u8]) -> Result<ClientResponse, ProtocolError> {
    let text = std::str::from_utf8(payload).map_err(|_| ProtocolError::Invalid)?;
    let value = Parser::parse(text)?;
    let op = value.field("op")?.as_str()?;
    let version = value.field("v")?.as_u64()?;
    if version != u64::from(PROTOCOL_VERSION) {
        return Err(ProtocolError::Invalid);
    }
    let id = value.field("id")?.as_u64()?;
    match op {
        "pong" => Ok(ClientResponse::Pong { id }),
        "error" => Ok(ClientResponse::Error {
            id,
            message: value.field("message")?.as_str()?.to_owned(),
        }),
        "update" => {
            // Parse candidates so a malformed list is a protocol error, then drop them.
            let _candidates = value.field("candidates")?.as_array()?;
            let commit = match value.field("commit")? {
                Json::Null => None,
                other => Some(other.as_str()?.to_owned()),
            };
            Ok(ClientResponse::Update(CompositionUpdate {
                id,
                preedit: value.field("preedit")?.as_str()?.to_owned(),
                commit,
                consumed: value.field("consumed")?.as_bool()?,
            }))
        }
        _ => Err(ProtocolError::Invalid),
    }
}

fn encode_key_json(id: u64, key: &KeyRequest) -> String {
    let mut out = format!(r#"{{"op":"key","v":1,"id":{id},"key":{{"kind":"#);
    match key.kind {
        KeyKind::Char(ch) => {
            out.push_str(r#""char","ch":"#);
            push_json_string(&mut out, &ch.to_string());
        }
        KeyKind::Backspace => out.push_str(r#""backspace""#),
        KeyKind::Escape => out.push_str(r#""escape""#),
        KeyKind::Space => out.push_str(r#""space""#),
        KeyKind::Enter => out.push_str(r#""enter""#),
        KeyKind::Left => out.push_str(r#""left""#),
        KeyKind::Right => out.push_str(r#""right""#),
        KeyKind::Up => out.push_str(r#""up""#),
        KeyKind::Down => out.push_str(r#""down""#),
        KeyKind::Digit(n) => {
            out.push_str(r#""digit","n":"#);
            let _ = write!(out, "{n}");
        }
        KeyKind::Other => out.push_str(r#""other""#),
    }
    out.push_str(r#","shift":"#);
    out.push_str(bool_json(key.shift));
    out.push_str(r#","ctrl":"#);
    out.push_str(bool_json(key.ctrl));
    out.push_str(r#","alt":"#);
    out.push_str(bool_json(key.alt));
    out.push_str("}}");
    out
}

fn bool_json(value: bool) -> &'static str {
    if value { "true" } else { "false" }
}

fn push_json_string(out: &mut String, value: &str) {
    out.push('"');
    for ch in value.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\u{0008}' => out.push_str("\\b"),
            '\u{000c}' => out.push_str("\\f"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            ch if (ch as u32) < 0x20 => {
                let _ = write!(out, "\\u{:04x}", ch as u32);
            }
            ch => out.push(ch),
        }
    }
    out.push('"');
}

#[derive(Debug, Clone, PartialEq)]
enum Json {
    Null,
    Bool(bool),
    Number(u64),
    String(String),
    Array(Vec<Json>),
    Object(Vec<(String, Json)>),
}

impl Json {
    fn field(&self, name: &str) -> Result<&Json, ProtocolError> {
        match self {
            Self::Object(fields) => fields
                .iter()
                .find(|(key, _)| key == name)
                .map(|(_, value)| value)
                .ok_or(ProtocolError::Invalid),
            _ => Err(ProtocolError::Invalid),
        }
    }

    fn as_str(&self) -> Result<&str, ProtocolError> {
        match self {
            Self::String(value) => Ok(value),
            _ => Err(ProtocolError::Invalid),
        }
    }

    fn as_bool(&self) -> Result<bool, ProtocolError> {
        match self {
            Self::Bool(value) => Ok(*value),
            _ => Err(ProtocolError::Invalid),
        }
    }

    fn as_u64(&self) -> Result<u64, ProtocolError> {
        match self {
            Self::Number(value) => Ok(*value),
            _ => Err(ProtocolError::Invalid),
        }
    }

    fn as_array(&self) -> Result<&[Json], ProtocolError> {
        match self {
            Self::Array(value) => Ok(value),
            _ => Err(ProtocolError::Invalid),
        }
    }
}

struct Parser<'a> {
    text: &'a str,
    i: usize,
}

impl<'a> Parser<'a> {
    fn parse(text: &'a str) -> Result<Json, ProtocolError> {
        let mut parser = Self { text, i: 0 };
        parser.skip();
        let value = parser.value(0)?;
        parser.skip();
        if parser.i != parser.text.len() {
            return Err(ProtocolError::Invalid);
        }
        Ok(value)
    }

    fn value(&mut self, depth: usize) -> Result<Json, ProtocolError> {
        if depth > 8 {
            return Err(ProtocolError::Invalid);
        }
        self.skip();
        let next = self.peek().ok_or(ProtocolError::Invalid)?;
        match next {
            'n' => self.keyword("null").map(|()| Json::Null),
            't' => self.keyword("true").map(|()| Json::Bool(true)),
            'f' => self.keyword("false").map(|()| Json::Bool(false)),
            '"' => Ok(Json::String(self.string()?)),
            '[' => self.array(depth),
            '{' => self.object(depth),
            '0'..='9' => Ok(Json::Number(self.number()?)),
            _ => Err(ProtocolError::Invalid),
        }
    }

    fn object(&mut self, depth: usize) -> Result<Json, ProtocolError> {
        self.expect('{')?;
        let mut fields = Vec::new();
        self.skip();
        if self.peek() == Some('}') {
            self.i += 1;
            return Ok(Json::Object(fields));
        }
        loop {
            self.skip();
            let key = self.string()?;
            self.skip();
            self.expect(':')?;
            let value = self.value(depth + 1)?;
            if let Some(slot) = fields.iter_mut().find(|(name, _)| *name == key) {
                slot.1 = value;
            } else {
                fields.push((key, value));
            }
            self.skip();
            match self.peek() {
                Some(',') => self.i += 1,
                Some('}') => {
                    self.i += 1;
                    break;
                }
                _ => return Err(ProtocolError::Invalid),
            }
        }
        Ok(Json::Object(fields))
    }

    fn array(&mut self, depth: usize) -> Result<Json, ProtocolError> {
        self.expect('[')?;
        let mut items = Vec::new();
        self.skip();
        if self.peek() == Some(']') {
            self.i += 1;
            return Ok(Json::Array(items));
        }
        loop {
            items.push(self.value(depth + 1)?);
            self.skip();
            match self.peek() {
                Some(',') => self.i += 1,
                Some(']') => {
                    self.i += 1;
                    break;
                }
                _ => return Err(ProtocolError::Invalid),
            }
        }
        Ok(Json::Array(items))
    }

    fn string(&mut self) -> Result<String, ProtocolError> {
        self.expect('"')?;
        let mut out = String::new();
        loop {
            let ch = self.bump().ok_or(ProtocolError::Invalid)?;
            match ch {
                '"' => return Ok(out),
                '\\' => out.push(self.escape()?),
                ch if (ch as u32) < 0x20 => return Err(ProtocolError::Invalid),
                ch => out.push(ch),
            }
        }
    }

    fn escape(&mut self) -> Result<char, ProtocolError> {
        match self.bump().ok_or(ProtocolError::Invalid)? {
            '"' => Ok('"'),
            '\\' => Ok('\\'),
            '/' => Ok('/'),
            'b' => Ok('\u{0008}'),
            'f' => Ok('\u{000c}'),
            'n' => Ok('\n'),
            'r' => Ok('\r'),
            't' => Ok('\t'),
            'u' => self.unicode_escape(),
            _ => Err(ProtocolError::Invalid),
        }
    }

    fn unicode_escape(&mut self) -> Result<char, ProtocolError> {
        let unit = self.hex4()?;
        if (0xD800..=0xDBFF).contains(&unit) {
            if self.bump() != Some('\\') || self.bump() != Some('u') {
                return Err(ProtocolError::Invalid);
            }
            let low = self.hex4()?;
            if !(0xDC00..=0xDFFF).contains(&low) {
                return Err(ProtocolError::Invalid);
            }
            let cp = 0x10000 + (((unit - 0xD800) << 10) | (low - 0xDC00));
            char::from_u32(cp).ok_or(ProtocolError::Invalid)
        } else {
            char::from_u32(unit).ok_or(ProtocolError::Invalid)
        }
    }

    fn hex4(&mut self) -> Result<u32, ProtocolError> {
        let mut value = 0u32;
        for _ in 0..4 {
            let ch = self.bump().ok_or(ProtocolError::Invalid)?;
            let digit = ch.to_digit(16).ok_or(ProtocolError::Invalid)?;
            value = (value << 4) | digit;
        }
        Ok(value)
    }

    fn number(&mut self) -> Result<u64, ProtocolError> {
        let start = self.i;
        while matches!(self.peek(), Some('0'..='9')) {
            self.i += 1;
        }
        if self.i == start {
            return Err(ProtocolError::Invalid);
        }
        self.text[start..self.i]
            .parse()
            .map_err(|_| ProtocolError::Invalid)
    }

    fn keyword(&mut self, expected: &str) -> Result<(), ProtocolError> {
        let end = self.i + expected.len();
        if self.text.get(self.i..end) != Some(expected) {
            return Err(ProtocolError::Invalid);
        }
        self.i = end;
        Ok(())
    }

    fn expect(&mut self, expected: char) -> Result<(), ProtocolError> {
        if self.bump() == Some(expected) {
            Ok(())
        } else {
            Err(ProtocolError::Invalid)
        }
    }

    fn skip(&mut self) {
        while matches!(self.peek(), Some(' ' | '\n' | '\r' | '\t')) {
            self.i += 1;
        }
    }

    fn peek(&self) -> Option<char> {
        self.text[self.i..].chars().next()
    }

    fn bump(&mut self) -> Option<char> {
        let ch = self.peek()?;
        self.i += ch.len_utf8();
        Some(ch)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(id: u64, kind: KeyKind, shift: bool, ctrl: bool, alt: bool) -> ClientRequest {
        ClientRequest::Key {
            id,
            key: KeyRequest {
                kind,
                shift,
                ctrl,
                alt,
            },
        }
    }

    #[test]
    fn pipe_name_and_limits_stay_fixed() {
        assert_eq!(PIPE_NAME, r"\\.\pipe\Graver");
        assert_eq!(PROTOCOL_VERSION, 1);
        assert_eq!(MAX_FRAME_LEN, 1024 * 1024);
    }

    #[test]
    fn golden_requests_match_the_service_wire_format() {
        assert_eq!(
            encode_request_json(&ClientRequest::Ping { id: 1 }),
            br#"{"op":"ping","v":1,"id":1}"#
        );
        assert_eq!(
            encode_request_json(&ClientRequest::Reset { id: 2 }),
            br#"{"op":"reset","v":1,"id":2}"#
        );
        assert_eq!(
            encode_request_json(&ClientRequest::Deactivate { id: 3 }),
            br#"{"op":"deactivate","v":1,"id":3}"#
        );
        assert_eq!(
            encode_request_json(&key(4, KeyKind::Char('a'), false, false, false)),
            br#"{"op":"key","v":1,"id":4,"key":{"kind":"char","ch":"a","shift":false,"ctrl":false,"alt":false}}"#
        );
        assert_eq!(
            encode_request_json(&key(5, KeyKind::Space, true, false, false)),
            br#"{"op":"key","v":1,"id":5,"key":{"kind":"space","shift":true,"ctrl":false,"alt":false}}"#
        );
        assert_eq!(
            encode_request_json(&key(12, KeyKind::Digit(3), false, true, false)),
            br#"{"op":"key","v":1,"id":12,"key":{"kind":"digit","n":3,"shift":false,"ctrl":true,"alt":false}}"#
        );
        assert_eq!(
            encode_request_json(&key(8, KeyKind::Backspace, false, false, false)),
            br#"{"op":"key","v":1,"id":8,"key":{"kind":"backspace","shift":false,"ctrl":false,"alt":false}}"#
        );
        assert_eq!(
            encode_request_json(&key(11, KeyKind::Char('"'), false, false, false)),
            br#"{"op":"key","v":1,"id":11,"key":{"kind":"char","ch":"\"","shift":false,"ctrl":false,"alt":false}}"#
        );
        assert_eq!(
            encode_request_json(&key(11, KeyKind::Char('\\'), false, false, false)),
            br#"{"op":"key","v":1,"id":11,"key":{"kind":"char","ch":"\\","shift":false,"ctrl":false,"alt":false}}"#
        );
        assert_eq!(
            encode_request_json(&key(11, KeyKind::Char('\n'), false, false, false)),
            br#"{"op":"key","v":1,"id":11,"key":{"kind":"char","ch":"\n","shift":false,"ctrl":false,"alt":false}}"#
        );
        assert_eq!(
            encode_request_json(&key(11, KeyKind::Char('你'), false, false, false)),
            "{\"op\":\"key\",\"v\":1,\"id\":11,\"key\":{\"kind\":\"char\",\"ch\":\"你\",\"shift\":false,\"ctrl\":false,\"alt\":false}}".as_bytes()
        );
    }

    #[test]
    fn golden_responses_parse_without_displaying_candidates() {
        assert_eq!(
            parse_response(br#"{"op":"pong","v":1,"id":1}"#).unwrap(),
            ClientResponse::Pong { id: 1 }
        );
        assert_eq!(
            parse_response(
                br#"{"op":"update","v":1,"id":4,"preedit":"a","candidates":[],"commit":null,"consumed":true}"#
            )
            .unwrap(),
            ClientResponse::Update(CompositionUpdate {
                id: 4,
                preedit: "a".into(),
                commit: None,
                consumed: true,
            })
        );
        assert_eq!(
            parse_response(
                br#"{"op":"update","v":1,"id":9,"preedit":"","candidates":[{"text":"ni"}],"commit":"ni","consumed":true}"#
            )
            .unwrap(),
            ClientResponse::Update(CompositionUpdate {
                id: 9,
                preedit: String::new(),
                commit: Some("ni".into()),
                consumed: true,
            })
        );
        assert_eq!(
            parse_response(
                br#"{"op":"error","v":1,"id":6,"message":"unsupported protocol version 99"}"#
            )
            .unwrap(),
            ClientResponse::Error {
                id: 6,
                message: "unsupported protocol version 99".into(),
            }
        );
    }

    #[test]
    fn invalid_payloads_fail_instead_of_panicking() {
        assert_eq!(parse_response(b"").unwrap_err(), ProtocolError::Invalid);
        assert_eq!(parse_response(b"{").unwrap_err(), ProtocolError::Invalid);
        assert_eq!(
            parse_response(br#"{"op":"update","v":2,"id":1}"#).unwrap_err(),
            ProtocolError::Invalid
        );
        assert_eq!(encode_frame(b"").unwrap_err(), ProtocolError::Empty);
        let frame = encode_request_frame(&ClientRequest::Ping { id: 1 }).unwrap();
        assert_eq!(
            u32::from_le_bytes(frame[..4].try_into().unwrap()) as usize,
            frame.len() - 4
        );
    }
}
