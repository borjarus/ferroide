//! Selections and multiple cursors.

use std::ops::Range;

use crate::{
    Bias,
    edit::{Edit, map_offset},
};

/// One selection: `anchor` stays put while `head` (the caret) moves.
/// An empty selection is a cursor.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Selection {
    pub anchor: usize,
    pub head: usize,
    /// Display column vertical moves try to return to.
    pub goal_column: Option<usize>,
}

impl Selection {
    pub fn new(anchor: usize, head: usize) -> Self {
        Self {
            anchor,
            head,
            goal_column: None,
        }
    }

    pub fn cursor(offset: usize) -> Self {
        Self::new(offset, offset)
    }

    /// A selection of `range`, caret at its end.
    pub fn range(range: Range<usize>) -> Self {
        Self::new(range.start, range.end)
    }

    pub fn start(&self) -> usize {
        self.anchor.min(self.head)
    }

    pub fn end(&self) -> usize {
        self.anchor.max(self.head)
    }

    pub fn to_range(&self) -> Range<usize> {
        self.start()..self.end()
    }

    pub fn is_empty(&self) -> bool {
        self.anchor == self.head
    }

    /// The caret is before the anchor.
    pub fn is_reversed(&self) -> bool {
        self.head < self.anchor
    }

    /// The same selection with the given bounds, keeping the direction.
    pub fn with_range(&self, range: Range<usize>) -> Self {
        if self.is_reversed() {
            Self::new(range.end, range.start)
        } else {
            Self::new(range.start, range.end)
        }
    }
}

/// All selections of an editor: sorted, never overlapping, at least one.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SelectionSet {
    selections: Vec<Selection>,
    /// Index of the primary (newest) selection.
    primary: usize,
}

impl Default for SelectionSet {
    fn default() -> Self {
        Self::single(Selection::cursor(0))
    }
}

impl SelectionSet {
    pub fn single(selection: Selection) -> Self {
        Self {
            selections: vec![selection],
            primary: 0,
        }
    }

    /// Selections in any order; overlapping ones are merged. `primary` is
    /// an index into `selections` (clamped). An empty vector gives a cursor
    /// at 0.
    pub fn from_vec(selections: Vec<Selection>, primary: usize) -> Self {
        if selections.is_empty() {
            return Self::default();
        }
        let primary = primary.min(selections.len() - 1);
        let primary_selection = selections[primary];
        let mut set = Self {
            selections,
            primary,
        };
        set.normalize(primary_selection);
        set
    }

    pub fn iter(&self) -> impl ExactSizeIterator<Item = &Selection> {
        self.selections.iter()
    }

    pub fn as_slice(&self) -> &[Selection] {
        &self.selections
    }

    pub fn len(&self) -> usize {
        self.selections.len()
    }

    /// Always false: a set holds at least one selection.
    pub fn is_empty(&self) -> bool {
        self.selections.is_empty()
    }

    pub fn primary(&self) -> Selection {
        self.selections[self.primary]
    }

    pub fn primary_index(&self) -> usize {
        self.primary
    }

    /// The first selection in the text.
    pub fn first(&self) -> Selection {
        self.selections[0]
    }

    /// The last selection in the text.
    pub fn last(&self) -> Selection {
        self.selections[self.selections.len() - 1]
    }

    /// Add a selection and make it primary; it merges with any it overlaps.
    pub fn add(&mut self, selection: Selection) {
        self.selections.push(selection);
        self.normalize(selection);
    }

    /// Keep only the primary selection.
    pub fn collapse_to_primary(&mut self) {
        let primary = self.primary();
        self.selections = vec![primary];
        self.primary = 0;
    }

    pub fn contains_range(&self, range: &Range<usize>) -> bool {
        self.selections.iter().any(|s| s.to_range() == *range)
    }

    /// Whether any selection overlaps `range` (touching does not count).
    pub fn overlaps(&self, range: &Range<usize>) -> bool {
        self.selections
            .iter()
            .any(|s| s.start() < range.end && range.start < s.end() || s.to_range() == *range)
    }

    /// The selections after `edits` (normalized, in the coordinates the
    /// selections are in). A cursor moves past text inserted at it; a
    /// non-empty selection grows to include text inserted at its edges.
    pub fn map_through(&self, edits: &[Edit]) -> Self {
        let selections = self
            .selections
            .iter()
            .map(|s| {
                if s.is_empty() {
                    let at = map_offset(s.head, edits, Bias::Right);
                    Selection::cursor(at)
                } else {
                    let start = map_offset(s.start(), edits, Bias::Left);
                    let end = map_offset(s.end(), edits, Bias::Right).max(start);
                    s.with_range(start..end)
                }
            })
            .collect();
        Self::from_vec(selections, self.primary)
    }

    /// Sort, merge overlapping selections, and point `primary` at the
    /// selection that now contains `primary_selection`.
    fn normalize(&mut self, primary_selection: Selection) {
        self.selections.sort_by_key(|s| (s.start(), s.end()));
        let mut merged: Vec<Selection> = Vec::with_capacity(self.selections.len());
        let mut primary = 0;
        for selection in self.selections.drain(..) {
            if let Some(last) = merged.last_mut() {
                let touches = selection.start() < last.end()
                    || (selection.start() == last.end()
                        && (selection.is_empty() || last.is_empty()));
                if touches {
                    let range = last.start()..last.end().max(selection.end());
                    *last = last.with_range(range);
                    if selection == primary_selection {
                        primary = merged.len() - 1;
                    }
                    continue;
                }
            }
            if selection == primary_selection {
                primary = merged.len();
            }
            merged.push(selection);
        }
        self.selections = merged;
        self.primary = primary;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn merges_overlapping_and_keeps_primary() {
        let set = SelectionSet::from_vec(
            vec![
                Selection::new(10, 12),
                Selection::new(0, 5),
                Selection::new(3, 8),
                Selection::cursor(12),
            ],
            0,
        );
        assert_eq!(set.len(), 2);
        assert_eq!(set.as_slice()[0].to_range(), 0..8);
        assert_eq!(set.as_slice()[1].to_range(), 10..12);
        assert_eq!(set.primary().to_range(), 10..12);
    }

    #[test]
    fn touching_ranges_stay_apart() {
        let set = SelectionSet::from_vec(vec![Selection::new(0, 2), Selection::new(2, 4)], 1);
        assert_eq!(set.len(), 2);
        assert_eq!(set.primary().to_range(), 2..4);
    }

    #[test]
    fn duplicate_cursors_collapse() {
        let mut set = SelectionSet::single(Selection::cursor(4));
        set.add(Selection::cursor(4));
        assert_eq!(set.len(), 1);
    }

    #[test]
    fn mapping_through_an_insert() {
        let set = SelectionSet::from_vec(vec![Selection::cursor(2), Selection::new(5, 7)], 0);
        let mapped = set.map_through(&[Edit::insert(2, "ab"), Edit::insert(5, "x")]);
        assert_eq!(mapped.as_slice()[0], Selection::cursor(4));
        assert_eq!(mapped.as_slice()[1].to_range(), 7..10);
    }

    #[test]
    fn reversed_selection_keeps_direction() {
        let s = Selection::new(9, 3);
        assert!(s.is_reversed());
        assert_eq!(s.with_range(1..2), Selection::new(2, 1));
    }
}
