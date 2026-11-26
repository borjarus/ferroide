//! FerroIDE's language server client (plan §8, Stage 6).
//!
//! - [`rpc`]: JSON-RPC 2.0 with `Content-Length` framing, request
//!   timeouts and cancellation (`$/cancelRequest` when a request future is
//!   dropped).
//! - [`server`]: one server process — `initialize`, document
//!   synchronization (incremental), server-to-client requests
//!   (`workspace/configuration`, dynamic registrations, file watchers,
//!   `workspace/applyEdit`), shutdown.
//! - [`service`]: [`LspLanguageService`], the `LanguageService` of projects
//!   on disk: servers per Language Pack started on demand, several per file,
//!   push and pull diagnostics, the governor (server limit, idle stop,
//!   restart after a crash, power save) and the tree-sitter fallback.
//! - [`templates`] and [`snippet`]: Live Templates, Postfix Completion and
//!   the LSP snippet format.
//! - [`install`]: finding and installing servers.
//! - [`position`] and [`uri`]: `char` columns ↔ UTF-16/UTF-8 and paths ↔
//!   `file://` URIs.

pub mod convert;
pub mod install;
pub mod position;
pub mod rpc;
pub mod server;
pub mod service;
pub mod snippet;
pub mod templates;
#[cfg(any(test, feature = "test-support"))]
pub mod testing;
pub mod uri;

pub use server::{LanguageServer, Launcher, ServerConfig, Transport};
pub use service::{LspLanguageService, LspOptions, ProcessLauncher, RootsFn, ServerSettings};
