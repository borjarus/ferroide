//! Editing commands as pure functions: given the text and the selections,
//! they return the edits to apply and the selections afterwards. They work
//! on every selection at once (multiple cursors).

use std::ops::{Range, RangeInclusive};

use ropey::Rope;

use crate::{
    Bias, LINES, Selection, SelectionSet,
    edit::{Edit, map_offset},
    search::SearchQuery,
    unicode::{
        display_width, is_whole_word, next_grapheme_boundary, offset_at_column,
        prev_grapheme_boundary, word_range_at,
    },
};

/// The result of an editing command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Change {
    /// Sorted and non-overlapping, in the coordinates of the text before.
    pub edits: Vec<Edit>,
    /// In the coordinates of the text after the edits.
    pub selections: SelectionSet,
}

// ---------------------------------------------------------------------------
// Line helpers
// ---------------------------------------------------------------------------

pub fn row_count(rope: &Rope) -> usize {
    rope.len_lines(LINES)
}

pub fn row_of(rope: &Rope, offset: usize) -> usize {
    rope.byte_to_line_idx(offset.min(rope.len()), LINES)
}

/// Byte offset where `row` starts (the end of the text past the last row).
pub fn row_start(rope: &Rope, row: usize) -> usize {
    if row >= row_count(rope) {
        rope.len()
    } else {
        rope.line_to_byte_idx(row, LINES)
    }
}

/// Byte offset where `row`'s content ends, before its `\n`.
pub fn row_end(rope: &Rope, row: usize) -> usize {
    if row + 1 >= row_count(rope) {
        rope.len()
    } else {
        rope.line_to_byte_idx(row + 1, LINES) - 1
    }
}

/// The row's content without its line break.
pub fn row_text(rope: &Rope, row: usize) -> String {
    rope.slice(row_start(rope, row)..row_end(rope, row))
        .to_string()
}

/// Display column of `offset` within its row.
pub fn column_of(rope: &Rope, offset: usize, tab_size: usize) -> usize {
    let start = row_start(rope, row_of(rope, offset));
    display_width(&rope.slice(start..offset).to_string(), 0, tab_size)
}

/// Rows covered by the selections, merged when they overlap or touch. A
/// selection ending at the very start of a row does not cover that row.
pub fn row_blocks(rope: &Rope, selections: &SelectionSet) -> Vec<RangeInclusive<usize>> {
    let mut blocks: Vec<RangeInclusive<usize>> = Vec::new();
    for s in selections.iter() {
        let first = row_of(rope, s.start());
        let mut last = row_of(rope, s.end());
        if !s.is_empty() && last > first && s.end() == row_start(rope, last) {
            last -= 1;
        }
        match blocks.last_mut() {
            Some(block) if first <= block.end() + 1 => {
                *block = *block.start()..=(*block.end()).max(last);
            }
            _ => blocks.push(first..=last),
        }
    }
    blocks
}

fn leading_whitespace(line: &str) -> usize {
    line.len() - line.trim_start_matches([' ', '\t']).len()
}

fn shifted(selection: Selection, delta: isize) -> Selection {
    Selection {
        anchor: (selection.anchor as isize + delta) as usize,
        head: (selection.head as isize + delta) as usize,
        goal_column: selection.goal_column,
    }
}

fn map_selection(selection: &Selection, edits: &[Edit], start: Bias, end: Bias) -> Selection {
    let new_start = map_offset(selection.start(), edits, start);
    let new_end = map_offset(selection.end(), edits, end).max(new_start);
    if selection.is_empty() {
        Selection::cursor(new_end)
    } else {
        selection.with_range(new_start..new_end)
    }
}

// ---------------------------------------------------------------------------
// Typing and deleting
// ---------------------------------------------------------------------------

/// Replace every selection with `text`; each caret ends after its copy.
pub fn insert(selections: &SelectionSet, text: &str) -> Change {
    insert_each(selections, |_| text.to_string())
}

/// Replace selection `i` with `text(i)`.
pub fn insert_each(selections: &SelectionSet, mut text: impl FnMut(usize) -> String) -> Change {
    let mut edits = Vec::with_capacity(selections.len());
    let mut after = Vec::with_capacity(selections.len());
    let mut delta: isize = 0;
    for (i, s) in selections.iter().enumerate() {
        let text = text(i);
        let start = (s.start() as isize + delta) as usize;
        after.push(Selection::cursor(start + text.len()));
        delta += text.len() as isize - s.to_range().len() as isize;
        edits.push(Edit::new(s.to_range(), text));
    }
    Change {
        edits,
        selections: SelectionSet::from_vec(after, selections.primary_index()),
    }
}

