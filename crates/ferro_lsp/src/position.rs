//! Positions: FerroIDE's `char` columns ↔ the server's position encoding,
//! and the incremental change between two versions of a document.

use ferro_services::{TextPos, TextRange};
use lsp_types::{Position, PositionEncodingKind, Range, TextDocumentContentChangeEvent};

/// The negotiated position encoding (LSP 3.17 `general.positionEncodings`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Encoding {
    Utf8,
    #[default]
    Utf16,
    Utf32,
}

impl Encoding {
    pub fn from_kind(kind: Option<&PositionEncodingKind>) -> Self {
        match kind.map(|k| k.as_str()) {
            Some("utf-8") => Encoding::Utf8,
            Some("utf-32") => Encoding::Utf32,
            _ => Encoding::Utf16,
        }
    }

    fn width(self, c: char) -> u32 {
        match self {
            Encoding::Utf8 => c.len_utf8() as u32,
            Encoding::Utf16 => c.len_utf16() as u32,
            Encoding::Utf32 => 1,
        }
    }
}

/// The lines of a text, split on `\n` with a trailing `\r` removed, for
/// converting many positions without rescanning the text.
pub struct Lines<'a> {
    lines: Vec<&'a str>,
}

impl<'a> Lines<'a> {
    pub fn new(text: &'a str) -> Self {
        Self {
            lines: text
                .split('\n')
                .map(|l| l.strip_suffix('\r').unwrap_or(l))
                .collect(),
        }
    }

    pub fn line(&self, index: u32) -> &'a str {
        self.lines.get(index as usize).copied().unwrap_or("")
    }

    pub fn len(&self) -> usize {
        self.lines.len()
    }

    pub fn is_empty(&self) -> bool {
        self.lines.is_empty()
    }

    pub fn to_lsp(&self, pos: TextPos, encoding: Encoding) -> Position {
        Position::new(
            pos.line,
            char_to_lsp_column(self.line(pos.line), pos.column, encoding),
        )
    }

    pub fn from_lsp(&self, pos: Position, encoding: Encoding) -> TextPos {
        TextPos::new(
            pos.line,
            lsp_to_char_column(self.line(pos.line), pos.character, encoding),
        )
    }

    pub fn range_to_lsp(&self, range: TextRange, encoding: Encoding) -> Range {
        Range::new(
            self.to_lsp(range.start, encoding),
            self.to_lsp(range.end, encoding),
        )
    }

    pub fn range_from_lsp(&self, range: Range, encoding: Encoding) -> TextRange {
        TextRange::new(
            self.from_lsp(range.start, encoding),
            self.from_lsp(range.end, encoding),
        )
    }
}

/// A `char` column of `line` in `encoding` units. Columns past the end of
/// the line count as one unit per `char`.
pub fn char_to_lsp_column(line: &str, column: u32, encoding: Encoding) -> u32 {
    let mut units = 0;
    let mut count = 0;
    for c in line.chars() {
        if count == column {
            return units;
        }
        units += encoding.width(c);
        count += 1;
    }
    units + column.saturating_sub(count)
}

/// An `encoding` column of `line` in `char`s. A column inside a character
/// (half a surrogate pair) rounds down; past the end it clamps.
pub fn lsp_to_char_column(line: &str, column: u32, encoding: Encoding) -> u32 {
    let mut units = 0;
    let mut count = 0;
    for c in line.chars() {
        let width = encoding.width(c);
        if units + width > column {
            return count;
        }
        units += width;
        count += 1;
    }
    count
}

/// The position of byte `offset` of `text` in `encoding`.
fn byte_to_lsp(text: &str, offset: usize, encoding: Encoding) -> Position {
    let before = &text[..offset];
    let line = before.matches('\n').count() as u32;
    let line_start = before.rfind('\n').map_or(0, |i| i + 1);
    let character = before[line_start..]
        .chars()
        .map(|c| encoding.width(c))
        .sum();
    Position::new(line, character)
}

