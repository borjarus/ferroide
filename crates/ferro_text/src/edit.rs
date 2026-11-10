//! Text edits and how offsets move through them.

use std::ops::Range;

use ropey::Rope;

use crate::Bias;

/// Replace `range` (byte offsets in the text before the edit) with `text`.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Edit {
    pub range: Range<usize>,
    pub text: String,
}

impl Edit {
    pub fn new(range: Range<usize>, text: impl Into<String>) -> Self {
        Self {
            range,
            text: text.into(),
        }
    }

    pub fn insert(at: usize, text: impl Into<String>) -> Self {
        Self::new(at..at, text)
    }

    pub fn delete(range: Range<usize>) -> Self {
        Self::new(range, String::new())
    }

    /// Bytes the edit adds (positive) or removes (negative).
    pub fn delta(&self) -> isize {
        self.text.len() as isize - self.range.len() as isize
    }

    pub fn is_noop(&self) -> bool {
        self.range.is_empty() && self.text.is_empty()
    }
}

/// Why a set of edits cannot be applied.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum EditError {
    #[error("edit {range:?} is outside the text (length {len})")]
    OutOfBounds { range: Range<usize>, len: usize },
    #[error("edit {range:?} does not start and end on character boundaries")]
    NotCharBoundary { range: Range<usize> },
    #[error("edits {first:?} and {second:?} overlap")]
    Overlapping {
        first: Range<usize>,
        second: Range<usize>,
    },
    #[error("edit range {range:?} is reversed")]
    Reversed { range: Range<usize> },
}

/// Check `edits` against `rope`, drop no-ops and sort them by position.
///
/// Edits may touch (`a..b` then `b..c`) and several insertions may share a
/// position (they keep their relative order), but they must not overlap.
pub fn normalize(mut edits: Vec<Edit>, rope: &Rope) -> Result<Vec<Edit>, EditError> {
    let len = rope.len();
    edits.retain(|e| !e.is_noop());
    for edit in &edits {
        let range = edit.range.clone();
        if range.start > range.end {
            return Err(EditError::Reversed { range });
        }
        if range.end > len {
            return Err(EditError::OutOfBounds { range, len });
        }
        if !rope.is_char_boundary(range.start) || !rope.is_char_boundary(range.end) {
            return Err(EditError::NotCharBoundary { range });
        }
    }
    // Stable, so insertions at the same position keep their order.
    edits.sort_by_key(|e| (e.range.start, e.range.end));
    for pair in edits.windows(2) {
        if pair[0].range.end > pair[1].range.start {
            return Err(EditError::Overlapping {
                first: pair[0].range.clone(),
                second: pair[1].range.clone(),
            });
        }
    }
    Ok(edits)
}

/// Apply normalized edits to `rope` and return the edits that undo them,
/// expressed in the coordinates of the edited text.
pub(crate) fn apply(rope: &mut Rope, edits: &[Edit]) -> Vec<Edit> {
    let mut inverse = Vec::with_capacity(edits.len());
    let mut delta: isize = 0;
    for edit in edits {
        let old = rope.slice(edit.range.clone()).to_string();
        let start = (edit.range.start as isize + delta) as usize;
        inverse.push(Edit::new(start..start + edit.text.len(), old));
        delta += edit.delta();
    }
    // Back to front, so earlier offsets stay valid.
    for edit in edits.iter().rev() {
        if !edit.range.is_empty() {
            rope.remove(edit.range.clone());
        }
        if !edit.text.is_empty() {
            rope.insert(edit.range.start, &edit.text);
        }
    }
    inverse
}

/// Apply normalized edits to a string (used for previews and tests).
pub fn apply_to_string(text: &str, edits: &[Edit]) -> String {
    let mut out = String::with_capacity(text.len());
    let mut last = 0;
    for edit in edits {
        out.push_str(&text[last..edit.range.start]);
        out.push_str(&edit.text);
        last = edit.range.end;
    }
    out.push_str(&text[last..]);
    out
}

