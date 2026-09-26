use graver_engine::{Engine, Input, KeyEvent, KeyKind, Modifiers, Update};
use graver_ipc::{
    CandidateMessage, KeyKindMessage, KeyMessage, PROTOCOL_VERSION, Request, Response,
};

pub fn dispatch(engine: &mut Engine, request: Request) -> Response {
    if request.version() != PROTOCOL_VERSION {
        return Response::Error {
            v: PROTOCOL_VERSION,
            id: request.id(),
            message: format!("unsupported protocol version {}", request.version()),
        };
    }

    match request {
        Request::Ping { id, .. } => Response::Pong {
            v: PROTOCOL_VERSION,
            id,
        },
        Request::Reset { id, .. } | Request::Deactivate { id, .. } => {
            update_response(id, &engine.handle(Input::Reset))
        }
        Request::Key { id, key, .. } => {
            update_response(id, &engine.handle(Input::Key(to_key(key))))
        }
    }
}

fn to_key(key: KeyMessage) -> KeyEvent {
    let kind = match key.kind {
        KeyKindMessage::Char { ch } => KeyKind::Char(ch),
        KeyKindMessage::Backspace => KeyKind::Backspace,
        KeyKindMessage::Escape => KeyKind::Escape,
        KeyKindMessage::Space => KeyKind::Space,
        KeyKindMessage::Enter => KeyKind::Enter,
        KeyKindMessage::Left => KeyKind::Left,
        KeyKindMessage::Right => KeyKind::Right,
        KeyKindMessage::Up => KeyKind::Up,
        KeyKindMessage::Down => KeyKind::Down,
        KeyKindMessage::Digit { n } => KeyKind::Digit(n),
        KeyKindMessage::Other => KeyKind::Other,
    };
    KeyEvent {
        kind,
        modifiers: Modifiers {
            shift: key.shift,
            ctrl: key.ctrl,
            alt: key.alt,
        },
    }
}

fn update_response(id: u64, update: &Update) -> Response {
    Response::Update {
        v: PROTOCOL_VERSION,
        id,
        preedit: update.preedit.clone(),
        candidates: update
            .candidates
            .iter()
            .map(|candidate| CandidateMessage {
                text: candidate.text.clone(),
                comment: candidate.comment.clone(),
            })
            .collect(),
        commit: update.commit.clone(),
        consumed: update.consumed,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use graver_ipc::KeyKindMessage;

    fn character(id: u64, ch: char) -> Request {
        Request::Key {
            v: PROTOCOL_VERSION,
            id,
            key: KeyMessage {
                kind: KeyKindMessage::Char { ch },
                shift: false,
                ctrl: false,
                alt: false,
            },
        }
    }

    #[test]
    fn key_sequence_commits_through_the_protocol() {
        let mut engine = Engine::new();
        let _ = dispatch(&mut engine, character(1, 'a'));
        let _ = dispatch(&mut engine, character(2, 'b'));
        let response = dispatch(
            &mut engine,
            Request::Key {
                v: 1,
                id: 3,
                key: KeyMessage {
                    kind: KeyKindMessage::Space,
                    shift: false,
                    ctrl: false,
                    alt: false,
                },
            },
        );
        match response {
            Response::Update {
                preedit,
                commit,
                consumed,
                candidates,
                ..
            } => {
                assert_eq!(preedit, "");
                assert_eq!(commit.as_deref(), Some("ab"));
                assert!(consumed);
                assert!(candidates.is_empty());
            }
            other => panic!("unexpected response: {other:?}"),
        }
    }

    #[test]
    fn unknown_version_is_an_error_response() {
        let mut engine = Engine::new();
        let response = dispatch(&mut engine, Request::Ping { v: 99, id: 4 });
        match response {
            Response::Error { id, message, .. } => {
                assert_eq!(id, 4);
                assert!(message.contains("99"));
            }
            other => panic!("unexpected response: {other:?}"),
        }
    }
}
