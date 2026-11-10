//! An editor model without a view: a buffer, its selections, and the
//! editing commands bound together with undo/redo.

use std::time::Instant;

use ropey::Rope;

use crate::{
    Buffer, EditKind, Selection, SelectionSet,
    comment::CommentTokens,
    edit::{Edit, EditError},
    ops::{self, Change, Clipboard, RowColumn},
    search::Matcher,
};

/// Selection changes remembered for "undo cursor" (separate from text
/// undo).
const SELECTION_HISTORY: usize = 100;

/// Indentation style.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Indent {
    pub tab_size: usize,
    pub hard_tabs: bool,
}

impl Default for Indent {
    fn default() -> Self {
        Self {
            tab_size: 4,
            hard_tabs: false,
        }
    }
}

impl Indent {
    /// One level of indentation.
    pub fn unit(&self) -> String {
        if self.hard_tabs {
            "\t".to_string()
        } else {
            " ".repeat(self.tab_size)
        }
    }
}

/// See the module documentation.
#[derive(Debug, Clone)]
pub struct Editor {
    buffer: Buffer,
    selections: SelectionSet,
    pub indent: Indent,
    pub comments: CommentTokens,
    selection_history: Vec<SelectionSet>,
    /// Ctrl+D started from a caret, so further occurrences must be whole
    /// words.
    occurrence_whole_word: bool,
}

impl Editor {
    pub fn new(text: &str) -> Self {
        Self::from_buffer(Buffer::new(text))
    }

    pub fn from_buffer(buffer: Buffer) -> Self {
        Self {
            buffer,
            selections: SelectionSet::default(),
            indent: Indent::default(),
            comments: CommentTokens::NONE,
            selection_history: Vec::new(),
            occurrence_whole_word: false,
        }
    }

    pub fn with_comments(mut self, comments: CommentTokens) -> Self {
        self.comments = comments;
        self
    }

    pub fn buffer(&self) -> &Buffer {
        &self.buffer
    }

    pub fn buffer_mut(&mut self) -> &mut Buffer {
        &mut self.buffer
    }

    pub fn text(&self) -> &Rope {
        self.buffer.text()
    }

    pub fn selections(&self) -> &SelectionSet {
        &self.selections
    }

    /// Replace the selections (a click, a caret move). Ends the current
    /// undo group.
    pub fn set_selections(&mut self, selections: SelectionSet) {
        let selections = self.clamp(selections);
        if selections != self.selections {
            self.push_selection_history();
            self.selections = selections;
            self.occurrence_whole_word = false;
            self.buffer.close_group();
        }
    }

    fn clamp(&self, selections: SelectionSet) -> SelectionSet {
        let rope = self.buffer.text();
        let fix = |offset: usize| rope.floor_char_boundary(offset.min(rope.len()));
        let primary = selections.primary_index();
        SelectionSet::from_vec(
            selections
                .iter()
                .map(|s| Selection {
                    anchor: fix(s.anchor),
                    head: fix(s.head),
                    goal_column: s.goal_column,
                })
                .collect(),
            primary,
        )
    }

    fn push_selection_history(&mut self) {
        self.selection_history.push(self.selections.clone());
        if self.selection_history.len() > SELECTION_HISTORY {
            self.selection_history.remove(0);
        }
    }

    /// Apply a change computed by one of the [`ops`] functions.
    pub fn apply(&mut self, change: Change, kind: EditKind, now: Instant) -> Result<(), EditError> {
        let before = self.selections.clone();
        self.buffer
            .edit(change.edits, kind, &before, &change.selections, now)?;
        self.selections = change.selections;
        Ok(())
    }

    fn apply_ops(&mut self, change: Change, kind: EditKind, now: Instant) {
        // Changes built by `ops` from the current text are always valid.
        let result = self.apply(change, kind, now);
        debug_assert!(result.is_ok(), "{result:?}");
    }

    pub fn insert(&mut self, text: &str, now: Instant) {
        let change = ops::insert(&self.selections, text);
        self.apply_ops(change, EditKind::Typing, now);
    }

    pub fn backspace(&mut self, now: Instant) {
        let change = ops::backspace(self.text(), &self.selections);
        self.apply_ops(change, EditKind::Deleting, now);
    }

