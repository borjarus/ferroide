//! The tree-sitter layer of an editor tab (Stage 5): the document's syntax
//! tree, kept current by incremental parsing on a background thread, and
//! what it gives the editor — the matching bracket at the caret, folding
//! regions from the pack's `folds.scm`, Expand / Shrink Selection and
//! Auto-Indent Lines.
//!
//! GPUI Kit's editor highlights with the same grammars and queries
//! (registered by [`crate::languages`]); this tree serves everything else.

use std::ops::Range;

use ferro_syntax::{BracketPair, Fold, SyntaxTree};
use ferro_text::{Edit, LINES, Selection, SelectionSet, ops::Change};
use gpui_kit::{
    AppContext as _, Context, Entity, Task, Window,
    base::input::{FoldRange, RangeDecoration, RangeDecorationCollection, RangeDecorationStyle},
    component::{ActiveTheme as _, input::EditorState},
};

use super::EditorPanel;
use crate::languages::{self, PLAIN_TEXT};

/// The syntax state of one editor tab.
#[derive(Default)]
pub(super) struct SyntaxState {
    /// `None` before the first parse and while a background reparse holds it.
    tree: Option<SyntaxTree>,
    /// A background parse is running.
    busy: bool,
    /// The text or caret changed while busy: run again when it finishes.
    stale: bool,
    task: Option<Task<()>>,
    /// Ranges Expand Selection grew from, innermost last.
    history: Vec<Range<usize>>,
    /// The range the last Expand / Shrink selected.
    expanded: Option<Range<usize>>,
    /// The matching-bracket highlight.
    brackets: Option<RangeDecorationCollection>,
    /// Bumped by every completed parse (tests wait on it).
    pub(super) parses: usize,
}

struct Parsed {
    tree: Option<SyntaxTree>,
    folds: Option<Vec<Fold>>,
    line_count: usize,
    brackets: Option<BracketPair>,
}

impl EditorPanel {
    /// Create the decorations the syntax layer draws into.
    pub(super) fn init_syntax(&mut self, state: &Entity<EditorState>, cx: &mut Context<Self>) {
        self.syntax = SyntaxState::default();
        let brackets = state.update(cx, |state, cx| {
            state.create_range_decorations_collection(Vec::new(), cx)
        });
        self.syntax.brackets = Some(brackets);
    }

    fn has_syntax(&self) -> bool {
        !self.disk.is_large() && self.language.as_ref() != PLAIN_TEXT
    }

    /// Bring the tree up to date with the text and caret, on a background
    /// thread; a request while a parse runs is folded into one more run.
    pub(super) fn refresh_syntax(&mut self, cx: &mut Context<Self>) {
        let Some(state) = self.state().cloned() else {
            return;
        };
        self.forget_expansion_if_moved(&state, cx);
        if !self.has_syntax() {
            return;
        }
        if self.syntax.busy {
            self.syntax.stale = true;
            return;
        }
        // The pack's queries compile on first use: on the background thread.
        let Some(pack) = languages::registry().get(&self.language).cloned() else {
            return;
        };
        let (rope, selection) = {
            let state = state.read(cx);
            (state.text().clone(), state.selected_range())
        };
        let caret = selection.is_empty().then_some(selection.start);
        let tree = self.syntax.tree.take();
        self.syntax.busy = true;
        let parse = cx.background_spawn(async move {
            let Ok(language) = pack.syntax() else {
                return Parsed {
                    tree,
                    folds: None,
                    line_count: 0,
                    brackets: None,
                };
            };
            let resolver = languages::registry().resolver();
            let (tree, changed) = match tree {
                Some(mut tree) if tree.language().id() == language.id() => {
                    let changed = tree.reparse(&rope).is_some();
                    (Some(tree), changed)
                }
                _ => (SyntaxTree::parse_with(language, &rope, resolver).ok(), true),
            };
            let folds = tree.as_ref().filter(|_| changed).map(SyntaxTree::folds);
            let brackets = caret.and_then(|c| tree.as_ref()?.matching_brackets(c));
            Parsed {
                tree,
                folds,
                line_count: rope.len_lines(LINES),
                brackets,
            }
        });
        self.syntax.task = Some(cx.spawn(async move |this, cx| {
            let parsed = parse.await;
            let _ = this.update(cx, |this, cx| this.syntax_ready(parsed, cx));
        }));
    }

