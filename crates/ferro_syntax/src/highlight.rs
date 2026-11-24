//! Syntax highlighting from `highlights.scm`, including injected languages.

use std::ops::Range;

use tree_sitter::{QueryCursor, StreamingIterator as _, Tree};

use crate::{
    language::SyntaxLanguage,
    tree::{SyntaxTree, rope_provider},
};

/// A highlighted byte range. `name` is the capture name of the highlight
/// query, e.g. `keyword`, `function.method`, `string.escape`; a theme falls
/// back from `a.b` to `a`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HighlightSpan {
    pub range: Range<usize>,
    pub name: String,
}

struct Captured {
    range: Range<usize>,
    name: String,
    /// Injection depth: deeper layers paint over their host.
    depth: usize,
    /// Order the query produced the capture in.
    order: usize,
}

impl SyntaxTree {
    /// Highlights of the bytes `range` (pass the visible range plus a margin
    /// — plan §6.3), as non-overlapping spans in document order. The
    /// innermost capture wins; for the same node, the first pattern wins.
    pub fn highlights(&self, range: Range<usize>) -> Vec<HighlightSpan> {
        let range = range.start.min(self.text.len())..range.end.min(self.text.len());
        let mut captured = Vec::new();
        collect(
            &self.language,
            &self.tree,
            self,
            range.clone(),
            0,
            &mut captured,
        );
        if self.language.injections.is_some() {
            for layer in self.injection_layers() {
                let extent = layer.extent();
                if extent.end <= range.start || extent.start >= range.end {
                    continue;
                }
                let clipped = range.start.max(extent.start)..range.end.min(extent.end);
                collect(
                    &layer.language,
                    &layer.tree,
                    self,
                    clipped,
                    layer.depth,
                    &mut captured,
                );
            }
        }
        flatten(captured)
    }
}

fn collect(
    language: &SyntaxLanguage,
    tree: &Tree,
    doc: &SyntaxTree,
    range: Range<usize>,
    depth: usize,
    out: &mut Vec<Captured>,
) {
    let Some(query) = &language.highlights else {
        return;
    };
    let names = query.capture_names();
    let mut cursor = QueryCursor::new();
    cursor.set_byte_range(range.clone());
    let mut captures = cursor.captures(query, tree.root_node(), rope_provider(&doc.text));
    let mut last: Option<Range<usize>> = None;
    while let Some((m, index)) = captures.next() {
        let capture = m.captures[*index];
        let name = names[capture.index as usize];
        // `_private` captures and locals bookkeeping are not highlights.
        if name.starts_with('_') || name.starts_with("local.") || name.starts_with("injection.") {
            continue;
        }
        let node_range = capture.node.byte_range();
        // The first pattern matching a node decides its highlight.
        if last.as_ref() == Some(&node_range) {
            continue;
        }
        last = Some(node_range.clone());
        let clipped = node_range.start.max(range.start)..node_range.end.min(range.end);
        if clipped.is_empty() {
            continue;
        }
        let order = out.len();
        out.push(Captured {
            range: clipped,
            name: name.to_string(),
            depth,
            order,
        });
    }
}

/// Turn nested captures into non-overlapping spans: inner (shorter, later,
/// deeper) captures paint over outer ones.
fn flatten(mut captured: Vec<Captured>) -> Vec<HighlightSpan> {
    captured.sort_by(|a, b| {
        a.range
            .start
            .cmp(&b.range.start)
            .then(b.range.end.cmp(&a.range.end))
            .then(a.depth.cmp(&b.depth))
            .then(a.order.cmp(&b.order))
    });
    let mut out: Vec<HighlightSpan> = Vec::new();
    let mut emit = |range: Range<usize>, name: &str| {
        if range.is_empty() {
            return;
        }
        if let Some(prev) = out.last_mut()
            && prev.range.end == range.start
            && prev.name == name
        {
            prev.range.end = range.end;
            return;
        }
        out.push(HighlightSpan {
            range,
            name: name.to_string(),
        });
    };
    // Stack of open captures: (end, name).
    let mut stack: Vec<(usize, String)> = Vec::new();
    let mut pos = 0usize;
    for capture in captured {
        // Close captures that end before this one starts.
        while let Some((end, name)) = stack.last() {
            if *end > capture.range.start {
                break;
            }
            emit(pos..*end, name);
            pos = pos.max(*end);
            stack.pop();
        }
        if let Some((_, name)) = stack.last()
            && capture.range.start > pos
        {
            emit(pos..capture.range.start, name);
        }
        pos = pos.max(capture.range.start);
        // A capture never extends past the one containing it.
        let end = match stack.last() {
            Some((outer_end, _)) => capture.range.end.min(*outer_end),
            None => capture.range.end,
        };
        if end <= pos {
            continue;
        }
        stack.push((end, capture.name));
    }
    while let Some((end, name)) = stack.pop() {
        if end > pos {
            emit(pos..end, &name);
            pos = end;
        }
    }
    out
}
