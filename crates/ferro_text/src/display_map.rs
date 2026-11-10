//! The display map: how buffer lines become screen rows, with folds, soft
//! wrap and tab expansion.
//!
//! Each buffer line keeps its own layout (width and wrap points); a Fenwick
//! tree over the number of screen rows per line turns "display row" into
//! "buffer line" and back in O(log n). An edit lays out again only the lines
//! it touched.

use std::ops::{Range, RangeInclusive};

use ropey::Rope;

use crate::{
    edit::Edit,
    ops::{row_count, row_end, row_of, row_start},
    unicode::{display_width, for_each_column, offset_at_column},
};

/// Layout settings.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct DisplayMapConfig {
    pub tab_size: usize,
    /// Soft wrap at this many columns; `None` turns soft wrap off.
    pub wrap_column: Option<usize>,
}

impl Default for DisplayMapConfig {
    fn default() -> Self {
        Self {
            tab_size: 4,
            wrap_column: None,
        }
    }
}

/// A position on screen: display row and column.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Default)]
pub struct DisplayPoint {
    pub row: usize,
    pub column: usize,
}

impl DisplayPoint {
    pub fn new(row: usize, column: usize) -> Self {
        Self { row, column }
    }
}

/// One screen row, for drawing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DisplayRow {
    pub buffer_row: usize,
    /// 0 for the line's first screen row, then 1, 2… for wrapped parts.
    pub wrap_index: usize,
    /// Byte range of the buffer shown on this row.
    pub range: Range<usize>,
    /// Display column the row's text starts at (tabs keep expanding
    /// relative to the line start).
    pub start_column: usize,
    /// The row is the header of a fold; the folded lines follow it hidden.
    pub folded: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct LineLayout {
    width: usize,
    /// Byte offsets within the line where wrapped rows start.
    wraps: Vec<usize>,
}

impl LineLayout {
    fn new(text: &str, config: &DisplayMapConfig) -> Self {
        let width = display_width(text, 0, config.tab_size);
        let wraps = match config.wrap_column {
            Some(limit) if width > limit => wrap_points(text, config.tab_size, limit.max(1)),
            _ => Vec::new(),
        };
        Self { width, wraps }
    }

    fn row_count(&self) -> usize {
        1 + self.wraps.len()
    }

    /// Byte range within the line of wrapped row `index`.
    fn segment(&self, index: usize, line_len: usize) -> Range<usize> {
        let start = if index == 0 { 0 } else { self.wraps[index - 1] };
        let end = self.wraps.get(index).copied().unwrap_or(line_len);
        start..end
    }
}

/// Greedy word wrap: break after the last space that fits, or inside a
/// word longer than the row.
fn wrap_points(text: &str, tab_size: usize, limit: usize) -> Vec<usize> {
    let mut wraps = Vec::new();
    let mut row_start = 0;
    let mut row_start_column = 0;
    let mut candidate: Option<(usize, usize)> = None;
    let bytes = text.as_bytes();
    for_each_column(text, 0, tab_size, |offset, before, after| {
        while offset > row_start && after - row_start_column > limit {
            let (at, column) = match candidate {
                Some((at, column)) if at > row_start && at <= offset => (at, column),
                _ => (offset, before),
            };
            wraps.push(at);
            row_start = at;
            row_start_column = column;
            candidate = None;
        }
        if matches!(bytes[offset], b' ' | b'\t') {
            candidate = Some((offset + 1, after));
        }
    });
    wraps
}

/// Prefix sums over screen rows per buffer line.
#[derive(Debug, Clone, Default)]
struct RowIndex {
    tree: Vec<usize>,
    values: Vec<usize>,
}

impl RowIndex {
    fn build(values: Vec<usize>) -> Self {
        let mut tree = vec![0; values.len() + 1];
        for (i, v) in values.iter().enumerate() {
            let i = i + 1;
            tree[i] += v;
            let parent = i + i.isolate_lowest_one();
            if parent < tree.len() {
                tree[parent] += tree[i];
            }
        }
        Self { tree, values }
    }

