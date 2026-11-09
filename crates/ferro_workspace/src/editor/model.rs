//! Pure computations behind the editor's view layer: breadcrumbs, sticky
//! lines, problem navigation, gutter ranges and end-of-line inlay hints.
//!
//! Nothing here touches GPUI, so it is unit-tested directly and its cost per
//! frame is measured in the performance test.

use std::ops::Range;

use ferro_services::{
    Diagnostic, InlayHint, LineChange, LineChangeKind, Severity, Symbol, SymbolKind, TextPos,
};

fn contains_inclusive(symbol: &Symbol, pos: TextPos) -> bool {
    symbol.range.start <= pos && pos <= symbol.range.end
}

/// The nested symbols enclosing `pos`, outermost first (the breadcrumb
/// chain): e.g. `mod tests › fn greets_by_name`.
pub fn symbol_chain(symbols: &[Symbol], pos: TextPos) -> Vec<&Symbol> {
    let mut chain = Vec::new();
    let mut level = symbols;
    while let Some(symbol) = level.iter().find(|s| contains_inclusive(s, pos)) {
        chain.push(symbol);
        level = &symbol.children;
    }
    chain
}

/// Whether a symbol opens a scope worth pinning (functions, types,
/// modules), as opposed to fields and constants.
fn is_scope(kind: SymbolKind) -> bool {
    matches!(
        kind,
        SymbolKind::Module
            | SymbolKind::Struct
            | SymbolKind::Enum
            | SymbolKind::Trait
            | SymbolKind::Function
            | SymbolKind::Method
            | SymbolKind::Test
            | SymbolKind::Macro
    )
}

/// Header lines that stay pinned at the top when `first_visible` is the
/// first line on screen: the scopes whose header has scrolled away while
/// their body is still visible, outermost first, at most `max`.
///
/// Each pinned header covers one more line, so the next level is decided
/// against the line just below the headers already pinned.
pub fn sticky_lines(symbols: &[Symbol], first_visible: u32, max: usize) -> Vec<u32> {
    let mut lines = Vec::new();
    let mut level = symbols;
    while lines.len() < max {
        let line = first_visible + lines.len() as u32;
        let Some(scope) = level
            .iter()
            .find(|s| is_scope(s.kind) && s.range.start.line < line && s.range.end.line > line)
        else {
            break;
        };
        lines.push(scope.range.start.line);
        level = &scope.children;
    }
    lines
}

/// The start of the next (or previous) diagnostic after (before) `caret`,
/// wrapping around the file.
pub fn next_problem(diagnostics: &[Diagnostic], caret: TextPos, forward: bool) -> Option<TextPos> {
    let mut starts: Vec<TextPos> = diagnostics.iter().map(|d| d.range.start).collect();
    starts.sort();
    starts.dedup();
    if forward {
        starts
            .iter()
            .find(|p| **p > caret)
            .or(starts.first())
            .copied()
    } else {
        starts
            .iter()
            .rev()
            .find(|p| **p < caret)
            .or(starts.last())
            .copied()
    }
}

/// Errors and warnings in a diagnostic list.
pub fn problem_counts(diagnostics: &[Diagnostic]) -> (usize, usize) {
    diagnostics
        .iter()
        .fold((0, 0), |(errors, warnings), d| match d.severity {
            Severity::Error => (errors + 1, warnings),
            Severity::Warning => (errors, warnings + 1),
            _ => (errors, warnings),
        })
}

/// Byte offset where each line starts.
pub fn line_starts(text: &str) -> Vec<usize> {
    std::iter::once(0)
        .chain(text.match_indices('\n').map(|(i, _)| i + 1))
        .collect()
}

/// Byte ranges for the gutter's change markers: whole lines for added and
/// modified ranges, an empty range at the line start for deletions (the
/// editor draws those as a boundary mark).
pub fn gutter_ranges(changes: &[LineChange], text: &str) -> Vec<(Range<usize>, LineChangeKind)> {
    let starts = line_starts(text);
    let line_end = |line: usize| -> usize {
        match starts.get(line + 1) {
            Some(next) => next - 1,
            None => text.len(),
        }
    };
    let line_start = |line: usize| -> usize { starts.get(line).copied().unwrap_or(text.len()) };
    changes
        .iter()
        .map(|change| {
            let (start, end) = (change.lines.start as usize, change.lines.end as usize);
            let range = match change.kind {
                LineChangeKind::Deleted => line_start(start)..line_start(start),
                _ if end > start => line_start(start)..line_end(end - 1),
                _ => line_start(start)..line_start(start),
            };
            (range, change.kind)
        })
        .collect()
}

/// An inlay hint drawn after the end of its line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LineHint {
    pub line: u32,
    /// Length of the line in characters: the hint starts one column after.
    pub line_len: u32,
    pub label: String,
}

