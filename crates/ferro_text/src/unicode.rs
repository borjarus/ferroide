//! Unicode-aware movement and measurement: grapheme clusters, words,
//! display columns of wide characters and tabs, and the base direction of
//! bidirectional text.

use std::ops::Range;

use ropey::Rope;
use unicode_segmentation::{GraphemeCursor, GraphemeIncomplete, UnicodeSegmentation as _};
use unicode_width::UnicodeWidthStr as _;

/// The next grapheme cluster boundary after `offset` (or the end).
pub fn next_grapheme_boundary(rope: &Rope, offset: usize) -> usize {
    let len = rope.len();
    if offset >= len {
        return len;
    }
    let offset = rope.floor_char_boundary(offset);
    let (mut chunk, mut chunk_start) = rope.chunk(offset);
    let mut cursor = GraphemeCursor::new(offset, len, true);
    loop {
        match cursor.next_boundary(chunk, chunk_start) {
            Ok(None) => return len,
            Ok(Some(n)) => return n,
            Err(GraphemeIncomplete::NextChunk) => {
                chunk_start += chunk.len();
                if chunk_start >= len {
                    return len;
                }
                (chunk, chunk_start) = rope.chunk(chunk_start);
            }
            Err(GraphemeIncomplete::PreContext(n)) => {
                let (context, context_start) = rope.chunk(n - 1);
                cursor.provide_context(context, context_start);
            }
            Err(_) => return rope.ceil_char_boundary(offset + 1),
        }
    }
}

/// The previous grapheme cluster boundary before `offset` (or 0).
pub fn prev_grapheme_boundary(rope: &Rope, offset: usize) -> usize {
    let len = rope.len();
    let offset = rope.floor_char_boundary(offset.min(len));
    if offset == 0 {
        return 0;
    }
    let (mut chunk, mut chunk_start) = rope.chunk(offset - 1);
    let mut cursor = GraphemeCursor::new(offset, len, true);
    loop {
        match cursor.prev_boundary(chunk, chunk_start) {
            Ok(None) => return 0,
            Ok(Some(n)) => return n,
            Err(GraphemeIncomplete::PrevChunk) => {
                if chunk_start == 0 {
                    return 0;
                }
                (chunk, chunk_start) = rope.chunk(chunk_start - 1);
            }
            Err(GraphemeIncomplete::PreContext(n)) => {
                let (context, context_start) = rope.chunk(n - 1);
                cursor.provide_context(context, context_start);
            }
            Err(_) => return rope.floor_char_boundary(offset - 1),
        }
    }
}

/// Whether `offset` sits between two grapheme clusters.
pub fn is_grapheme_boundary(rope: &Rope, offset: usize) -> bool {
    let len = rope.len();
    if offset == 0 || offset >= len {
        return offset <= len;
    }
    if !rope.is_char_boundary(offset) {
        return false;
    }
    next_grapheme_boundary(rope, prev_grapheme_boundary(rope, offset)) == offset
}

/// Characters that make up identifiers.
pub fn is_word_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// The word touching `offset` (a word just before the caret counts), if any.
pub fn word_range_at(rope: &Rope, offset: usize) -> Option<Range<usize>> {
    let offset = rope.floor_char_boundary(offset.min(rope.len()));
    let after = rope.chars_at(offset).next().filter(|c| is_word_char(*c));
    let before = if offset > 0 {
        let prev = rope.floor_char_boundary(offset - 1);
        Some(rope.char(prev)).filter(|c| is_word_char(*c))
    } else {
        None
    };
    if after.is_none() && before.is_none() {
        return None;
    }
    let mut start = offset;
    while start > 0 {
        let prev = rope.floor_char_boundary(start - 1);
        if !is_word_char(rope.char(prev)) {
            break;
        }
        start = prev;
    }
    let mut end = offset;
    for c in rope.chars_at(offset) {
        if !is_word_char(c) {
            break;
        }
        end += c.len_utf8();
    }
    Some(start..end)
}

