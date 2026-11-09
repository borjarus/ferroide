//! Code intelligence by text scanning. Nothing here parses properly: the
//! heuristics are tuned to the demo project so every popup and gutter mark
//! has believable data, and stay harmless on any other text.

use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
    sync::Arc,
};

use anyhow::{anyhow, bail};
use futures::{FutureExt as _, future::BoxFuture};

use super::{Dice, MockConfig, MockProjectService, hash_str, mix, search::todo_in_line, simulate};
use crate::{
    CompletionItem, Diagnostic, FileEdit, HoverInfo, InlayHint, InlayHintKind, LanguageService,
    PreviewedEdit, QuickFix, QuickFixKind, Result, Runnable, RunnableKind, Severity, SignatureHelp,
    Symbol, SymbolKind, TextEdit, TextPos, TextRange, WorkspaceEdit, fuzzy,
};

pub struct MockLanguageService {
    config: MockConfig,
    dice: Dice,
    project: Arc<MockProjectService>,
}

impl MockLanguageService {
    pub fn new(config: MockConfig, project: Arc<MockProjectService>) -> Self {
        Self {
            dice: Dice::new(config.seed ^ 0x1a2b),
            config,
            project,
        }
    }

    fn should_fail(&self) -> bool {
        self.config.failure_rate > 0.0 && self.dice.roll() < self.config.failure_rate
    }

    fn answer<T: Send + 'static>(
        &self,
        result: Result<T>,
        fail: bool,
        what: &'static str,
    ) -> BoxFuture<'static, Result<T>> {
        let latency = self.config.latency;
        async move {
            simulate(latency, fail, what).await?;
            result
        }
        .boxed()
    }

    fn rel(&self, path: &Path) -> String {
        self.project.relative(path).unwrap_or_default()
    }
}

// ---------------------------------------------------------------------------
// Text helpers
// ---------------------------------------------------------------------------

