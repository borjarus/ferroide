//! Code intelligence without a language server (plan §7.3 "Fallback without
//! LSP"): what `ferro_lsp::LspLanguageService` answers with when no server
//! runs for a file (none installed, untrusted project, crashed for good).
//!
//! From the Language Packs' tree-sitter queries: the file structure, ▶ icons,
//! syntax errors as diagnostics, completion of keywords and words of the
//! file, and a project-wide symbol index from `tags.scm` for Go to Symbol.
//! Hover, signature help, quick fixes, inlay hints and rename need a
//! language server and return nothing.

use std::{
    collections::{HashMap, HashSet},
    fs,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, MutexGuard},
    time::SystemTime,
};

use ferro_services::{
    CompletionItem, Diagnostic, HoverInfo, InlayHint, LanguageService, QuickFix, Runnable,
    RunnableKind, Severity, SignatureHelp, Symbol, SymbolKind, TextPos, TextRange, WorkspaceEdit,
    fuzzy,
};
use ferro_syntax::{OutlineItem, RunnableTag, SyntaxTree};
use ferro_text::{LINES, Rope};
use futures::future::BoxFuture;

use crate::{LanguagePack, LanguageRegistry};

/// Files larger than this are not indexed (generated or minified code).
const MAX_INDEXED_FILE: u64 = 1024 * 1024;
/// Parsed documents kept for incremental reparsing.
const CACHED_TREES: usize = 32;
/// Syntax errors reported per file.
const MAX_ERRORS: usize = 50;

/// Lists the files of the project, for the symbol index.
pub type FileLister = Arc<dyn Fn() -> Vec<PathBuf> + Send + Sync>;

/// Searches a persistent symbol index (`ferro_index`) by name: query and
/// limit in, best matches out.
pub type SymbolSearch = Arc<dyn Fn(&str, usize) -> Vec<Symbol> + Send + Sync>;

/// A file's definitions and the modification time they were read at.
type IndexedFile = (Option<SystemTime>, Vec<Symbol>);

/// The tree-sitter language service.
pub struct SyntaxLanguageService {
    inner: Arc<Inner>,
}

struct Inner {
    registry: Arc<LanguageRegistry>,
    files: FileLister,
    /// The last parse of each open document, reparsed incrementally.
    trees: Mutex<Vec<(PathBuf, SyntaxTree)>>,
    /// The latest diagnostics of each document, for the Problems view.
    diagnostics: Mutex<HashMap<PathBuf, Vec<Diagnostic>>>,
    /// Definitions per file on disk, with the file's modification time.
    index: Mutex<HashMap<PathBuf, IndexedFile>>,
    /// A persistent index that answers Go to Symbol instead of `index`.
    symbol_search: Option<SymbolSearch>,
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|e| e.into_inner())
}

impl SyntaxLanguageService {
    pub fn new(registry: Arc<LanguageRegistry>, files: FileLister) -> Self {
        Self {
            inner: Arc::new(Inner {
                registry,
                files,
                trees: Mutex::new(Vec::new()),
                diagnostics: Mutex::new(HashMap::new()),
                index: Mutex::new(HashMap::new()),
                symbol_search: None,
            }),
        }
    }

    /// A service whose Go to Symbol in Workspace asks a persistent index
    /// (Stage 7) instead of parsing project files on demand.
    pub fn with_symbol_index(registry: Arc<LanguageRegistry>, search: SymbolSearch) -> Self {
        Self {
            inner: Arc::new(Inner {
                registry,
                files: Arc::new(Vec::new),
                trees: Mutex::new(Vec::new()),
                diagnostics: Mutex::new(HashMap::new()),
                index: Mutex::new(HashMap::new()),
                symbol_search: Some(search),
            }),
        }
    }
}