    fn syntax_ready(&mut self, parsed: Parsed, cx: &mut Context<Self>) {
        self.syntax.busy = false;
        self.syntax.parses += 1;
        self.syntax.tree = parsed.tree;
        if let Some(state) = self.state().cloned() {
            if let Some(folds) = parsed.folds {
                let ranges = fold_ranges(&folds, parsed.line_count);
                state.update(cx, |state, cx| {
                    state.apply_highlighter_fold_candidates(ranges, cx)
                });
            }
            self.show_brackets(parsed.brackets, cx);
        }
        if std::mem::take(&mut self.syntax.stale) {
            self.refresh_syntax(cx);
        }
    }

    fn show_brackets(&mut self, pair: Option<BracketPair>, cx: &mut Context<Self>) {
        let Some(collection) = &self.syntax.brackets else {
            return;
        };
        let color = cx.theme().info.opacity(0.9);
        let decorations = pair
            .map(|pair| {
                [pair.open, pair.close]
                    .into_iter()
                    .map(|range| {
                        RangeDecoration::new(range)
                            .with_style(RangeDecorationStyle::Frame)
                            .with_color(color)
                    })
                    .collect()
            })
            .unwrap_or_default();
        collection.set(decorations, cx);
    }

    /// The tree for the current text, reparsed now. `None` while a
    /// background parse runs or for files without a grammar.
    pub(super) fn current_syntax(&mut self, cx: &mut Context<Self>) -> Option<&SyntaxTree> {
        if !self.has_syntax() || self.syntax.busy {
            return None;
        }
        let rope = self.state()?.read(cx).text().clone();
        match &mut self.syntax.tree {
            Some(tree) => {
                tree.reparse(&rope);
            }
            None => {
                let language = languages::registry().get(&self.language)?.syntax().ok()?;
                let resolver = languages::registry().resolver();
                self.syntax.tree = SyntaxTree::parse_with(language, &rope, resolver).ok();
            }
        }
        self.syntax.tree.as_ref()
    }

    /// Whether the syntax tree has caught up with the text (tests).
    pub fn syntax_parses(&self) -> usize {
        self.syntax.parses
    }

    fn forget_expansion_if_moved(&mut self, state: &Entity<EditorState>, cx: &Context<Self>) {
        let selection = state.read(cx).selected_range();
        if self.syntax.expanded.as_ref() != Some(&selection) {
            self.syntax.expanded = None;
            self.syntax.history.clear();
        }
    }

    /// Expand Selection: the next larger syntax node.
    pub(super) fn expand_selection(&mut self, state: &Entity<EditorState>, cx: &mut Context<Self>) {
        self.forget_expansion_if_moved(state, cx);
        let range = state.read(cx).selected_range();
        let Some(next) = self
            .current_syntax(cx)
            .and_then(|t| t.expand_selection(range.clone()))
        else {
            return;
        };
        self.syntax.history.push(range);
        self.syntax.expanded = Some(next.clone());
        state.update(cx, |state, cx| state.set_selected_range(next, cx));
    }

    /// Shrink Selection: back to the range Expand grew from.
    pub(super) fn shrink_selection(&mut self, state: &Entity<EditorState>, cx: &mut Context<Self>) {
        self.forget_expansion_if_moved(state, cx);
        let Some(previous) = self.syntax.history.pop() else {
            return;
        };
        self.syntax.expanded = Some(previous.clone());
        state.update(cx, |state, cx| state.set_selected_range(previous, cx));
    }

