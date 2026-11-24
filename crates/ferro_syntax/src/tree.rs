//! A document's syntax tree, kept current by incremental parsing, with the
//! trees of injected languages parsed on demand.

use std::{cell::OnceCell, ops::Range, sync::Arc};

use ferro_text::{LINES, Rope};
use tree_sitter::{
    InputEdit, Node, Parser, Point, Query, QueryCursor, StreamingIterator as _, Tree,
};

use crate::language::{LanguageResolver, NoInjections, SyntaxLanguage};

/// How deep injections nest (Markdown → HTML → JavaScript → …).
const MAX_INJECTION_DEPTH: usize = 4;

/// A tree of an injected language over parts of the document.
pub struct InjectionLayer {
    pub language: Arc<SyntaxLanguage>,
    pub tree: Tree,
    /// The byte ranges of the document the layer was parsed from.
    pub ranges: Vec<Range<usize>>,
    /// 1 for an injection into the document's language, 2 for one inside
    /// that, …
    pub depth: usize,
}

impl InjectionLayer {
    /// The smallest byte range covering every range of the layer.
    pub fn extent(&self) -> Range<usize> {
        let start = self.ranges.first().map_or(0, |r| r.start);
        let end = self.ranges.last().map_or(0, |r| r.end);
        start..end
    }
}

/// The parse of one document in one language.
pub struct SyntaxTree {
    pub(crate) language: Arc<SyntaxLanguage>,
    pub(crate) text: Rope,
    pub(crate) tree: Tree,
    parser: Parser,
    resolver: Arc<dyn LanguageResolver>,
    layers: OnceCell<Vec<InjectionLayer>>,
}

/// The text of a parse failed to produce a tree: the grammar was built for
/// an incompatible tree-sitter version.
#[derive(Debug, thiserror::Error)]
#[error("the {0} grammar cannot be loaded by this tree-sitter version")]
pub struct IncompatibleGrammar(pub String);

impl SyntaxTree {
    /// Parse `text` from scratch, without injections.
    pub fn parse(language: Arc<SyntaxLanguage>, text: &Rope) -> Result<Self, IncompatibleGrammar> {
        Self::parse_with(language, text, Arc::new(NoInjections))
    }

    /// Parse `text` from scratch; injected languages are looked up with
    /// `resolver` when first needed.
    pub fn parse_with(
        language: Arc<SyntaxLanguage>,
        text: &Rope,
        resolver: Arc<dyn LanguageResolver>,
    ) -> Result<Self, IncompatibleGrammar> {
        let mut parser = Parser::new();
        parser
            .set_language(&language.grammar)
            .map_err(|_| IncompatibleGrammar(language.id.clone()))?;
        let tree = parse_rope(&mut parser, text, None)
            .ok_or_else(|| IncompatibleGrammar(language.id.clone()))?;
        Ok(Self {
            language,
            text: text.clone(),
            tree,
            parser,
            resolver,
            layers: OnceCell::new(),
        })
    }

    pub fn language(&self) -> &Arc<SyntaxLanguage> {
        &self.language
    }

    pub fn text(&self) -> &Rope {
        &self.text
    }

    pub fn tree(&self) -> &Tree {
        &self.tree
    }