fn delete_ranges(selections: &SelectionSet, ranges: Vec<Range<usize>>) -> Change {
    let mut edits = Vec::new();
    let mut after = Vec::new();
    let mut delta: isize = 0;
    let mut previous_end = 0;
    for range in ranges {
        let range = range.start.max(previous_end)..range.end.max(previous_end);
        previous_end = range.end;
        after.push(Selection::cursor((range.start as isize + delta) as usize));
        delta -= range.len() as isize;
        if !range.is_empty() {
            edits.push(Edit::delete(range));
        }
    }
    Change {
        edits,
        selections: SelectionSet::from_vec(after, selections.primary_index()),
    }
}

/// Backspace: delete each selection, or the grapheme before each cursor.
pub fn backspace(rope: &Rope, selections: &SelectionSet) -> Change {
    let ranges = selections
        .iter()
        .map(|s| {
            if s.is_empty() {
                prev_grapheme_boundary(rope, s.head)..s.head
            } else {
                s.to_range()
            }
        })
        .collect();
    delete_ranges(selections, ranges)
}

/// Delete: delete each selection, or the grapheme after each cursor.
pub fn delete_forward(rope: &Rope, selections: &SelectionSet) -> Change {
    let ranges = selections
        .iter()
        .map(|s| {
            if s.is_empty() {
                s.head..next_grapheme_boundary(rope, s.head)
            } else {
                s.to_range()
            }
        })
        .collect();
    delete_ranges(selections, ranges)
}

// ---------------------------------------------------------------------------
// Lines
// ---------------------------------------------------------------------------

/// Copy the selected lines below (`down`) or above them. The carets follow
/// the copy below, or stay on the upper copy.
pub fn duplicate_lines(rope: &Rope, selections: &SelectionSet, down: bool) -> Change {
    let blocks = row_blocks(rope, selections);
    let mut edits = Vec::with_capacity(blocks.len());
    // Per block: (rows, shift of selections inside it).
    let mut shifts = Vec::with_capacity(blocks.len());
    let mut delta: isize = 0;
    for block in &blocks {
        let start = row_start(rope, *block.start());
        let end = row_end(rope, *block.end());
        let text = rope.slice(start..end).to_string();
        let added = text.len() as isize + 1;
        if down {
            edits.push(Edit::insert(end, format!("\n{text}")));
            shifts.push((block.clone(), delta + added));
        } else {
            edits.push(Edit::insert(start, format!("{text}\n")));
            shifts.push((block.clone(), delta));
        }
        delta += added;
    }
    let after = selections
        .iter()
        .map(|s| {
            let row = row_of(rope, s.start());
            let shift = shifts
                .iter()
                .find(|(rows, _)| rows.contains(&row))
                .map_or(0, |(_, shift)| *shift);
            shifted(*s, shift)
        })
        .collect();
    Change {
        edits,
        selections: SelectionSet::from_vec(after, selections.primary_index()),
    }
}

/// Move the selected lines one line up or down. `None` when no block can
/// move (already at the top or bottom).
pub fn move_lines(rope: &Rope, selections: &SelectionSet, up: bool) -> Option<Change> {
    let last_row = row_count(rope) - 1;
    let mut edits = Vec::new();
    let mut shifts = Vec::new();
    for block in row_blocks(rope, selections) {
        let (first, last) = (*block.start(), *block.end());
        if (up && first == 0) || (!up && last >= last_row) {
            continue;
        }
        let block_text = rope
            .slice(row_start(rope, first)..row_end(rope, last))
            .to_string();
        if up {
            let neighbour = row_text(rope, first - 1);
            edits.push(Edit::new(
                row_start(rope, first - 1)..row_end(rope, last),
                format!("{block_text}\n{neighbour}"),
            ));
            shifts.push((block, -(neighbour.len() as isize + 1)));
        } else {
            let neighbour = row_text(rope, last + 1);
            edits.push(Edit::new(
                row_start(rope, first)..row_end(rope, last + 1),
                format!("{neighbour}\n{block_text}"),
            ));
            shifts.push((block, neighbour.len() as isize + 1));
        }
    }
    if edits.is_empty() {
        return None;
    }
    // A selection ending at the start of the line after a block moved to
    // the end of the text has no line break left to end after.
    let len = rope.len();
    let after = selections
        .iter()
        .map(|s| {
            let row = row_of(rope, s.start());
            let shift = shifts
                .iter()
                .find(|(rows, _)| rows.contains(&row))
                .map_or(0, |(_, shift)| *shift);
            let moved = shifted(*s, shift);
            Selection {
                anchor: moved.anchor.min(len),
                head: moved.head.min(len),
                goal_column: moved.goal_column,
            }
        })
        .collect();
    Some(Change {
        edits,
        selections: SelectionSet::from_vec(after, selections.primary_index()),
    })
}