/// The definitions of a file for a project-wide symbol index: flat, with
/// their containers, without children. `None` when no Language Pack
/// matches or the file does not parse.
pub fn file_symbols(
    registry: &Arc<LanguageRegistry>,
    path: &Path,
    text: &str,
) -> Option<Vec<Symbol>> {
    let head: String = text.lines().take(5).collect::<Vec<_>>().join(
        "
",
    );
    let pack = registry.detect(path, Some(&head))?.clone();
    let mut symbols = index_file(&pack, registry, path, text)?;
    for symbol in &mut symbols {
        symbol.children.clear();
    }
    Some(symbols)
}

impl Inner {
    fn pack(&self, path: &Path, text: &str) -> Option<Arc<LanguagePack>> {
        let head: String = text.lines().take(5).collect::<Vec<_>>().join("\n");
        let tail: String = {
            let lines: Vec<&str> = text.lines().rev().take(5).collect();
            lines.into_iter().rev().collect::<Vec<_>>().join("\n")
        };
        let probe = format!("{head}\n{tail}");
        self.registry.detect(path, Some(&probe)).cloned()
    }

    /// Run `f` on the document's current tree, reparsing the cached tree
    /// incrementally when there is one.
    fn with_tree<R>(&self, path: &Path, text: &str, f: impl FnOnce(&SyntaxTree) -> R) -> Option<R> {
        let pack = self.pack(path, text)?;
        let syntax = pack.syntax().ok()?;
        let rope = Rope::from_str(text);
        let mut trees = lock(&self.trees);
        if let Some(index) = trees.iter().position(|(p, _)| p == path) {
            let (path_buf, mut tree) = trees.remove(index);
            if tree.language().id() == syntax.id() {
                tree.reparse(&rope);
            } else {
                tree = SyntaxTree::parse_with(syntax, &rope, self.registry.resolver()).ok()?;
            }
            let result = f(&tree);
            trees.push((path_buf, tree));
            return Some(result);
        }
        let tree = SyntaxTree::parse_with(syntax, &rope, self.registry.resolver()).ok()?;
        let result = f(&tree);
        trees.push((path.to_path_buf(), tree));
        if trees.len() > CACHED_TREES {
            trees.remove(0);
        }
        Some(result)
    }

    fn diagnostics(&self, path: &Path, text: &str) -> Vec<Diagnostic> {
        let reports_errors = self
            .pack(path, text)
            .is_some_and(|p| p.manifest().syntax_errors);
        let diagnostics = if reports_errors {
            self.with_tree(path, text, |tree| {
                tree.syntax_errors(MAX_ERRORS)
                    .into_iter()
                    .map(|error| {
                        let message = match &error.missing {
                            Some(token) => format!("Syntax error: missing `{token}`"),
                            None => {
                                let found = tree.text().slice(error.range.clone()).to_string();
                                let found = found.split_whitespace().next().unwrap_or_default();
                                if found.is_empty() || found.chars().count() > 24 {
                                    "Syntax error".to_string()
                                } else {
                                    format!("Syntax error: unexpected `{found}`")
                                }
                            }
                        };
                        Diagnostic {
                            path: path.to_path_buf(),
                            range: text_range(tree.text(), error.range.clone()),
                            severity: Severity::Error,
                            message,
                            source: "tree-sitter".into(),
                            code: None,
                        }
                    })
                    .collect()
            })
            .unwrap_or_default()
        } else {
            Vec::new()
        };
        let mut all = lock(&self.diagnostics);
        if diagnostics.is_empty() {
            all.remove(path);
        } else {
            all.insert(path.to_path_buf(), diagnostics.clone());
        }
        diagnostics
    }

    fn document_symbols(&self, path: &Path, text: &str) -> Vec<Symbol> {
        self.with_tree(path, text, |tree| {
            tree.outline()
                .into_iter()
                .map(|item| symbol(tree.text(), path, item, None))
                .collect()
        })
        .unwrap_or_default()
    }