    pub fn delete(&mut self, now: Instant) {
        let change = ops::delete_forward(self.text(), &self.selections);
        self.apply_ops(change, EditKind::Deleting, now);
    }

    pub fn duplicate_lines(&mut self, down: bool, now: Instant) {
        let change = ops::duplicate_lines(self.text(), &self.selections, down);
        self.apply_ops(change, EditKind::Other, now);
    }

    pub fn move_lines(&mut self, up: bool, now: Instant) -> bool {
        match ops::move_lines(self.text(), &self.selections, up) {
            Some(change) => {
                self.apply_ops(change, EditKind::Other, now);
                true
            }
            None => false,
        }
    }

    pub fn delete_lines(&mut self, now: Instant) {
        let change = ops::delete_lines(self.text(), &self.selections);
        self.apply_ops(change, EditKind::Other, now);
    }

    pub fn indent_lines(&mut self, now: Instant) {
        let change = ops::indent_lines(self.text(), &self.selections, &self.indent.unit());
        self.apply_ops(change, EditKind::Other, now);
    }

    pub fn outdent_lines(&mut self, now: Instant) {
        let change = ops::outdent_lines(self.text(), &self.selections, self.indent.tab_size);
        self.apply_ops(change, EditKind::Other, now);
    }

    /// Toggle line comments; languages without line comments get a block
    /// comment. Returns false when the language has no comments.
    pub fn toggle_line_comment(&mut self, now: Instant) -> bool {
        if let Some(token) = self.comments.line {
            let change = ops::toggle_line_comment(self.text(), &self.selections, token);
            self.apply_ops(change, EditKind::Other, now);
            true
        } else {
            self.toggle_block_comment(now)
        }
    }

    /// Toggle a block comment; languages without block comments get line
    /// comments. Returns false when the language has no comments.
    pub fn toggle_block_comment(&mut self, now: Instant) -> bool {
        if let Some((open, close)) = self.comments.block {
            let change = ops::toggle_block_comment(self.text(), &self.selections, open, close);
            self.apply_ops(change, EditKind::Other, now);
            true
        } else if let Some(token) = self.comments.line {
            let change = ops::toggle_line_comment(self.text(), &self.selections, token);
            self.apply_ops(change, EditKind::Other, now);
            true
        } else {
            false
        }
    }

    /// Ctrl+D. Returns false when there is nothing (more) to select.
    pub fn select_next_occurrence(&mut self) -> bool {
        let from_caret = self.selections.primary().is_empty();
        let whole_word = self.occurrence_whole_word;
        match ops::select_next_occurrence(self.text(), &self.selections, whole_word) {
            Some(selections) => {
                self.set_selections(selections);
                self.occurrence_whole_word = whole_word || from_caret;
                true
            }
            None => false,
        }
    }

    /// Ctrl+Shift+L.
    pub fn select_all_occurrences(&mut self) -> bool {
        match ops::select_all_occurrences(self.text(), &self.selections, self.occurrence_whole_word)
        {
            Some(selections) => {
                self.set_selections(selections);
                true
            }
            None => false,
        }
    }

    pub fn add_cursor_above(&mut self) {
        let set =
            ops::add_cursor_vertical(self.text(), &self.selections, true, self.indent.tab_size);
        self.set_selections(set);
    }

    pub fn add_cursor_below(&mut self) {
        let set =
            ops::add_cursor_vertical(self.text(), &self.selections, false, self.indent.tab_size);
        self.set_selections(set);
    }

    /// Alt+drag: a box of selections between two screen positions.
    pub fn select_column(&mut self, from: RowColumn, to: RowColumn) {
        let set = ops::column_selection(self.text(), from, to, self.indent.tab_size);
        self.set_selections(set);
    }

    pub fn copy(&self) -> Clipboard {
        ops::copy(self.text(), &self.selections)
    }

    pub fn cut(&mut self, now: Instant) -> Clipboard {
        let (clipboard, change) = ops::cut(self.text(), &self.selections);
        self.apply_ops(change, EditKind::Other, now);
        clipboard
    }

    pub fn paste(&mut self, clipboard: &Clipboard, now: Instant) {
        let change = ops::paste(self.text(), &self.selections, clipboard);
        self.apply_ops(change, EditKind::Other, now);
    }