/// One incremental change turning `old` into `new`: the differing middle,
/// found from the common prefix and suffix.
pub fn incremental_change(
    old: &str,
    new: &str,
    encoding: Encoding,
) -> TextDocumentContentChangeEvent {
    let (a, b) = (old.as_bytes(), new.as_bytes());
    let mut prefix = a.iter().zip(b).take_while(|(x, y)| x == y).count();
    while !old.is_char_boundary(prefix) || !new.is_char_boundary(prefix) {
        prefix -= 1;
    }
    let max_suffix = (a.len() - prefix).min(b.len() - prefix);
    let mut suffix = a
        .iter()
        .rev()
        .zip(b.iter().rev())
        .take(max_suffix)
        .take_while(|(x, y)| x == y)
        .count();
    while !old.is_char_boundary(a.len() - suffix) || !new.is_char_boundary(b.len() - suffix) {
        suffix -= 1;
    }
    // Keep `\r\n` together, so a range never ends between them.
    if prefix > 0 && a.get(prefix - 1) == Some(&b'\r') {
        prefix -= 1;
    }
    let start = byte_to_lsp(old, prefix, encoding);
    let end = byte_to_lsp(old, a.len() - suffix, encoding);
    TextDocumentContentChangeEvent {
        range: Some(Range::new(start, end)),
        range_length: None,
        text: new[prefix..b.len() - suffix].to_string(),
    }
}

/// Apply LSP-style changes to `text` (used by tests and the fake server).
pub fn apply_change(
    text: &str,
    change: &TextDocumentContentChangeEvent,
    encoding: Encoding,
) -> String {
    let Some(range) = change.range else {
        return change.text.clone();
    };
    let offset = |pos: Position| -> usize {
        let mut line_start = 0;
        for _ in 0..pos.line {
            match text[line_start..].find('\n') {
                Some(i) => line_start += i + 1,
                None => return text.len(),
            }
        }
        let line_end = text[line_start..]
            .find('\n')
            .map_or(text.len(), |i| line_start + i);
        let mut units = 0;
        for (i, c) in text[line_start..line_end].char_indices() {
            if units >= pos.character {
                return line_start + i;
            }
            units += encoding.width(c);
        }
        line_end
    };
    let (start, end) = (offset(range.start), offset(range.end));
    format!("{}{}{}", &text[..start], change.text, &text[end..])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn columns_convert_through_surrogate_pairs() {
        let line = "a😀ż = 1";
        assert_eq!(char_to_lsp_column(line, 2, Encoding::Utf16), 3);
        assert_eq!(char_to_lsp_column(line, 3, Encoding::Utf16), 4);
        assert_eq!(char_to_lsp_column(line, 3, Encoding::Utf8), 7);
        assert_eq!(char_to_lsp_column(line, 3, Encoding::Utf32), 3);
        assert_eq!(lsp_to_char_column(line, 3, Encoding::Utf16), 2);
        assert_eq!(
            lsp_to_char_column(line, 2, Encoding::Utf16),
            1,
            "inside the pair"
        );
        assert_eq!(lsp_to_char_column(line, 7, Encoding::Utf8), 3);
        assert_eq!(lsp_to_char_column(line, 99, Encoding::Utf16), 7);
        assert_eq!(char_to_lsp_column("ab", 4, Encoding::Utf16), 4);
    }

    #[test]
    fn incremental_changes_rebuild_the_new_text() {
        let cases = [
            ("fn main() {}\n", "fn main() { x }\n"),
            ("a\nb\nc", "a\nc"),
            ("", "hello"),
            ("hello", ""),
            ("😀😀", "😀a😀"),
            ("line1\r\nline2\r\n", "line1\r\nline1.5\r\nline2\r\n"),
            ("same", "same"),
            ("aaa", "aaaa"),
        ];
        for encoding in [Encoding::Utf8, Encoding::Utf16, Encoding::Utf32] {
            for (old, new) in cases {
                let change = incremental_change(old, new, encoding);
                assert_eq!(
                    apply_change(old, &change, encoding),
                    new,
                    "{old:?} → {new:?}"
                );
            }
        }
    }

    #[test]
    fn lines_strip_carriage_returns() {
        let lines = Lines::new("ab\r\nc😀d\n");
        assert_eq!(lines.line(0), "ab");
        assert_eq!(
            lines.to_lsp(TextPos::new(1, 2), Encoding::Utf16),
            Position::new(1, 3)
        );
        assert_eq!(
            lines.from_lsp(Position::new(1, 3), Encoding::Utf16),
            TextPos::new(1, 2)
        );
    }
}