/// Delete the selected lines entirely.
pub fn delete_lines(rope: &Rope, selections: &SelectionSet) -> Change {
    let last_row = row_count(rope) - 1;
    let ranges: Vec<Range<usize>> = row_blocks(rope, selections)
        .into_iter()
        .map(|block| {
            let start = row_start(rope, *block.start());
            if *block.end() < last_row {
                start..row_start(rope, block.end() + 1)
            } else if start > 0 {
                // The last line has no line break of its own; take the
                // previous one.
                start - 1..rope.len()
            } else {
                0..rope.len()
            }
        })
        .collect();
    let mut edits = Vec::new();
    let mut after = Vec::new();
    let mut delta: isize = 0;
    for range in ranges {
        let at = (range.start as isize + delta) as usize;
        delta -= range.len() as isize;
        edits.push(Edit::delete(range));
        after.push(at);
    }
    // Carets go to the start of the line that moved up into place.
    let after = after.into_iter().map(Selection::cursor).collect();
    Change {
        edits,
        selections: SelectionSet::from_vec(after, 0),
    }
}

/// Indent the selected lines by `unit` (blank lines are left alone unless
/// a single empty line is selected).
pub fn indent_lines(rope: &Rope, selections: &SelectionSet, unit: &str) -> Change {
    let rows: Vec<usize> = row_blocks(rope, selections).into_iter().flatten().collect();
    let single = rows.len() == 1;
    let edits: Vec<Edit> = rows
        .into_iter()
        .filter(|row| single || !row_text(rope, *row).trim().is_empty())
        .map(|row| Edit::insert(row_start(rope, row), unit))
        .collect();
    let selections = selections.map_through(&edits);
    Change { edits, selections }
}

/// Remove one level of indentation (a tab, or up to `tab_size` spaces) from
/// the selected lines.
pub fn outdent_lines(rope: &Rope, selections: &SelectionSet, tab_size: usize) -> Change {
    let edits: Vec<Edit> = row_blocks(rope, selections)
        .into_iter()
        .flatten()
        .filter_map(|row| {
            let text = row_text(rope, row);
            let remove = if text.starts_with('\t') {
                1
            } else {
                text.bytes()
                    .take(tab_size)
                    .take_while(|b| *b == b' ')
                    .count()
            };
            let start = row_start(rope, row);
            (remove > 0).then(|| Edit::delete(start..start + remove))
        })
        .collect();
    let selections = selections.map_through(&edits);
    Change { edits, selections }
}

// ---------------------------------------------------------------------------
// Comments
// ---------------------------------------------------------------------------

/// Comment out the selected lines with `token` (`//`, `#`…), or uncomment
/// them when every non-blank one is already commented.
pub fn toggle_line_comment(rope: &Rope, selections: &SelectionSet, token: &str) -> Change {
    let rows: Vec<(usize, String)> = row_blocks(rope, selections)
        .into_iter()
        .flatten()
        .map(|row| (row, row_text(rope, row)))
        .collect();
    let non_blank: Vec<&(usize, String)> =
        rows.iter().filter(|(_, t)| !t.trim().is_empty()).collect();
    let candidates = if non_blank.is_empty() {
        rows.iter().collect()
    } else {
        non_blank
    };
    let commented = candidates
        .iter()
        .all(|(_, text)| text.trim_start().starts_with(token));
    let edits: Vec<Edit> = if commented {
        candidates
            .iter()
            .map(|(row, text)| {
                let at = row_start(rope, *row) + leading_whitespace(text);
                let rest = &text[leading_whitespace(text) + token.len()..];
                let space = usize::from(rest.starts_with(' '));
                Edit::delete(at..at + token.len() + space)
            })
            .collect()
    } else {
        let indent = candidates
            .iter()
            .map(|(_, text)| leading_whitespace(text))
            .min()
            .unwrap_or(0);
        candidates
            .iter()
            .map(|(row, _)| Edit::insert(row_start(rope, *row) + indent, format!("{token} ")))
            .collect()
    };
    let selections = selections.map_through(&edits);
    Change { edits, selections }
}

