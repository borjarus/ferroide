//! Text commands of an editor tab, computed by `ferro_text::ops` and applied
//! to GPUI Kit's editor state.
//!
//! GPUI Kit's editor exposes only its active selection (the first one in the
//! text), so line commands act on it; Select Next / All Occurrences keeps
//! its own list of the selections it added.

use ferro_text::{
    Rope, Selection, SelectionSet,
    comment::CommentTokens,
    edit::apply_to_string,
    ops::{self, Change},
};
use gpui_kit::{
    Context, Entity, Window,
    component::input::{self as kit_input, EditorState},
};

use super::{EditorCommand, EditorPanel, language_for};

impl EditorPanel {
    pub(super) fn run_text_command(
        &mut self,
        command: EditorCommand,
        state: &Entity<EditorState>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.disk.is_read_only() {
            return;
        }
        use EditorCommand::*;
        match command {
            // GPUI Kit's own commands know every cursor.
            AddCursorAbove => {
                state.update(cx, |state, cx| state.focus(window, cx));
                window.dispatch_action(Box::new(kit_input::AddCursorAbove), cx);
                return;
            }
            AddCursorBelow => {
                state.update(cx, |state, cx| state.focus(window, cx));
                window.dispatch_action(Box::new(kit_input::AddCursorBelow), cx);
                return;
            }
            SelectNextOccurrence => {
                self.select_next_occurrence(state, cx);
                return;
            }
            SelectAllOccurrences => {
                self.select_all_occurrences(state, cx);
                return;
            }
            _ => {}
        }
        let (rope, selections) = active_selection(state, cx);
        let comments = CommentTokens::for_language(language_for(&self.path));
        let change = match command {
            DuplicateLineDown => Some(ops::duplicate_lines(&rope, &selections, true)),
            DuplicateLineUp => Some(ops::duplicate_lines(&rope, &selections, false)),
            MoveLineUp => ops::move_lines(&rope, &selections, true),
            MoveLineDown => ops::move_lines(&rope, &selections, false),
            DeleteLine => Some(ops::delete_lines(&rope, &selections)),
            ToggleLineComment => match (comments.line, comments.block) {
                (Some(token), _) => Some(ops::toggle_line_comment(&rope, &selections, token)),
                (None, Some((open, close))) => {
                    Some(ops::toggle_block_comment(&rope, &selections, open, close))
                }
                (None, None) => None,
            },
            ToggleBlockComment => match (comments.block, comments.line) {
                (Some((open, close)), _) => {
                    Some(ops::toggle_block_comment(&rope, &selections, open, close))
                }
                (None, Some(token)) => Some(ops::toggle_line_comment(&rope, &selections, token)),
                (None, None) => None,
            },
            _ => None,
        };
        if let Some(change) = change {
            apply_change(state, &rope, change, window, cx);
        }
    }

    /// Ctrl+D: select the word at the caret, then add the next occurrence.
    fn select_next_occurrence(&mut self, state: &Entity<EditorState>, cx: &mut Context<Self>) {
        let (rope, active) = active_selection(state, cx);
        let active_range = active.primary().to_range();
        // Our list is still current while GPUI Kit's active selection (the
        // first in the text) is the first one we added.
        let ours_current = self
            .occurrences
            .iter()
            .min_by_key(|r| r.start)
            .is_some_and(|first| *first == active_range);
        if !ours_current {
            self.occurrences.clear();
            self.occurrence_whole_word = false;
        }
        let set = if self.occurrences.is_empty() {
            active
        } else {
            SelectionSet::from_vec(
                self.occurrences
                    .iter()
                    .map(|r| Selection::range(r.clone()))
                    .collect(),
                self.occurrences.len() - 1,
            )
        };
        let from_caret = set.primary().is_empty();
        let Some(next) = ops::select_next_occurrence(&rope, &set, self.occurrence_whole_word)
        else {
            return;
        };
        let range = next.primary().to_range();
        state.update(cx, |state, cx| {
            if from_caret {
                state.set_selected_range(range.clone(), cx);
            } else {
                state.add_selection(range.clone(), cx);
            }
        });
        if from_caret {
            self.occurrences = vec![range];
            self.occurrence_whole_word = true;
        } else {
            if self.occurrences.is_empty() {
                self.occurrences.push(active_range);
            }
            self.occurrences.push(range);
        }
    }

    /// Ctrl+Shift+L: select every occurrence of the selection or word.
    fn select_all_occurrences(&mut self, state: &Entity<EditorState>, cx: &mut Context<Self>) {
        let (rope, active) = active_selection(state, cx);
        let Some(all) = ops::select_all_occurrences(&rope, &active, self.occurrence_whole_word)
        else {
            return;
        };
        let primary = all.primary().to_range();
        let ranges: Vec<_> = all.iter().map(Selection::to_range).collect();
        state.update(cx, |state, cx| {
            // Others first, the primary last, so the view scrolls to it.
            let mut others = ranges.iter().filter(|r| **r != primary);
            match others.next() {
                Some(first) => {
                    state.set_selected_range(first.clone(), cx);
                    for range in others {
                        state.add_selection(range.clone(), cx);
                    }
                    state.add_selection(primary.clone(), cx);
                }
                None => state.set_selected_range(primary.clone(), cx),
            }
        });
        self.occurrences = ranges;
    }
}

/// The text and GPUI Kit's active selection.
fn active_selection(
    state: &Entity<EditorState>,
    cx: &Context<EditorPanel>,
) -> (Rope, SelectionSet) {
    let state = state.read(cx);
    let range = state.selected_range();
    let selection = if state.is_selection_reversed() {
        Selection::new(range.end, range.start)
    } else {
        Selection::new(range.start, range.end)
    };
    (state.text().clone(), SelectionSet::single(selection))
}

/// Apply a change as one undoable replacement of the span it touches, then
/// put the selections where the change says.
fn apply_change(
    state: &Entity<EditorState>,
    rope: &Rope,
    change: Change,
    window: &mut Window,
    cx: &mut Context<EditorPanel>,
) {
    let (Some(first), Some(last)) = (change.edits.first(), change.edits.last()) else {
        return;
    };
    let span = first.range.start..last.range.end;
    let old = rope.slice(span.clone()).to_string();
    let relative: Vec<_> = change
        .edits
        .iter()
        .map(|e| {
            ferro_text::Edit::new(
                e.range.start - span.start..e.range.end - span.start,
                e.text.clone(),
            )
        })
        .collect();
    let new = apply_to_string(&old, &relative);
    state.update(cx, |state, cx| {
        state.set_selected_range(span, cx);
        state.replace(new, window, cx);
        let mut selections = change.selections.iter();
        if let Some(first) = selections.next() {
            state.set_selected_range_with_direction(first.to_range(), first.is_reversed(), cx);
        }
        for selection in selections {
            state.add_selection(selection.to_range(), cx);
        }
    });
}
