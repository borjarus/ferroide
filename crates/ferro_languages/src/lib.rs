//! FerroIDE Language Packs (plan §7, Stage 5).
//!
//! Every language is a pack: a `language.toml` manifest ([`manifest`]) plus
//! tree-sitter queries (`.scm`). The built-in packs live in `languages/` in
//! the repository and are embedded at build time; more packs can be loaded
//! from a folder ([`LanguageRegistry::load_dir`]) and replace built-in ones
//! with the same id.
//!
//! - [`LanguageRegistry::detect`]: a file's language from a modeline, its
//!   name, its extension, a shebang or a first-line pattern.
//! - [`LanguagePack::syntax`]: the grammar and compiled queries, built when
//!   the first file of the language is opened (grammars are linked in —
//!   [`grammars`] — and cost nothing until then).
//! - [`LanguageRegistry::resolver`]: resolves injected languages for
//!   [`ferro_syntax::SyntaxTree`].
//! - [`SyntaxLanguageService`]: code intelligence without a language
//!   server, the fallback of `ferro_lsp` — outline, ▶ icons, syntax errors,
//!   keyword and word completion, a tags-based symbol index.

pub mod detect;
pub mod grammars;
pub mod manifest;
mod registry;
pub mod service;

mod builtin {
    include!(concat!(env!("OUT_DIR"), "/builtin_packs.rs"));
}

pub use manifest::LanguageManifest;
pub use registry::{LanguagePack, LanguageRegistry, PackError};
pub use service::SyntaxLanguageService;

#[cfg(test)]
mod tests;