    fn set(&mut self, index: usize, value: usize) {
        let old = self.values[index];
        if old == value {
            return;
        }
        self.values[index] = value;
        let mut i = index + 1;
        while i < self.tree.len() {
            self.tree[i] = self.tree[i] + value - old;
            i += i.isolate_lowest_one();
        }
    }

    /// Sum of the first `count` values.
    fn prefix(&self, count: usize) -> usize {
        let mut sum = 0;
        let mut i = count.min(self.values.len());
        while i > 0 {
            sum += self.tree[i];
            i -= i.isolate_lowest_one();
        }
        sum
    }

    fn total(&self) -> usize {
        self.prefix(self.values.len())
    }

    /// The index whose rows contain row `target`, and the row within it.
    /// Indexes with zero rows are skipped.
    fn find(&self, target: usize) -> (usize, usize) {
        let mut position = 0;
        let mut remaining = target;
        let mut step = (self.tree.len()).next_power_of_two() / 2;
        while step > 0 {
            let next = position + step;
            if next < self.tree.len() && self.tree[next] <= remaining {
                position = next;
                remaining -= self.tree[next];
            }
            step /= 2;
        }
        (position, remaining)
    }
}

/// Folded line range: `start` stays visible, `start + 1 ..= end` is
/// hidden.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Fold {
    pub start: usize,
    pub end: usize,
}

impl Fold {
    fn hides(&self, row: usize) -> bool {
        row > self.start && row <= self.end
    }
}

/// See the module documentation.
#[derive(Debug, Clone)]
pub struct DisplayMap {
    config: DisplayMapConfig,
    layouts: Vec<LineLayout>,
    folds: Vec<Fold>,
    rows: RowIndex,
}

impl DisplayMap {
    pub fn new(rope: &Rope, config: DisplayMapConfig) -> Self {
        let mut map = Self {
            config,
            layouts: Vec::new(),
            folds: Vec::new(),
            rows: RowIndex::default(),
        };
        map.relayout(rope);
        map
    }

    pub fn config(&self) -> DisplayMapConfig {
        self.config
    }

    /// Change tab size or soft wrap; lays every line out again.
    pub fn set_config(&mut self, rope: &Rope, config: DisplayMapConfig) {
        if self.config != config {
            self.config = config;
            self.relayout(rope);
        }
    }

    fn relayout(&mut self, rope: &Rope) {
        let config = self.config;
        self.layouts = rope
            .lines(crate::LINES)
            .map(|slice| {
                // Most lines sit in one rope chunk: no copy needed.
                let text = match slice.as_str() {
                    Some(text) => std::borrow::Cow::Borrowed(text),
                    None => std::borrow::Cow::Owned(slice.to_string()),
                };
                LineLayout::new(text.strip_suffix('\n').unwrap_or(&text), &config)
            })
            .collect();
        // `lines` yields no empty line after a final `\n`; rows do.
        while self.layouts.len() < row_count(rope) {
            self.layouts.push(LineLayout::new("", &config));
        }
        self.rebuild_index();
    }

    fn visible_rows(&self, row: usize) -> usize {
        if self.folds.iter().any(|f| f.hides(row)) {
            0
        } else {
            self.layouts[row].row_count()
        }
    }

    fn rebuild_index(&mut self) {
        let mut values: Vec<usize> = self.layouts.iter().map(LineLayout::row_count).collect();
        for fold in &self.folds {
            for value in &mut values[fold.start + 1..=fold.end.min(self.layouts.len() - 1)] {
                *value = 0;
            }
        }
        self.rows = RowIndex::build(values);
    }

    /// Update after `edits` (normalized, in `before`'s coordinates) turned
    /// `before` into `after`. Only touched lines are laid out again; folds
    /// touching an edit open.
    pub fn sync(&mut self, before: &Rope, after: &Rope, edits: &[Edit]) {
        let summaries: Vec<(usize, usize, usize)> = edits
            .iter()
            .map(|e| (e.range.start, e.range.end, e.text.len()))
            .collect();
        self.sync_summaries(before, after, &summaries);
    }