    fn runnables(&self, path: &Path, text: &str) -> Vec<Runnable> {
        self.with_tree(path, text, |tree| {
            tree.runnables()
                .into_iter()
                .map(|r| Runnable {
                    line: r.line as u32,
                    kind: match r.tag {
                        RunnableTag::Main => RunnableKind::Main,
                        RunnableTag::Test => RunnableKind::Test,
                        RunnableTag::TestModule => RunnableKind::TestModule,
                    },
                    label: if r.name.is_empty() {
                        "main".to_string()
                    } else {
                        r.name
                    },
                })
                .collect()
        })
        .unwrap_or_default()
    }

    fn completions(&self, path: &Path, text: &str, pos: TextPos) -> Vec<CompletionItem> {
        let Some(pack) = self.pack(path, text) else {
            return word_completions(text, pos, &[], &HashMap::new());
        };
        let keywords = pack.syntax().map(|s| keywords(&s)).unwrap_or_default();
        let definitions: HashMap<String, SymbolKind> = self
            .with_tree(path, text, |tree| {
                tree.tags()
                    .into_iter()
                    .filter(|t| t.is_definition)
                    .map(|t| (t.name, symbol_kind(&t.kind)))
                    .collect()
            })
            .unwrap_or_default();
        word_completions(text, pos, &keywords, &definitions)
    }

    /// Definitions of every project file, from the index (files changed on
    /// disk since they were indexed are parsed again).
    fn workspace_symbols(&self, query: &str) -> Vec<Symbol> {
        if let Some(search) = &self.symbol_search {
            return search(query, 200);
        }
        let files = (self.files)();
        let mut index = lock(&self.index);
        let present: HashSet<&PathBuf> = files.iter().collect();
        index.retain(|path, _| present.contains(path));
        for path in &files {
            let Some(pack) = self.registry.detect_path(path) else {
                continue;
            };
            let Ok(meta) = fs::metadata(path) else {
                continue;
            };
            if meta.len() > MAX_INDEXED_FILE {
                continue;
            }
            let modified = meta.modified().ok();
            if index
                .get(path)
                .is_some_and(|(stamp, _)| *stamp == modified && modified.is_some())
            {
                continue;
            }
            let symbols = fs::read_to_string(path)
                .ok()
                .and_then(|text| index_file(pack, &self.registry, path, &text))
                .unwrap_or_default();
            index.insert(path.clone(), (modified, symbols));
        }
        let all: Vec<&Symbol> = index.values().flat_map(|(_, symbols)| symbols).collect();
        let names: Vec<&str> = all.iter().map(|s| s.name.as_str()).collect();
        fuzzy::rank(query, names, 200)
            .into_iter()
            .map(|(i, _)| all[i].clone())
            .collect()
    }
}

/// The definitions of one file, flat, with their containers.
fn index_file(
    pack: &LanguagePack,
    registry: &Arc<LanguageRegistry>,
    path: &Path,
    text: &str,
) -> Option<Vec<Symbol>> {
    let syntax = pack.syntax().ok()?;
    let rope = Rope::from_str(text);
    let tree = SyntaxTree::parse_with(syntax, &rope, registry.resolver()).ok()?;
    let mut out = Vec::new();
    fn flatten(
        rope: &Rope,
        path: &Path,
        items: Vec<OutlineItem>,
        container: Option<&str>,
        out: &mut Vec<Symbol>,
    ) {
        for mut item in items {
            let children = std::mem::take(&mut item.children);
            let symbol = symbol(rope, path, item, container.map(str::to_string));
            let name = symbol.name.clone();
            // Fields and nested keys of data files are not worth a global
            // search; top-level keys are.
            if symbol.kind != SymbolKind::Field || container.is_none() {
                out.push(symbol);
            }
            flatten(rope, path, children, Some(&name), out);
        }
    }
    let items = tree.outline();
    flatten(&rope, path, items, None, &mut out);
    Some(out)
}