/// Wrap each selection in a block comment, or unwrap it when it already is
/// one. A cursor gets an empty comment with the caret inside.
pub fn toggle_block_comment(
    rope: &Rope,
    selections: &SelectionSet,
    open: &str,
    close: &str,
) -> Change {
    let mut edits = Vec::new();
    // Per selection: how to map it afterwards.
    enum Kind {
        Cursor(usize),
        Wrapped,
        Unwrapped,
    }
    let mut kinds = Vec::new();
    for s in selections.iter() {
        if s.is_empty() {
            edits.push(Edit::insert(s.head, format!("{open}  {close}")));
            kinds.push(Kind::Cursor(open.len() + 1));
            continue;
        }
        let text = rope.slice(s.to_range()).to_string();
        let lead = leading_whitespace(&text);
        let inner = text.trim();
        let trail = text.len() - text.trim_end().len();
        if inner.len() >= open.len() + close.len()
            && inner.starts_with(open)
            && inner.ends_with(close)
        {
            let open_start = s.start() + lead;
            let open_space = usize::from(inner[open.len()..].starts_with(' '));
            let close_end = s.end() - trail;
            let close_space = usize::from(
                inner.len() > open.len() + close.len() + open_space
                    && inner[..inner.len() - close.len()].ends_with(' '),
            );
            edits.push(Edit::delete(
                open_start..open_start + open.len() + open_space,
            ));
            edits.push(Edit::delete(
                close_end - close.len() - close_space..close_end,
            ));
            kinds.push(Kind::Unwrapped);
        } else {
            edits.push(Edit::insert(s.start(), format!("{open} ")));
            edits.push(Edit::insert(s.end(), format!(" {close}")));
            kinds.push(Kind::Wrapped);
        }
    }
    let mut delta: isize = 0;
    let mut cursor_targets = Vec::new();
    for (s, kind) in selections.iter().zip(&kinds) {
        if let Kind::Cursor(inside) = kind {
            cursor_targets.push(Some((s.head as isize + delta) as usize + inside));
            delta += (open.len() + close.len() + 2) as isize;
        } else {
            cursor_targets.push(None);
            let own: isize = edits
                .iter()
                .filter(|e| e.range.start >= s.start() && e.range.end <= s.end())
                .map(Edit::delta)
                .sum();
            delta += own;
        }
    }
    let after = selections
        .iter()
        .zip(&kinds)
        .zip(cursor_targets)
        .map(|((s, kind), target)| match kind {
            Kind::Cursor(_) => Selection::cursor(target.unwrap_or(0)),
            // Keep the original text selected, without the delimiters.
            Kind::Wrapped => map_selection(s, &edits, Bias::Right, Bias::Left),
            Kind::Unwrapped => map_selection(s, &edits, Bias::Left, Bias::Right),
        })
        .collect();
    Change {
        edits,
        selections: SelectionSet::from_vec(after, selections.primary_index()),
    }
}

// ---------------------------------------------------------------------------
// Multiple cursors
// ---------------------------------------------------------------------------

/// Ctrl+D. With an empty primary selection, select the word at the caret.
/// Otherwise add a selection at the next occurrence of the primary
/// selection's text (wrapping around), skipping occurrences already
/// selected. `whole_word` restricts matches to whole words (set after the
/// first Ctrl+D expanded a word). `None` when nothing changes.
pub fn select_next_occurrence(
    rope: &Rope,
    selections: &SelectionSet,
    whole_word: bool,
) -> Option<SelectionSet> {
    let primary = selections.primary();
    if primary.is_empty() {
        let word = word_range_at(rope, primary.head)?;
        let mut all = selections.as_slice().to_vec();
        all[selections.primary_index()] = Selection::range(word);
        return Some(SelectionSet::from_vec(all, selections.primary_index()));
    }
    let matches = occurrences(rope, &primary.to_range(), whole_word)?;
    let free = |r: &&Range<usize>| !selections.overlaps(r);
    let next = matches
        .iter()
        .filter(|r| r.start >= primary.end())
        .find(free)
        .or_else(|| matches.iter().find(free))?;
    let mut set = selections.clone();
    let mut selection = Selection::range(next.clone());
    if primary.is_reversed() {
        selection = Selection::new(next.end, next.start);
    }
    set.add(selection);
    Some(set)
}

