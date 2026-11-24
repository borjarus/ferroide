//! A language as the syntax layer sees it: a grammar and its compiled
//! queries.

use std::{fmt, sync::Arc};

use tree_sitter::{Language, Query, QueryError};

/// The query files a language may provide. Each is optional; a missing one
/// turns the matching feature off or falls back to a generic heuristic.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum QueryKind {
    /// `highlights.scm`: capture names are highlight names (`keyword`,
    /// `function.method`, …).
    Highlights,
    /// `injections.scm`: `@injection.content` with `@injection.language` or
    /// `#set! injection.language "…"`.
    Injections,
    /// `locals.scm`: scopes, definitions and references.
    Locals,
    /// `tags.scm`: `@definition.<kind>` / `@reference.<kind>` with `@name`;
    /// the symbol index and the outline.
    Tags,
    /// `outline.scm`: like `tags.scm`, preferred for the outline when present
    /// (languages whose grammar ships no tags query, e.g. TOML or Markdown).
    Outline,
    /// `folds.scm`: `@fold`.
    Folds,
    /// `indents.scm`: `@indent` (children are indented one level),
    /// `@outdent` (a line starting with this node goes back one level).
    Indents,
    /// `runnables.scm`: `@run` (the line the ▶ icon goes on), `@name`, and
    /// `#set! tag main|test|test-module`.
    Runnables,
    /// `brackets.scm`: `@open` / `@close` pairs, for languages whose brackets
    /// are not plain tokens.
    Brackets,
}

impl QueryKind {
    pub const ALL: [QueryKind; 9] = [
        QueryKind::Highlights,
        QueryKind::Injections,
        QueryKind::Locals,
        QueryKind::Tags,
        QueryKind::Outline,
        QueryKind::Folds,
        QueryKind::Indents,
        QueryKind::Runnables,
        QueryKind::Brackets,
    ];

    /// The file name in a Language Pack, e.g. `highlights.scm`.
    pub fn file_name(self) -> &'static str {
        match self {
            QueryKind::Highlights => "highlights.scm",
            QueryKind::Injections => "injections.scm",
            QueryKind::Locals => "locals.scm",
            QueryKind::Tags => "tags.scm",
            QueryKind::Outline => "outline.scm",
            QueryKind::Folds => "folds.scm",
            QueryKind::Indents => "indents.scm",
            QueryKind::Runnables => "runnables.scm",
            QueryKind::Brackets => "brackets.scm",
        }
    }

    pub fn from_file_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|kind| kind.file_name() == name)
    }
}

/// Everything needed to build a [`SyntaxLanguage`].
#[derive(Clone)]
pub struct LanguageConfig {
    /// The language id (`rust`, `python`, …); injections name languages by it.
    pub id: String,
    pub grammar: Language,
    /// Query sources by kind. Empty sources are ignored.
    pub queries: Vec<(QueryKind, String)>,
    /// Bracket pairs, e.g. `("(", ")")`. Used for matching and indentation.
    pub brackets: Vec<(String, String)>,
    /// One indentation level, e.g. four spaces or a tab.
    pub indent_unit: String,
}

impl fmt::Debug for LanguageConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LanguageConfig")
            .field("id", &self.id)
            .field(
                "queries",
                &self.queries.iter().map(|(k, _)| k).collect::<Vec<_>>(),
            )
            .field("brackets", &self.brackets)
            .finish_non_exhaustive()
    }
}

/// A query of a language that failed to compile.
#[derive(Debug, thiserror::Error)]
#[error("{language}/{file}: {error}")]
pub struct LanguageError {
    pub language: String,
    pub file: &'static str,
    #[source]
    pub error: QueryError,
}

