//! The draft being written: its text (several lines allowed), a cursor that moves and deletes by
//! grapheme, so an emoji sequence or an accented letter is one step, and the message it replies
//! to. Each chat has its own composer, kept by [`Model`](super::Model) under the chat's JID.
//! Sending it, and the rule that a draft never changes recipient, live in
//! [`Model::send`](super::Model::send).

use unicode_segmentation::UnicodeSegmentation;

/// The composer (see the module docs).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Composer {
    text: String,
    /// A byte offset into `text`, on a grapheme boundary.
    caret: usize,
    /// The id of the message the draft will quote.
    reply_to: Option<String>,
}

impl Composer {
    /// A composer with nothing written, for a chat that has no draft.
    pub(crate) const EMPTY: Composer = Composer {
        text: String::new(),
        caret: 0,
        reply_to: None,
    };

    /// The draft.
    pub fn text(&self) -> &str {
        &self.text
    }

    /// The cursor: a byte offset into [`Composer::text`], on a grapheme boundary.
    pub fn caret(&self) -> usize {
        self.caret
    }

    pub fn is_empty(&self) -> bool {
        self.text.is_empty()
    }

    /// The id of the message the draft replies to.
    pub fn reply_to(&self) -> Option<&str> {
        self.reply_to.as_deref()
    }

    /// Write a plain message, not a reply.
    pub fn cancel_reply(&mut self) {
        self.reply_to = None;
    }

    pub(crate) fn set_reply_to(&mut self, id: String) {
        self.reply_to = Some(id);
    }

    /// Put `c` into the draft at the cursor and move the cursor after it.
    pub fn insert(&mut self, c: char) {
        self.text.insert(self.caret, c);
        self.caret += c.len_utf8();
    }

    /// Replace the whole draft with `text`, the cursor at its end; the reply target stays. For a
    /// UI whose own text widget edits the draft.
    pub fn set_text(&mut self, text: &str) {
        text.clone_into(&mut self.text);
        self.caret = self.text.len();
    }

    /// Start a new line at the cursor.
    pub fn newline(&mut self) {
        self.insert('\n');
    }

    /// Erase the grapheme before the cursor.
    pub fn backspace(&mut self) {
        if let Some(start) = previous_grapheme(&self.text, self.caret) {
            self.text.replace_range(start..self.caret, "");
            self.caret = start;
        }
    }

    /// Erase the grapheme after the cursor.
    pub fn delete(&mut self) {
        if let Some(end) = next_grapheme(&self.text, self.caret) {
            self.text.replace_range(self.caret..end, "");
        }
    }

    /// Move the cursor one grapheme back.
    pub fn left(&mut self) {
        if let Some(start) = previous_grapheme(&self.text, self.caret) {
            self.caret = start;
        }
    }

    /// Move the cursor one grapheme on.
    pub fn right(&mut self) {
        if let Some(end) = next_grapheme(&self.text, self.caret) {
            self.caret = end;
        }
    }

    /// Move the cursor to the start of its line.
    pub fn home(&mut self) {
        self.caret = self.text[..self.caret]
            .rfind('\n')
            .map_or(0, |newline| newline + 1);
    }

    /// Move the cursor to the end of its line.
    pub fn end(&mut self) {
        self.caret = self.text[self.caret..]
            .find('\n')
            .map_or(self.text.len(), |newline| self.caret + newline);
    }

    /// Take the draft to send it, leaving the composer empty (the reply target is taken apart).
    pub(crate) fn take(&mut self) -> String {
        self.caret = 0;
        std::mem::take(&mut self.text)
    }

    pub(crate) fn take_reply_to(&mut self) -> Option<String> {
        self.reply_to.take()
    }

    /// Whether nothing is written and nothing quoted.
    pub(crate) fn is_blank(&self) -> bool {
        self.text.is_empty() && self.reply_to.is_none()
    }

    /// Fold `other`, the draft of a chat merged into this one, in: its text after this one's
    /// (on a new line), so neither is lost, and its reply when this one quotes nothing.
    pub(crate) fn absorb(&mut self, other: Composer) {
        if self.text.is_empty() {
            self.text = other.text;
        } else if !other.text.is_empty() {
            self.text.push('\n');
            self.text.push_str(&other.text);
        }
        self.caret = self.text.len();
        if self.reply_to.is_none() {
            self.reply_to = other.reply_to;
        }
    }
}

/// The start of the grapheme before byte `at` of `text`, if there is one.
fn previous_grapheme(text: &str, at: usize) -> Option<usize> {
    text.grapheme_indices(true)
        .map(|(start, _)| start)
        .take_while(|&start| start < at)
        .last()
}

/// The end of the grapheme after byte `at` of `text`, if there is one.
fn next_grapheme(text: &str, at: usize) -> Option<usize> {
    (at < text.len()).then(|| {
        text.grapheme_indices(true)
            .map(|(start, _)| start)
            .find(|&start| start > at)
            .unwrap_or(text.len())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn typed(text: &str) -> Composer {
        let mut composer = Composer::default();
        text.chars().for_each(|c| composer.insert(c));
        composer
    }

    #[test]
    fn backspace_and_delete_erase_a_whole_grapheme() {
        // A family emoji is seven code points and one grapheme; "é" here is two code points.
        let mut composer = typed("a👨‍👩‍👧e\u{301}");
        composer.backspace();
        assert_eq!(composer.text(), "a👨‍👩‍👧");
        composer.left();
        assert_eq!(composer.caret(), 1);
        composer.delete();
        assert_eq!(composer.text(), "a");
        assert_eq!(composer.caret(), 1);
    }

    #[test]
    fn the_cursor_moves_by_grapheme_and_home_end_stay_on_its_line() {
        let mut composer = typed("ab");
        composer.newline();
        "cñd".chars().for_each(|c| composer.insert(c));
        composer.home();
        assert_eq!(composer.caret(), 3);
        composer.right();
        composer.right();
        assert_eq!(&composer.text()[composer.caret()..], "d");
        composer.end();
        assert_eq!(composer.caret(), composer.text().len());
        composer.left();
        composer.left();
        composer.left();
        composer.left();
        composer.end();
        assert_eq!(composer.caret(), 2, "the first line's end");
    }

    #[test]
    fn a_draft_set_whole_keeps_its_reply_and_puts_the_cursor_at_its_end() {
        // A UI with its own text widget (the desktop app) hands the whole draft over.
        let mut composer = typed("old");
        composer.set_reply_to("M1".to_owned());
        composer.set_text("new\nañ");
        assert_eq!(composer.text(), "new\nañ");
        assert_eq!(composer.caret(), composer.text().len());
        assert_eq!(composer.reply_to(), Some("M1"));
    }

    #[test]
    fn taking_the_draft_empties_it() {
        let mut composer = typed("hi");
        assert_eq!(composer.take(), "hi");
        assert!(composer.is_empty());
        assert_eq!(composer.caret(), 0);
    }
}
