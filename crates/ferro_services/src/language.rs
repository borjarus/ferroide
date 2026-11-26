//! Code intelligence: diagnostics, completion, hover, signature help, quick
//! fixes, inlay hints, symbols, runnables, rename, navigation (definition,
//! usages, hierarchies), formatting, semantic tokens, code lens and links.
//!
//! Positions are 0-based lines and 0-based columns counted in `char`s, which
//! is what the editor view works in. The LSP implementation (`ferro_lsp`)
//! maps them to the server's negotiated position encoding (UTF-16 by
//! default).

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
    /// The text the item replaces; `None` replaces the word before the
    /// caret. Postfix templates replace the expression before the dot.
    pub replace: Option<TextRange>,
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
    /// An opaque token the service resolves the fix by when it is chosen
    /// ([`LanguageService::resolve_quick_fix`]): language servers compute
    /// the edits of most fixes lazily, and some touch other files.
    pub data: Option<String>,
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

/// A place in a file, with its line's text for previews (Find Usages, Go to
/// Implementation with several targets).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Location {
    pub path: PathBuf,
    pub range: TextRange,
    /// The text of the line `range` starts on, without the line break.
    pub line_text: String,
}

/// Which Go to … navigation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum GotoKind {
    Definition,
    Declaration,
    TypeDefinition,
    Implementation,
}

/// A token classified by a language server (semantic highlighting).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SemanticToken {
    pub line: u32,
    /// Start column in `char`s.
    pub start: u32,
    /// Length in `char`s.
    pub length: u32,
    /// The token type as LSP names it: `function`, `parameter`, `macro`, …
    pub kind: String,
    /// Modifiers: `declaration`, `readonly`, `deprecated`, …
    pub modifiers: Vec<String>,
}

/// An actionable annotation above a line (references count, ▶ Run, …).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodeLens {
    pub range: TextRange,
    pub title: String,
    /// The server command it runs, if any.
    pub command: Option<String>,
}

/// A link in a document: a URL or a path in a comment, an import, …
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DocumentLink {
    pub range: TextRange,
    /// A URL (`https://…`, `file://…`).
    pub target: String,
}

/// A node of a call or type hierarchy.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HierarchyItem {
    pub name: String,
    pub kind: SymbolKind,
    /// e.g. the signature or the containing type.
    pub detail: Option<String>,
    pub path: PathBuf,
    /// The whole element.
    pub range: TextRange,
    /// Where the name is.
    pub selection: TextRange,
}

/// Which side of a hierarchy to list.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum HierarchyDirection {
    /// Callers of a function.
    Incoming,
    /// Functions a function calls.
    Outgoing,
    /// Supertypes of a type.
    Supertypes,
    /// Subtypes of a type.
    Subtypes,
}

