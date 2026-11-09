//! Project-wide search: files by name (Quick Open), text (Find in Files) and
//! TODO comments.

use std::{ops::Range, path::PathBuf};

use futures::future::BoxFuture;

use crate::Result;

/// A file matching a Quick Open query.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileMatch {
    pub path: PathBuf,
    /// Path relative to the project root, with `/` separators.
    pub relative: String,
    pub score: i64,
    /// Byte offsets of the matched characters in `relative`.
    pub positions: Vec<usize>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TextQuery {
    pub text: String,
    pub case_sensitive: bool,
    pub whole_word: bool,
    /// Treat `text` as a regular expression. The mock supports only literal
    /// text and reports an error for regex queries it cannot handle.
    pub regex: bool,
    /// Only files whose relative path contains this (e.g. `src/`, `.rs`).
    pub path_filter: Option<String>,
}

/// One line containing at least one match.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TextMatch {
    pub path: PathBuf,
    pub relative: String,
    /// 0-based.
    pub line: u32,
    pub line_text: String,
    /// Byte ranges of the matches within `line_text`.
    pub ranges: Vec<Range<usize>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TodoKind {
    Todo,
    Fixme,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TodoItem {
    pub path: PathBuf,
    pub relative: String,
    pub line: u32,
    pub kind: TodoKind,
    /// The comment text after the marker.
    pub text: String,
}

pub trait SearchService: Send + Sync + 'static {
    /// Files whose relative path fuzzy-matches `query`, best first.
    fn find_files(&self, query: &str, limit: usize) -> BoxFuture<'static, Result<Vec<FileMatch>>>;
    /// Lines matching `query`, in path order.
    fn find_text(
        &self,
        query: TextQuery,
        limit: usize,
    ) -> BoxFuture<'static, Result<Vec<TextMatch>>>;
    fn todos(&self) -> BoxFuture<'static, Result<Vec<TodoItem>>>;
}