fn is_ident(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Lang {
    Rust,
    TypeScript,
    Other,
}

fn lang(path: &Path) -> Lang {
    match path.extension().and_then(|e| e.to_str()) {
        Some("rs") => Lang::Rust,
        Some("ts" | "tsx" | "js" | "jsx") => Lang::TypeScript,
        _ => Lang::Other,
    }
}

fn char_col(line: &str, byte: usize) -> u32 {
    line[..byte].chars().count() as u32
}

fn byte_of_col(line: &str, col: u32) -> usize {
    line.char_indices()
        .nth(col as usize)
        .map(|(b, _)| b)
        .unwrap_or(line.len())
}

/// Whole-word occurrences of `word` in `line`, as byte offsets.
fn word_occurrences(line: &str, word: &str) -> Vec<usize> {
    line.match_indices(word)
        .map(|(b, _)| b)
        .filter(|&b| {
            let before = line[..b].chars().next_back().is_none_or(|c| !is_ident(c));
            let after = line[b + word.len()..]
                .chars()
                .next()
                .is_none_or(|c| !is_ident(c));
            before && after
        })
        .collect()
}

/// The first whole-word occurrence of `word`, as a range.
fn find_word(text: &str, word: &str) -> Option<TextRange> {
    text.lines().enumerate().find_map(|(ix, line)| {
        word_occurrences(line, word).first().map(|&b| {
            let col = char_col(line, b);
            TextRange::on_line(ix as u32, col, col + word.chars().count() as u32)
        })
    })
}

/// The first occurrence of `needle` anywhere, as a range.
fn find_str(text: &str, needle: &str) -> Option<TextRange> {
    text.lines().enumerate().find_map(|(ix, line)| {
        line.find(needle).map(|b| {
            let col = char_col(line, b);
            TextRange::on_line(ix as u32, col, col + needle.chars().count() as u32)
        })
    })
}

/// The identifier under or just before `pos`: (word, start column, end column).
fn word_at(text: &str, pos: TextPos) -> Option<(String, u32, u32)> {
    let line = text.lines().nth(pos.line as usize)?;
    let chars: Vec<char> = line.chars().collect();
    let mut col = (pos.column as usize).min(chars.len());
    if col == chars.len() || !is_ident(chars[col]) {
        if col == 0 || !is_ident(chars[col - 1]) {
            return None;
        }
        col -= 1;
    }
    let start = (0..=col).rev().take_while(|&i| is_ident(chars[i])).last()?;
    let end = (col..chars.len())
        .take_while(|&i| is_ident(chars[i]))
        .last()?
        + 1;
    let word: String = chars[start..end].iter().collect();
    Some((word, start as u32, end as u32))
}

/// The identifier prefix typed just before `pos`.
fn prefix_at(text: &str, pos: TextPos) -> String {
    let Some(line) = text.lines().nth(pos.line as usize) else {
        return String::new();
    };
    let chars: Vec<char> = line.chars().take(pos.column as usize).collect();
    let start = chars
        .iter()
        .rposition(|c| !is_ident(*c))
        .map(|i| i + 1)
        .unwrap_or(0);
    chars[start..].iter().collect()
}

fn line_len(line: &str) -> u32 {
    line.chars().count() as u32
}

// ---------------------------------------------------------------------------
// Diagnostics
// ---------------------------------------------------------------------------

fn diag(
    path: &Path,
    range: TextRange,
    severity: Severity,
    message: &str,
    source: &str,
    code: Option<&str>,
) -> Diagnostic {
    Diagnostic {
        path: path.to_path_buf(),
        range,
        severity,
        message: message.into(),
        source: source.into(),
        code: code.map(str::to_string),
    }
}

/// Diagnostics of a demo file, placed on the identifiers they talk about.
fn demo_diagnostics(rel: &str, path: &Path, text: &str) -> Vec<Diagnostic> {
    use Severity::*;
    let mut out = Vec::new();
    let mut add = |range: Option<TextRange>, severity, message: &str, source, code| {
        if let Some(range) = range {
            out.push(diag(path, range, severity, message, source, code));
        }
    };
    match rel {
        "src/config.rs" => {
            add(
                find_str(text, "pub retries")
                    .map(|r| TextRange::on_line(r.start.line, r.start.column + 4, r.end.column)),
                Warning,
                "field `retries` is never read",
                "rustc",
                Some("dead_code"),
            );
            add(
                find_word(text, "from_str"),
                Error,
                "type annotations needed: cannot infer type of the type parameter `T` declared on the function `from_str`",
                "rustc",
                Some("E0282"),
            );
        }
        "src/main.rs" => add(
            find_str(text, ".unwrap_or_default()"),
            Warning,
            "errors are silently ignored: `unwrap_or_default` on a `Result`",
            "clippy",
            Some("clippy::unwrap_or_default"),
        ),
        "src/lib.rs" => add(
            find_str(text, "\"Hello, stranger!\".to_string()"),
            Hint,
            "use `String::from` to convert a literal",
            "clippy",
            Some("clippy::str_to_string"),
        ),
        "README.md" => add(
            find_word(text, "FerroIDE's"),
            Info,
            "Typo: In word 'FerroIDE'",
            "typos",
            None,
        ),
        "web/src/app.ts" => add(
            find_str(text, ")!.")
                .map(|r| TextRange::on_line(r.start.line, r.start.column + 1, r.start.column + 2)),
            Warning,
            "Forbidden non-null assertion.",
            "eslint",
            Some("@typescript-eslint/no-non-null-assertion"),
        ),
        "Cargo.toml" => add(
            find_str(text, "toml = \"0.9\"")
                .map(|r| TextRange::on_line(r.start.line, r.start.column + 7, r.end.column)),
            Info,
            "A newer version of `toml` is available: 0.9.8",
            "crates",
            None,
        ),
        "src/merge_me.rs" => {
            for (ix, line) in text.lines().enumerate() {
                if line.starts_with("<<<<<<<") {
                    add(
                        Some(TextRange::on_line(ix as u32, 0, line_len(line))),
                        Error,
                        "unresolved merge conflict",
                        "git",
                        None,
                    );
                }
            }
        }
        _ => {}
    }
    out
}

fn todo_diagnostics(path: &Path, text: &str) -> Vec<Diagnostic> {
    text.lines()
        .enumerate()
        .filter_map(|(ix, line)| {
            let (kind, byte, body) = todo_in_line(line)?;
            let marker = if kind == crate::TodoKind::Todo {
                "TODO"
            } else {
                "FIXME"
            };
            Some(diag(
                path,
                TextRange::on_line(ix as u32, char_col(line, byte), line_len(line)),
                Severity::Info,
                &format!("{marker}: {body}"),
                "todo",
                None,
            ))
        })
        .collect()
}

/// Generated diagnostics for a synthetic stress file: one in nine files has
/// an unused function.
fn stress_diagnostics(seed: u64, path: &Path, text: &str) -> Vec<Diagnostic> {
    let h = hash_str(seed ^ 0xd1a9, &path.to_string_lossy());
    if !h.is_multiple_of(9) {
        return Vec::new();
    }
    let lines: Vec<&str> = text.lines().collect();
    let pick = 1 + (mix(h) as usize % lines.len().saturating_sub(1).max(1));
    let Some(line) = lines.get(pick) else {
        return Vec::new();
    };
    let Some(name) = line.strip_prefix("fn ").and_then(|l| l.split('(').next()) else {
        return Vec::new();
    };
    let mut out = vec![diag(
        path,
        TextRange::on_line(pick as u32, 3, 3 + name.chars().count() as u32),
        Severity::Warning,
        &format!("function `{name}` is never used"),
        "rustc",
        Some("dead_code"),
    )];
    if h.is_multiple_of(27) {
        out.push(diag(
            path,
            TextRange::on_line(0, 0, 2),
            Severity::Error,
            "expected item, found `//`",
            "rustc",
            None,
        ));
    }
    out
}

fn diagnostics_for(rel: &str, path: &Path, text: &str) -> Vec<Diagnostic> {
    let mut out = demo_diagnostics(rel, path, text);
    out.extend(todo_diagnostics(path, text));
    out.sort_by_key(|d| (d.range.start, d.severity));
    out
}

/// The quick fix for a diagnostic, when there is one.
fn fix_for(d: &Diagnostic, text: &str) -> Option<QuickFix> {
    let line = text.lines().nth(d.range.start.line as usize)?;
    let fix = |title: &str, edits: Vec<TextEdit>| QuickFix {
        title: title.into(),
        kind: QuickFixKind::QuickFix,
        edits,
        preferred: true,
    };
    let edit = |range, new_text: &str| TextEdit {
        range,
        new_text: new_text.into(),
    };
    match d.code.as_deref() {
        Some("E0282") => Some(fix(
            "Specify the type: `toml::from_str::<Self>`",
            vec![edit(TextRange::new(d.range.end, d.range.end), "::<Self>")],
        )),
        Some("clippy::unwrap_or_default") => Some(fix(
            "Propagate the error with `?`",
            vec![edit(d.range, "?")],
        )),
        Some("dead_code") => {
            let indent: String = line.chars().take_while(|c| c.is_whitespace()).collect();
            let start = TextPos::new(d.range.start.line, 0);
            Some(fix(
                "Add `#[allow(dead_code)]`",
                vec![edit(
                    TextRange::new(start, start),
                    &format!("{indent}#[allow(dead_code)]\n"),
                )],
            ))
        }
        Some("clippy::str_to_string") => {
            let literal = "\"Hello, stranger!\"";
            Some(fix(
                "Replace with `String::from`",
                vec![edit(d.range, &format!("String::from({literal})"))],
            ))
        }
        Some("@typescript-eslint/no-non-null-assertion") => {
            let indent: String = line.chars().take_while(|c| c.is_whitespace()).collect();
            let start = TextPos::new(d.range.start.line, 0);
            Some(fix(
                "Suppress with an eslint-disable comment",
                vec![edit(
                    TextRange::new(start, start),
                    &format!(
                        "{indent}// eslint-disable-next-line @typescript-eslint/no-non-null-assertion\n"
                    ),
                )],
            ))
        }
        _ if d.source == "crates" => {
            let r = d.range;
            Some(fix("Update to 0.9.8", vec![edit(r, "\"0.9.8\"")]))
        }
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Outline
// ---------------------------------------------------------------------------

/// Count `{` and `}` outside string literals and line comments.
fn braces(line: &str) -> (i32, i32) {
    let (mut open, mut close) = (0, 0);
    let mut in_str: Option<char> = None;
    let mut prev = '\0';
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        match in_str {
            Some(q) => {
                if c == q && prev != '\\' {
                    in_str = None;
                }
            }
            None => match c {
                '"' | '`' => in_str = Some(c),
                '\'' if line.contains("'\"'") => {}
                '/' if chars.peek() == Some(&'/') => break,
                '{' => open += 1,
                '}' => close += 1,
                _ => {}
            },
        }
        prev = if prev == '\\' && c == '\\' { '\0' } else { c };
    }
    (open, close)
}

fn leading_ident(s: &str) -> Option<&str> {
    let end = s.find(|c: char| !is_ident(c)).unwrap_or(s.len());
    (end > 0 && !s.starts_with(|c: char| c.is_ascii_digit())).then(|| &s[..end])
}

/// A declaration found on a line: kind, name, and the byte offset of the name.
type Decl = (SymbolKind, String, usize);

fn rust_decl(line: &str, prev: &str, parent: Option<&Symbol>) -> Option<Decl> {
    let trimmed = line.trim_start();
    let mut rest = trimmed;
    loop {
        let before = rest.len();
        for p in ["pub(crate) ", "pub(super) ", "pub ", "async ", "unsafe "] {
            rest = rest.strip_prefix(p).unwrap_or(rest);
        }
        // `const fn x` is a function, not a constant.
        if rest.starts_with("const fn ") {
            rest = &rest["const ".len()..];
        }
        if rest.len() == before {
            break;
        }
    }
    let offset = |name: &str, after: &str| -> usize {
        let base = line.len() - after.len();
        base + after.find(name).unwrap_or(0)
    };
    let keywords: [(&str, SymbolKind); 9] = [
        ("fn ", SymbolKind::Function),
        ("struct ", SymbolKind::Struct),
        ("enum ", SymbolKind::Enum),
        ("trait ", SymbolKind::Trait),
        ("mod ", SymbolKind::Module),
        ("const ", SymbolKind::Constant),
        ("static ", SymbolKind::Constant),
        ("type ", SymbolKind::Struct),
        ("macro_rules! ", SymbolKind::Macro),
    ];
    for (kw, kind) in keywords {
        if let Some(after) = rest.strip_prefix(kw) {
            let name = leading_ident(after)?;
            let kind = match kind {
                SymbolKind::Function
                    if prev.trim_start().starts_with("#[test]")
                        || prev.trim_start().starts_with("#[tokio::test]") =>
                {
                    SymbolKind::Test
                }
                SymbolKind::Function
                    if parent.is_some_and(|p| {
                        p.kind == SymbolKind::Trait || p.name.starts_with("impl ")
                    }) =>
                {
                    SymbolKind::Method
                }
                k => k,
            };
            return Some((kind, name.to_string(), offset(name, after)));
        }
    }
    if let Some(after) = rest.strip_prefix("impl") {
        if !after.starts_with([' ', '<']) {
            return None;
        }
        let mut after = after;
        if after.starts_with('<') {
            let mut depth = 0;
            let close = after.char_indices().find(|&(_, c)| {
                depth += match c {
                    '<' => 1,
                    '>' => -1,
                    _ => 0,
                };
                depth == 0
            })?;
            after = &after[close.0 + 1..];
        }
        let header = after.split('{').next()?.trim();
        let header = header.split(" where").next()?.trim();
        if header.is_empty() {
            return None;
        }
        return Some((
            SymbolKind::Struct,
            format!("impl {header}"),
            line.len() - rest.len(),
        ));
    }
    // Fields and enum variants.
    let parent = parent?;
    if parent.name.starts_with("impl ") || rest.starts_with("#") || rest.starts_with("//") {
        return None;
    }
    let name = leading_ident(rest)?;
    let after = &rest[name.len()..];
    match parent.kind {
        SymbolKind::Struct if after.starts_with(':') && !after.starts_with("::") => {
            Some((SymbolKind::Field, name.to_string(), line.len() - rest.len()))
        }
        SymbolKind::Enum if after.is_empty() || after.starts_with([',', '(', ' ', '{']) => Some((
            SymbolKind::Constant,
            name.to_string(),
            line.len() - rest.len(),
        )),
        _ => None,
    }
}

fn ts_decl(line: &str, depth: i32, parent: Option<&Symbol>) -> Option<Decl> {
    let mut rest = line.trim_start();
    for p in ["export ", "default ", "async ", "declare "] {
        rest = rest.strip_prefix(p).unwrap_or(rest);
    }
    let base = line.len() - rest.len();
    let keywords: [(&str, SymbolKind); 6] = [
        ("function ", SymbolKind::Function),
        ("class ", SymbolKind::Struct),
        ("interface ", SymbolKind::Trait),
        ("type ", SymbolKind::Struct),
        ("const ", SymbolKind::Constant),
        ("let ", SymbolKind::Variable),
    ];
    for (kw, kind) in keywords {
        if let Some(after) = rest.strip_prefix(kw) {
            if matches!(kind, SymbolKind::Constant | SymbolKind::Variable) && depth > 0 {
                return None;
            }
            let name = leading_ident(after)?;
            return Some((kind, name.to_string(), base + kw.len()));
        }
    }
    let parent = parent?;
    let name = leading_ident(rest)?;
    let after = &rest[name.len()..];
    match parent.kind {
        SymbolKind::Trait if after.starts_with([':', '?']) => {
            Some((SymbolKind::Field, name.to_string(), base))
        }
        SymbolKind::Struct
            if after.starts_with('(') && !["if", "for", "while", "switch"].contains(&name) =>
        {
            Some((SymbolKind::Method, name.to_string(), base))
        }
        SymbolKind::Struct if after.starts_with([':', ' ', ';', '=']) => {
            Some((SymbolKind::Field, name.to_string(), base))
        }
        _ => None,
    }
}

struct Open {
    symbol: Symbol,
    depth: i32,
}

/// Nested document symbols, by declaration keywords and brace depth.
fn outline(path: &Path, text: &str) -> Vec<Symbol> {
    let lang = lang(path);
    if lang == Lang::Other {
        return Vec::new();
    }
    let mut roots: Vec<Symbol> = Vec::new();
    let mut stack: Vec<Open> = Vec::new();
    let mut pending: Option<Symbol> = None;
    let mut depth = 0i32;
    let mut prev = "";

    fn attach(stack: &mut [Open], roots: &mut Vec<Symbol>, symbol: Symbol) {
        match stack.last_mut() {
            Some(open) => open.symbol.children.push(symbol),
            None => roots.push(symbol),
        }
    }

    for (ix, line) in text.lines().enumerate() {
        let ix = ix as u32;
        let (opens, closes) = braces(line);
        let parent = stack.last().map(|o| &o.symbol);
        let decl = match lang {
            Lang::Rust => rust_decl(line, prev, parent),
            _ => ts_decl(line, depth, parent),
        };
        if let Some((kind, name, byte)) = decl {
            let indent = char_col(line, line.len() - line.trim_start().len());
            let symbol = Symbol {
                container: parent.map(|p| p.name.clone()),
                name,
                kind,
                path: path.to_path_buf(),
                range: TextRange::new(TextPos::new(ix, indent), TextPos::new(ix, line_len(line))),
                selection: TextPos::new(ix, char_col(line, byte.min(line.len()))),
                children: Vec::new(),
            };
            let leaf = matches!(kind, SymbolKind::Field)
                || (kind == SymbolKind::Constant && opens == 0)
                || line.trim_end().ends_with(';')
                || (opens > 0 && opens == closes);
            if opens > closes {
                stack.push(Open { symbol, depth });
            } else if leaf {
                attach(&mut stack, &mut roots, symbol);
            } else {
                pending = Some(symbol);
            }
        } else if let Some(symbol) = pending.take() {
            if opens > closes {
                stack.push(Open { symbol, depth });
            } else if line.trim_end().ends_with(';') {
                let mut symbol = symbol;
                symbol.range.end = TextPos::new(ix, line_len(line));
                attach(&mut stack, &mut roots, symbol);
            } else {
                pending = Some(symbol);
            }
        }
        depth += opens - closes;
        while stack.last().is_some_and(|o| depth <= o.depth) {
            if let Some(mut open) = stack.pop() {
                open.symbol.range.end = TextPos::new(ix, line_len(line));
                attach(&mut stack, &mut roots, open.symbol);
            }
        }
        if !line.trim().is_empty() {
            prev = line;
        }
    }
    let last = text.lines().count().saturating_sub(1) as u32;
    while let Some(mut open) = stack.pop() {
        open.symbol.range.end = TextPos::new(last, 0);
        attach(&mut stack, &mut roots, open.symbol);
    }
    roots
}

fn flatten(symbols: &[Symbol], out: &mut Vec<Symbol>) {
    for s in symbols {
        let mut flat = s.clone();
        flat.children.clear();
        out.push(flat);
        flatten(&s.children, out);
    }
}

// ---------------------------------------------------------------------------
// Completion, hover, signatures
// ---------------------------------------------------------------------------

const RUST_KEYWORDS: &[&str] = &[
    "as", "async", "await", "break", "const", "continue", "crate", "else", "enum", "fn", "for",
    "if", "impl", "in", "let", "loop", "match", "mod", "move", "mut", "pub", "ref", "return",
    "self", "Self", "static", "struct", "super", "trait", "type", "use", "where", "while",
];

const TS_KEYWORDS: &[&str] = &[
    "async",
    "await",
    "boolean",
    "class",
    "const",
    "else",
    "export",
    "extends",
    "false",
    "for",
    "from",
    "function",
    "if",
    "import",
    "interface",
    "let",
    "new",
    "number",
    "return",
    "string",
    "true",
    "type",
    "typeof",
    "undefined",
];

/// (label, kind, detail, documentation, insert text)
type StdItem = (
    &'static str,
    SymbolKind,
    &'static str,
    &'static str,
    &'static str,
);

const RUST_STD: &[StdItem] = &[
    (
        "String",
        SymbolKind::Struct,
        "std::string::String",
        "A UTF-8–encoded, growable string.",
        "String",
    ),
    (
        "Vec",
        SymbolKind::Struct,
        "std::vec::Vec<T>",
        "A contiguous growable array type.",
        "Vec",
    ),
    (
        "Option",
        SymbolKind::Enum,
        "core::option::Option<T>",
        "Optional values: `Some(T)` or `None`.",
        "Option",
    ),
    (
        "Result",
        SymbolKind::Enum,
        "core::result::Result<T, E>",
        "Error handling with `Ok(T)` and `Err(E)`.",
        "Result",
    ),
    (
        "HashMap",
        SymbolKind::Struct,
        "std::collections::HashMap<K, V>",
        "A hash map implemented with quadratic probing and SIMD lookup.",
        "HashMap",
    ),
    (
        "Some",
        SymbolKind::Constant,
        "Option::Some(T)",
        "Some value of type `T`.",
        "Some()",
    ),
    (
        "None",
        SymbolKind::Constant,
        "Option::None",
        "No value.",
        "None",
    ),
    (
        "Ok",
        SymbolKind::Constant,
        "Result::Ok(T)",
        "Contains the success value.",
        "Ok()",
    ),
    (
        "Err",
        SymbolKind::Constant,
        "Result::Err(E)",
        "Contains the error value.",
        "Err()",
    ),
    (
        "println!",
        SymbolKind::Macro,
        "macro_rules! println",
        "Prints to the standard output, with a newline.",
        "println!()",
    ),
    (
        "format!",
        SymbolKind::Macro,
        "macro_rules! format",
        "Creates a `String` using interpolation of runtime expressions.",
        "format!()",
    ),
    (
        "vec!",
        SymbolKind::Macro,
        "macro_rules! vec",
        "Creates a `Vec` containing the arguments.",
        "vec![]",
    ),
    (
        "assert_eq!",
        SymbolKind::Macro,
        "macro_rules! assert_eq",
        "Asserts that two expressions are equal to each other.",
        "assert_eq!()",
    ),
    (
        "tfn",
        SymbolKind::Snippet,
        "#[test] fn …",
        "Insert a test function.",
        "#[test]\nfn name() {\n    \n}",
    ),
];

const TS_STD: &[StdItem] = &[
    (
        "console",
        SymbolKind::Variable,
        "var console: Console",
        "The console object provides access to the debugging console.",
        "console",
    ),
    (
        "document",
        SymbolKind::Variable,
        "var document: Document",
        "The page's Document.",
        "document",
    ),
    (
        "Promise",
        SymbolKind::Struct,
        "interface Promise<T>",
        "Represents the completion of an asynchronous operation.",
        "Promise",
    ),
    (
        "Array",
        SymbolKind::Struct,
        "interface Array<T>",
        "A list of values.",
        "Array",
    ),
];

/// Markdown hover for well-known names.
fn known_hover(word: &str, lang: Lang) -> Option<String> {
    let (code, doc): (&str, &str) = match (lang, word) {
        (Lang::TypeScript, "greet") => (
            "function greet({ name, excited }: Greeting): string",
            "Build a greeting for `name`; `excited` adds an exclamation mark.",
        ),
        (Lang::TypeScript, "Greeting") => (
            "interface Greeting {\n  name: string;\n  excited?: boolean;\n}",
            "Input of `greet`.",
        ),
        (Lang::TypeScript, "document") => ("var document: Document", "The page's Document."),
        (Lang::Rust, "greet") => (
            "pub fn greet(name: &str) -> String",
            "Build a greeting for `name`.",
        ),
        (Lang::Rust, "Config") => (
            "pub struct Config {\n    pub names: Vec<String>,\n    pub retries: u32,\n}",
            "Settings loaded from `demo.toml`.",
        ),
        (Lang::Rust, "load") => (
            "pub fn load(path: impl AsRef<Path>) -> Result<Self>",
            "Read and parse a TOML config file. Errors carry the file path.",
        ),
        (Lang::Rust, "main") => ("fn main() -> Result<()>", "Entry point of `demo-app`."),
        (Lang::Rust, "Result") => (
            "pub type Result<T, E = anyhow::Error> = core::result::Result<T, E>",
            "`Result<T, Error>`, from **anyhow**.",
        ),
        (Lang::Rust, "String") => ("pub struct String", "A UTF-8–encoded, growable string."),
        (Lang::Rust, "Vec") => (
            "pub struct Vec<T, A: Allocator = Global>",
            "A contiguous growable array type.",
        ),
        (Lang::Rust, "Option") => ("pub enum Option<T> { None, Some(T) }", "Optional values."),
        (Lang::Rust, "println") => (
            "macro_rules! println",
            "Prints to the standard output, with a newline.",
        ),
        (Lang::Rust, "format") => (
            "macro_rules! format",
            "Creates a `String` using interpolation of runtime expressions.",
        ),
        (Lang::Rust, "default_retries") => (
            "fn default_retries() -> u32",
            "Retries used when `demo.toml` does not set them: 3.",
        ),
        (Lang::Rust, "names") => ("pub names: Vec<String>", "People to greet."),
        (Lang::Rust, "retries") => ("pub retries: u32", "How often to retry reading the file."),
        (Lang::Rust, "name") => ("name: &str", "The name to greet."),
        (Lang::Rust, "from_str") => (
            "pub fn from_str<T>(s: &str) -> Result<T, toml::de::Error>\nwhere\n    T: DeserializeOwned",
            "Deserializes a string into a type.",
        ),
        _ => return None,
    };
    let fence = if lang == Lang::TypeScript {
        "ts"
    } else {
        "rust"
    };
    Some(format!("```{fence}\n{code}\n```\n\n{doc}"))
}

fn symbol_keyword(kind: SymbolKind) -> &'static str {
    match kind {
        SymbolKind::Module => "mod",
        SymbolKind::Struct => "struct",
        SymbolKind::Enum => "enum",
        SymbolKind::Trait => "trait",
        SymbolKind::Function | SymbolKind::Method | SymbolKind::Test => "fn",
        SymbolKind::Field => "field",
        SymbolKind::Variable => "let",
        SymbolKind::Constant => "const",
        SymbolKind::Macro => "macro_rules!",
        SymbolKind::Keyword | SymbolKind::Snippet => "",
    }
}

/// (label, parameters)
fn known_signature(
    name: &str,
    lang: Lang,
) -> Option<(&'static str, &'static [&'static str], &'static str)> {
    Some(match (lang, name) {
        (Lang::TypeScript, "greet") => (
            "function greet({ name, excited }: Greeting): string",
            &["{ name, excited }: Greeting"],
            "Build a greeting.",
        ),
        (_, "greet") => (
            "fn greet(name: &str) -> String",
            &["name: &str"],
            "Build a greeting for `name`.",
        ),
        (_, "load") => (
            "pub fn load(path: impl AsRef<Path>) -> Result<Config>",
            &["path: impl AsRef<Path>"],
            "Read and parse a TOML config file.",
        ),
        (_, "format!" | "println!" | "print!" | "eprintln!") => (
            "macro format!(fmt: &'static str, args: impl Display...)",
            &["fmt: &'static str", "args: impl Display..."],
            "Formats its arguments according to `fmt`.",
        ),
        (_, "assert_eq!") => (
            "macro assert_eq!(left: T, right: T)",
            &["left: T", "right: T"],
            "Asserts that two expressions are equal.",
        ),
        (_, "from_str") => (
            "pub fn from_str<T>(s: &str) -> Result<T, Error>",
            &["s: &str"],
            "Deserializes a string.",
        ),
        (_, "read_to_string") => (
            "pub fn read_to_string<P: AsRef<Path>>(path: P) -> io::Result<String>",
            &["path: P"],
            "Reads the entire contents of a file into a string.",
        ),
        _ => return None,
    })
}

fn offset_of(text: &str, pos: TextPos) -> usize {
    let mut offset = 0;
    for (ix, line) in text.split_inclusive('\n').enumerate() {
        if ix as u32 == pos.line {
            let body = line.trim_end_matches(['\n', '\r']);
            return offset + byte_of_col(body, pos.column);
        }
        offset += line.len();
    }
    text.len()
}

/// The call whose parentheses contain `offset`: (callee name, active parameter).
fn enclosing_call(text: &str, offset: usize) -> Option<(String, usize)> {
    let before = &text[..offset];
    let window_start = before.len().saturating_sub(2000);
    let window_start = (window_start..=before.len()).find(|&i| before.is_char_boundary(i))?;
    let window = &before[window_start..];
    let mut depth = 0;
    let open = window.char_indices().rev().find(|&(_, c)| {
        match c {
            ')' => depth += 1,
            '(' if depth == 0 => return true,
            '(' => depth -= 1,
            _ => {}
        }
        false
    })?;
    let callee_end = open.0;
    let callee: String = window[..callee_end]
        .chars()
        .rev()
        .take_while(|&c| is_ident(c) || c == '!')
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    if callee.is_empty() {
        return None;
    }
    // Count top-level commas between the parenthesis and the cursor.
    let (mut active, mut nest, mut in_str) = (0, 0, false);
    let mut prev = '\0';
    for c in window[callee_end + 1..].chars() {
        match c {
            '"' if prev != '\\' => in_str = !in_str,
            '(' | '[' | '{' if !in_str => nest += 1,
            ')' | ']' | '}' if !in_str => nest -= 1,
            ',' if !in_str && nest == 0 => active += 1,
            _ => {}
        }
        prev = c;
    }
    Some((callee, active))
}

// ---------------------------------------------------------------------------
// Inlay hints and runnables
// ---------------------------------------------------------------------------

fn guess_type(rhs: &str, lang: Lang) -> &'static str {
    let rhs = rhs.trim();
    match lang {
        Lang::TypeScript => {
            if rhs.starts_with(['"', '\'', '`']) || rhs.contains(" ? `") || rhs.contains("? \"") {
                "string"
            } else if rhs.starts_with(|c: char| c.is_ascii_digit()) {
                "number"
            } else if rhs.starts_with("true") || rhs.starts_with("false") {
                "boolean"
            } else {
                "any"
            }
        }
        _ => {
            if rhs.starts_with('"') {
                "&str"
            } else if rhs.starts_with("format!")
                || rhs.contains(".to_string()")
                || rhs.starts_with("String::")
            {
                "String"
            } else if rhs.contains("Config::load") {
                if rhs.contains("unwrap_or_default") || rhs.contains('?') {
                    "Config"
                } else {
                    "Result<Config>"
                }
            } else if rhs.starts_with("vec!") || rhs.starts_with("Vec::") {
                "Vec<_>"
            } else if rhs.starts_with(|c: char| c.is_ascii_digit()) {
                if rhs.contains('.') { "f64" } else { "i32" }
            } else if rhs.starts_with("true") || rhs.starts_with("false") {
                "bool"
            } else if rhs.starts_with("fs::read_to_string") {
                "Result<String>"
            } else {
                "_"
            }
        }
    }
}

fn inlay_hints_for(path: &Path, text: &str) -> Vec<InlayHint> {
    let lang = lang(path);
    if lang == Lang::Other {
        return Vec::new();
    }
    let mut hints = Vec::new();
    for (ix, line) in text.lines().enumerate() {
        let ix = ix as u32;
        let trimmed = line.trim_start();
        // `let x = …` / `const x = …` without a type.
        let binding = match lang {
            Lang::Rust => trimmed
                .strip_prefix("let ")
                .map(|r| r.strip_prefix("mut ").unwrap_or(r)),
            _ => trimmed
                .strip_prefix("const ")
                .or_else(|| trimmed.strip_prefix("let ")),
        };
        if let Some(rest) = binding
            && let Some(name) = leading_ident(rest)
        {
            let after = rest[name.len()..].trim_start();
            if let Some(rhs) = after.strip_prefix('=') {
                let end = line.len() - rest.len() + name.len();
                hints.push(InlayHint {
                    position: TextPos::new(ix, char_col(line, end)),
                    label: format!(": {}", guess_type(rhs, lang)),
                    kind: InlayHintKind::Type,
                });
            }
        }
        // `for name in &config.names`.
        if lang == Lang::Rust
            && let Some(rest) = trimmed.strip_prefix("for ")
            && let Some(name) = leading_ident(rest)
            && rest[name.len()..].trim_start().starts_with("in &")
        {
            let end = line.len() - rest.len() + name.len();
            hints.push(InlayHint {
                position: TextPos::new(ix, char_col(line, end)),
                label: ": &String".into(),
                kind: InlayHintKind::Type,
            });
        }
        // Parameter names of known calls with plain arguments.
        for (callee, param) in [("greet(", "name:"), ("load(", "path:")] {
            for (b, _) in line.match_indices(callee) {
                let is_decl = line[..b].trim_end().ends_with("fn");
                let word_start = line[..b].chars().next_back().is_none_or(|c| !is_ident(c));
                let arg = &line[b + callee.len()..];
                if is_decl || !word_start || arg.starts_with([')', '{']) {
                    continue;
                }
                hints.push(InlayHint {
                    position: TextPos::new(ix, char_col(line, b + callee.len())),
                    label: param.into(),
                    kind: InlayHintKind::Parameter,
                });
            }
        }
        // `?` on a Result-returning call: the type it unwraps to.
        if lang == Lang::Rust && line.trim_end().ends_with("})?;") {
            hints.push(InlayHint {
                position: TextPos::new(ix, line_len(line.trim_end()) - 1),
                label: "-> Config".into(),
                kind: InlayHintKind::Chaining,
            });
        }
    }
    hints
}

fn runnables_for(path: &Path, text: &str) -> Vec<Runnable> {
    if lang(path) != Lang::Rust {
        return Vec::new();
    }
    let mut out = Vec::new();
    let mut prev = "";
    for (ix, line) in text.lines().enumerate() {
        let trimmed = line.trim_start();
        let attr = prev.trim_start();
        if (trimmed.starts_with("fn main(") || trimmed.starts_with("async fn main("))
            && !attr.starts_with("#[test]")
        {
            out.push(Runnable {
                line: ix as u32,
                kind: RunnableKind::Main,
                label: "main".into(),
            });
        } else if (attr.starts_with("#[test]") || attr.starts_with("#[tokio::test]"))
            && let Some(name) = trimmed
                .strip_prefix("fn ")
                .or_else(|| trimmed.strip_prefix("async fn "))
                .and_then(leading_ident)
        {
            out.push(Runnable {
                line: ix as u32,
                kind: RunnableKind::Test,
                label: name.into(),
            });
        } else if attr.starts_with("#[cfg(test)]")
            && let Some(name) = trimmed.strip_prefix("mod ").and_then(leading_ident)
        {
            out.push(Runnable {
                line: ix as u32,
                kind: RunnableKind::TestModule,
                label: name.into(),
            });
        }
        if !trimmed.is_empty() {
            prev = line;
        }
    }
    out
}

// ---------------------------------------------------------------------------
// The service
// ---------------------------------------------------------------------------

impl LanguageService for MockLanguageService {
    fn diagnostics(&self, path: &Path, text: &str) -> BoxFuture<'static, Result<Vec<Diagnostic>>> {
        let fail = self.should_fail();
        let rel = self.rel(path);
        let mut out = diagnostics_for(&rel, path, text);
        if rel.starts_with("stress/") {
            out.extend(stress_diagnostics(self.config.seed, path, text));
        }
        self.answer(Ok(out), fail, "computing diagnostics")
    }

    fn project_diagnostics(&self) -> BoxFuture<'static, Result<Vec<Diagnostic>>> {
        let fail = self.should_fail();
        let latency = self.config.latency * 3;
        let project = self.project.clone();
        let seed = self.config.seed;
        async move {
            simulate(latency, fail, "collecting problems").await?;
            let mut out = Vec::new();
            for (path, text) in project.demo_files() {
                let rel = project.relative(&path).unwrap_or_default();
                out.extend(diagnostics_for(&rel, &path, &text));
            }
            for rel in project.stress_files().iter().take(600) {
                let path = project.absolute(rel);
                if let Some(text) = project.file_text(&path) {
                    out.extend(stress_diagnostics(seed, &path, &text));
                }
            }
            Ok(out)
        }
        .boxed()
    }

    fn completions(
        &self,
        path: &Path,
        text: &str,
        pos: TextPos,
    ) -> BoxFuture<'static, Result<Vec<CompletionItem>>> {
        let fail = self.should_fail();
        let lang = lang(path);
        let prefix = prefix_at(text, pos);
        let (keywords, std_items): (&[&str], &[StdItem]) = match lang {
            Lang::Rust => (RUST_KEYWORDS, RUST_STD),
            Lang::TypeScript => (TS_KEYWORDS, TS_STD),
            Lang::Other => (&[], &[]),
        };

        let mut symbols = Vec::new();
        flatten(&outline(path, text), &mut symbols);
        let mut items: Vec<CompletionItem> = Vec::new();
        let mut seen = BTreeSet::new();
        for s in &symbols {
            if seen.insert(s.name.clone()) && !s.name.starts_with("impl ") {
                items.push(CompletionItem {
                    label: s.name.clone(),
                    kind: s.kind,
                    detail: Some(
                        format!("{} {}", symbol_keyword(s.kind), s.name)
                            .trim()
                            .to_string(),
                    ),
                    documentation: known_hover(&s.name, lang),
                    insert_text: s.name.clone(),
                });
            }
        }
        // Other identifiers in the file, except the word being typed.
        let typed_once = text.matches(prefix.as_str()).count() <= 1;
        let mut words = BTreeSet::new();
        for line in text.lines() {
            for word in line.split(|c: char| !is_ident(c)) {
                if word.chars().count() >= 3
                    && !word.starts_with(|c: char| c.is_ascii_digit())
                    && !keywords.contains(&word)
                    && !(typed_once && word == prefix)
                {
                    words.insert(word.to_string());
                }
            }
        }
        for word in words {
            if seen.insert(word.clone()) {
                items.push(CompletionItem {
                    label: word.clone(),
                    kind: SymbolKind::Variable,
                    detail: None,
                    documentation: None,
                    insert_text: word,
                });
            }
        }
        for &(label, kind, detail, doc, insert) in std_items {
            if seen.insert(label.to_string()) {
                items.push(CompletionItem {
                    label: label.into(),
                    kind,
                    detail: Some(detail.into()),
                    documentation: Some(doc.into()),
                    insert_text: insert.into(),
                });
            }
        }
        for &kw in keywords {
            if seen.insert(kw.to_string()) {
                items.push(CompletionItem {
                    label: kw.into(),
                    kind: SymbolKind::Keyword,
                    detail: None,
                    documentation: None,
                    insert_text: kw.into(),
                });
            }
        }

        let ranked: Vec<CompletionItem> = if prefix.is_empty() {
            items.truncate(200);
            items
        } else {
            fuzzy::rank(&prefix, items.iter().map(|i| i.label.as_str()), 100)
                .into_iter()
                .map(|(ix, _)| items[ix].clone())
                .collect()
        };
        self.answer(Ok(ranked), fail, "computing completions")
    }

    fn hover(
        &self,
        path: &Path,
        text: &str,
        pos: TextPos,
    ) -> BoxFuture<'static, Result<Option<HoverInfo>>> {
        let fail = self.should_fail();
        let lang = lang(path);
        let hover = word_at(text, pos).and_then(|(word, start, end)| {
            let contents = known_hover(&word, lang).or_else(|| {
                let mut symbols = Vec::new();
                flatten(&outline(path, text), &mut symbols);
                symbols.iter().find(|s| s.name == word).map(|s| {
                    let fence = if lang == Lang::TypeScript {
                        "ts"
                    } else {
                        "rust"
                    };
                    let container = s
                        .container
                        .as_ref()
                        .map(|c| format!("{c}\n"))
                        .unwrap_or_default();
                    format!(
                        "```{fence}\n{container}{} {}\n```",
                        symbol_keyword(s.kind),
                        s.name
                    )
                })
            })?;
            Some(HoverInfo {
                contents,
                range: TextRange::on_line(pos.line, start, end),
            })
        });
        self.answer(Ok(hover), fail, "computing hover")
    }

    fn signature_help(
        &self,
        path: &Path,
        text: &str,
        pos: TextPos,
    ) -> BoxFuture<'static, Result<Option<SignatureHelp>>> {
        let fail = self.should_fail();
        let lang = lang(path);
        let offset = offset_of(text, pos);
        let help = enclosing_call(text, offset).and_then(|(callee, active)| {
            let (label, params, doc) = known_signature(&callee, lang)?;
            let parameters: Vec<_> = params
                .iter()
                .filter_map(|p| label.find(p).map(|b| b..b + p.len()))
                .collect();
            Some(SignatureHelp {
                label: label.into(),
                active_parameter: active.min(parameters.len().saturating_sub(1)),
                parameters,
                documentation: Some(doc.into()),
            })
        });
        self.answer(Ok(help), fail, "computing signature help")
    }

    fn code_actions(
        &self,
        path: &Path,
        text: &str,
        range: TextRange,
    ) -> BoxFuture<'static, Result<Vec<QuickFix>>> {
        let fail = self.should_fail();
        let lang = lang(path);
        let rel = self.rel(path);
        let mut actions: Vec<QuickFix> = diagnostics_for(&rel, path, text)
            .iter()
            .filter(|d| {
                d.range.start.line <= range.end.line && range.start.line <= d.range.end.line
            })
            .filter_map(|d| fix_for(d, text))
            .collect();

        let intention = |title: &str, kind, edits| QuickFix {
            title: title.into(),
            kind,
            edits,
            preferred: false,
        };
        if lang != Lang::Other {
            // "Add explicit type" for a binding without one on this line.
            if let Some(hint) = inlay_hints_for(path, text).into_iter().find(|h| {
                h.kind == InlayHintKind::Type
                    && h.position.line == range.start.line
                    && !h.label.ends_with('_')
                    && !h.label.ends_with("any")
            }) {
                actions.push(intention(
                    &format!(
                        "Add explicit type `{}`",
                        hint.label.trim_start_matches(": ")
                    ),
                    QuickFixKind::Intention,
                    vec![TextEdit {
                        range: TextRange::new(hint.position, hint.position),
                        new_text: hint.label.clone(),
                    }],
                ));
            }
            if range.start != range.end {
                actions.push(intention(
                    "Extract function…",
                    QuickFixKind::Refactor,
                    Vec::new(),
                ));
                actions.push(intention(
                    "Introduce variable…",
                    QuickFixKind::Refactor,
                    Vec::new(),
                ));
            } else if word_at(text, range.start).is_some() {
                actions.push(intention("Rename…", QuickFixKind::Refactor, Vec::new()));
                actions.push(intention("Inline", QuickFixKind::Refactor, Vec::new()));
            }
        }
        if rel == "README.md" {
            actions.push(intention(
                "Save 'FerroIDE' to the project dictionary",
                QuickFixKind::Intention,
                Vec::new(),
            ));
        }
        self.answer(Ok(actions), fail, "computing code actions")
    }

    fn inlay_hints(&self, path: &Path, text: &str) -> BoxFuture<'static, Result<Vec<InlayHint>>> {
        let fail = self.should_fail();
        self.answer(
            Ok(inlay_hints_for(path, text)),
            fail,
            "computing inlay hints",
        )
    }

    fn document_symbols(&self, path: &Path, text: &str) -> BoxFuture<'static, Result<Vec<Symbol>>> {
        let fail = self.should_fail();
        self.answer(Ok(outline(path, text)), fail, "computing the outline")
    }

    fn workspace_symbols(&self, query: &str) -> BoxFuture<'static, Result<Vec<Symbol>>> {
        let fail = self.should_fail();
        let latency = self.config.latency;
        let project = self.project.clone();
        let query = query.to_string();
        async move {
            simulate(latency, fail, "searching symbols").await?;
            let mut symbols = Vec::new();
            for (path, text) in project.demo_files() {
                flatten(&outline(&path, &text), &mut symbols);
            }
            symbols.retain(|s| s.kind != SymbolKind::Field || !query.is_empty());
            let ranked = fuzzy::rank(&query, symbols.iter().map(|s| s.name.as_str()), 100);
            Ok(ranked
                .into_iter()
                .map(|(ix, _)| symbols[ix].clone())
                .collect())
        }
        .boxed()
    }

    fn runnables(&self, path: &Path, text: &str) -> Vec<Runnable> {
        runnables_for(path, text)
    }

    fn rename(
        &self,
        path: &Path,
        text: &str,
        pos: TextPos,
        new_name: &str,
    ) -> BoxFuture<'static, Result<WorkspaceEdit>> {
        let fail = self.should_fail();
        let result = rename_now(&self.project, path, text, pos, new_name);
        self.answer(result, fail, "renaming")
    }
}