    pub fn root(&self) -> Node<'_> {
        self.tree.root_node()
    }

    /// Bring the tree up to date with `new_text`, reusing the unchanged parts
    /// of the old tree. The edit is found by comparing the old and new text
    /// (common prefix and suffix), so any change — typing, paste, undo,
    /// reload — is handled; callers that know the exact edit can use
    /// [`SyntaxTree::edit`] instead.
    ///
    /// Returns the changed byte range of the new text, or `None` when the
    /// text did not change.
    pub fn reparse(&mut self, new_text: &Rope) -> Option<Range<usize>> {
        let (start, old_end, new_end) = diff(&self.text, new_text)?;
        self.apply(start, old_end, new_end, new_text);
        Some(start..new_end)
    }

    /// Apply a known edit: bytes `range` of the old text were replaced by
    /// `new_text`'s bytes `range.start..range.start + inserted_len`.
    pub fn edit(&mut self, range: Range<usize>, inserted_len: usize, new_text: &Rope) {
        self.apply(range.start, range.end, range.start + inserted_len, new_text);
    }

    fn apply(&mut self, start: usize, old_end: usize, new_end: usize, new_text: &Rope) {
        let edit = InputEdit {
            start_byte: start,
            old_end_byte: old_end,
            new_end_byte: new_end,
            start_position: point(&self.text, start),
            old_end_position: point(&self.text, old_end),
            new_end_position: point(new_text, new_end),
        };
        self.tree.edit(&edit);
        if let Some(tree) = parse_rope(&mut self.parser, new_text, Some(&self.tree)) {
            self.tree = tree;
        }
        self.text = new_text.clone();
        self.layers = OnceCell::new();
    }

    /// The trees of injected languages (JavaScript in HTML, code blocks in
    /// Markdown, SQL in strings…), parsed on first use after each edit.
    pub fn injection_layers(&self) -> &[InjectionLayer] {
        self.layers.get_or_init(|| {
            let mut layers = Vec::new();
            collect_injections(
                &self.language,
                &self.tree,
                &self.text,
                self.resolver.as_ref(),
                1,
                &mut layers,
            );
            layers
        })
    }

    /// The text of a node.
    pub fn node_text(&self, node: Node<'_>) -> String {
        slice_text(&self.text, node.byte_range())
    }

    /// The innermost tree covering `range`: an injection layer when the range
    /// lies inside one, otherwise the document's tree. Returns the tree and
    /// its language.
    pub(crate) fn tree_at(&self, range: Range<usize>) -> (&Tree, &Arc<SyntaxLanguage>) {
        let mut best: Option<&InjectionLayer> = None;
        if !self.language.injections.is_none() {
            for layer in self.injection_layers() {
                let inside = layer
                    .ranges
                    .iter()
                    .any(|r| r.start <= range.start && range.end <= r.end);
                if inside && best.is_none_or(|b| layer.depth > b.depth) {
                    best = Some(layer);
                }
            }
        }
        match best {
            Some(layer) => (&layer.tree, &layer.language),
            None => (&self.tree, &self.language),
        }
    }

    /// The 0-based line of a byte offset.
    pub fn line_of(&self, byte: usize) -> usize {
        self.text.byte_to_line_idx(byte.min(self.text.len()), LINES)
    }

    /// Byte offset of the start of a line.
    pub fn line_start(&self, line: usize) -> usize {
        let lines = self.text.len_lines(LINES);
        if line >= lines {
            self.text.len()
        } else {
            self.text.line_to_byte_idx(line, LINES)
        }
    }

    /// The text of one line, without its line break.
    pub fn line_text(&self, line: usize) -> String {
        let start = self.line_start(line);
        let end = self.line_start(line + 1);
        let mut text = slice_text(&self.text, start..end);
        while text.ends_with('\n') || text.ends_with('\r') {
            text.pop();
        }
        text
    }

    /// Whether the document has syntax errors.
    pub fn has_errors(&self) -> bool {
        self.tree.root_node().has_error()
    }
}

/// Text of a byte range of a rope.
pub(crate) fn slice_text(text: &Rope, range: Range<usize>) -> String {
    let end = range.end.min(text.len());
    let start = range.start.min(end);
    text.slice(start..end).to_string()
}

/// A text provider for query predicates (`#eq?`, `#match?`) over a rope.
pub(crate) fn rope_provider(
    text: &Rope,
) -> impl FnMut(Node<'_>) -> std::vec::IntoIter<Vec<u8>> + '_ {
    move |node: Node<'_>| {
        let range = node.byte_range();
        let end = range.end.min(text.len());
        let start = range.start.min(end);
        let chunks: Vec<Vec<u8>> = text
            .slice(start..end)
            .chunks()
            .map(|c| c.as_bytes().to_vec())
            .collect();
        chunks.into_iter()
    }
}

