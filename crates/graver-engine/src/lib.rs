//! Composition sessions.
//!
//! [`Engine`] is the latin-buffer scaffold used by graver-service.
//! [`pinyin_min`] is a separate exact-match schema. It is not selected by
//! that service or by the text service. Both use [`Input`] and [`Update`].
//! [`Engine::handle`] stays on latin-buffer.
//!
//! This crate must stay free of Win32 and UI dependencies so the out-of-process
//! service can own the scaffold, and the in-process TSF DLL does not have to.

pub mod pinyin_min;

/// Identifier of the scaffold schema used by [`Engine`]. It buffers characters
/// and commits them. It is not [`pinyin_min`].
pub const SCHEMA_ID: &str = "latin-buffer";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Modifiers {
    pub shift: bool,
    pub ctrl: bool,
    pub alt: bool,
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
    /// `1` through `9` select a candidate once a schema provides them.
    Digit(u8),
    Other,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KeyEvent {
    pub kind: KeyKind,
    pub modifiers: Modifiers,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Input {
    Key(KeyEvent),
    /// Drop the current composition without committing it.
    Reset,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Candidate {
    pub text: String,
    pub comment: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Update {
    pub preedit: String,
    pub candidates: Vec<Candidate>,
    /// Text to insert into the focused application. Present only on the event that commits.
    pub commit: Option<String>,
    /// `false` means the host should keep the key.
    pub consumed: bool,
}

#[derive(Debug, Default)]
pub struct Engine {
    preedit: String,
}

impl Engine {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn preedit(&self) -> &str {
        &self.preedit
    }

    pub fn handle(&mut self, input: Input) -> Update {
        match input {
            Input::Reset => {
                self.preedit.clear();
                self.snapshot(false, None)
            }
            Input::Key(event) => self.handle_key(event),
        }
    }

    fn handle_key(&mut self, event: KeyEvent) -> Update {
        if event.modifiers.ctrl || event.modifiers.alt {
            return self.snapshot(false, None);
        }

        match event.kind {
            KeyKind::Char(ch) if !ch.is_control() => {
                self.preedit.push(ch);
                self.snapshot(true, None)
            }
            KeyKind::Backspace if !self.preedit.is_empty() => {
                self.preedit.pop();
                self.snapshot(true, None)
            }
            KeyKind::Escape if !self.preedit.is_empty() => {
                self.preedit.clear();
                self.snapshot(true, None)
            }
            KeyKind::Space | KeyKind::Enter if !self.preedit.is_empty() => {
                let text = std::mem::take(&mut self.preedit);
                self.snapshot(true, Some(text))
            }
            _ => self.snapshot(false, None),
        }
    }

    fn snapshot(&self, consumed: bool, commit: Option<String>) -> Update {
        Update {
            preedit: self.preedit.clone(),
            candidates: Vec::new(),
            commit,
            consumed,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(kind: KeyKind) -> Input {
        Input::Key(KeyEvent {
            kind,
            modifiers: Modifiers::default(),
        })
    }

    fn modified(kind: KeyKind, modifiers: Modifiers) -> Input {
        Input::Key(KeyEvent { kind, modifiers })
    }

    #[test]
    fn characters_compose_until_space_commits() {
        let mut engine = Engine::new();
        let first = engine.handle(key(KeyKind::Char('n')));
        assert_eq!(first.preedit, "n");
        assert!(first.commit.is_none());
        assert!(first.consumed);

        engine.handle(key(KeyKind::Char('i')));
        let committed = engine.handle(key(KeyKind::Space));
        assert_eq!(committed.commit.as_deref(), Some("ni"));
        assert_eq!(committed.preedit, "");
        assert!(engine.preedit().is_empty());

        let passed = engine.handle(key(KeyKind::Space));
        assert!(!passed.consumed);
        assert!(passed.commit.is_none());
    }

    #[test]
    fn backspace_and_escape_only_apply_while_composing() {
        let mut engine = Engine::new();
        assert!(!engine.handle(key(KeyKind::Backspace)).consumed);
        assert!(!engine.handle(key(KeyKind::Escape)).consumed);

        engine.handle(key(KeyKind::Char('你')));
        engine.handle(key(KeyKind::Char('好')));
        let trimmed = engine.handle(key(KeyKind::Backspace));
        assert_eq!(trimmed.preedit, "你");
        assert!(trimmed.consumed);

        let cleared = engine.handle(key(KeyKind::Escape));
        assert_eq!(cleared.preedit, "");
        assert!(cleared.commit.is_none());
    }

    #[test]
    fn shortcuts_pass_through() {
        let mut engine = Engine::new();
        engine.handle(key(KeyKind::Char('a')));
        let update = engine.handle(modified(
            KeyKind::Backspace,
            Modifiers {
                ctrl: true,
                ..Modifiers::default()
            },
        ));
        assert!(!update.consumed);
        assert_eq!(engine.preedit(), "a");
    }

    #[test]
    fn reset_discards_without_committing() {
        let mut engine = Engine::new();
        engine.handle(key(KeyKind::Char('a')));
        let update = engine.handle(Input::Reset);
        assert!(update.commit.is_none());
        assert_eq!(update.preedit, "");
        assert!(!update.consumed);
    }

    #[test]
    fn navigation_is_not_consumed_yet() {
        let mut engine = Engine::new();
        let update = engine.handle(key(KeyKind::Digit(1)));
        assert!(!update.consumed);
        assert!(update.candidates.is_empty());
    }

    #[test]
    fn scaffold_schema_still_commits_raw_letters() {
        assert_eq!(SCHEMA_ID, "latin-buffer");
        let mut engine = Engine::new();
        for ch in ['n', 'i', 'h', 'a', 'o'] {
            let update = engine.handle(key(KeyKind::Char(ch)));
            assert!(update.candidates.is_empty());
            assert!(update.commit.is_none());
        }
        let committed = engine.handle(key(KeyKind::Space));
        assert_eq!(committed.commit.as_deref(), Some("nihao"));
        assert!(committed.candidates.is_empty());
        assert_eq!(committed.preedit, "");
    }
}
