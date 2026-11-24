//! `language.toml`: a Language Pack's manifest (plan Appendix B).

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// A Language Pack's manifest.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LanguageManifest {
    /// Stable id, also the name injections and settings refer to.
    pub id: String,
    /// Display name, e.g. `C#`.
    pub name: String,
    #[serde(default = "default_version")]
    pub version: String,
    /// Other names: Markdown fence info strings, modelines, injections.
    #[serde(default)]
    pub aliases: Vec<String>,
    /// Without the dot; compound extensions (`d.ts`) are allowed.
    #[serde(default)]
    pub file_extensions: Vec<String>,
    /// Exact file names, e.g. `Makefile`, `Cargo.lock`.
    #[serde(default)]
    pub file_names: Vec<String>,
    /// Interpreters named on a `#!` line (`python3`, `bash`); a version
    /// suffix is ignored (`python3.12` matches `python3` and `python`).
    #[serde(default)]
    pub shebangs: Vec<String>,
    /// A regular expression matched against the first line, e.g. `^<\?xml`.
    #[serde(default)]
    pub first_line: Option<String>,
    /// Files whose presence in a project root activates the pack.
    #[serde(default)]
    pub project_markers: Vec<String>,
    /// Packs only used inside others (Markdown's inline grammar) are hidden
    /// from language pickers and never detected for files.
    #[serde(default)]
    pub hidden: bool,
    /// Whether the parser's syntax errors are shown as diagnostics. Off for
    /// grammars that do not cover the whole language (SQL dialects) or
    /// accept anything (Markdown).
    #[serde(default = "yes")]
    pub syntax_errors: bool,

    #[serde(default)]
    pub line_comment: Option<String>,
    /// `["/*", "*/"]`.
    #[serde(default)]
    pub block_comment: Option<[String; 2]>,
    #[serde(default)]
    pub indent: IndentConfig,
    /// Structural pairs: matching and indentation.
    #[serde(default = "default_brackets")]
    pub brackets: Vec<[String; 2]>,
    /// Pairs closed automatically; defaults to the brackets plus `"` and
    /// `'`. Never inside strings or comments.
    #[serde(default)]
    pub auto_pairs: Option<Vec<[String; 2]>>,
    /// Automatic closing only before these characters, whitespace or the end
    /// of the line.
    #[serde(default)]
    pub auto_close_before: Option<String>,
    /// Enter after text matching this regex indents the new line.
    #[serde(default)]
    pub increase_indent_pattern: Option<String>,
    /// A line matching this regex is outdented.
    #[serde(default)]
    pub decrease_indent_pattern: Option<String>,

    #[serde(default)]
    pub grammar: Option<GrammarManifest>,
    #[serde(default)]
    pub language_servers: Vec<LanguageServerManifest>,
    #[serde(default)]
    pub debugger: Option<DebuggerManifest>,
    #[serde(default)]
    pub run_templates: Vec<RunTemplate>,
    #[serde(default)]
    pub test_frameworks: Vec<TestFramework>,
}

fn yes() -> bool {
    true
}

fn default_version() -> String {
    "1.0.0".into()
}

fn default_brackets() -> Vec<[String; 2]> {
    [("(", ")"), ("[", "]"), ("{", "}")]
        .into_iter()
        .map(|(a, b)| [a.to_string(), b.to_string()])
        .collect()
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IndentConfig {
    /// One level: spaces or `"\t"`.
    #[serde(default = "default_unit")]
    pub unit: String,
    #[serde(default = "default_tab_size")]
    pub tab_size: u32,
}

impl Default for IndentConfig {
    fn default() -> Self {
        Self {
            unit: default_unit(),
            tab_size: default_tab_size(),
        }
    }
}

fn default_unit() -> String {
    "    ".into()
}

fn default_tab_size() -> u32 {
    4
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GrammarManifest {
    /// The grammar's name; built-in grammars are looked up by it.
    pub name: String,
    #[serde(default)]
    pub source: GrammarSource,
    /// Query files of the pack. A file missing from the pack's directory is
    /// taken from the grammar's own queries when it ships one.
    #[serde(default)]
    pub queries: Vec<String>,
}

/// Where a grammar comes from.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(untagged)]
pub enum GrammarSource {
    /// Linked into FerroIDE.
    #[default]
    #[serde(skip)]
    Builtin,
    /// A compiled WebAssembly grammar, relative to the pack's directory.
    Wasm { wasm: String },
    /// A repository to build the grammar from (recorded for the pack
    /// registry; FerroIDE does not compile grammars itself).
    Git { git: String, rev: String },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LanguageServerManifest {
    pub id: String,
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    /// e.g. `{ pypi = "ruff" }`, `{ npm = "…" }`, `{ github = "…" }`.
    #[serde(default)]
    pub install: BTreeMap<String, String>,
    #[serde(default)]
    pub features: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DebuggerManifest {
    pub adapter: String,
    #[serde(default)]
    pub install: BTreeMap<String, String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RunTemplate {
    pub name: String,
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TestFramework {
    pub name: String,
    #[serde(default)]
    pub detect: Vec<String>,
}

impl LanguageManifest {
    pub fn parse(source: &str) -> Result<Self, toml::de::Error> {
        toml::from_str(source)
    }

    /// The id and every alias, lower-case.
    pub fn names(&self) -> impl Iterator<Item = String> + '_ {
        std::iter::once(self.id.to_lowercase()).chain(self.aliases.iter().map(|a| a.to_lowercase()))
    }

    /// The auto-closing pairs: the configured ones, or the brackets plus
    /// double and single quotes.
    pub fn auto_pairs(&self) -> Vec<[String; 2]> {
        self.auto_pairs.clone().unwrap_or_else(|| {
            let mut pairs = self.brackets.clone();
            pairs.push(["\"".into(), "\"".into()]);
            pairs.push(["'".into(), "'".into()]);
            pairs
        })
    }
}
