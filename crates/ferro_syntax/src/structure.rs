//! Folding, bracket matching, Expand / Shrink Selection, syntax errors and
//! the syntax context used by auto-pairs.

use std::ops::Range;

use tree_sitter::{Node, QueryCursor, StreamingIterator as _, Tree};

use crate::{
    language::SyntaxLanguage,
    tree::{SyntaxTree, rope_provider},
};

/// A foldable region. The first line stays visible; `end_line` is the last
/// line of the region.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Fold {
    pub start_line: usize,
    pub end_line: usize,
    /// The last line holds only the closing token (`}`, `end`, `</div>`), so
    /// it can stay visible when the region is folded. When `false` (Python,
    /// YAML) the last line belongs to the body.
    pub closed_by_token: bool,
}

/// Where a position is, for auto-pairs and smart typing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SyntaxContext {
    Code,
    String,
    Comment,
}

/// A syntax error found by the parser.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyntaxError {
    pub range: Range<usize>,
    /// For a missing token, its text (e.g. `;`, `}`).
    pub missing: Option<String>,
}

/// A bracket pair: the byte ranges of the opening and closing tokens.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BracketPair {
    pub open: Range<usize>,
    pub close: Range<usize>,
}

impl SyntaxTree {
    /// Foldable regions, outermost first. From `folds.scm` when the language
    /// has one; otherwise every multi-line node delimited by brackets, and
    /// multi-line comments.
    pub fn folds(&self) -> Vec<Fold> {
        let mut folds = Vec::new();
        match &self.language.folds {
            Some(query) => {
                let mut cursor = QueryCursor::new();
                let mut captures =
                    cursor.captures(query, self.tree.root_node(), rope_provider(&self.text));
                while let Some((m, index)) = captures.next() {
                    let node = m.captures[*index].node;
                    if let Some(fold) = self.fold_for(node) {
                        folds.push(fold);
                    }
                }
            }
            None => self.bracket_folds(self.tree.root_node(), &mut folds),
        }
        folds.sort();
        // One fold per start line: the largest.
        folds.dedup_by(|later, kept| {
            if later.start_line == kept.start_line {
                kept.end_line = kept.end_line.max(later.end_line);
                kept.closed_by_token |= later.closed_by_token;
                true
            } else {
                false
            }
        });
        folds
    }

    fn fold_for(&self, node: Node<'_>) -> Option<Fold> {
        let start_line = node.start_position().row;
        let mut end = node.end_position();
        // A node ending at column 0 ends on the previous line.
        if end.column == 0 && end.row > start_line {
            end.row -= 1;
        }
        let end_line = end.row;
        if end_line <= start_line {
            return None;
        }
        let last = self.line_text(end_line);
        let first_token = last.trim_start();
        let closed_by_token = if node.kind().contains("comment") {
            let last = last.trim_end();
            last.ends_with("*/") || last.ends_with("-->") || last.ends_with("]]")
        } else {
            let keyword_end = first_token
                .strip_prefix("end")
                .is_some_and(|rest| !rest.starts_with(|c: char| c.is_alphanumeric() || c == '_'));
            first_token.starts_with(['}', ')', ']', '>'])
                || first_token.starts_with("</")
                || first_token.starts_with("```")
                || keyword_end
        };
        Some(Fold {
            start_line,
            end_line,
            closed_by_token,
        })
    }

