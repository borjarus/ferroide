//! Smart indentation from `indents.scm`, with a bracket-counting fallback.

use tree_sitter::{QueryCursor, StreamingIterator as _};

use crate::tree::{SyntaxTree, rope_provider};

impl SyntaxTree {
    /// The indentation level a line should have, counted in indentation
    /// units, judged from the syntax tree around it.
    ///
    /// With `indents.scm`: one level deeper than the line where the innermost
    /// `@indent` node containing the line starts (when it started on an
    /// earlier line), one level less when the line starts with an `@outdent`
    /// token (`}`, `end`, `else`…). Without the query: the previous non-blank
    /// line's indentation, plus one level after an opening bracket, minus one
    /// before a closing bracket.
    pub fn indent_level(&self, line: usize) -> usize {
        let unit = self.language.indent_unit.as_str();
        self.level_with(line, &|row| indent_width(&self.line_text(row), unit))
    }

    /// Indentation levels for a run of lines, as Auto-Indent Lines applies
    /// them: each line is judged with the lines before it in the run already
    /// re-indented, so nested blocks settle in one pass.
    pub fn indent_levels(&self, lines: std::ops::RangeInclusive<usize>) -> Vec<usize> {
        let unit = self.language.indent_unit.as_str();
        let first = *lines.start();
        let mut levels: Vec<usize> = Vec::new();
        for line in lines {
            let level = self.level_with(line, &|row| {
                if row >= first && row - first < levels.len() {
                    levels[row - first]
                } else {
                    indent_width(&self.line_text(row), unit)
                }
            });
            levels.push(level);
        }
        levels
    }

    fn level_with(&self, line: usize, width_of: &dyn Fn(usize) -> usize) -> usize {
        match &self.language.indents {
            Some(_) => self.query_indent(line, width_of),
            None => self.bracket_indent(line, width_of),
        }
    }

    /// The indentation text for `line`: [`Self::indent_level`] units of the
    /// language's indentation unit (the editor setting may override it).
    pub fn indent_for_line(&self, line: usize, unit: &str) -> String {
        unit.repeat(self.indent_level(line))
    }

    fn query_indent(&self, line: usize, width_of: &dyn Fn(usize) -> usize) -> usize {
        let Some(query) = &self.language.indents else {
            return 0;
        };
        let line_start = self.line_start(line);
        let text = self.line_text(line);
        let first = line_start + (text.len() - text.trim_start().len());
        let indent_index = query.capture_index_for_name("indent");
        let outdent_index = query.capture_index_for_name("outdent");

        // The innermost `@indent` node that started on an earlier line and
        // contains the line's first character: the line is indented one level
        // deeper than the line that node starts on.
        let mut anchor: Option<(usize, usize)> = None; // (start byte, start row)
        let mut outdent = false;
        let mut cursor = QueryCursor::new();
        let mut captures = cursor.captures(query, self.tree.root_node(), rope_provider(&self.text));
        captures.set_byte_range(first.saturating_sub(1)..first + 1);
        while let Some((m, index)) = captures.next() {
            let capture = m.captures[*index];
            let node = capture.node;
            if Some(capture.index) == indent_index {
                let row = node.start_position().row;
                let contains = node.start_byte() < first
                    && (first < node.end_byte() || node.end_byte() >= self.text.len());
                if row < line
                    && contains
                    && anchor.is_none_or(|(start, _)| node.start_byte() > start)
                {
                    anchor = Some((node.start_byte(), row));
                }
            } else if Some(capture.index) == outdent_index && node.start_byte() == first {
                outdent = true;
            }
        }
        let Some((_, row)) = anchor else {
            return 0;
        };
        let level = width_of(row) + 1;
        if outdent && !text.trim().is_empty() {
            level - 1
        } else {
            level
        }
    }

    fn bracket_indent(&self, line: usize, width_of: &dyn Fn(usize) -> usize) -> usize {
        // The previous non-blank line.
        let Some(previous) = (0..line)
            .rev()
            .find(|l| !self.line_text(*l).trim().is_empty())
        else {
            return 0;
        };
        let previous_text = self.line_text(previous);
        let mut level = width_of(previous);
        let trimmed = previous_text.trim_end();
        if self
            .language
            .brackets
            .iter()
            .any(|(open, _)| trimmed.ends_with(open.as_str()))
        {
            level += 1;
        }
        let current = self.line_text(line);
        let current = current.trim_start();
        if !current.is_empty()
            && self
                .language
                .brackets
                .iter()
                .any(|(_, close)| current.starts_with(close.as_str()))
        {
            level = level.saturating_sub(1);
        }
        level
    }
}

/// How many indentation units `line` starts with (a tab counts as one).
pub fn indent_width(line: &str, unit: &str) -> usize {
    let unit_width = if unit == "\t" { 4 } else { unit.len().max(1) };
    let mut columns = 0;
    for c in line.chars() {
        match c {
            ' ' => columns += 1,
            '\t' => columns += unit_width,
            _ => break,
        }
    }
    columns / unit_width
}