/// Ctrl+Shift+L: select every occurrence of the primary selection's text
/// (or of the word at the caret).
pub fn select_all_occurrences(
    rope: &Rope,
    selections: &SelectionSet,
    whole_word: bool,
) -> Option<SelectionSet> {
    let primary = selections.primary();
    let (range, whole_word) = if primary.is_empty() {
        (word_range_at(rope, primary.head)?, true)
    } else {
        (primary.to_range(), whole_word)
    };
    let matches = occurrences(rope, &range, whole_word)?;
    let primary_index = matches.iter().position(|r| *r == range).unwrap_or(0);
    Some(SelectionSet::from_vec(
        matches.into_iter().map(Selection::range).collect(),
        primary_index,
    ))
}

fn occurrences(rope: &Rope, range: &Range<usize>, whole_word: bool) -> Option<Vec<Range<usize>>> {
    let needle = rope.slice(range.clone()).to_string();
    let query = SearchQuery {
        pattern: needle,
        regex: false,
        case_sensitive: true,
        whole_word: whole_word && is_whole_word(rope, range),
    };
    let matcher = query.compile().ok()?;
    Some(matcher.find_all(&rope.to_string()))
}

/// Add a cursor on the line above the topmost selection (or below the
/// bottom one), at the same display column. Unchanged at the first or last
/// line.
pub fn add_cursor_vertical(
    rope: &Rope,
    selections: &SelectionSet,
    up: bool,
    tab_size: usize,
) -> SelectionSet {
    let edge = if up {
        selections.first()
    } else {
        selections.last()
    };
    let row = row_of(rope, edge.head);
    let target = if up {
        match row.checked_sub(1) {
            Some(r) => r,
            None => return selections.clone(),
        }
    } else if row + 1 < row_count(rope) {
        row + 1
    } else {
        return selections.clone();
    };
    let column = edge
        .goal_column
        .unwrap_or_else(|| column_of(rope, edge.head, tab_size));
    let (offset, _) = offset_at_column(&row_text(rope, target), column, tab_size);
    let mut cursor = Selection::cursor(row_start(rope, target) + offset);
    cursor.goal_column = Some(column);
    let mut set = selections.clone();
    set.add(cursor);
    set
}

/// A row and display column.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct RowColumn {
    pub row: usize,
    pub column: usize,
}

impl RowColumn {
    pub fn new(row: usize, column: usize) -> Self {
        Self { row, column }
    }
}

/// Column (box) selection from `from` to `to`: one selection per row
/// between the two display columns. Rows too short to reach the box are
/// skipped, unless every row is.
pub fn column_selection(
    rope: &Rope,
    from: RowColumn,
    to: RowColumn,
    tab_size: usize,
) -> SelectionSet {
    let last_row = row_count(rope) - 1;
    let (top, bottom) = (from.row.min(to.row), from.row.max(to.row).min(last_row));
    let left = from.column.min(to.column);
    let mut selections = Vec::new();
    let mut primary = 0;
    for row in top..=bottom {
        let line = row_text(rope, row);
        let width = display_width(&line, 0, tab_size);
        if width < left && from.column != to.column {
            continue;
        }
        let start = row_start(rope, row);
        let (anchor, _) = offset_at_column(&line, from.column, tab_size);
        let (head, _) = offset_at_column(&line, to.column, tab_size);
        let mut selection = Selection::new(start + anchor, start + head);
        selection.goal_column = Some(to.column);
        if row == to.row {
            primary = selections.len();
        }
        selections.push(selection);
    }
    if selections.is_empty() {
        let row = to.row.min(last_row);
        let line = row_text(rope, row);
        let (head, _) = offset_at_column(&line, to.column, tab_size);
        selections.push(Selection::cursor(row_start(rope, row) + head));
    }
    SelectionSet::from_vec(selections, primary)
}

// ---------------------------------------------------------------------------
// Clipboard
// ---------------------------------------------------------------------------

/// What Copy/Cut put on the clipboard.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Clipboard {
    /// The text as other programs see it.
    pub text: String,
    /// One piece per selection, for pasting back into as many cursors.
    pub pieces: Vec<String>,
    /// Whole lines were copied (copy with no selection): paste inserts
    /// them above the caret's line instead of at the caret.
    pub whole_lines: bool,
}