    /// Auto-Indent Lines: re-indent the selected lines (or the caret's line)
    /// from the syntax tree.
    pub(super) fn auto_indent_lines(
        &mut self,
        state: &Entity<EditorState>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let selection = state.read(cx).selected_range();
        let unit = languages::registry()
            .get(&self.language)
            .map(|p| p.manifest().indent.unit.clone())
            .unwrap_or_else(|| "    ".into());
        let Some(tree) = self.current_syntax(cx) else {
            return;
        };
        let rope = tree.text().clone();
        let first = tree.line_of(selection.start);
        let mut last = tree.line_of(selection.end);
        if last > first && selection.end == tree.line_start(last) {
            last -= 1;
        }
        let levels = tree.indent_levels(first..=last);
        let mut edits = Vec::new();
        for (line, level) in (first..=last).zip(levels) {
            let text = tree.line_text(line);
            if text.trim().is_empty() {
                continue;
            }
            let have = text.len() - text.trim_start().len();
            let want = unit.repeat(level);
            if text[..have] != want {
                let start = tree.line_start(line);
                edits.push(Edit::new(start..start + have, want));
            }
        }
        if edits.is_empty() {
            return;
        }
        let map = |offset: usize| map_offset(&edits, offset);
        let selections = SelectionSet::single(if selection.is_empty() {
            // The caret goes to the first non-blank character if it was in
            // the indentation.
            let line_start = tree.line_start(first);
            let text = tree.line_text(first);
            let indent_end = line_start + (text.len() - text.trim_start().len());
            let caret = map(selection.start.max(indent_end));
            Selection::new(caret, caret)
        } else {
            Selection::new(map(selection.start), map(selection.end))
        });
        super::editing::apply_change(state, &rope, Change { edits, selections }, window, cx);
    }
}

/// Where `offset` of the old text lands after `edits` (sorted, disjoint).
fn map_offset(edits: &[Edit], offset: usize) -> usize {
    let mut delta: isize = 0;
    for edit in edits {
        if edit.range.end <= offset {
            delta += edit.text.len() as isize - edit.range.len() as isize;
        } else if edit.range.start < offset {
            return (edit.range.start + edit.text.len()).saturating_add_signed(delta);
        }
    }
    offset.saturating_add_signed(delta)
}

/// Folds as GPUI Kit's fold ranges, which keep the first and last line
/// visible: a region whose last line belongs to the body (Python) reaches
/// one line further.
fn fold_ranges(folds: &[Fold], line_count: usize) -> Vec<FoldRange> {
    let last_line = line_count.saturating_sub(1);
    folds
        .iter()
        .map(|fold| {
            let end = if fold.closed_by_token {
                fold.end_line
            } else {
                (fold.end_line + 1).min(last_line)
            };
            (fold.start_line, end)
        })
        .filter(|(start, end)| end.saturating_sub(*start) >= 2)
        .map(|(start, end)| FoldRange::new(start, end))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn offsets_follow_indentation_edits() {
        let edits = vec![Edit::new(0..0, "    "), Edit::new(10..12, "")];
        assert_eq!(map_offset(&edits, 0), 4);
        assert_eq!(map_offset(&edits, 5), 9);
        assert_eq!(map_offset(&edits, 11), 14);
        assert_eq!(map_offset(&edits, 20), 22);
    }

    #[test]
    fn python_bodies_fold_through_their_last_line() {
        let folds = [
            Fold {
                start_line: 0,
                end_line: 3,
                closed_by_token: true,
            },
            Fold {
                start_line: 5,
                end_line: 7,
                closed_by_token: false,
            },
            Fold {
                start_line: 9,
                end_line: 10,
                closed_by_token: true,
            },
        ];
        let ranges = fold_ranges(&folds, 9);
        let pairs: Vec<(usize, usize)> =
            ranges.iter().map(|r| (r.start_line, r.end_line)).collect();
        assert_eq!(pairs, [(0, 3), (5, 8)]);
    }
}