/// Where `offset` in the text before `edits` (normalized) lands afterwards.
///
/// An offset strictly inside a replaced range goes to the start of the
/// replacement with [`Bias::Left`] and to its end with [`Bias::Right`]. An
/// offset where text is inserted stays before the insertion with
/// [`Bias::Left`] and moves after it with [`Bias::Right`]. Offsets at the
/// edges of a replaced range stay at the matching edge of the replacement.
pub fn map_offset(offset: usize, edits: &[Edit], bias: Bias) -> usize {
    map_offset_summaries(
        offset,
        edits
            .iter()
            .map(|e| (e.range.start, e.range.end, e.text.len())),
        bias,
    )
}

/// [`map_offset`] over `(start, end, new_len)` triples.
pub(crate) fn map_offset_summaries(
    offset: usize,
    edits: impl IntoIterator<Item = (usize, usize, usize)>,
    bias: Bias,
) -> usize {
    let mut delta: isize = 0;
    for (start, end, new_len) in edits {
        if offset < start {
            break;
        }
        if start == end {
            if offset == start && bias == Bias::Left {
                break;
            }
            delta += new_len as isize;
            continue;
        }
        if offset >= end {
            delta += new_len as isize - (end - start) as isize;
            continue;
        }
        // start <= offset < end: the offset's text was replaced.
        let mapped = if offset == start || bias == Bias::Left {
            start
        } else {
            start + new_len
        };
        return (mapped as isize + delta) as usize;
    }
    (offset as isize + delta) as usize
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rope(s: &str) -> Rope {
        Rope::from_str(s)
    }

    #[test]
    fn normalize_sorts_and_rejects_overlaps() {
        let r = rope("hello world");
        let edits = normalize(vec![Edit::insert(6, "big "), Edit::delete(0..1)], &r).unwrap();
        assert_eq!(edits[0].range, 0..1);
        assert!(matches!(
            normalize(vec![Edit::delete(0..5), Edit::delete(3..7)], &r),
            Err(EditError::Overlapping { .. })
        ));
        assert!(matches!(
            normalize(vec![Edit::delete(0..50)], &r),
            Err(EditError::OutOfBounds { .. })
        ));
        // Touching edits and shared insertion points are fine.
        assert!(
            normalize(
                vec![Edit::delete(0..5), Edit::delete(5..6), Edit::insert(5, "x")],
                &r
            )
            .is_ok()
        );
    }

    #[test]
    fn rejects_split_characters() {
        let r = rope("zażółć");
        assert!(matches!(
            normalize(vec![Edit::delete(0..3)], &r),
            Err(EditError::NotCharBoundary { .. })
        ));
    }

    #[test]
    fn apply_returns_inverse() {
        let mut r = rope("one two three");
        let edits = normalize(
            vec![
                Edit::new(0..3, "1"),
                Edit::new(8..13, "3!"),
                Edit::insert(4, "<"),
            ],
            &r,
        )
        .unwrap();
        let inverse = apply(&mut r, &edits);
        assert_eq!(r.to_string(), "1 <two 3!");
        let inverse = normalize(inverse, &r).unwrap();
        apply(&mut r, &inverse);
        assert_eq!(r.to_string(), "one two three");
    }

    #[test]
    fn same_point_insertions_keep_order() {
        let mut r = rope("ab");
        let edits = normalize(vec![Edit::insert(1, "x"), Edit::insert(1, "y")], &r).unwrap();
        apply(&mut r, &edits);
        assert_eq!(r.to_string(), "axyb");
        assert_eq!(apply_to_string("ab", &edits), "axyb");
    }

    #[test]
    fn offsets_follow_edits() {
        let edits = vec![Edit::insert(2, "xx"), Edit::new(5..8, "y")];
        assert_eq!(map_offset(0, &edits, Bias::Left), 0);
        assert_eq!(map_offset(2, &edits, Bias::Left), 2);
        assert_eq!(map_offset(2, &edits, Bias::Right), 4);
        assert_eq!(map_offset(5, &edits, Bias::Right), 7);
        assert_eq!(map_offset(6, &edits, Bias::Left), 7);
        assert_eq!(map_offset(6, &edits, Bias::Right), 8);
        assert_eq!(map_offset(8, &edits, Bias::Left), 8);
        assert_eq!(map_offset(10, &edits, Bias::Left), 10);
    }
}