fn symbol(rope: &Rope, path: &Path, item: OutlineItem, container: Option<String>) -> Symbol {
    let name = item.name.clone();
    Symbol {
        kind: symbol_kind(&item.kind),
        path: path.to_path_buf(),
        range: text_range(rope, item.range.clone()),
        selection: text_pos(rope, item.name_range.start),
        container,
        children: item
            .children
            .into_iter()
            .map(|child| symbol(rope, path, child, Some(name.clone())))
            .collect(),
        name: item.name,
    }
}

/// A tags capture kind (`definition.<kind>`) as a symbol kind.
pub fn symbol_kind(kind: &str) -> SymbolKind {
    match kind {
        "function" => SymbolKind::Function,
        "method" | "constructor" => SymbolKind::Method,
        "class" | "struct" | "impl" | "type" | "object" | "rule" | "element" => SymbolKind::Struct,
        "interface" | "trait" | "protocol" => SymbolKind::Trait,
        "enum" => SymbolKind::Enum,
        "module" | "namespace" | "package" | "section" | "heading" | "table" => SymbolKind::Module,
        "constant" | "variant" => SymbolKind::Constant,
        "field" | "property" | "key" => SymbolKind::Field,
        "macro" => SymbolKind::Macro,
        _ => SymbolKind::Variable,
    }
}

fn text_pos(rope: &Rope, byte: usize) -> TextPos {
    let byte = rope.floor_char_boundary(byte.min(rope.len()));
    let line = rope.byte_to_line_idx(byte, LINES);
    let line_start = rope.line_to_byte_idx(line, LINES);
    let column = rope.byte_to_char_idx(byte) - rope.byte_to_char_idx(line_start);
    TextPos::new(line as u32, column as u32)
}

fn text_range(rope: &Rope, range: std::ops::Range<usize>) -> TextRange {
    TextRange::new(text_pos(rope, range.start), text_pos(rope, range.end))
}

/// The grammar's keywords: its anonymous, word-like tokens.
fn keywords(syntax: &ferro_syntax::SyntaxLanguage) -> Vec<String> {
    let grammar = syntax.grammar();
    let mut out: Vec<String> = (0..grammar.node_kind_count() as u16)
        .filter(|&id| !grammar.node_kind_is_named(id) && grammar.node_kind_is_visible(id))
        .filter_map(|id| grammar.node_kind_for_id(id))
        .filter(|kind| kind.len() >= 2 && kind.chars().all(|c| c.is_ascii_alphabetic() || c == '_'))
        .map(str::to_string)
        .collect();
    out.sort();
    out.dedup();
    out
}

fn is_word_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// Keywords and words of the document starting with the word at the caret.
fn word_completions(
    text: &str,
    pos: TextPos,
    keywords: &[String],
    definitions: &HashMap<String, SymbolKind>,
) -> Vec<CompletionItem> {
    let Some(line) = text.lines().nth(pos.line as usize) else {
        return Vec::new();
    };
    let before: String = line.chars().take(pos.column as usize).collect();
    let prefix: String = before
        .chars()
        .rev()
        .take_while(|c| is_word_char(*c))
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    if prefix.is_empty() {
        return Vec::new();
    }
    let lower = prefix.to_lowercase();
    let mut counts: HashMap<&str, usize> = HashMap::new();
    for word in text.split(|c: char| !is_word_char(c)) {
        if word.chars().count() >= 3
            && !word.starts_with(|c: char| c.is_ascii_digit())
            && word != prefix
            && word.to_lowercase().starts_with(&lower)
        {
            *counts.entry(word).or_default() += 1;
        }
    }
    let mut words: Vec<(&str, usize)> = counts.into_iter().collect();
    words.sort_by(|a, b| {
        b.1.cmp(&a.1)
            .then(a.0.len().cmp(&b.0.len()))
            .then(a.0.cmp(b.0))
    });
    let mut items: Vec<CompletionItem> = Vec::new();
    for keyword in keywords {
        if keyword.starts_with(&prefix) && *keyword != prefix {
            items.push(CompletionItem {
                label: keyword.clone(),
                kind: SymbolKind::Keyword,
                detail: None,
                documentation: None,
                insert_text: keyword.clone(),
                replace: None,
            });
        }
    }
    for (word, _) in words {
        if items.iter().any(|i| i.label == word) {
            continue;
        }
        items.push(CompletionItem {
            label: word.to_string(),
            kind: definitions
                .get(word)
                .copied()
                .unwrap_or(SymbolKind::Variable),
            detail: None,
            documentation: None,
            insert_text: word.to_string(),
            replace: None,
        });
        if items.len() >= 50 {
            break;
        }
    }
    items
}