fn parse_rope(parser: &mut Parser, text: &Rope, old: Option<&Tree>) -> Option<Tree> {
    let len = text.len();
    // The parser mostly reads forward; remember the last chunk so most reads
    // skip the rope lookup.
    let mut cached: (&[u8], usize) = (&[], 0);
    parser.parse_with_options(
        &mut |byte: usize, _: Point| -> &[u8] {
            if byte >= len {
                return &[];
            }
            let (chunk, start) = cached;
            if byte >= start && byte < start + chunk.len() {
                return &chunk[byte - start..];
            }
            let (chunk, start) = text.chunk(byte);
            cached = (chunk.as_bytes(), start);
            &chunk.as_bytes()[byte - start..]
        },
        old,
        None,
    )
}

/// A tree-sitter point (row, byte column) of a byte offset.
pub(crate) fn point(text: &Rope, byte: usize) -> Point {
    let byte = byte.min(text.len());
    let row = text.byte_to_line_idx(byte, LINES);
    let column = byte - text.line_to_byte_idx(row, LINES);
    Point::new(row, column)
}

/// The edit turning `old` into `new`: `(start, old_end, new_end)`, or `None`
/// when they are equal. Compares whole chunks first, so the common case (one
/// local edit) costs a few `memcmp`s over the text.
pub(crate) fn diff(old: &Rope, new: &Rope) -> Option<(usize, usize, usize)> {
    let prefix = common_prefix(old, new);
    let (old_len, new_len) = (old.len(), new.len());
    if prefix == old_len && prefix == new_len {
        return None;
    }
    let max_suffix = (old_len - prefix).min(new_len - prefix);
    let suffix = common_suffix(old, new).min(max_suffix);
    Some((prefix, old_len - suffix, new_len - suffix))
}

fn common_prefix(a: &Rope, b: &Rope) -> usize {
    let mut a_chunks = a.chunks();
    let mut b_chunks = b.chunks();
    let (mut ca, mut cb): (&[u8], &[u8]) = (&[], &[]);
    let mut total = 0;
    loop {
        if ca.is_empty() {
            match a_chunks.next() {
                Some(c) => ca = c.as_bytes(),
                None => return total,
            }
            continue;
        }
        if cb.is_empty() {
            match b_chunks.next() {
                Some(c) => cb = c.as_bytes(),
                None => return total,
            }
            continue;
        }
        let n = ca.len().min(cb.len());
        if ca[..n] == cb[..n] {
            total += n;
            ca = &ca[n..];
            cb = &cb[n..];
        } else {
            return total + ca.iter().zip(cb).take_while(|(x, y)| x == y).count();
        }
    }
}

fn common_suffix(a: &Rope, b: &Rope) -> usize {
    let a_chunks: Vec<&str> = a.chunks().collect();
    let b_chunks: Vec<&str> = b.chunks().collect();
    let mut a_iter = a_chunks.iter().rev();
    let mut b_iter = b_chunks.iter().rev();
    let (mut ca, mut cb): (&[u8], &[u8]) = (&[], &[]);
    let mut total = 0;
    loop {
        if ca.is_empty() {
            match a_iter.next() {
                Some(c) => ca = c.as_bytes(),
                None => return total,
            }
            continue;
        }
        if cb.is_empty() {
            match b_iter.next() {
                Some(c) => cb = c.as_bytes(),
                None => return total,
            }
            continue;
        }
        let n = ca.len().min(cb.len());
        if ca[ca.len() - n..] == cb[cb.len() - n..] {
            total += n;
            ca = &ca[..ca.len() - n];
            cb = &cb[..cb.len() - n];
        } else {
            return total
                + ca.iter()
                    .rev()
                    .zip(cb.iter().rev())
                    .take_while(|(x, y)| x == y)
                    .count();
        }
    }
}