/// Whether `range` holds a whole word: word characters only, not touching
/// other word characters.
pub fn is_whole_word(rope: &Rope, range: &Range<usize>) -> bool {
    if range.is_empty() || !rope.slice(range.clone()).chars().all(is_word_char) {
        return false;
    }
    let before = range.start > 0 && {
        let prev = rope.floor_char_boundary(range.start - 1);
        is_word_char(rope.char(prev))
    };
    let after = rope.chars_at(range.end).next().is_some_and(is_word_char);
    !before && !after
}

/// Display columns a grapheme cluster takes: 2 for wide (CJK, most emoji),
/// 0 for a lone combining mark, otherwise 1. Emoji sequences joined with
/// ZWJ take two columns, like their glyph.
pub fn grapheme_width(grapheme: &str) -> usize {
    let width = grapheme.width();
    if grapheme.chars().nth(1).is_some() {
        width.min(2)
    } else {
        width
    }
}

/// Display width of `text` starting at column `start_column`, expanding
/// tabs to the next multiple of `tab_size`.
pub fn display_width(text: &str, start_column: usize, tab_size: usize) -> usize {
    let mut column = start_column;
    for_each_column(text, start_column, tab_size, |_, _, next| column = next);
    column - start_column
}

/// Call `f(byte_offset, column_before, column_after)` for every grapheme of
/// `text`, columns counted from `start_column`.
pub fn for_each_column(
    text: &str,
    start_column: usize,
    tab_size: usize,
    mut f: impl FnMut(usize, usize, usize),
) {
    let tab_size = tab_size.max(1);
    let mut column = start_column;
    if text.is_ascii() {
        for (i, b) in text.bytes().enumerate() {
            let next = if b == b'\t' {
                (column / tab_size + 1) * tab_size
            } else if b == b'\n' {
                column
            } else {
                column + 1
            };
            f(i, column, next);
            column = next;
        }
        return;
    }
    for (i, g) in text.grapheme_indices(true) {
        let next = match g {
            "\t" => (column / tab_size + 1) * tab_size,
            "\n" | "\r\n" => column,
            _ => column + grapheme_width(g),
        };
        f(i, column, next);
        column = next;
    }
}

/// The byte offset in `line` closest to display column `column` (never in
/// the middle of a grapheme), and the column it is actually at.
pub fn offset_at_column(line: &str, column: usize, tab_size: usize) -> (usize, usize) {
    let mut result = None;
    let mut last = 0;
    for_each_column(line, 0, tab_size, |offset, before, after| {
        if result.is_some() {
            return;
        }
        if line.as_bytes()[offset] == b'\n' || before >= column {
            // The caret goes before the first grapheme at or past the
            // column (zero-width ones included).
            result = Some((offset, before));
        } else if after > column {
            // Snap to the nearer edge of a wide grapheme or tab.
            result = Some(if column - before <= after - column {
                (offset, before)
            } else {
                (offset + next_len(line, offset), after)
            });
        }
        last = after;
    });
    result.unwrap_or_else(|| {
        let end = line.strip_suffix('\n').map_or(line.len(), str::len);
        (end, last)
    })
}

fn next_len(text: &str, offset: usize) -> usize {
    text[offset..].graphemes(true).next().map_or(0, str::len)
}

/// Writing direction of a paragraph.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    LeftToRight,
    RightToLeft,
}

/// The base direction of a line: that of its first strong character
/// (Unicode Bidirectional Algorithm, rules P2–P3). Lines without strong
/// characters are left-to-right.
pub fn base_direction(line: &str) -> Direction {
    match unicode_bidi::get_base_direction(line) {
        unicode_bidi::Direction::Rtl => Direction::RightToLeft,
        _ => Direction::LeftToRight,
    }
}

/// Whether the line contains any right-to-left text, i.e. needs bidi
/// reordering when drawn.
pub fn has_rtl(line: &str) -> bool {
    !line.is_ascii() && {
        let info = unicode_bidi::BidiInfo::new(line, None);
        info.has_rtl()
    }
}

