//! `ferro_tasks`: running, building and testing (plan §8, Stage 8).
//!
//! [`TaskRunService`] is the project's [`RunService`](ferro_services::RunService):
//!
//! - [`store`]: run configurations as committable TOML files in
//!   `.ferroide/run/`, re-read when they change on disk; temporary ones
//!   (▶ icons, build tool tasks) are kept in memory;
//! - [`exec`] / [`command`] / [`dotenv`]: from a configuration to a
//!   process — shell-like splitting without a shell, `PATH`/`PATHEXT`
//!   resolution with Maven/Gradle wrappers, layered environment with
//!   `.env` files and forced colours;
//! - [`process`]: output read line by line on threads, the process tree
//!   killed on stop;
//! - [`testing`]: detecting the test framework of a command (Cargo,
//!   nextest, pytest, Go, Jest, Vitest, Maven, Gradle, .NET), reading its
//!   results live or from reports into a test tree, locating tests in the
//!   sources and narrowing a command to the failed tests;
//! - [`runnables`]: gutter ▶ icons → configurations per language;
//! - [`build_tools`]: Cargo / npm / Maven / Gradle / Go / CMake / .NET
//!   projects and their tasks, defaults and editor templates.
//!
//! GUI-free and tested headless; the integration tests run real
//! processes (including `cargo test` on a tiny crate). Whether a project
//! is trusted to run anything is decided by the UI before calling here.

pub mod build_tools;
pub mod command;
pub mod dotenv;
pub mod exec;
pub mod process;
pub mod runnables;
mod service;
pub mod store;
pub mod testing;
pub mod walk;

pub use service::{RunOptions, TaskRunService};