/// Find the injections of `tree` and parse each injected language,
/// recursively.
fn collect_injections(
    language: &SyntaxLanguage,
    tree: &Tree,
    text: &Rope,
    resolver: &dyn LanguageResolver,
    depth: usize,
    out: &mut Vec<InjectionLayer>,
) {
    if depth > MAX_INJECTION_DEPTH {
        return;
    }
    let Some(query) = &language.injections else {
        return;
    };
    let found = injection_ranges(query, &language.id, tree, text);
    for (name, ranges) in found {
        let Some(injected) = resolver.resolve(&name) else {
            continue;
        };
        let mut parser = Parser::new();
        if parser.set_language(&injected.grammar).is_err() {
            continue;
        }
        let ts_ranges: Vec<tree_sitter::Range> = ranges
            .iter()
            .map(|r| tree_sitter::Range {
                start_byte: r.start,
                end_byte: r.end,
                start_point: point(text, r.start),
                end_point: point(text, r.end),
            })
            .collect();
        if parser.set_included_ranges(&ts_ranges).is_err() {
            continue;
        }
        let Some(layer_tree) = parse_rope(&mut parser, text, None) else {
            continue;
        };
        collect_injections(&injected, &layer_tree, text, resolver, depth + 1, out);
        out.push(InjectionLayer {
            language: injected,
            tree: layer_tree,
            ranges,
            depth,
        });
    }
    out.sort_by_key(|layer| (layer.extent().start, layer.depth));
}

/// `(language name, ranges)` of every injection. Separate matches make
/// separate layers, except for patterns with `#set! injection.combined`,
/// whose ranges are parsed as one document.
fn injection_ranges(
    query: &Query,
    host: &str,
    tree: &Tree,
    text: &Rope,
) -> Vec<(String, Vec<Range<usize>>)> {
    let content_index = query.capture_index_for_name("injection.content");
    let language_index = query.capture_index_for_name("injection.language");
    let mut combined: Vec<(usize, String, Vec<Range<usize>>)> = Vec::new();
    let mut separate: Vec<(String, Vec<Range<usize>>)> = Vec::new();
    let mut cursor = QueryCursor::new();
    let mut matches = cursor.matches(query, tree.root_node(), rope_provider(text));
    while let Some(m) = matches.next() {
        let settings = query.property_settings(m.pattern_index);
        let setting = |key: &str| {
            settings
                .iter()
                .find(|p| &*p.key == key)
                .map(|p| p.value.as_deref().unwrap_or("").to_string())
        };
        let mut name = setting("injection.language");
        if setting("injection.self").is_some() || setting("injection.parent").is_some() {
            name = Some(host.to_string());
        }
        let mut contents = Vec::new();
        for capture in m.captures {
            if Some(capture.index) == language_index {
                let raw = slice_text(text, capture.node.byte_range());
                name = Some(raw.trim().to_string());
            } else if Some(capture.index) == content_index {
                contents.push(capture.node.byte_range());
            }
        }
        let Some(name) = name.filter(|n| !n.is_empty()) else {
            continue;
        };
        // Markdown fences: "```rust,ignore" or "```js {1,3}".
        let name = name
            .split(|c: char| c == ',' || c.is_whitespace() || c == '{')
            .next()
            .unwrap_or_default()
            .to_lowercase();
        if contents.is_empty() {
            continue;
        }
        if setting("injection.combined").is_some() {
            match combined
                .iter_mut()
                .find(|(p, n, _)| *p == m.pattern_index && *n == name)
            {
                Some((_, _, ranges)) => ranges.extend(contents),
                None => combined.push((m.pattern_index, name, contents)),
            }
        } else {
            separate.push((name, contents));
        }
    }
    separate.extend(combined.into_iter().map(|(_, name, ranges)| (name, ranges)));
    for (_, ranges) in &mut separate {
        ranges.sort_by_key(|r| r.start);
        ranges.retain(|r| r.start < r.end);
    }
    separate.retain(|(_, ranges)| !ranges.is_empty());
    separate
}