/// The line's byte ranges in visual (left-to-right on screen) order, each
/// with its direction, per the Unicode Bidirectional Algorithm.
pub fn visual_runs(line: &str) -> Vec<(Range<usize>, Direction)> {
    let line = line.strip_suffix('\n').unwrap_or(line);
    if line.is_empty() {
        return Vec::new();
    }
    if !has_rtl(line) {
        return vec![(0..line.len(), Direction::LeftToRight)];
    }
    let info = unicode_bidi::BidiInfo::new(line, None);
    let mut runs = Vec::new();
    for paragraph in &info.paragraphs {
        let (levels, ranges) = info.visual_runs(paragraph, paragraph.range.clone());
        for range in ranges {
            let direction = if levels[range.start].is_rtl() {
                Direction::RightToLeft
            } else {
                Direction::LeftToRight
            };
            runs.push((range, direction));
        }
    }
    runs
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn graphemes_keep_clusters_together() {
        // e + combining acute, family emoji (ZWJ sequence), flag.
        let text = "ae\u{301}👨‍👩‍👧🇵🇱z";
        let rope = Rope::from_str(text);
        let mut offsets = vec![0];
        let mut at = 0;
        while at < rope.len() {
            at = next_grapheme_boundary(&rope, at);
            offsets.push(at);
        }
        let expected: Vec<usize> = std::iter::once(0)
            .chain(text.grapheme_indices(true).skip(1).map(|(i, _)| i))
            .chain(std::iter::once(text.len()))
            .collect();
        assert_eq!(offsets, expected);
        let mut back = vec![rope.len()];
        let mut at = rope.len();
        while at > 0 {
            at = prev_grapheme_boundary(&rope, at);
            back.push(at);
        }
        back.reverse();
        assert_eq!(back, expected);
        assert!(!is_grapheme_boundary(&rope, 2));
    }

    #[test]
    fn widths() {
        assert_eq!(display_width("abc", 0, 4), 3);
        assert_eq!(display_width("\tx", 0, 4), 5);
        assert_eq!(display_width("ab\tx", 0, 4), 5);
        assert_eq!(display_width("漢字", 0, 4), 4);
        assert_eq!(display_width("👨‍👩‍👧", 0, 4), 2);
        assert_eq!(display_width("e\u{301}", 0, 4), 1);
        assert_eq!(display_width("zażółć", 0, 4), 6);
    }

    #[test]
    fn column_to_offset() {
        assert_eq!(offset_at_column("abc\n", 2, 4), (2, 2));
        assert_eq!(offset_at_column("abc\n", 9, 4), (3, 3));
        assert_eq!(offset_at_column("\tx", 1, 4), (0, 0));
        assert_eq!(offset_at_column("\tx", 3, 4), (1, 4));
        assert_eq!(offset_at_column("漢字", 2, 4), (3, 2));
        assert_eq!(offset_at_column("ąb", 1, 4), (2, 1));
    }

    #[test]
    fn words() {
        let rope = Rope::from_str("let zażółć_1 = x;");
        assert_eq!(word_range_at(&rope, 6), Some(4..16));
        assert_eq!(word_range_at(&rope, 16), Some(4..16));
        assert_eq!(word_range_at(&rope, 18), None);
        assert!(is_whole_word(&rope, &(4..16)));
        assert!(!is_whole_word(&rope, &(4..6)));
    }

    #[test]
    fn bidi() {
        assert_eq!(base_direction("hello"), Direction::LeftToRight);
        assert_eq!(base_direction("שלום world"), Direction::RightToLeft);
        assert!(!has_rtl("zażółć"));
        let line = "abc שלום def";
        let runs = visual_runs(line);
        assert_eq!(runs.len(), 3);
        assert_eq!(runs[1].1, Direction::RightToLeft);
        assert_eq!(&line[runs[1].0.clone()].trim(), &"שלום");
    }
}
