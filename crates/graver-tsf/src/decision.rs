//! Key decisions for the text service. No Win32 types.
//!
//! The service response is authoritative. Ctrl and Alt, a missing service, a
//! timeout, or a protocol error always return the key to the host.

use crate::protocol::CompositionUpdate;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KeyModifiers {
    pub ctrl: bool,
    pub alt: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeyDecision {
    /// Do not eat the key. Leave the host composition unchanged.
    ReturnToHost,
    /// Eat the key and show `preedit`. An empty string ends composition
    /// without inserting text.
    UpdatePreedit(String),
    /// Eat the key, insert `text`, and end composition.
    CommitAndEnd(String),
}

pub fn decide_key(modifiers: KeyModifiers, outcome: Result<CompositionUpdate, ()>) -> KeyDecision {
    if modifiers.ctrl || modifiers.alt {
        return KeyDecision::ReturnToHost;
    }
    let update = match outcome {
        Ok(update) => update,
        Err(()) => return KeyDecision::ReturnToHost,
    };
    if !update.consumed {
        return KeyDecision::ReturnToHost;
    }
    match update.commit {
        Some(text) => KeyDecision::CommitAndEnd(text),
        None => KeyDecision::UpdatePreedit(update.preedit),
    }
}

/// Predict whether `OnTestKeyDown` should eat the key without mutating the
/// service session. This matches the current latin-buffer rules so a test
/// key and the following key agree. The response from [`decide_key`] remains
/// authoritative once the key is sent.
pub fn preview_eaten(
    session_open: bool,
    preedit_empty: bool,
    kind: PreviewKey,
    modifiers: KeyModifiers,
) -> bool {
    if !session_open || modifiers.ctrl || modifiers.alt {
        return false;
    }
    match kind {
        PreviewKey::Char => true,
        PreviewKey::Backspace | PreviewKey::Escape | PreviewKey::Space | PreviewKey::Enter => {
            !preedit_empty
        }
        PreviewKey::Other => false,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PreviewKey {
    Char,
    Backspace,
    Escape,
    Space,
    Enter,
    Other,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn update(preedit: &str, commit: Option<&str>, consumed: bool) -> CompositionUpdate {
        CompositionUpdate {
            id: 1,
            preedit: preedit.to_owned(),
            commit: commit.map(str::to_owned),
            consumed,
        }
    }

    fn plain() -> KeyModifiers {
        KeyModifiers {
            ctrl: false,
            alt: false,
        }
    }

    #[test]
    fn consumed_preedit_updates_composition() {
        assert_eq!(
            decide_key(plain(), Ok(update("a", None, true))),
            KeyDecision::UpdatePreedit("a".into())
        );
    }

    #[test]
    fn commit_ends_composition() {
        assert_eq!(
            decide_key(plain(), Ok(update("", Some("ab"), true))),
            KeyDecision::CommitAndEnd("ab".into())
        );
    }

    #[test]
    fn empty_preedit_without_commit_clears_composition() {
        assert_eq!(
            decide_key(plain(), Ok(update("", None, true))),
            KeyDecision::UpdatePreedit(String::new())
        );
    }

    #[test]
    fn unconsumed_key_returns_to_the_host() {
        assert_eq!(
            decide_key(plain(), Ok(update("a", Some("a"), false))),
            KeyDecision::ReturnToHost
        );
    }

    #[test]
    fn ctrl_or_alt_is_not_consumed() {
        let consumed = Ok(update("a", None, true));
        assert_eq!(
            decide_key(
                KeyModifiers {
                    ctrl: true,
                    alt: false
                },
                consumed.clone()
            ),
            KeyDecision::ReturnToHost
        );
        assert_eq!(
            decide_key(
                KeyModifiers {
                    ctrl: false,
                    alt: true
                },
                consumed
            ),
            KeyDecision::ReturnToHost
        );
    }

    #[test]
    fn missing_service_timeout_or_protocol_error_returns_the_key() {
        assert_eq!(decide_key(plain(), Err(())), KeyDecision::ReturnToHost);
        assert_eq!(
            decide_key(
                KeyModifiers {
                    ctrl: true,
                    alt: false
                },
                Err(())
            ),
            KeyDecision::ReturnToHost
        );
    }

    #[test]
    fn preview_matches_latin_buffer_without_calling_win32() {
        let plain = plain();
        assert!(!preview_eaten(false, true, PreviewKey::Char, plain));
        assert!(preview_eaten(true, true, PreviewKey::Char, plain));
        assert!(!preview_eaten(true, true, PreviewKey::Space, plain));
        assert!(preview_eaten(true, false, PreviewKey::Space, plain));
        assert!(preview_eaten(true, false, PreviewKey::Backspace, plain));
        assert!(preview_eaten(true, false, PreviewKey::Escape, plain));
        assert!(preview_eaten(true, false, PreviewKey::Enter, plain));
        assert!(!preview_eaten(true, false, PreviewKey::Other, plain));
        assert!(!preview_eaten(
            true,
            false,
            PreviewKey::Char,
            KeyModifiers {
                ctrl: true,
                alt: false
            }
        ));
        assert!(!preview_eaten(
            true,
            false,
            PreviewKey::Char,
            KeyModifiers {
                ctrl: false,
                alt: true
            }
        ));
    }
}