/// Group inlay hints by line into end-of-line labels. GPUI Kit's editor has
/// no inline virtual text, so hints sit after the code (Stage 3's own editor
/// renders them in place).
pub fn end_of_line_hints(hints: &[InlayHint], text: &str) -> Vec<LineHint> {
    let lines: Vec<&str> = text.split('\n').collect();
    let mut out: Vec<LineHint> = Vec::new();
    let mut sorted: Vec<&InlayHint> = hints.iter().collect();
    sorted.sort_by_key(|h| h.position);
    for hint in sorted {
        let Some(line_text) = lines.get(hint.position.line as usize) else {
            continue;
        };
        let label = hint.label.trim().to_string();
        match out.last_mut() {
            Some(last) if last.line == hint.position.line => {
                last.label.push_str("  ");
                last.label.push_str(&label);
            }
            _ => out.push(LineHint {
                line: hint.position.line,
                line_len: line_text.trim_end_matches('\r').chars().count() as u32,
                label,
            }),
        }
    }
    out
}

/// The first line on screen for a vertical scroll distance (positive,
/// pixels below the top) and line height.
pub fn first_visible_line(scrolled: f32, top_inset: f32, line_height: f32) -> u32 {
    if line_height <= 0.0 {
        return 0;
    }
    ((scrolled - top_inset).max(0.0) / line_height).floor() as u32
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use ferro_services::{InlayHintKind, TextRange};

    use super::*;

    fn sym(name: &str, kind: SymbolKind, lines: (u32, u32), children: Vec<Symbol>) -> Symbol {
        Symbol {
            name: name.into(),
            kind,
            path: PathBuf::from("x.rs"),
            range: TextRange::new(TextPos::new(lines.0, 0), TextPos::new(lines.1, 1)),
            selection: TextPos::new(lines.0, 3),
            container: None,
            children,
        }
    }

    fn outline() -> Vec<Symbol> {
        vec![
            sym("greet", SymbolKind::Function, (6, 12), vec![]),
            sym(
                "tests",
                SymbolKind::Module,
                (15, 30),
                vec![
                    sym("greets_by_name", SymbolKind::Test, (19, 22), vec![]),
                    sym("greets_strangers", SymbolKind::Test, (24, 28), vec![]),
                ],
            ),
        ]
    }

    #[test]
    fn breadcrumbs_follow_nesting() {
        let symbols = outline();
        let names = |pos| -> Vec<String> {
            symbol_chain(&symbols, pos)
                .iter()
                .map(|s| s.name.clone())
                .collect()
        };
        assert_eq!(names(TextPos::new(8, 4)), ["greet"]);
        assert_eq!(names(TextPos::new(20, 0)), ["tests", "greets_by_name"]);
        assert!(names(TextPos::new(2, 0)).is_empty());
    }

    #[test]
    fn sticky_lines_pin_enclosing_scopes() {
        let symbols = outline();
        assert!(sticky_lines(&symbols, 0, 3).is_empty());
        assert_eq!(sticky_lines(&symbols, 8, 3), [6]);
        // Inside the test module: the module header, then the test's.
        assert_eq!(sticky_lines(&symbols, 20, 3), [15, 19]);
        // The test ends at 22: with one header pinned the next line is 22.
        assert_eq!(sticky_lines(&symbols, 21, 3), [15]);
        assert_eq!(sticky_lines(&symbols, 20, 1), [15]);
    }

    #[test]
    fn problems_wrap_both_ways() {
        let d = |line| Diagnostic {
            path: PathBuf::from("x"),
            range: TextRange::on_line(line, 2, 4),
            severity: if line == 3 {
                Severity::Error
            } else {
                Severity::Warning
            },
            message: String::new(),
            source: String::new(),
            code: None,
        };
        let diags = [d(10), d(3)];
        assert_eq!(
            next_problem(&diags, TextPos::new(0, 0), true),
            Some(TextPos::new(3, 2))
        );
        assert_eq!(
            next_problem(&diags, TextPos::new(3, 2), true),
            Some(TextPos::new(10, 2))
        );
        assert_eq!(
            next_problem(&diags, TextPos::new(10, 2), true),
            Some(TextPos::new(3, 2))
        );
        assert_eq!(
            next_problem(&diags, TextPos::new(3, 2), false),
            Some(TextPos::new(10, 2))
        );
        assert_eq!(next_problem(&[], TextPos::default(), true), None);
        assert_eq!(problem_counts(&diags), (1, 1));
    }

    #[test]
    fn gutter_ranges_cover_lines() {
        let text = "a\nbb\nccc\n";
        let changes = [
            LineChange {
                lines: 1..3,
                kind: LineChangeKind::Modified,
            },
            LineChange {
                lines: 0..0,
                kind: LineChangeKind::Deleted,
            },
        ];
        let ranges = gutter_ranges(&changes, text);
        assert_eq!(&text[ranges[0].0.clone()], "bb\nccc");
        assert_eq!(ranges[1].0, 0..0);
    }

    #[test]
    fn hints_group_at_line_end() {
        let hint = |line, column, label: &str| InlayHint {
            position: TextPos::new(line, column),
            label: label.into(),
            kind: InlayHintKind::Type,
        };
        let text = "let a = 1;\nlet name = \"x\";";
        let hints = end_of_line_hints(&[hint(1, 8, ": &str"), hint(0, 5, ": i32")], text);
        assert_eq!(hints.len(), 2);
        assert_eq!(hints[0].line_len, 10);
        assert_eq!(hints[1].label, ": &str");
    }

    #[test]
    fn first_visible_line_from_scroll() {
        assert_eq!(first_visible_line(0.0, 8.0, 20.0), 0);
        assert_eq!(first_visible_line(208.0, 8.0, 20.0), 10);
    }
}
