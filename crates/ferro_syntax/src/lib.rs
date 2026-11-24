//! FerroIDE's syntax layer on tree-sitter (plan §8, Stage 5), independent of
//! any GUI.
//!
//! - [`SyntaxLanguage`]: a grammar with its compiled queries — highlights,
//!   injections, tags / outline, folds, indents, runnables, brackets.
//! - [`SyntaxTree`]: one document's parse. [`SyntaxTree::reparse`] finds the
//!   edit by comparing the old and new rope and reparses incrementally;
//!   injected languages (JavaScript and CSS in HTML, code blocks in
//!   Markdown, SQL in strings…) are parsed on first use.
//! - Features over the tree: [`SyntaxTree::highlights`] (visible range),
//!   [`SyntaxTree::outline`] and [`SyntaxTree::tags`] (file structure and the
//!   fallback symbol index), [`SyntaxTree::folds`],
//!   [`SyntaxTree::matching_brackets`], [`SyntaxTree::indent_level`],
//!   [`SyntaxTree::expand_selection`], [`SyntaxTree::runnables`],
//!   [`SyntaxTree::syntax_errors`] and [`SyntaxTree::context_at`].
//!
//! Offsets are UTF-8 byte offsets into text whose lines end with `\n`, as in
//! `ferro_text`.

mod highlight;
mod indent;
mod language;
mod structure;
mod symbols;
mod tree;

pub use highlight::HighlightSpan;
pub use indent::indent_width;
pub use language::{
    LanguageConfig, LanguageError, LanguageResolver, NoInjections, QueryKind, SyntaxLanguage,
};
pub use structure::{BracketPair, Fold, SyntaxContext, SyntaxError};
pub use symbols::{OutlineItem, RunnableMatch, RunnableTag, Tag};
pub use tree::{IncompatibleGrammar, InjectionLayer, SyntaxTree};

/// Re-exported so callers build [`LanguageConfig`]s with the same version.
pub use tree_sitter;