    /// [`DisplayMap::sync`] with edits as `(start, end, new_len)`, as
    /// [`crate::Buffer::last_change`] returns them.
    pub fn sync_summaries(&mut self, before: &Rope, after: &Rope, edits: &[(usize, usize, usize)]) {
        if edits.is_empty() {
            return;
        }
        // Row ranges per edit, merged where they share lines.
        let mut groups: Vec<(RangeInclusive<usize>, RangeInclusive<usize>)> = Vec::new();
        // Where each edit lands: after everything the earlier edits did,
        // including insertions at the same position.
        let mut delta: isize = 0;
        for &(start, end, new_len) in edits {
            let old = row_of(before, start)..=row_of(before, end);
            let new_start = (start as isize + delta) as usize;
            delta += new_len as isize - (end - start) as isize;
            let new = row_of(after, new_start)..=row_of(after, new_start + new_len);
            match groups.last_mut() {
                Some((g_old, g_new)) if *old.start() <= *g_old.end() => {
                    *g_old = *g_old.start()..=(*g_old.end()).max(*old.end());
                    *g_new = *g_new.start()..=(*g_new.end()).max(*new.end());
                }
                _ => groups.push((old, new)),
            }
        }
        let fold_count = self.folds.len();
        // Lines moved when any group changes its number of lines (even if
        // the total stays the same); then the row index is rebuilt.
        let mut lines_moved = false;
        let mut changed_rows = Vec::new();
        for (old, new) in groups.iter().rev() {
            let layouts: Vec<LineLayout> = new
                .clone()
                .map(|row| LineLayout::new(&line(after, row), &self.config))
                .collect();
            self.layouts.splice(old.clone(), layouts);
            let delta = (new.end() - new.start()) as isize - (old.end() - old.start()) as isize;
            lines_moved |= delta != 0;
            self.folds
                .retain(|f| f.end < *old.start() || f.start > *old.end());
            for fold in &mut self.folds {
                if fold.start > *old.end() {
                    fold.start = (fold.start as isize + delta) as usize;
                    fold.end = (fold.end as isize + delta) as usize;
                }
            }
            changed_rows.push(new.clone());
        }
        debug_assert_eq!(self.layouts.len(), row_count(after));
        // Opened folds show rows the edit did not touch.
        if lines_moved || self.folds.len() != fold_count {
            self.rebuild_index();
        } else {
            for rows in changed_rows {
                for row in rows {
                    let value = self.visible_rows(row);
                    self.rows.set(row, value);
                }
            }
        }
    }

    /// Fold buffer rows `rows` (the first stays visible). Overlapping folds
    /// are replaced. A range of one line does nothing.
    pub fn fold(&mut self, rows: RangeInclusive<usize>) {
        let last = self.layouts.len().saturating_sub(1);
        let fold = Fold {
            start: *rows.start(),
            end: (*rows.end()).min(last),
        };
        if fold.end <= fold.start {
            return;
        }
        self.folds
            .retain(|f| f.end < fold.start || f.start > fold.end);
        let at = self.folds.partition_point(|f| f.start < fold.start);
        self.folds.insert(at, fold);
        for row in fold.start + 1..=fold.end {
            self.rows.set(row, 0);
        }
    }

    /// Open the folds that hide or start at `row`. Returns whether any
    /// opened.
    pub fn unfold(&mut self, row: usize) -> bool {
        let opened: Vec<Fold> = self
            .folds
            .iter()
            .filter(|f| f.start == row || f.hides(row))
            .copied()
            .collect();
        if opened.is_empty() {
            return false;
        }
        self.folds.retain(|f| !opened.contains(f));
        for fold in opened {
            for row in fold.start + 1..=fold.end {
                let value = self.visible_rows(row);
                self.rows.set(row, value);
            }
        }
        true
    }

    pub fn folds(&self) -> &[Fold] {
        &self.folds
    }

    pub fn is_hidden(&self, buffer_row: usize) -> bool {
        self.folds.iter().any(|f| f.hides(buffer_row))
    }

    /// Screen rows in total.
    pub fn row_count(&self) -> usize {
        self.rows.total()
    }

    /// Display width of the widest line, for horizontal scrolling.
    pub fn max_width(&self) -> usize {
        self.layouts.iter().map(|l| l.width).max().unwrap_or(0)
    }