fn rename_now(
    project: &MockProjectService,
    path: &Path,
    text: &str,
    pos: TextPos,
    new_name: &str,
) -> Result<WorkspaceEdit> {
    let (old, ..) = word_at(text, pos).ok_or_else(|| anyhow!("no symbol at the cursor"))?;
    let lang = lang(path);
    if RUST_KEYWORDS.contains(&old.as_str()) || TS_KEYWORDS.contains(&old.as_str()) {
        bail!("`{old}` is a keyword and cannot be renamed");
    }
    let valid = leading_ident(new_name).is_some_and(|n| n.len() == new_name.len());
    if !valid {
        bail!("`{new_name}` is not a valid identifier");
    }
    if new_name == old {
        return Ok(WorkspaceEdit::default());
    }

    // The requesting file first (with its unsaved text), then the rest.
    let mut files: Vec<(PathBuf, String)> = vec![(path.to_path_buf(), text.to_string())];
    let this = super::norm(path);
    files.extend(
        project
            .demo_files()
            .into_iter()
            .filter(|(p, _)| super::norm(p) != this && self::lang(p) == lang),
    );

    let mut edit = WorkspaceEdit::default();
    for (file, content) in files {
        let mut edits = Vec::new();
        for (ix, line) in content.lines().enumerate() {
            let hits = word_occurrences(line, &old);
            if hits.is_empty() {
                continue;
            }
            let line_after = {
                let mut s = String::new();
                let mut last = 0;
                for &b in &hits {
                    s.push_str(&line[last..b]);
                    s.push_str(new_name);
                    last = b + old.len();
                }
                s.push_str(&line[last..]);
                s
            };
            for &b in &hits {
                let col = char_col(line, b);
                edits.push(PreviewedEdit {
                    edit: TextEdit {
                        range: TextRange::on_line(ix as u32, col, col + old.chars().count() as u32),
                        new_text: new_name.into(),
                    },
                    line_before: line.to_string(),
                    line_after: line_after.clone(),
                });
            }
        }
        if !edits.is_empty() {
            edit.files.push(FileEdit { path: file, edits });
        }
    }
    Ok(edit)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn block<T>(f: BoxFuture<'static, T>) -> T {
        smol::block_on(f)
    }

    fn service(stress_files: usize) -> (MockLanguageService, Arc<MockProjectService>) {
        let config = MockConfig {
            stress_files,
            ..MockConfig::instant()
        };
        let project = Arc::new(MockProjectService::demo(config.clone()));
        (MockLanguageService::new(config, project.clone()), project)
    }

    fn file(project: &MockProjectService, rel: &str) -> (PathBuf, String) {
        let path = project.absolute(rel);
        let text = project.file_text(&path).unwrap();
        (path, text)
    }

    fn slice(text: &str, range: TextRange) -> String {
        let line = text.lines().nth(range.start.line as usize).unwrap();
        line.chars()
            .skip(range.start.column as usize)
            .take((range.end.column - range.start.column) as usize)
            .collect()
    }

    #[test]
    fn diagnostics_sit_on_real_identifiers() {
        let (lsp, project) = service(0);
        let (path, text) = file(&project, "src/config.rs");
        let diags = block(lsp.diagnostics(&path, &text)).unwrap();
        let dead = diags
            .iter()
            .find(|d| d.code.as_deref() == Some("dead_code"))
            .unwrap();
        assert_eq!(slice(&text, dead.range), "retries");
        let infer = diags
            .iter()
            .find(|d| d.severity == Severity::Error)
            .unwrap();
        assert_eq!(slice(&text, infer.range), "from_str");
        assert!(diags.iter().any(|d| d.source == "todo"));
        assert_eq!(
            diags,
            block(lsp.diagnostics(&path, &text)).unwrap(),
            "deterministic"
        );

        // Edits move them; unknown text has none.
        let unrelated = block(lsp.diagnostics(&path, "fn x() {}\n")).unwrap();
        assert!(unrelated.is_empty());
    }

    #[test]
    fn project_diagnostics_include_the_stress_tree() {
        let (lsp, _) = service(5_000);
        let all = block(lsp.project_diagnostics()).unwrap();
        assert!(all.iter().any(|d| d.path.ends_with("config.rs")));
        let stress = all
            .iter()
            .filter(|d| d.path.to_string_lossy().contains("stress"))
            .count();
        assert!(stress >= 20, "{stress}");
    }

    #[test]
    fn quick_fixes_have_real_edits() {
        let (lsp, project) = service(0);
        let (path, text) = file(&project, "src/main.rs");
        let range = find_str(&text, ".unwrap_or_default()").unwrap();
        let actions = block(lsp.code_actions(&path, &text, range)).unwrap();
        let fix = actions.iter().find(|a| a.preferred).unwrap();
        assert_eq!(fix.kind, QuickFixKind::QuickFix);
        assert_eq!(fix.edits[0].new_text, "?");
        assert_eq!(slice(&text, fix.edits[0].range), ".unwrap_or_default()");
        assert!(
            actions.iter().any(|a| a.kind == QuickFixKind::Intention),
            "{actions:?}"
        );
    }

    #[test]
    fn completions_filter_by_prefix() {
        let (lsp, project) = service(0);
        let (path, text) = file(&project, "src/lib.rs");
        // Inside `pub fn greet`, after typing `gr` on a new line.
        let text = format!("{text}\nfn x() {{ gr }}\n");
        let line = text.lines().count() as u32 - 1;
        let items = block(lsp.completions(&path, &text, TextPos::new(line, 11))).unwrap();
        assert_eq!(items[0].label, "greet");
        assert_eq!(items[0].kind, SymbolKind::Function);
        let all = block(lsp.completions(&path, &text, TextPos::new(0, 0))).unwrap();
        assert!(all.iter().any(|i| i.label == "println!"));
        assert!(all.iter().any(|i| i.kind == SymbolKind::Keyword));
    }

    #[test]
    fn hover_and_signature_help_know_the_demo_api() {
        let (lsp, project) = service(0);
        let (path, text) = file(&project, "src/main.rs");
        let greet = find_word(&text, "greet").unwrap();
        let hover = block(lsp.hover(&path, &text, greet.start))
            .unwrap()
            .unwrap();
        assert!(hover.contents.starts_with("```rust\npub fn greet"));
        assert_eq!(hover.range, greet);
        assert!(
            block(lsp.hover(&path, &text, TextPos::new(1, 0)))
                .unwrap()
                .is_none()
        );

        // Inside `greet(name)` in `println!("{}", greet(name));`.
        let call = find_str(&text, "greet(name)").unwrap();
        let inside = TextPos::new(call.start.line, call.start.column + 7);
        let help = block(lsp.signature_help(&path, &text, inside))
            .unwrap()
            .unwrap();
        assert_eq!(help.label, "fn greet(name: &str) -> String");
        assert_eq!(&help.label[help.parameters[0].clone()], "name: &str");
        // After the comma of `println!("{}", …`: second parameter.
        let args = find_str(&text, "println!(\"{}\", ").unwrap();
        let after_comma = TextPos::new(args.start.line, args.end.column);
        let help = block(lsp.signature_help(&path, &text, after_comma))
            .unwrap()
            .unwrap();
        assert_eq!(help.active_parameter, 1);
    }

    #[test]
    fn inlay_hints_guess_types_and_parameters() {
        let (lsp, project) = service(0);
        let (path, text) = file(&project, "src/main.rs");
        let hints = block(lsp.inlay_hints(&path, &text)).unwrap();
        let config = hints
            .iter()
            .find(|h| h.kind == InlayHintKind::Type)
            .unwrap();
        assert_eq!(config.label, ": Config");
        let config_line = text.lines().nth(config.position.line as usize).unwrap();
        let before: String = config_line
            .chars()
            .take(config.position.column as usize)
            .collect();
        assert!(before.ends_with("let config"), "{before}");
        assert!(
            hints
                .iter()
                .any(|h| h.kind == InlayHintKind::Parameter && h.label == "name:")
        );
        assert!(hints.iter().any(|h| h.label == ": &String"));
    }

    #[test]
    fn the_outline_nests_items() {
        let (lsp, project) = service(0);
        let (path, text) = file(&project, "src/config.rs");
        let symbols = block(lsp.document_symbols(&path, &text)).unwrap();
        let names: Vec<_> = symbols.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, ["Config", "default_retries", "impl Config"]);
        let fields: Vec<_> = symbols[0]
            .children
            .iter()
            .map(|s| (s.kind, s.name.as_str()))
            .collect();
        assert_eq!(
            fields,
            [(SymbolKind::Field, "names"), (SymbolKind::Field, "retries")]
        );
        let load = &symbols[2].children[0];
        assert_eq!(
            (load.kind, load.name.as_str()),
            (SymbolKind::Method, "load")
        );
        assert_eq!(load.container.as_deref(), Some("impl Config"));
        assert!(load.range.end.line > load.range.start.line);

        let (path, text) = file(&project, "src/lib.rs");
        let symbols = block(lsp.document_symbols(&path, &text)).unwrap();
        let tests = symbols.iter().find(|s| s.name == "tests").unwrap();
        assert!(tests.children.iter().all(|s| s.kind == SymbolKind::Test));
        assert_eq!(tests.children.len(), 2);

        let (path, text) = file(&project, "web/src/app.ts");
        let symbols = block(lsp.document_symbols(&path, &text)).unwrap();
        let names: Vec<_> = symbols.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, ["Greeting", "greet"]);
        assert_eq!(symbols[0].children.len(), 2);
    }

    #[test]
    fn workspace_symbols_and_runnables() {
        let (lsp, project) = service(0);
        let found = block(lsp.workspace_symbols("defret")).unwrap();
        assert_eq!(found[0].name, "default_retries");
        let (path, text) = file(&project, "src/lib.rs");
        let runnables = lsp.runnables(&path, &text);
        let kinds: Vec<_> = runnables
            .iter()
            .map(|r| (r.kind, r.label.as_str()))
            .collect();
        assert_eq!(
            kinds,
            [
                (RunnableKind::TestModule, "tests"),
                (RunnableKind::Test, "greets_by_name"),
                (RunnableKind::Test, "greets_strangers"),
            ]
        );
        let (path, text) = file(&project, "src/main.rs");
        assert_eq!(lsp.runnables(&path, &text)[0].kind, RunnableKind::Main);
    }

    #[test]
    fn rename_covers_every_file_using_the_symbol() {
        let (lsp, project) = service(0);
        let (path, text) = file(&project, "src/lib.rs");
        let at = find_word(&text, "greet").unwrap().start;
        let edit = block(lsp.rename(&path, &text, at, "salute")).unwrap();
        let files: Vec<String> = edit
            .files
            .iter()
            .map(|f| project.relative(&f.path).unwrap())
            .collect();
        assert_eq!(files[0], "src/lib.rs");
        assert!(files.contains(&"src/main.rs".to_string()));
        assert!(!files.iter().any(|f| f.ends_with(".ts")), "only Rust files");
        let main = &edit.files[files.iter().position(|f| f == "src/main.rs").unwrap()];
        assert!(main.edits.iter().all(|e| e.line_after.contains("salute")));
        assert!(edit.edit_count() >= 4);

        assert!(block(lsp.rename(&path, &text, at, "not valid")).is_err());
        assert!(
            block(lsp.rename(&path, &text, TextPos::new(2, 0), "x")).is_err(),
            "keyword `mod`"
        );
    }
}