/// Something changed without the editor asking: a language server published
/// diagnostics, a server started or stopped.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LanguageEvent {
    /// New diagnostics for a file; ask [`LanguageService::diagnostics`].
    DiagnosticsChanged(PathBuf),
    /// A server started, became ready, crashed or stopped.
    ServersChanged,
    /// A server asks for a refresh (semantic tokens, inlay hints, code
    /// lens, diagnostics) of every open document.
    RefreshRequested,
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

    // ---- Stage 6. Defaults, so services without a language server (the
    // mock, tree-sitter) only implement what they can answer.

    /// Edits of a quick fix chosen from [`LanguageService::code_actions`],
    /// grouped by file. The default applies the fix's own edits to `path`.
    fn resolve_quick_fix(
        &self,
        path: &Path,
        text: &str,
        fix: &QuickFix,
    ) -> BoxFuture<'static, Result<WorkspaceEdit>> {
        let edit = WorkspaceEdit {
            files: vec![FileEdit {
                path: path.to_path_buf(),
                edits: preview_edits(text, &fix.edits),
            }],
        };
        Box::pin(async move { Ok(edit) })
    }
    /// Go to Definition / Declaration / Type Definition / Implementation.
    fn goto(
        &self,
        _path: &Path,
        _text: &str,
        _pos: TextPos,
        _kind: GotoKind,
    ) -> BoxFuture<'static, Result<Vec<Location>>> {
        Box::pin(async { Ok(Vec::new()) })
    }
    /// Find Usages: every reference to the symbol at `pos`.
    fn references(
        &self,
        _path: &Path,
        _text: &str,
        _pos: TextPos,
        _include_declaration: bool,
    ) -> BoxFuture<'static, Result<Vec<Location>>> {
        Box::pin(async { Ok(Vec::new()) })
    }
    /// Format the document, or only `range`.
    fn format(
        &self,
        _path: &Path,
        _text: &str,
        _range: Option<TextRange>,
    ) -> BoxFuture<'static, Result<Vec<TextEdit>>> {
        Box::pin(async { Ok(Vec::new()) })
    }
    /// Edits after typing `ch` at `pos` (on-type formatting).
    fn format_on_type(
        &self,
        _path: &Path,
        _text: &str,
        _pos: TextPos,
        _ch: char,
    ) -> BoxFuture<'static, Result<Vec<TextEdit>>> {
        Box::pin(async { Ok(Vec::new()) })
    }
    /// The characters that trigger [`LanguageService::format_on_type`] in
    /// this document (empty when no server formats on typing).
    fn format_on_type_triggers(&self, _path: &Path) -> Vec<char> {
        Vec::new()
    }
    /// Optimize Imports: remove unused ones, sort the rest.
    fn organize_imports(
        &self,
        _path: &Path,
        _text: &str,
    ) -> BoxFuture<'static, Result<Vec<TextEdit>>> {
        Box::pin(async { Ok(Vec::new()) })
    }
    /// Semantic tokens of the whole document; `None` when nothing provides
    /// them (tree-sitter highlighting stays).
    fn semantic_tokens(
        &self,
        _path: &Path,
        _text: &str,
    ) -> BoxFuture<'static, Result<Option<Vec<SemanticToken>>>> {
        Box::pin(async { Ok(None) })
    }
    fn code_lenses(&self, _path: &Path, _text: &str) -> BoxFuture<'static, Result<Vec<CodeLens>>> {
        Box::pin(async { Ok(Vec::new()) })
    }
    fn document_links(
        &self,
        _path: &Path,
        _text: &str,
    ) -> BoxFuture<'static, Result<Vec<DocumentLink>>> {
        Box::pin(async { Ok(Vec::new()) })
    }
    /// Callers / callees of the function at `pos`, or supertypes / subtypes
    /// of the type at `pos`.
    fn hierarchy(
        &self,
        _path: &Path,
        _text: &str,
        _pos: TextPos,
        _direction: HierarchyDirection,
    ) -> BoxFuture<'static, Result<Vec<HierarchyItem>>> {
        Box::pin(async { Ok(Vec::new()) })
    }
    /// The editor closed the document (`textDocument/didClose`).
    fn document_closed(&self, _path: &Path) {}
    /// The document was saved (`textDocument/didSave`).
    fn document_saved(&self, _path: &Path, _text: &str) {}
    /// Files changed on disk (client-side file watching for the servers).
    fn files_changed(&self, _events: &[crate::ProjectEvent]) {}
    /// The project was trusted, or trust was withdrawn: language servers
    /// run only in trusted projects.
    fn set_trusted(&self, _trusted: bool) {}
    /// Running language servers, for the status bar.
    fn servers(&self) -> Vec<crate::LanguageServerStatus> {
        Vec::new()
    }
    /// Changes the editor did not ask for (pushed diagnostics, server
    /// state). `None` when the service has none.
    fn subscribe(&self) -> Option<async_channel::Receiver<LanguageEvent>> {
        None
    }
}

/// `edits` with the before/after line of each, for previews.
pub fn preview_edits(text: &str, edits: &[TextEdit]) -> Vec<PreviewedEdit> {
    let lines: Vec<&str> = text.lines().collect();
    edits
        .iter()
        .map(|edit| {
            let line = lines
                .get(edit.range.start.line as usize)
                .copied()
                .unwrap_or_default();
            let line_after =
                if edit.range.start.line == edit.range.end.line && !edit.new_text.contains('\n') {
                    let chars: Vec<char> = line.chars().collect();
                    let start = (edit.range.start.column as usize).min(chars.len());
                    let end = (edit.range.end.column as usize).clamp(start, chars.len());
                    let mut after: String = chars[..start].iter().collect();
                    after.push_str(&edit.new_text);
                    after.extend(&chars[end..]);
                    after
                } else {
                    edit.new_text.lines().next().unwrap_or_default().to_string()
                };
            PreviewedEdit {
                edit: edit.clone(),
                line_before: line.to_string(),
                line_after,
            }
        })
        .collect()
}