    /// The screen position of a buffer offset. Offsets inside a fold map
    /// to the end of its header row.
    pub fn to_display(&self, rope: &Rope, offset: usize) -> DisplayPoint {
        let offset = offset.min(rope.len());
        let mut row = row_of(rope, offset);
        let mut column_offset = offset - row_start(rope, row);
        if let Some(fold) = self.folds.iter().find(|f| f.hides(row)) {
            row = fold.start;
            column_offset = row_end(rope, row) - row_start(rope, row);
        }
        let layout = &self.layouts[row];
        let wrap_index = layout.wraps.partition_point(|w| *w <= column_offset);
        let text = line(rope, row);
        let segment = layout.segment(wrap_index, text.len());
        let segment_start_column = display_width(&text[..segment.start], 0, self.config.tab_size);
        let column = display_width(
            &text[segment.start..column_offset],
            segment_start_column,
            self.config.tab_size,
        );
        DisplayPoint::new(self.rows.prefix(row) + wrap_index, column)
    }

    /// The buffer offset at a screen position (clamped to the row's text).
    pub fn to_buffer(&self, rope: &Rope, point: DisplayPoint) -> usize {
        if self.layouts.is_empty() {
            return 0;
        }
        let total = self.row_count();
        if point.row >= total {
            return rope.len();
        }
        let (row, wrap_index) = self.rows.find(point.row);
        let text = line(rope, row);
        let segment = self.layouts[row].segment(wrap_index, text.len());
        let start_column = display_width(&text[..segment.start], 0, self.config.tab_size);
        // Columns within the segment count from the segment's start.
        let offset = offset_in_segment(
            &text[segment.clone()],
            start_column,
            point.column,
            self.config.tab_size,
        );
        row_start(rope, row) + segment.start + offset
    }

    /// The screen rows in `rows` (clamped), for drawing.
    pub fn display_rows(&self, rope: &Rope, rows: Range<usize>) -> Vec<DisplayRow> {
        let end = rows.end.min(self.row_count());
        if rows.start >= end {
            return Vec::new();
        }
        let mut out = Vec::with_capacity(end - rows.start);
        let (mut buffer_row, mut wrap_index) = self.rows.find(rows.start);
        while out.len() < end - rows.start && buffer_row < self.layouts.len() {
            if self.rows.values[buffer_row] == 0 {
                buffer_row += 1;
                wrap_index = 0;
                continue;
            }
            let layout = &self.layouts[buffer_row];
            let start = row_start(rope, buffer_row);
            let line_len = row_end(rope, buffer_row) - start;
            let segment = layout.segment(wrap_index, line_len);
            let start_column = if segment.start == 0 {
                0
            } else {
                display_width(
                    &rope.slice(start..start + segment.start).to_string(),
                    0,
                    self.config.tab_size,
                )
            };
            out.push(DisplayRow {
                buffer_row,
                wrap_index,
                range: start + segment.start..start + segment.end,
                start_column,
                folded: wrap_index == 0 && self.folds.iter().any(|f| f.start == buffer_row),
            });
            wrap_index += 1;
            if wrap_index >= layout.row_count() {
                buffer_row += 1;
                wrap_index = 0;
            }
        }
        out
    }
}

fn line(rope: &Rope, row: usize) -> String {
    rope.slice(row_start(rope, row)..row_end(rope, row))
        .to_string()
}