/// A grammar with its compiled queries. Cheap to share: wrap it in an
/// [`Arc`].
pub struct SyntaxLanguage {
    pub(crate) id: String,
    pub(crate) grammar: Language,
    pub(crate) highlights: Option<Query>,
    pub(crate) injections: Option<Query>,
    pub(crate) tags: Option<Query>,
    pub(crate) outline: Option<Query>,
    pub(crate) folds: Option<Query>,
    pub(crate) indents: Option<Query>,
    pub(crate) runnables: Option<Query>,
    pub(crate) brackets_query: Option<Query>,
    pub(crate) brackets: Vec<(String, String)>,
    pub(crate) indent_unit: String,
    /// The original sources, for callers that compile them again (GPUI
    /// Kit's highlighter).
    sources: Vec<(QueryKind, String)>,
}

impl fmt::Debug for SyntaxLanguage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SyntaxLanguage")
            .field("id", &self.id)
            .finish_non_exhaustive()
    }
}

impl SyntaxLanguage {
    /// Compile every query of `config`.
    pub fn new(config: LanguageConfig) -> Result<Self, LanguageError> {
        let compile = |kind: QueryKind| -> Result<Option<Query>, LanguageError> {
            let source: String = config
                .queries
                .iter()
                .filter(|(k, _)| *k == kind)
                .map(|(_, s)| s.as_str())
                .collect::<Vec<_>>()
                .join("\n");
            if source.trim().is_empty() {
                return Ok(None);
            }
            Query::new(&config.grammar, &source)
                .map(Some)
                .map_err(|error| LanguageError {
                    language: config.id.clone(),
                    file: kind.file_name(),
                    error,
                })
        };
        Ok(Self {
            tags: compile(QueryKind::Tags)?,
            outline: compile(QueryKind::Outline)?,
            highlights: compile(QueryKind::Highlights)?,
            injections: compile(QueryKind::Injections)?,
            folds: compile(QueryKind::Folds)?,
            indents: compile(QueryKind::Indents)?,
            runnables: compile(QueryKind::Runnables)?,
            brackets_query: compile(QueryKind::Brackets)?,
            id: config.id,
            grammar: config.grammar,
            brackets: config.brackets,
            indent_unit: if config.indent_unit.is_empty() {
                "    ".into()
            } else {
                config.indent_unit
            },
            sources: config.queries,
        })
    }

    pub fn id(&self) -> &str {
        &self.id
    }

    pub fn grammar(&self) -> &Language {
        &self.grammar
    }

    pub fn brackets(&self) -> &[(String, String)] {
        &self.brackets
    }

    pub fn indent_unit(&self) -> &str {
        &self.indent_unit
    }

    /// The source of one kind of query (all files of that kind joined), or
    /// an empty string.
    pub fn query_source(&self, kind: QueryKind) -> String {
        self.sources
            .iter()
            .filter(|(k, _)| *k == kind)
            .map(|(_, s)| s.as_str())
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// Whether the language has a query of `kind`.
    pub fn has_query(&self, kind: QueryKind) -> bool {
        match kind {
            QueryKind::Highlights => self.highlights.is_some(),
            QueryKind::Injections => self.injections.is_some(),
            QueryKind::Tags => self.tags.is_some(),
            QueryKind::Outline => self.outline.is_some(),
            QueryKind::Folds => self.folds.is_some(),
            QueryKind::Indents => self.indents.is_some(),
            QueryKind::Runnables => self.runnables.is_some(),
            QueryKind::Brackets => self.brackets_query.is_some(),
            QueryKind::Locals => !self.query_source(QueryKind::Locals).trim().is_empty(),
        }
    }
}

/// Finds the language an injection names (`"javascript"`, `"js"`, a
/// Markdown fence's info string, …). Returns `None` for unknown names.
pub trait LanguageResolver: Send + Sync {
    fn resolve(&self, name: &str) -> Option<Arc<SyntaxLanguage>>;
}

impl<F> LanguageResolver for F
where
    F: Fn(&str) -> Option<Arc<SyntaxLanguage>> + Send + Sync,
{
    fn resolve(&self, name: &str) -> Option<Arc<SyntaxLanguage>> {
        self(name)
    }
}

/// A resolver that knows no languages: injections stay unparsed.
pub struct NoInjections;

impl LanguageResolver for NoInjections {
    fn resolve(&self, _: &str) -> Option<Arc<SyntaxLanguage>> {
        None
    }
}
