//! Language Packs in the window (Stage 5).
//!
//! The packs of `ferro_languages` — built in, plus the user's packs from
//! `<config>/languages/` — are registered with GPUI Kit's editor: each
//! pack's grammar with its highlight, injection and locals queries goes to
//! the highlighter, its brackets, auto-closing pairs and indentation
//! patterns become the editor's editing rules. Grammars are linked in, so
//! registering costs nothing; queries are compiled when the first file of a
//! language opens.

use std::{
    collections::HashMap,
    path::Path,
    sync::{Arc, Mutex, OnceLock},
};

use ferro_languages::{LanguageManifest, LanguageRegistry};
use ferro_syntax::QueryKind;
use ferro_text::comment::CommentTokens;
use gpui_kit::{
    App, SharedString,
    component::{
        highlighter::{GrammarConfig, LanguageRegistry as KitRegistry},
        input::{
            AutoClosingPair, BracketPair, IndentationRules, SyntaxContext,
            language_config::LanguageConfig as EditingConfig, set_language_config,
        },
    },
};

/// The language id GPUI Kit's editor uses for files without a pack.
pub const PLAIN_TEXT: &str = "text";

static REGISTRY: OnceLock<Arc<LanguageRegistry>> = OnceLock::new();

/// The packs of this process: set by [`init`]; the built-in packs before
/// that (tests, the storybook).
pub fn registry() -> Arc<LanguageRegistry> {
    REGISTRY
        .get()
        .cloned()
        .unwrap_or_else(LanguageRegistry::shared_builtin)
}

/// Load the packs (built-in, then `user_dir` replacing packs of the same id)
/// and register them with the editor. Call once at startup.
pub fn init(user_dir: Option<&Path>, cx: &mut App) {
    let registry = REGISTRY.get_or_init(|| {
        let mut registry = LanguageRegistry::builtin();
        if let Some(dir) = user_dir {
            for err in registry.load_dir(dir) {
                tracing::warn!("language pack: {err}");
            }
        }
        Arc::new(registry)
    });
    register_with_editor(registry, cx);
}

/// Register every pack with GPUI Kit's highlighter and editing rules.
pub fn register_with_editor(registry: &LanguageRegistry, cx: &mut App) {
    let started = std::time::Instant::now();
    let names: Vec<SharedString> = registry
        .packs()
        .flat_map(|p| p.manifest().names().collect::<Vec<_>>())
        .map(SharedString::from)
        .collect();
    for pack in registry.packs() {
        let manifest = pack.manifest();
        let editing = editing_rules(manifest);
        match pack.grammar() {
            Ok(grammar) => {
                let config = GrammarConfig::new(
                    pack.id().to_string(),
                    grammar,
                    names.clone(),
                    &pack.query_source(QueryKind::Highlights),
                    &pack.query_source(QueryKind::Injections),
                    &pack.query_source(QueryKind::Locals),
                );
                for name in manifest.names() {
                    KitRegistry::singleton().register(&name, &config);
                }
            }
            Err(err) => tracing::warn!("language {}: {err}", pack.id()),
        }
        for name in manifest.names() {
            set_language_config(name, editing.clone(), cx);
        }
    }
    tracing::debug!(
        "{} language packs registered in {:.1} ms",
        registry.packs().count(),
        started.elapsed().as_secs_f64() * 1000.0
    );
}

/// A pack's brackets, auto-closing pairs and indentation patterns as GPUI
/// Kit's editing rules. Pairs never close inside strings and comments.
fn editing_rules(manifest: &LanguageManifest) -> EditingConfig {
    let brackets = manifest
        .brackets
        .iter()
        .map(|[open, close]| BracketPair::new(open.clone(), close.clone()));
    let pairs = manifest.auto_pairs().into_iter().map(|[open, close]| {
        AutoClosingPair::new(open, close).not_in([SyntaxContext::String, SyntaxContext::Comment])
    });
    let mut config = EditingConfig::default()
        .brackets(brackets)
        .auto_closing_pairs(pairs);
    if let Some(before) = &manifest.auto_close_before {
        config = config.auto_close_before(before.clone());
    }
    let compile = |pattern: &Option<String>| {
        pattern.as_deref().and_then(|p| match regex::Regex::new(p) {
            Ok(regex) => Some(Arc::new(regex)),
            Err(err) => {
                tracing::warn!("language {}: bad indentation pattern: {err}", manifest.id);
                None
            }
        })
    };
    let increase = compile(&manifest.increase_indent_pattern);
    let decrease = compile(&manifest.decrease_indent_pattern);
    if increase.is_some() || decrease.is_some() {
        let mut rules = IndentationRules::default();
        rules.increase_indent_pattern = increase;
        rules.decrease_indent_pattern = decrease;
        config = config.indentation_rules(rules);
    }
    config
}

/// The language of a file from its path alone (diff views, previews).
pub fn language_for(path: &Path) -> SharedString {
    registry()
        .detect_path(path)
        .map_or_else(|| PLAIN_TEXT.into(), |p| p.id().to_string().into())
}

/// The language of a file from its path and text: a modeline, the name,
/// the extension, a shebang or the first line decide.
pub fn detect(path: &Path, text: &str) -> SharedString {
    let lines: Vec<&str> = text.lines().take(5).collect();
    let mut probe = lines.join("\n");
    let tail: Vec<&str> = text.lines().rev().take(5).collect();
    for line in tail.into_iter().rev() {
        probe.push('\n');
        probe.push_str(line);
    }
    registry()
        .detect(path, Some(&probe))
        .map_or_else(|| PLAIN_TEXT.into(), |p| p.id().to_string().into())
}

/// The display name of a language id, e.g. `C#` for `csharp`.
pub fn display_name(id: &str) -> SharedString {
    match registry().get(id) {
        Some(pack) => pack.name().to_string().into(),
        None => ferro_ui::tr("language-plain-text"),
    }
}

/// Comment delimiters of a language, from its pack.
pub fn comment_tokens(id: &str) -> CommentTokens {
    static CACHE: OnceLock<Mutex<HashMap<String, CommentTokens>>> = OnceLock::new();
    let cache = CACHE.get_or_init(Default::default);
    let mut cache = cache.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(tokens) = cache.get(id) {
        return *tokens;
    }
    let tokens = match registry().get(id) {
        // `CommentTokens` holds `'static` strings; a pack's few tokens are
        // kept for the life of the process.
        Some(pack) => {
            let manifest = pack.manifest();
            let leak = |s: &String| -> &'static str { Box::leak(s.clone().into_boxed_str()) };
            CommentTokens {
                line: manifest.line_comment.as_ref().map(leak),
                block: manifest
                    .block_comment
                    .as_ref()
                    .map(|[open, close]| (leak(open), leak(close))),
            }
        }
        None => CommentTokens::for_language(id),
    };
    cache.insert(id.to_string(), tokens);
    tokens
}
