//! Grammars linked into FerroIDE, with the queries they ship.
//!
//! A grammar costs nothing until a file of its language is opened: the
//! table holds function pointers, and a pack's [`ferro_syntax::SyntaxLanguage`]
//! (grammar + compiled queries) is built on first use
//! ([`crate::LanguagePack::syntax`]).

use tree_sitter::Language;

/// A grammar linked into the binary.
pub struct BuiltinGrammar {
    pub name: &'static str,
    pub language: fn() -> Language,
    /// Queries shipped with the grammar crate, by file name. A pack's own
    /// file of the same name replaces one; several entries of the same name
    /// are joined in order (TypeScript extends JavaScript's highlights).
    pub queries: &'static [(&'static str, &'static str)],
}

macro_rules! grammar {
    ($name:literal, $language:expr, [$(($file:literal, $query:expr)),* $(,)?]) => {
        BuiltinGrammar {
            name: $name,
            language: || $language.into(),
            queries: &[$(($file, $query)),*],
        }
    };
}

/// Every built-in grammar.
pub static BUILTIN_GRAMMARS: &[BuiltinGrammar] = &[
    grammar!(
        "bash",
        tree_sitter_bash::LANGUAGE,
        [("highlights.scm", tree_sitter_bash::HIGHLIGHT_QUERY)]
    ),
    grammar!(
        "c",
        tree_sitter_c::LANGUAGE,
        [
            ("highlights.scm", tree_sitter_c::HIGHLIGHT_QUERY),
            ("tags.scm", tree_sitter_c::TAGS_QUERY),
        ]
    ),
    grammar!(
        "cpp",
        tree_sitter_cpp::LANGUAGE,
        [
            ("highlights.scm", tree_sitter_cpp::HIGHLIGHT_QUERY),
            ("highlights.scm", tree_sitter_c::HIGHLIGHT_QUERY),
            ("tags.scm", tree_sitter_cpp::TAGS_QUERY),
        ]
    ),
    grammar!(
        "c_sharp",
        tree_sitter_c_sharp::LANGUAGE,
        [
            ("highlights.scm", tree_sitter_c_sharp::HIGHLIGHTS_QUERY),
            ("tags.scm", tree_sitter_c_sharp::TAGS_QUERY),
        ]
    ),
    grammar!(
        "css",
        tree_sitter_css::LANGUAGE,
        [("highlights.scm", tree_sitter_css::HIGHLIGHTS_QUERY)]
    ),
    grammar!(
        "go",
        tree_sitter_go::LANGUAGE,
        [
            ("highlights.scm", tree_sitter_go::HIGHLIGHTS_QUERY),
            ("tags.scm", tree_sitter_go::TAGS_QUERY),
        ]
    ),
    grammar!(
        "html",
        tree_sitter_html::LANGUAGE,
        [
            ("highlights.scm", tree_sitter_html::HIGHLIGHTS_QUERY),
            ("injections.scm", tree_sitter_html::INJECTIONS_QUERY),
        ]
    ),
    grammar!(
        "java",
        tree_sitter_java::LANGUAGE,
        [
            ("highlights.scm", tree_sitter_java::HIGHLIGHTS_QUERY),
            ("tags.scm", tree_sitter_java::TAGS_QUERY),
        ]
    ),
    grammar!(
        "javascript",
        tree_sitter_javascript::LANGUAGE,
        [
            (
                "highlights.scm",
                tree_sitter_javascript::JSX_HIGHLIGHT_QUERY
            ),
            ("highlights.scm", tree_sitter_javascript::HIGHLIGHT_QUERY),
            ("injections.scm", tree_sitter_javascript::INJECTIONS_QUERY),
            ("locals.scm", tree_sitter_javascript::LOCALS_QUERY),
            ("tags.scm", tree_sitter_javascript::TAGS_QUERY),
        ]
    ),
    grammar!(
        "json",
        tree_sitter_json::LANGUAGE,
        [("highlights.scm", tree_sitter_json::HIGHLIGHTS_QUERY)]
    ),
    grammar!(
        "lua",
        tree_sitter_lua::LANGUAGE,
        [
            ("highlights.scm", tree_sitter_lua::HIGHLIGHTS_QUERY),
            ("injections.scm", tree_sitter_lua::INJECTIONS_QUERY),
            ("locals.scm", tree_sitter_lua::LOCALS_QUERY),
            ("tags.scm", tree_sitter_lua::TAGS_QUERY),
        ]
    ),
    grammar!(
        "markdown",
        tree_sitter_md::LANGUAGE,
        [
            ("highlights.scm", tree_sitter_md::HIGHLIGHT_QUERY_BLOCK),
            ("injections.scm", tree_sitter_md::INJECTION_QUERY_BLOCK),
        ]
    ),
    grammar!(
        "markdown_inline",
        tree_sitter_md::INLINE_LANGUAGE,
        [
            ("highlights.scm", tree_sitter_md::HIGHLIGHT_QUERY_INLINE),
            ("injections.scm", tree_sitter_md::INJECTION_QUERY_INLINE),
        ]
    ),
    grammar!(
        "python",
        tree_sitter_python::LANGUAGE,
        [
            ("highlights.scm", tree_sitter_python::HIGHLIGHTS_QUERY),
            ("tags.scm", tree_sitter_python::TAGS_QUERY),
        ]
    ),
    grammar!(
        "ruby",
        tree_sitter_ruby::LANGUAGE,
        [
            ("highlights.scm", tree_sitter_ruby::HIGHLIGHTS_QUERY),
            ("locals.scm", tree_sitter_ruby::LOCALS_QUERY),
            ("tags.scm", tree_sitter_ruby::TAGS_QUERY),
        ]
    ),
    grammar!(
        "rust",
        tree_sitter_rust::LANGUAGE,
        [
            ("highlights.scm", tree_sitter_rust::HIGHLIGHTS_QUERY),
            ("injections.scm", tree_sitter_rust::INJECTIONS_QUERY),
            ("tags.scm", tree_sitter_rust::TAGS_QUERY),
        ]
    ),
    grammar!(
        "sql",
        tree_sitter_sequel::LANGUAGE,
        [("highlights.scm", tree_sitter_sequel::HIGHLIGHTS_QUERY)]
    ),
    grammar!(
        "toml",
        tree_sitter_toml_ng::LANGUAGE,
        [("highlights.scm", tree_sitter_toml_ng::HIGHLIGHTS_QUERY)]
    ),
    grammar!(
        "tsx",
        tree_sitter_typescript::LANGUAGE_TSX,
        [
            ("highlights.scm", tree_sitter_typescript::HIGHLIGHTS_QUERY),
            (
                "highlights.scm",
                tree_sitter_javascript::JSX_HIGHLIGHT_QUERY
            ),
            ("highlights.scm", tree_sitter_javascript::HIGHLIGHT_QUERY),
            ("injections.scm", tree_sitter_javascript::INJECTIONS_QUERY),
            ("locals.scm", tree_sitter_typescript::LOCALS_QUERY),
            ("tags.scm", tree_sitter_typescript::TAGS_QUERY),
            ("tags.scm", tree_sitter_javascript::TAGS_QUERY),
        ]
    ),
    grammar!(
        "typescript",
        tree_sitter_typescript::LANGUAGE_TYPESCRIPT,
        [
            ("highlights.scm", tree_sitter_typescript::HIGHLIGHTS_QUERY),
            ("highlights.scm", tree_sitter_javascript::HIGHLIGHT_QUERY),
            ("injections.scm", tree_sitter_javascript::INJECTIONS_QUERY),
            ("locals.scm", tree_sitter_typescript::LOCALS_QUERY),
            ("tags.scm", tree_sitter_typescript::TAGS_QUERY),
            ("tags.scm", tree_sitter_javascript::TAGS_QUERY),
        ]
    ),
    grammar!(
        "yaml",
        tree_sitter_yaml::LANGUAGE,
        [("highlights.scm", tree_sitter_yaml::HIGHLIGHTS_QUERY)]
    ),
];

/// The built-in grammar called `name`.
pub fn builtin_grammar(name: &str) -> Option<&'static BuiltinGrammar> {
    BUILTIN_GRAMMARS.iter().find(|g| g.name == name)
}
