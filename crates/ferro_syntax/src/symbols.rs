//! File structure, the tags-based symbol index and runnables.

use std::ops::Range;

use tree_sitter::{Query, QueryCursor, StreamingIterator as _};

use crate::tree::{SyntaxTree, rope_provider, slice_text};

/// A definition or reference found by `tags.scm` / `outline.scm`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tag {
    pub name: String,
    /// The part after `definition.` / `reference.`: `function`, `method`,
    /// `class`, `module`, `interface`, `constant`, `call`, …
    pub kind: String,
    pub is_definition: bool,
    /// The whole element, e.g. a function with its body.
    pub range: Range<usize>,
    /// Where the name is.
    pub name_range: Range<usize>,
}

/// A node of the file structure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutlineItem {
    pub name: String,
    pub kind: String,
    pub range: Range<usize>,
    pub name_range: Range<usize>,
    pub children: Vec<OutlineItem>,
}

/// What a ▶ icon runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RunnableTag {
    Main,
    Test,
    TestModule,
}

/// A ▶ icon from `runnables.scm`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunnableMatch {
    /// The line the icon goes on.
    pub line: usize,
    pub tag: RunnableTag,
    /// e.g. the test's name.
    pub name: String,
    pub range: Range<usize>,
}

impl SyntaxTree {
    /// Every definition and reference of the document's language (not of
    /// injected languages), in document order: the symbol index. From
    /// `tags.scm`, or `outline.scm` when the language has no tags query.
    pub fn tags(&self) -> Vec<Tag> {
        match self
            .language
            .tags
            .as_ref()
            .or(self.language.outline.as_ref())
        {
            Some(query) => self.collect_tags(query),
            None => Vec::new(),
        }
    }

    fn collect_tags(&self, query: &Query) -> Vec<Tag> {
        let names = query.capture_names();
        let name_index = query.capture_index_for_name("name");
        let mut cursor = QueryCursor::new();
        let mut matches = cursor.matches(query, self.tree.root_node(), rope_provider(&self.text));
        let mut tags = Vec::new();
        while let Some(m) = matches.next() {
            let mut name_node = None;
            let mut element = None;
            for capture in m.captures {
                if Some(capture.index) == name_index {
                    name_node = Some(capture.node);
                    continue;
                }
                let capture_name = names[capture.index as usize];
                if let Some(kind) = capture_name.strip_prefix("definition.") {
                    element = Some((capture.node, kind, true));
                } else if let Some(kind) = capture_name.strip_prefix("reference.") {
                    element = Some((capture.node, kind, false));
                }
            }
            let (Some(name_node), Some((node, kind, is_definition))) = (name_node, element) else {
                continue;
            };
            let name = slice_text(&self.text, name_node.byte_range());
            let name = name.split_whitespace().collect::<Vec<_>>().join(" ");
            if name.is_empty() {
                continue;
            }
            tags.push(Tag {
                name,
                kind: kind.to_string(),
                is_definition,
                range: node.byte_range(),
                name_range: name_node.byte_range(),
            });
        }
        tags.sort_by_key(|t| (t.range.start, std::cmp::Reverse(t.range.end)));
        tags.dedup_by(|a, b| a.name_range == b.name_range && a.is_definition == b.is_definition);
        tags
    }

    /// The file structure: definitions nested by containment. From
    /// `outline.scm`, or `tags.scm` when the language has no outline query.
    pub fn outline(&self) -> Vec<OutlineItem> {
        let tags = match self
            .language
            .outline
            .as_ref()
            .or(self.language.tags.as_ref())
        {
            Some(query) => self.collect_tags(query),
            None => Vec::new(),
        };
        let definitions: Vec<Tag> = tags.into_iter().filter(|t| t.is_definition).collect();
        nest(definitions)
    }

    /// The ▶ icons of the document.
    pub fn runnables(&self) -> Vec<RunnableMatch> {
        let Some(query) = &self.language.runnables else {
            return Vec::new();
        };
        let run_index = query.capture_index_for_name("run");
        let name_index = query.capture_index_for_name("name");
        let mut cursor = QueryCursor::new();
        let mut matches = cursor.matches(query, self.tree.root_node(), rope_provider(&self.text));
        let mut out: Vec<RunnableMatch> = Vec::new();
        while let Some(m) = matches.next() {
            let tag = query
                .property_settings(m.pattern_index)
                .iter()
                .find(|p| &*p.key == "tag")
                .and_then(|p| p.value.as_deref())
                .map(|v| match v {
                    "main" => RunnableTag::Main,
                    "test-module" => RunnableTag::TestModule,
                    _ => RunnableTag::Test,
                })
                .unwrap_or(RunnableTag::Test);
            let mut run = None;
            let mut name = None;
            for capture in m.captures {
                if Some(capture.index) == run_index {
                    run = Some(capture.node);
                } else if Some(capture.index) == name_index {
                    name = Some(capture.node);
                }
            }
            let Some(run) = run else {
                continue;
            };
            let name = name
                .map(|n| slice_text(&self.text, n.byte_range()))
                .unwrap_or_default();
            let line = run.start_position().row;
            if out.iter().any(|r| r.line == line) {
                continue;
            }
            out.push(RunnableMatch {
                line,
                tag,
                name,
                range: run.byte_range(),
            });
        }
        out.sort_by_key(|r| r.line);
        out
    }
}

/// Nest tags sorted by (start, longest first) into a tree.
fn nest(tags: Vec<Tag>) -> Vec<OutlineItem> {
    fn insert(items: &mut Vec<OutlineItem>, item: OutlineItem) {
        if let Some(parent) = items.last_mut()
            && parent.range.start <= item.range.start
            && item.range.end <= parent.range.end
            && parent.range != item.range
        {
            insert(&mut parent.children, item);
            return;
        }
        items.push(item);
    }
    let mut roots = Vec::new();
    for tag in tags {
        insert(
            &mut roots,
            OutlineItem {
                name: tag.name,
                kind: tag.kind,
                range: tag.range,
                name_range: tag.name_range,
                children: Vec::new(),
            },
        );
    }
    roots
}