    fn bracket_folds(&self, node: Node<'_>, out: &mut Vec<Fold>) {
        if node.end_position().row <= node.start_position().row {
            return;
        }
        let closes = node
            .child(node.child_count().saturating_sub(1) as u32)
            .is_some_and(|last| {
                !last.is_named()
                    && self
                        .language
                        .brackets
                        .iter()
                        .any(|(_, close)| close == last.kind())
            });
        if (closes || node.kind().contains("comment")) && node.parent().is_some() {
            out.extend(self.fold_for(node));
        }
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            self.bracket_folds(child, out);
        }
    }

    /// The bracket pair whose opening or closing token touches `offset`
    /// (the caret is right before or right after it).
    pub fn matching_brackets(&self, offset: usize) -> Option<BracketPair> {
        let candidates = [offset, offset.saturating_sub(1)];
        for at in candidates {
            if at >= self.text.len() {
                continue;
            }
            let (tree, language) = self.tree_at(at..at + 1);
            if let Some(pair) = bracket_at(tree, language, self, at) {
                return Some(pair);
            }
        }
        None
    }

    /// The next larger syntax node around `range`: Expand Selection.
    /// Returns `None` when `range` already covers the whole document.
    pub fn expand_selection(&self, range: Range<usize>) -> Option<Range<usize>> {
        let len = self.text.len();
        let range = range.start.min(len)..range.end.min(len);
        let (tree, _) = self.tree_at(range.clone());
        let expanded = expand_in(tree, range.clone()).or_else(|| {
            // The injection is exhausted: continue in the document's tree.
            expand_in(&self.tree, range.clone())
        })?;
        // Selecting a word first: an empty range expands to the token under it.
        Some(expanded).filter(|r| *r != range)
    }

    /// Syntax errors: `ERROR` nodes and tokens the parser had to assume
    /// (`MISSING`). At most `limit` are returned.
    pub fn syntax_errors(&self, limit: usize) -> Vec<SyntaxError> {
        let mut out = Vec::new();
        let mut stack = vec![self.tree.root_node()];
        while let Some(node) = stack.pop() {
            if out.len() >= limit {
                break;
            }
            if !node.has_error() {
                continue;
            }
            if node.is_missing() {
                out.push(SyntaxError {
                    range: node.byte_range(),
                    missing: Some(node.kind().to_string()),
                });
                continue;
            }
            if node.is_error() {
                out.push(SyntaxError {
                    range: node.byte_range(),
                    missing: None,
                });
                continue;
            }
            let mut cursor = node.walk();
            let children: Vec<Node<'_>> = node.children(&mut cursor).collect();
            stack.extend(children.into_iter().rev());
        }
        out.sort_by_key(|e| e.range.start);
        out
    }

    /// Whether `offset` is inside a string, a comment or plain code.
    pub fn context_at(&self, offset: usize) -> SyntaxContext {
        let len = self.text.len();
        if len == 0 {
            return SyntaxContext::Code;
        }
        let at = offset.min(len);
        let (tree, _) = self.tree_at(at..at);
        let Some(mut node) = tree.root_node().descendant_for_byte_range(at, at) else {
            return SyntaxContext::Code;
        };
        loop {
            let kind = node.kind();
            // Inside the token, not at its edges: `"|"` is a string, `|"…"` is
            // not.
            let inside = node.start_byte() < at && at < node.end_byte()
                || (node.start_byte() < at && at == node.end_byte() && node.is_error());
            if inside {
                if kind.contains("comment") {
                    return SyntaxContext::Comment;
                }
                if kind.contains("string")
                    || kind.contains("char_literal")
                    || kind == "character_literal"
                    || kind == "template_string"
                    || kind == "heredoc_body"
                {
                    return SyntaxContext::String;
                }
            }
            match node.parent() {
                Some(parent) => node = parent,
                None => return SyntaxContext::Code,
            }
        }
    }

    /// The smallest named node that contains `range`, e.g. for breadcrumbs.
    pub fn named_node_at(&self, range: Range<usize>) -> Option<Node<'_>> {
        self.tree
            .root_node()
            .named_descendant_for_byte_range(range.start, range.end)
    }
}

fn expand_in(tree: &Tree, range: Range<usize>) -> Option<Range<usize>> {
    let mut node = tree
        .root_node()
        .descendant_for_byte_range(range.start, range.end)?;
    loop {
        let r = node.byte_range();
        let contains = r.start <= range.start && range.end <= r.end;
        if contains && r != range {
            // Skip a node whose text equals its only child's (wrappers like
            // `expression_statement` around a call without a semicolon).
            return Some(r);
        }
        node = node.parent()?;
    }
}

fn bracket_at(
    tree: &Tree,
    language: &SyntaxLanguage,
    doc: &SyntaxTree,
    at: usize,
) -> Option<BracketPair> {
    // brackets.scm: explicit pairs.
    if let Some(query) = &language.brackets_query {
        let open_index = query.capture_index_for_name("open");
        let close_index = query.capture_index_for_name("close");
        let mut cursor = QueryCursor::new();
        cursor.set_byte_range(at.saturating_sub(1)..at + 1);
        let mut matches = cursor.matches(query, tree.root_node(), rope_provider(&doc.text));
        while let Some(m) = matches.next() {
            let mut open = None;
            let mut close = None;
            for capture in m.captures {
                if Some(capture.index) == open_index {
                    open = Some(capture.node.byte_range());
                } else if Some(capture.index) == close_index {
                    close = Some(capture.node.byte_range());
                }
            }
            if let (Some(open), Some(close)) = (open, close)
                && (open.contains(&at) || close.contains(&at))
            {
                return Some(BracketPair { open, close });
            }
        }
    }
    // Plain tokens: an opening bracket and its closing sibling.
    let token = tree.root_node().descendant_for_byte_range(at, at + 1)?;
    if token.is_named() || token.byte_range().is_empty() {
        return None;
    }
    let kind = token.kind();
    let parent = token.parent()?;
    let mut cursor = parent.walk();
    let siblings: Vec<Node<'_>> = parent.children(&mut cursor).collect();
    let position = siblings.iter().position(|n| n.id() == token.id())?;
    if let Some((open, close)) = language.brackets.iter().find(|(open, _)| open == kind) {
        let mut depth = 0usize;
        for sibling in &siblings[position + 1..] {
            if sibling.kind() == open.as_str() {
                depth += 1;
            } else if sibling.kind() == close.as_str() {
                if depth == 0 {
                    return Some(BracketPair {
                        open: token.byte_range(),
                        close: sibling.byte_range(),
                    });
                }
                depth -= 1;
            }
        }
    }
    if let Some((open, close)) = language.brackets.iter().find(|(_, close)| close == kind) {
        let mut depth = 0usize;
        for sibling in siblings[..position].iter().rev() {
            if sibling.kind() == close.as_str() {
                depth += 1;
            } else if sibling.kind() == open.as_str() {
                if depth == 0 {
                    return Some(BracketPair {
                        open: sibling.byte_range(),
                        close: token.byte_range(),
                    });
                }
                depth -= 1;
            }
        }
    }
    None
}
