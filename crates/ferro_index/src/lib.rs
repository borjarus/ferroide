//! `ferro_index`: the persistent project index behind navigation and search
//! (plan §8, Stage 7).
//!
//! - [`ProjectIndex`]: files, symbols (from the Language Packs' `tags.scm`)
//!   and TODO comments in a per-project SQLite database, loaded at startup
//!   and kept current in the background (paused while typing); it is the
//!   project's [`SearchService`](ferro_services::SearchService).
//! - [`fuzzy`]: Quick Open matching with `nucleo-matcher`, narrowing the
//!   previous results while a query grows and using several threads for
//!   large projects.
//! - [`text`]: Find in Files with ripgrep's libraries (`grep-searcher`,
//!   `grep-regex`), file masks, scopes and replacement with regex groups.
//! - [`trigram`]: an optional trigram index for repositories above a few
//!   gigabytes.
//!
//! GUI-free: tested headless.

pub mod db;
pub mod fuzzy;
mod index;
pub mod text;
pub mod trigram;

pub use index::{IndexOptions, ProjectIndex, db_path_for, find_todos};
pub use text::{Candidate, FileMask, Replacer, query_regex, search_files};
pub use trigram::TrigramMode;

#[cfg(test)]
mod tests;