    /// Select the next match after the primary selection (wrapping).
    pub fn find_next(&mut self, matcher: &Matcher) -> bool {
        let text = self.text().to_string();
        let from = self.selections.primary().end();
        match matcher.find_next(&text, from, true) {
            Some(range) => {
                self.set_selections(SelectionSet::single(Selection::range(range)));
                true
            }
            None => false,
        }
    }

    /// Replace every match as one undo step; returns how many there were.
    pub fn replace_all(&mut self, matcher: &Matcher, replacement: &str, now: Instant) -> usize {
        let text = self.text().to_string();
        let edits = matcher.replace_all(&text, replacement);
        let count = edits.len();
        if count > 0 {
            self.edit(edits, now);
        }
        count
    }

    /// Apply arbitrary edits as one undo step; selections follow them.
    pub fn edit(&mut self, edits: Vec<Edit>, now: Instant) {
        let Ok(edits) = crate::edit::normalize(edits, self.text()) else {
            return;
        };
        let selections = self.selections.map_through(&edits);
        self.apply_ops(Change { edits, selections }, EditKind::Other, now);
    }

    pub fn undo(&mut self) -> bool {
        match self.buffer.undo() {
            Some(selections) => {
                self.selections = self.clamp(selections);
                true
            }
            None => false,
        }
    }

    pub fn redo(&mut self) -> bool {
        match self.buffer.redo() {
            Some(selections) => {
                self.selections = self.clamp(selections);
                true
            }
            None => false,
        }
    }

    /// Return to the previous selections without touching the text (VS
    /// Code's "Cursor Undo", Ctrl+U).
    pub fn undo_selection(&mut self) -> bool {
        match self.selection_history.pop() {
            Some(selections) => {
                self.selections = self.clamp(selections);
                true
            }
            None => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;
    use crate::search::SearchQuery;

    #[test]
    fn multi_cursor_session_with_undo() {
        let t0 = Instant::now();
        let mut editor = Editor::new("let a = 1;\nlet b = a + a;\n")
            .with_comments(CommentTokens::for_language("rust"));
        editor.set_selections(SelectionSet::single(Selection::cursor(4)));
        assert!(editor.select_next_occurrence()); // selects `a`
        assert!(editor.select_next_occurrence()); // second `a` (whole word)
        assert!(editor.select_next_occurrence());
        assert_eq!(editor.selections().len(), 3);
        for (i, ch) in "xyz".chars().enumerate() {
            editor.insert(&ch.to_string(), t0 + Duration::from_millis(i as u64 * 50));
        }
        assert_eq!(
            editor.text().to_string(),
            "let xyz = 1;\nlet b = xyz + xyz;\n"
        );
        editor.toggle_line_comment(t0 + Duration::from_secs(1));
        assert_eq!(
            editor.text().to_string(),
            "// let xyz = 1;\n// let b = xyz + xyz;\n"
        );
        assert!(editor.undo());
        assert!(editor.undo());
        assert_eq!(editor.text().to_string(), "let a = 1;\nlet b = a + a;\n");
        assert_eq!(editor.selections().len(), 3);
        assert!(editor.undo_selection());
        assert!(editor.redo());
        assert_eq!(
            editor.text().to_string(),
            "let xyz = 1;\nlet b = xyz + xyz;\n"
        );
    }

    #[test]
    fn replace_all_is_one_undo_step() {
        let now = Instant::now();
        let mut editor = Editor::new("a1 b2 c3");
        let matcher = SearchQuery::regex(r"(\w)(\d)").compile().unwrap();
        assert_eq!(editor.replace_all(&matcher, "$2$1", now), 3);
        assert_eq!(editor.text().to_string(), "1a 2b 3c");
        editor.undo();
        assert_eq!(editor.text().to_string(), "a1 b2 c3");
        assert!(editor.find_next(&matcher));
        assert_eq!(editor.selections().primary().to_range(), 0..2);
    }

    #[test]
    fn languages_without_line_comments_use_blocks() {
        let mut editor = Editor::new("<p>x</p>").with_comments(CommentTokens::for_language("html"));
        editor.set_selections(SelectionSet::single(Selection::range(0..8)));
        assert!(editor.toggle_line_comment(Instant::now()));
        assert_eq!(editor.text().to_string(), "<!-- <p>x</p> -->");
        let mut plain = Editor::new("x");
        assert!(!plain.toggle_line_comment(Instant::now()));
    }
}
