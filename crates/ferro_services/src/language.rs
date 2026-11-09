//! Code intelligence: diagnostics, completion, hover, signature help, quick
//! fixes, inlay hints, symbols, runnables and rename.
//!
//! Positions are 0-based lines and 0-based columns counted in `char`s, which
//! is what the editor view works in. The real implementation (Stage 6) maps
//! them to LSP's UTF-16 positions.

use std::path::{Path, PathBuf};

use futures::future::BoxFuture;

use crate::Result;

/// A position in a text document.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct TextPos {
    pub line: u32,
    /// Column in `char`s.
    pub column: u32,
}

impl TextPos {
    pub const fn new(line: u32, column: u32) -> Self {
        Self { line, column }
    }
}

/// A half-open range in a text document.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct TextRange {
    pub start: TextPos,
    pub end: TextPos,
}

impl TextRange {
    pub const fn new(start: TextPos, end: TextPos) -> Self {
        Self { start, end }
    }

    /// A range within one line.
    pub const fn on_line(line: u32, start: u32, end: u32) -> Self {
        Self {
            start: TextPos::new(line, start),
            end: TextPos::new(line, end),
        }
    }

    pub fn contains(&self, pos: TextPos) -> bool {
        self.start <= pos && pos < self.end
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Severity {
    Error,
    Warning,
    Info,
    Hint,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Diagnostic {
    pub path: PathBuf,
    pub range: TextRange,
    pub severity: Severity,
    pub message: String,
    /// e.g. `rustc`, `clippy`, `typos`.
    pub source: String,
    pub code: Option<String>,
}

/// What a completion item, a symbol or an outline node is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SymbolKind {
    Module,
    Struct,
    Enum,
    Trait,
    Function,
    Method,
    Field,
    Variable,
    Constant,
    Keyword,
    Snippet,
    Macro,
    Test,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompletionItem {
    pub label: String,
    pub kind: SymbolKind,
    /// Short type or signature, shown right-aligned.
    pub detail: Option<String>,
    /// Markdown documentation, shown beside the list.
    pub documentation: Option<String>,
    pub insert_text: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HoverInfo {
    /// Markdown.
    pub contents: String,
    /// The word or expression the hover is about.
    pub range: TextRange,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SignatureHelp {
    /// e.g. `fn greet(name: &str) -> String`.
    pub label: String,
    /// Byte ranges of each parameter within `label`.
    pub parameters: Vec<std::ops::Range<usize>>,
    pub active_parameter: usize,
    pub documentation: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TextEdit {
    pub range: TextRange,
    pub new_text: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QuickFixKind {
    /// Fixes a diagnostic.
    QuickFix,
    /// Restructures code (extract, inline, …).
    Refactor,
    /// An optional improvement (IntelliJ "intention").
    Intention,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QuickFix {
    pub title: String,
    pub kind: QuickFixKind,
    /// Edits to the requesting document.
    pub edits: Vec<TextEdit>,
    /// Whether it is the preferred fix (shown first, bound to Alt+Enter twice).
    pub preferred: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InlayHintKind {
    /// `: u32` after a binding.
    Type,
    /// `name:` before an argument.
    Parameter,
    /// `-> Result<()>` after a closure or chain.
    Chaining,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InlayHint {
    pub position: TextPos,
    pub label: String,
    pub kind: InlayHintKind,
}

/// A named code element: an outline node or a search result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Symbol {
    pub name: String,
    pub kind: SymbolKind,
    pub path: PathBuf,
    /// The whole element, e.g. a function including its body.
    pub range: TextRange,
    /// Where the name is.
    pub selection: TextPos,
    /// The enclosing element, e.g. `impl Config`, `mod tests`.
    pub container: Option<String>,
    pub children: Vec<Symbol>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunnableKind {
    /// `fn main`.
    Main,
    /// One test.
    Test,
    /// A test module or file: runs all tests in it.
    TestModule,
}

/// Something the gutter shows a ▶ icon for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Runnable {
    pub line: u32,
    pub kind: RunnableKind,
    /// e.g. `greets_by_name`, `tests`.
    pub label: String,
}

/// The edits of a rename, grouped by file, with before/after lines for the
/// preview dialog.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct WorkspaceEdit {
    pub files: Vec<FileEdit>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileEdit {
    pub path: PathBuf,
    pub edits: Vec<PreviewedEdit>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreviewedEdit {
    pub edit: TextEdit,
    pub line_before: String,
    pub line_after: String,
}

impl WorkspaceEdit {
    pub fn edit_count(&self) -> usize {
        self.files.iter().map(|f| f.edits.len()).sum()
    }
}

/// Code intelligence for open documents. Every call receives the current
/// text, so unsaved edits are taken into account.
pub trait LanguageService: Send + Sync + 'static {
    /// Diagnostics of one document.
    fn diagnostics(&self, path: &Path, text: &str) -> BoxFuture<'static, Result<Vec<Diagnostic>>>;
    /// Diagnostics of the whole project, for the Problems tool window.
    fn project_diagnostics(&self) -> BoxFuture<'static, Result<Vec<Diagnostic>>>;
    fn completions(
        &self,
        path: &Path,
        text: &str,
        pos: TextPos,
    ) -> BoxFuture<'static, Result<Vec<CompletionItem>>>;
    fn hover(
        &self,
        path: &Path,
        text: &str,
        pos: TextPos,
    ) -> BoxFuture<'static, Result<Option<HoverInfo>>>;
    fn signature_help(
        &self,
        path: &Path,
        text: &str,
        pos: TextPos,
    ) -> BoxFuture<'static, Result<Option<SignatureHelp>>>;
    /// Quick fixes, refactorings and intentions available at `range`.
    fn code_actions(
        &self,
        path: &Path,
        text: &str,
        range: TextRange,
    ) -> BoxFuture<'static, Result<Vec<QuickFix>>>;
    fn inlay_hints(&self, path: &Path, text: &str) -> BoxFuture<'static, Result<Vec<InlayHint>>>;
    /// The outline of a document, nested.
    fn document_symbols(&self, path: &Path, text: &str) -> BoxFuture<'static, Result<Vec<Symbol>>>;
    /// Symbols across the project matching `query` (fuzzy), best first.
    fn workspace_symbols(&self, query: &str) -> BoxFuture<'static, Result<Vec<Symbol>>>;
    /// `main` functions and tests, for the gutter's ▶ icons. Synchronous:
    /// it must be cheap enough to run on every edit.
    fn runnables(&self, path: &Path, text: &str) -> Vec<Runnable>;
    /// Every edit renaming the symbol at `pos` to `new_name` would make.
    fn rename(
        &self,
        path: &Path,
        text: &str,
        pos: TextPos,
        new_name: &str,
    ) -> BoxFuture<'static, Result<WorkspaceEdit>>;
}