/// The byte offset in a wrapped segment nearest to `target` columns from
/// the segment's start; tabs expand relative to the line start, which is
/// `start_column` columns before the segment.
fn offset_in_segment(segment: &str, start_column: usize, target: usize, tab_size: usize) -> usize {
    if start_column == 0 {
        return offset_at_column(segment, target, tab_size).0;
    }
    let target = start_column + target;
    let mut result = None;
    for_each_column(segment, start_column, tab_size, |offset, before, after| {
        if result.is_none() && before >= target {
            result = Some(offset);
        } else if result.is_none() && after > target {
            result = Some(if target - before <= after - target {
                offset
            } else {
                offset + segment[offset..].chars().next().map_or(0, char::len_utf8)
            });
        }
    });
    result.unwrap_or(segment.len())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::edit::{apply_to_string, normalize};

    fn map(text: &str, wrap: Option<usize>) -> (Rope, DisplayMap) {
        let rope = Rope::from_str(text);
        let map = DisplayMap::new(
            &rope,
            DisplayMapConfig {
                tab_size: 4,
                wrap_column: wrap,
            },
        );
        (rope, map)
    }

    fn rows_text(rope: &Rope, map: &DisplayMap) -> Vec<String> {
        map.display_rows(rope, 0..usize::MAX)
            .into_iter()
            .map(|r| rope.slice(r.range).to_string())
            .collect()
    }

    #[test]
    fn soft_wrap_breaks_at_spaces() {
        let (rope, map) = map("aaa bbb ccc ddd\nshort\nxxxxxxxxxx", Some(8));
        assert_eq!(
            rows_text(&rope, &map),
            vec!["aaa bbb ", "ccc ddd", "short", "xxxxxxxx", "xx"]
        );
        assert_eq!(map.row_count(), 5);
        // "ccc" starts the second screen row.
        assert_eq!(map.to_display(&rope, 8), DisplayPoint::new(1, 0));
        assert_eq!(map.to_buffer(&rope, DisplayPoint::new(1, 2)), 10);
        assert_eq!(map.to_buffer(&rope, DisplayPoint::new(4, 1)), 31);
    }

    #[test]
    fn tabs_and_wide_characters() {
        let (rope, map) = map("\tx\n漢字z", None);
        assert_eq!(map.to_display(&rope, 1), DisplayPoint::new(0, 4));
        assert_eq!(map.to_display(&rope, 9), DisplayPoint::new(1, 4));
        assert_eq!(map.to_buffer(&rope, DisplayPoint::new(1, 2)), 6);
        assert_eq!(map.to_buffer(&rope, DisplayPoint::new(0, 9)), 2);
        assert_eq!(map.max_width(), 5);
    }

    #[test]
    fn folds_hide_rows() {
        let (rope, mut map) = map("fn a() {\n  1\n  2\n}\nend", None);
        map.fold(0..=3);
        assert_eq!(map.row_count(), 2);
        assert_eq!(rows_text(&rope, &map), vec!["fn a() {", "end"]);
        assert!(map.display_rows(&rope, 0..1)[0].folded);
        // Inside the fold maps to the end of the header.
        assert_eq!(map.to_display(&rope, 12), DisplayPoint::new(0, 8));
        assert_eq!(map.to_buffer(&rope, DisplayPoint::new(1, 0)), 19);
        assert!(map.unfold(2));
        assert_eq!(map.row_count(), 5);
    }

    #[test]
    fn edits_update_only_what_changed() {
        let text = "one\ntwo\nthree\nfour";
        let (before, mut map) = map(text, None);
        map.fold(2..=3);
        let edits = normalize(
            vec![Edit::insert(3, "\nnew"), Edit::insert(0, "0")],
            &before,
        )
        .unwrap();
        let after = Rope::from_str(&apply_to_string(text, &edits));
        map.sync(&before, &after, &edits);
        assert_eq!(map.folds(), &[Fold { start: 3, end: 4 }]);
        assert_eq!(rows_text(&after, &map), vec!["0one", "new", "two", "three"]);
        // A fold the edit touches opens.
        let edits = normalize(vec![Edit::delete(14..15)], &after).unwrap();
        let after2 = Rope::from_str(&apply_to_string(&after.to_string(), &edits));
        map.sync(&after, &after2, &edits);
        assert!(map.folds().is_empty());
        assert_eq!(map.row_count(), row_count(&after2));
    }

    #[test]
    fn fenwick_find_skips_empty_lines() {
        let index = RowIndex::build(vec![1, 0, 0, 3, 1]);
        assert_eq!(index.total(), 5);
        assert_eq!(index.find(0), (0, 0));
        assert_eq!(index.find(1), (3, 0));
        assert_eq!(index.find(3), (3, 2));
        assert_eq!(index.find(4), (4, 0));
        assert_eq!(index.prefix(4), 4);
    }
}