impl Clipboard {
    /// Text that came from another program.
    pub fn from_text(text: impl Into<String>) -> Self {
        let text = text.into();
        Self {
            pieces: vec![text.clone()],
            text,
            whole_lines: false,
        }
    }
}

/// Copy: the selected texts, or the caret lines when nothing is selected.
pub fn copy(rope: &Rope, selections: &SelectionSet) -> Clipboard {
    if selections.iter().all(Selection::is_empty) {
        let mut rows: Vec<usize> = selections.iter().map(|s| row_of(rope, s.head)).collect();
        rows.dedup();
        let pieces: Vec<String> = rows
            .into_iter()
            .map(|row| format!("{}\n", row_text(rope, row)))
            .collect();
        return Clipboard {
            text: pieces.concat(),
            pieces,
            whole_lines: true,
        };
    }
    let pieces: Vec<String> = selections
        .iter()
        .filter(|s| !s.is_empty())
        .map(|s| rope.slice(s.to_range()).to_string())
        .collect();
    Clipboard {
        text: pieces.join("\n"),
        pieces,
        whole_lines: false,
    }
}

/// Cut: copy, then delete what was copied (whole lines without a
/// selection).
pub fn cut(rope: &Rope, selections: &SelectionSet) -> (Clipboard, Change) {
    let clipboard = copy(rope, selections);
    let change = if clipboard.whole_lines {
        delete_lines(rope, selections)
    } else {
        let ranges = selections.iter().map(Selection::to_range).collect();
        delete_ranges(selections, ranges)
    };
    (clipboard, change)
}

