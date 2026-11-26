//! Project-wide search: files by name (Quick Open), text (Find in Files) and
//! TODO comments.
//!
//! Stage 7 adds search scopes and file masks to [`TextQuery`], index events
//! ([`SearchEvent`]) and a typing signal that pauses background indexing.

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
    /// File mask: comma-separated globs (`*.rs, src/**`) or plain text the
    /// relative path must contain (`src/`); `!` excludes (`!target/**`). A
    /// pattern without `/` matches the file name anywhere.
    pub path_filter: Option<String>,
    /// Which files to search.
    pub scope: SearchScope,
}

/// Where Find in Files looks.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum SearchScope {
    /// Every file of the project.
    #[default]
    Project,
    /// Files under a directory (recursively).
    Directory(PathBuf),
    /// Exactly these files (e.g. the open editors).
    Files(Vec<PathBuf>),
    /// Files whose relative path matches a file mask (a named scope from
    /// the settings); evaluated by the service, like `path_filter`.
    Mask(String),
}

impl SearchScope {
    /// Whether `path` is inside the scope.
    pub fn contains(&self, path: &std::path::Path) -> bool {
        match self {
            SearchScope::Project => true,
            SearchScope::Directory(dir) => path.starts_with(dir),
            SearchScope::Files(files) => files.iter().any(|f| f == path),
            SearchScope::Mask(_) => true,
        }
    }
}

/// Progress of the project index, for the status bar and views that show
/// indexed data (TODO).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SearchEvent {
    /// Background indexing: `done` of `total` files.
    Indexing { done: usize, total: usize },
    /// Indexed data changed (files, symbols or TODO comments).
    Updated,
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
    /// Index progress and updates. The default never sends anything.
    fn subscribe(&self) -> async_channel::Receiver<SearchEvent> {
        async_channel::unbounded().1
    }
    /// The user is typing: background indexing pauses for a moment so it
    /// does not compete with the editor.
    fn user_activity(&self) {}
}