impl LanguageService for SyntaxLanguageService {
    fn diagnostics(
        &self,
        path: &Path,
        text: &str,
    ) -> BoxFuture<'static, ferro_services::Result<Vec<Diagnostic>>> {
        let (inner, path, text) = (self.inner.clone(), path.to_path_buf(), text.to_string());
        Box::pin(async move { Ok(inner.diagnostics(&path, &text)) })
    }

    fn project_diagnostics(&self) -> BoxFuture<'static, ferro_services::Result<Vec<Diagnostic>>> {
        let inner = self.inner.clone();
        Box::pin(async move {
            let mut all: Vec<Diagnostic> = lock(&inner.diagnostics)
                .values()
                .flatten()
                .cloned()
                .collect();
            all.sort_by(|a, b| a.path.cmp(&b.path).then(a.range.start.cmp(&b.range.start)));
            Ok(all)
        })
    }

    fn completions(
        &self,
        path: &Path,
        text: &str,
        pos: TextPos,
    ) -> BoxFuture<'static, ferro_services::Result<Vec<CompletionItem>>> {
        let (inner, path, text) = (self.inner.clone(), path.to_path_buf(), text.to_string());
        Box::pin(async move { Ok(inner.completions(&path, &text, pos)) })
    }

    fn hover(
        &self,
        _: &Path,
        _: &str,
        _: TextPos,
    ) -> BoxFuture<'static, ferro_services::Result<Option<HoverInfo>>> {
        Box::pin(async { Ok(None) })
    }

    fn signature_help(
        &self,
        _: &Path,
        _: &str,
        _: TextPos,
    ) -> BoxFuture<'static, ferro_services::Result<Option<SignatureHelp>>> {
        Box::pin(async { Ok(None) })
    }

    fn code_actions(
        &self,
        _: &Path,
        _: &str,
        _: TextRange,
    ) -> BoxFuture<'static, ferro_services::Result<Vec<QuickFix>>> {
        Box::pin(async { Ok(Vec::new()) })
    }

    fn inlay_hints(
        &self,
        _: &Path,
        _: &str,
    ) -> BoxFuture<'static, ferro_services::Result<Vec<InlayHint>>> {
        Box::pin(async { Ok(Vec::new()) })
    }

    fn document_symbols(
        &self,
        path: &Path,
        text: &str,
    ) -> BoxFuture<'static, ferro_services::Result<Vec<Symbol>>> {
        let (inner, path, text) = (self.inner.clone(), path.to_path_buf(), text.to_string());
        Box::pin(async move { Ok(inner.document_symbols(&path, &text)) })
    }

    fn workspace_symbols(
        &self,
        query: &str,
    ) -> BoxFuture<'static, ferro_services::Result<Vec<Symbol>>> {
        let (inner, query) = (self.inner.clone(), query.to_string());
        Box::pin(async move { Ok(inner.workspace_symbols(&query)) })
    }

    fn runnables(&self, path: &Path, text: &str) -> Vec<Runnable> {
        self.inner.runnables(path, text)
    }

    fn rename(
        &self,
        _: &Path,
        _: &str,
        _: TextPos,
        _: &str,
    ) -> BoxFuture<'static, ferro_services::Result<WorkspaceEdit>> {
        Box::pin(async {
            anyhow::bail!("Rename needs a language server (Code ▸ Install Language Server)")
        })
    }
}