/// Paste. With as many cursors as copied pieces (or lines of foreign text)
/// each cursor gets one; whole lines go above the caret line.
pub fn paste(rope: &Rope, selections: &SelectionSet, clipboard: &Clipboard) -> Change {
    let n = selections.len();
    if n > 1 && clipboard.pieces.len() == n {
        return insert_each(selections, |i| {
            clipboard.pieces[i]
                .strip_suffix('\n')
                .filter(|_| clipboard.whole_lines)
                .unwrap_or(&clipboard.pieces[i])
                .to_string()
        });
    }
    if n > 1 && !clipboard.whole_lines {
        let lines: Vec<&str> = clipboard.text.lines().collect();
        if lines.len() == n {
            return insert_each(selections, |i| lines[i].to_string());
        }
    }
    if clipboard.whole_lines && selections.iter().all(Selection::is_empty) {
        let mut rows: Vec<usize> = selections.iter().map(|s| row_of(rope, s.head)).collect();
        rows.dedup();
        let edits: Vec<Edit> = rows
            .iter()
            .map(|row| Edit::insert(row_start(rope, *row), clipboard.text.clone()))
            .collect();
        // Carets keep their place in their (moved down) line.
        let selections = selections.map_through(&edits);
        return Change { edits, selections };
    }
    insert(selections, &clipboard.text)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::edit::apply_to_string;

    /// Text with `|` marking cursors and `[`…`]` selections (caret at `]`).
    fn parse(marked: &str) -> (Rope, SelectionSet) {
        let mut text = String::new();
        let mut selections = Vec::new();
        let mut open = None;
        for c in marked.chars() {
            match c {
                '|' => selections.push(Selection::cursor(text.len())),
                '[' => open = Some(text.len()),
                ']' => selections.push(Selection::new(open.take().unwrap(), text.len())),
                _ => text.push(c),
            }
        }
        (Rope::from_str(&text), SelectionSet::from_vec(selections, 0))
    }

    fn render(text: &str, selections: &SelectionSet) -> String {
        let mut marks: Vec<(usize, char)> = Vec::new();
        for s in selections.iter() {
            if s.is_empty() {
                marks.push((s.head, '|'));
            } else {
                marks.push((s.start(), '['));
                marks.push((s.end(), ']'));
            }
        }
        marks.sort_by_key(|(at, c)| (*at, *c == '['));
        let mut out = String::new();
        let mut last = 0;
        for (at, c) in marks {
            out.push_str(&text[last..at]);
            out.push(c);
            last = at;
        }
        out.push_str(&text[last..]);
        out
    }

    fn run(marked: &str, f: impl Fn(&Rope, &SelectionSet) -> Change) -> String {
        let (rope, selections) = parse(marked);
        let change = f(&rope, &selections);
        let text = apply_to_string(&rope.to_string(), &change.edits);
        render(&text, &change.selections)
    }

    #[test]
    fn typing_at_several_cursors() {
        assert_eq!(run("a|b|c", |_, s| insert(s, "xy")), "axy|bxy|c");
        assert_eq!(run("[ab]c[d]", |_, s| insert(s, "-")), "-|c-|");
        assert_eq!(run("ab|c|", backspace), "a|");
        assert_eq!(run("a|b|c", backspace), "|c");
        assert_eq!(run("|e\u{301}x", delete_forward), "|x");
        assert_eq!(run("👍|", backspace), "|");
    }

    #[test]
    fn duplicate_lines_down_and_up() {
        assert_eq!(
            run("one\nt|wo\nthree", |r, s| duplicate_lines(r, s, true)),
            "one\ntwo\nt|wo\nthree"
        );
        assert_eq!(
            run("one\nt|wo\nthree", |r, s| duplicate_lines(r, s, false)),
            "one\nt|wo\ntwo\nthree"
        );
        assert_eq!(
            run("a|\nb|\nc", |r, s| duplicate_lines(r, s, true)),
            "a\nb\na|\nb|\nc"
        );
        assert_eq!(
            run("last|", |r, s| duplicate_lines(r, s, true)),
            "last\nlast|"
        );
    }

    #[test]
    fn move_lines_up_and_down() {
        assert_eq!(
            run("one\ntwo\nth|ree", |r, s| move_lines(r, s, true).unwrap()),
            "one\nth|ree\ntwo"
        );
        assert_eq!(
            run("o|ne\ntwo\nthree", |r, s| move_lines(r, s, false).unwrap()),
            "two\no|ne\nthree"
        );
        assert_eq!(
            run("a\n[b\nc]\nd", |r, s| move_lines(r, s, true).unwrap()),
            "[b\nc]\na\nd"
        );
        let (rope, selections) = parse("|top\nbottom");
        assert!(move_lines(&rope, &selections, true).is_none());
        // A selection ending at the start of a line does not move that line.
        assert_eq!(
            run("a\n[b\n]c", |r, s| move_lines(r, s, true).unwrap()),
            "[b\n]a\nc"
        );
    }

    #[test]
    fn delete_lines_removes_whole_lines() {
        assert_eq!(run("a\nb|\nc", delete_lines), "a\n|c");
        assert_eq!(run("a\nb\nc|", delete_lines), "a\nb|");
        assert_eq!(run("only|", delete_lines), "|");
    }

    #[test]
    fn line_comments_toggle() {
        let commented = run("fn a() {\n    [let x = 1;\n\n    let y = 2;]\n}", |r, s| {
            toggle_line_comment(r, s, "//")
        });
        assert_eq!(
            commented,
            "fn a() {\n    [// let x = 1;\n\n    // let y = 2;]\n}"
        );
        let uncommented = run(
            "fn a() {\n    [// let x = 1;\n\n    //let y = 2;]\n}",
            |r, s| toggle_line_comment(r, s, "//"),
        );
        assert_eq!(
            uncommented,
            "fn a() {\n    [let x = 1;\n\n    let y = 2;]\n}"
        );
        // Mixed: some lines commented → comment all.
        assert_eq!(
            run("[# a\nb]", |r, s| toggle_line_comment(r, s, "#")),
            "[# # a\n# b]"
        );
        assert_eq!(
            run("x = 1|", |r, s| toggle_line_comment(r, s, "#")),
            "# x = 1|"
        );
    }

    #[test]
    fn block_comments_toggle() {
        assert_eq!(
            run("a [b c] d", |r, s| toggle_block_comment(r, s, "/*", "*/")),
            "a /* [b c] */ d"
        );
        assert_eq!(
            run("a [/* b c */] d", |r, s| toggle_block_comment(
                r, s, "/*", "*/"
            )),
            "a [b c] d"
        );
        assert_eq!(
            run("x|", |r, s| toggle_block_comment(r, s, "<!--", "-->")),
            "x<!-- | -->"
        );
    }

    #[test]
    fn indent_and_outdent() {
        assert_eq!(
            run("[a\n\nb]", |r, s| indent_lines(r, s, "    ")),
            "[    a\n\n    b]"
        );
        assert_eq!(run("    a|\n\tb", |r, s| outdent_lines(r, s, 4)), "a|\n\tb");
        assert_eq!(run("[  a\n\tb]", |r, s| outdent_lines(r, s, 4)), "[a\nb]");
    }

    #[test]
    fn next_occurrence() {
        let (rope, selections) = parse("foo f|oo food foo");
        let selections = select_next_occurrence(&rope, &selections, false).unwrap();
        assert_eq!(render(&rope.to_string(), &selections), "foo [foo] food foo");
        let selections = select_next_occurrence(&rope, &selections, true).unwrap();
        assert_eq!(
            render(&rope.to_string(), &selections),
            "foo [foo] food [foo]"
        );
        let selections = select_next_occurrence(&rope, &selections, true).unwrap();
        assert_eq!(
            render(&rope.to_string(), &selections),
            "[foo] [foo] food [foo]"
        );
        // Everything selected: nothing more to add.
        assert!(select_next_occurrence(&rope, &selections, true).is_none());
        // Without whole words, "food" matches too.
        let (rope, selections) = parse("[foo] food");
        let selections = select_next_occurrence(&rope, &selections, false).unwrap();
        assert_eq!(render(&rope.to_string(), &selections), "[foo] [foo]d");
    }

    #[test]
    fn all_occurrences() {
        let (rope, selections) = parse("ab x|y ab xy xyz");
        let selections = select_all_occurrences(&rope, &selections, false).unwrap();
        assert_eq!(
            render(&rope.to_string(), &selections),
            "ab [xy] ab [xy] xyz"
        );
        assert_eq!(selections.primary().to_range(), 3..5);
    }

    #[test]
    fn cursors_above_and_below_keep_the_column() {
        let (rope, selections) = parse("long line\nab\nlong li|ne");
        let up = add_cursor_vertical(&rope, &selections, true, 4);
        assert_eq!(render(&rope.to_string(), &up), "long line\nab|\nlong li|ne");
        let up = add_cursor_vertical(&rope, &up, true, 4);
        assert_eq!(
            render(&rope.to_string(), &up),
            "long li|ne\nab|\nlong li|ne"
        );
        let again = add_cursor_vertical(&rope, &up, true, 4);
        assert_eq!(again, up);
        let (rope, selections) = parse("\tx|\nabcdefgh");
        let down = add_cursor_vertical(&rope, &selections, false, 4);
        assert_eq!(render(&rope.to_string(), &down), "\tx|\nabcde|fgh");
    }

    #[test]
    fn box_selection() {
        let rope = Rope::from_str("abcdef\nab\nabcdef");
        let set = column_selection(&rope, RowColumn::new(0, 1), RowColumn::new(2, 4), 4);
        assert_eq!(render(&rope.to_string(), &set), "a[bcd]ef\na[b]\na[bcd]ef");
        let set = column_selection(&rope, RowColumn::new(0, 4), RowColumn::new(2, 5), 4);
        assert_eq!(set.len(), 2);
    }

    #[test]
    fn clipboard_with_several_cursors() {
        let (rope, selections) = parse("[a]1 [b]2");
        let clipboard = copy(&rope, &selections);
        assert_eq!(clipboard.text, "a\nb");
        let (rope2, targets) = parse("x| y|");
        let change = paste(&rope2, &targets, &clipboard);
        let text = apply_to_string(&rope2.to_string(), &change.edits);
        assert_eq!(render(&text, &change.selections), "xa| yb|");
        // Foreign text with as many lines as cursors is spread too.
        let change = paste(&rope2, &targets, &Clipboard::from_text("1\n2"));
        assert_eq!(apply_to_string(&rope2.to_string(), &change.edits), "x1 y2");
        // A single cursor gets everything.
        let (rope3, one) = parse("|");
        let change = paste(&rope3, &one, &clipboard);
        assert_eq!(apply_to_string(&rope3.to_string(), &change.edits), "a\nb");
    }

    #[test]
    fn whole_line_copy_and_paste() {
        let (rope, selections) = parse("one\nt|wo\nthree");
        let (clipboard, change) = cut(&rope, &selections);
        assert_eq!(clipboard.text, "two\n");
        assert!(clipboard.whole_lines);
        let text = apply_to_string(&rope.to_string(), &change.edits);
        assert_eq!(render(&text, &change.selections), "one\n|three");
        let rope = Rope::from_str(&text);
        let change = paste(&rope, &change.selections, &clipboard);
        let text = apply_to_string(&rope.to_string(), &change.edits);
        assert_eq!(render(&text, &change.selections), "one\ntwo\n|three");
    }
}
