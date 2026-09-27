//! Exact-match pinyin schema.
//!
//! Not selected by `graver-service` or the text service. The four entries are
//! compiled into this file; nothing is read from disk or the network.

use crate::{Candidate, Input, KeyEvent, KeyKind, Update};

/// Identifier of this schema. [`crate::Engine`] keeps `latin-buffer`.
pub const SCHEMA_ID: &str = "pinyin-min";

const ENTRIES: &[(&str, &str)] = &[
    ("nihao", "你好"),
    ("zhongguo", "中国"),
    ("pinyin", "拼音"),
    ("ceshi", "测试"),
];

#[derive(Debug, Default)]
pub struct PinyinMin {
    preedit: String,
}

impl PinyinMin {
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
            KeyKind::Char(ch) if ch.is_ascii_alphabetic() => {
                self.preedit.push(ch);
                self.snapshot(true, None)
            }
            // Backspace and Escape edit the buffer the same way as latin-buffer.
            // Candidates are derived from the buffer after that edit.
            KeyKind::Backspace if !self.preedit.is_empty() => {
                self.preedit.pop();
                self.snapshot(true, None)
            }
            KeyKind::Escape if !self.preedit.is_empty() => {
                self.preedit.clear();
                self.snapshot(true, None)
            }
            KeyKind::Space | KeyKind::Enter if !self.preedit.is_empty() => self.commit_current(),
            KeyKind::Digit(1) if self.matched_text().is_some() => self.commit_current(),
            _ => self.snapshot(false, None),
        }
    }

    fn matched_text(&self) -> Option<&'static str> {
        ENTRIES
            .iter()
            .find(|(key, _)| *key == self.preedit)
            .map(|(_, text)| *text)
    }

    fn commit_current(&mut self) -> Update {
        let text = match self.matched_text() {
            Some(text) => text.to_owned(),
            None => std::mem::take(&mut self.preedit),
        };
        self.preedit.clear();
        self.snapshot(true, Some(text))
    }

    fn snapshot(&self, consumed: bool, commit: Option<String>) -> Update {
        Update {
            preedit: self.preedit.clone(),
            candidates: self.candidates(),
            commit,
            consumed,
        }
    }

    fn candidates(&self) -> Vec<Candidate> {
        match self.matched_text() {
            Some(text) => vec![Candidate {
                text: text.to_owned(),
                comment: String::new(),
            }],
            None => Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{KeyKind, Modifiers};

    fn key(kind: KeyKind) -> Input {
        Input::Key(KeyEvent {
            kind,
            modifiers: Modifiers::default(),
        })
    }

    fn modified(kind: KeyKind, modifiers: Modifiers) -> Input {
        Input::Key(KeyEvent { kind, modifiers })
    }

    fn type_letters(engine: &mut PinyinMin, letters: &str) -> Update {
        let mut last = None;
        for ch in letters.chars() {
            last = Some(engine.handle(key(KeyKind::Char(ch))));
        }
        last.expect("letters")
    }

    #[test]
    fn schema_id_is_pinyin_min() {
        assert_eq!(SCHEMA_ID, "pinyin-min");
        assert_eq!(crate::SCHEMA_ID, "latin-buffer");
    }

    #[test]
    fn four_exact_keys_show_one_candidate_and_commit_it() {
        let cases = [
            ("nihao", "你好"),
            ("zhongguo", "中国"),
            ("pinyin", "拼音"),
            ("ceshi", "测试"),
        ];
        let commit_keys = [KeyKind::Space, KeyKind::Enter, KeyKind::Digit(1)];
        for (letters, text) in cases {
            for commit_key in commit_keys {
                let mut engine = PinyinMin::new();
                let shown = type_letters(&mut engine, letters);
                assert_eq!(shown.preedit, letters);
                assert!(shown.commit.is_none());
                assert!(shown.consumed);
                assert_eq!(shown.candidates.len(), 1);
                assert_eq!(shown.candidates[0].text, text);
                assert!(shown.candidates[0].comment.is_empty());

                let committed = engine.handle(key(commit_key));
                assert_eq!(committed.commit.as_deref(), Some(text));
                assert_eq!(committed.preedit, "");
                assert!(committed.candidates.is_empty());
                assert!(committed.consumed);
                assert!(engine.preedit().is_empty());
            }
        }
    }

    #[test]
    fn unknown_letters_commit_raw_on_space_or_enter() {
        let cases = [
            ("hello", KeyKind::Space),
            ("niha", KeyKind::Enter),
            ("nihaoo", KeyKind::Space),
            ("NIHAO", KeyKind::Enter),
        ];
        for (letters, commit_key) in cases {
            let mut engine = PinyinMin::new();
            let shown = type_letters(&mut engine, letters);
            assert_eq!(shown.preedit, letters);
            assert!(shown.candidates.is_empty());
            assert!(shown.commit.is_none());

            let committed = engine.handle(key(commit_key));
            assert_eq!(committed.commit.as_deref(), Some(letters));
            assert_eq!(committed.preedit, "");
            assert!(committed.candidates.is_empty());
            assert!(committed.consumed);
        }

        let mut engine = PinyinMin::new();
        let space = engine.handle(key(KeyKind::Space));
        assert!(!space.consumed);
        assert!(space.commit.is_none());
        let enter = engine.handle(key(KeyKind::Enter));
        assert!(!enter.consumed);
        assert!(enter.commit.is_none());
    }

    #[test]
    fn ctrl_is_not_consumed() {
        let mut engine = PinyinMin::new();
        type_letters(&mut engine, "nihao");
        let update = engine.handle(modified(
            KeyKind::Space,
            Modifiers {
                ctrl: true,
                ..Modifiers::default()
            },
        ));
        assert!(!update.consumed);
        assert!(update.commit.is_none());
        assert_eq!(engine.preedit(), "nihao");
        assert_eq!(update.candidates.len(), 1);

        let backspace = engine.handle(modified(
            KeyKind::Backspace,
            Modifiers {
                ctrl: true,
                ..Modifiers::default()
            },
        ));
        assert!(!backspace.consumed);
        assert_eq!(engine.preedit(), "nihao");
    }

    #[test]
    fn alt_is_not_consumed() {
        let mut engine = PinyinMin::new();
        type_letters(&mut engine, "ceshi");
        let update = engine.handle(modified(
            KeyKind::Enter,
            Modifiers {
                alt: true,
                ..Modifiers::default()
            },
        ));
        assert!(!update.consumed);
        assert!(update.commit.is_none());
        assert_eq!(engine.preedit(), "ceshi");
    }

    #[test]
    fn backspace_and_escape_match_latin_buffer() {
        let mut engine = PinyinMin::new();
        assert!(!engine.handle(key(KeyKind::Backspace)).consumed);
        assert!(!engine.handle(key(KeyKind::Escape)).consumed);

        type_letters(&mut engine, "ab");
        let trimmed = engine.handle(key(KeyKind::Backspace));
        assert_eq!(trimmed.preedit, "a");
        assert!(trimmed.consumed);
        assert!(trimmed.commit.is_none());

        let cleared = engine.handle(key(KeyKind::Escape));
        assert_eq!(cleared.preedit, "");
        assert!(cleared.commit.is_none());
        assert!(cleared.consumed);
        assert!(engine.preedit().is_empty());

        assert!(!engine.handle(key(KeyKind::Backspace)).consumed);
        assert!(!engine.handle(key(KeyKind::Escape)).consumed);
    }

    #[test]
    fn only_an_exact_buffer_has_a_candidate() {
        let mut engine = PinyinMin::new();
        let partial = type_letters(&mut engine, "niha");
        assert!(partial.candidates.is_empty());
        let exact = engine.handle(key(KeyKind::Char('o')));
        assert_eq!(exact.candidates.len(), 1);
        let longer = engine.handle(key(KeyKind::Char('x')));
        assert_eq!(longer.preedit, "nihaox");
        assert!(longer.candidates.is_empty());
        assert!(longer.commit.is_none());

        let trimmed = engine.handle(key(KeyKind::Backspace));
        assert_eq!(trimmed.preedit, "nihao");
        assert_eq!(trimmed.candidates.len(), 1);
        assert_eq!(trimmed.candidates[0].text, "你好");
    }

    #[test]
    fn only_letters_are_buffered() {
        let mut engine = PinyinMin::new();
        type_letters(&mut engine, "ni");
        for ch in ['!', '1', ' ', '你'] {
            let update = engine.handle(key(KeyKind::Char(ch)));
            assert!(!update.consumed, "{ch}");
            assert_eq!(engine.preedit(), "ni");
        }
    }

    #[test]
    fn digit_one_without_a_candidate_is_not_consumed() {
        let mut engine = PinyinMin::new();
        type_letters(&mut engine, "hello");
        let update = engine.handle(key(KeyKind::Digit(1)));
        assert!(!update.consumed);
        assert!(update.commit.is_none());
        assert_eq!(engine.preedit(), "hello");
    }

    #[test]
    fn other_digit_does_not_commit_a_hit() {
        let mut engine = PinyinMin::new();
        type_letters(&mut engine, "pinyin");
        let update = engine.handle(key(KeyKind::Digit(2)));
        assert!(!update.consumed);
        assert!(update.commit.is_none());
        assert_eq!(engine.preedit(), "pinyin");
        assert_eq!(update.candidates.len(), 1);
    }

    #[test]
    fn reset_discards_without_committing() {
        let mut engine = PinyinMin::new();
        type_letters(&mut engine, "nihao");
        let update = engine.handle(Input::Reset);
        assert!(update.commit.is_none());
        assert_eq!(update.preedit, "");
        assert!(update.candidates.is_empty());
        assert!(!update.consumed);
        assert!(engine.preedit().is_empty());
    }
}
