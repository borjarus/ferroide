//! Service contracts between the FerroIDE UI and its logic.
//!
//! Every view depends on these traits, never on a concrete implementation.
//! Stages 1–2 run entirely on the mocks in [`mock`]: synthetic data —
//! including extreme cases — with simulated latency and failures, so the
//! "loading", "empty" and "error" states are designed from day one. Later
//! stages swap in real implementations (`FsProjectService` in Stage 4, LSP in
//! Stage 6, …) without touching the views.
//!
//! The traits are executor-agnostic: async methods return boxed `Send`
//! futures and events arrive over `async_channel` receivers, so they run on
//! GPUI's background executor, on Tokio, or in a plain test.

use std::path::{Path, PathBuf};

use futures::future::BoxFuture;

pub mod mock;

pub type Result<T> = anyhow::Result<T>;

// ---------------------------------------------------------------------------
// Project
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum EntryKind {
    Directory,
    File,
}

/// One file-system entry, as the project tree shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub path: PathBuf,
    pub name: String,
    pub kind: EntryKind,
    /// Size in bytes; zero for directories.
    pub size: u64,
}

impl Entry {
    pub fn is_dir(&self) -> bool {
        self.kind == EntryKind::Directory
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProjectEvent {
    Added(PathBuf),
    Removed(PathBuf),
    Changed(PathBuf),
}

/// The files of the open project.
pub trait ProjectService: Send + Sync + 'static {
    /// The project's display name.
    fn name(&self) -> String;
    fn roots(&self) -> Vec<PathBuf>;
    /// Direct children of `dir`, directories first, then by name.
    fn children(&self, dir: &Path) -> BoxFuture<'static, Result<Vec<Entry>>>;
    fn read_file(&self, path: &Path) -> BoxFuture<'static, Result<String>>;
    /// Added/removed/changed notifications.
    fn subscribe(&self) -> async_channel::Receiver<ProjectEvent>;
}

/// A project the welcome screen offers to reopen.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecentProject {
    pub name: String,
    pub path: PathBuf,
    /// Days since it was last opened.
    pub opened_days_ago: u32,
    pub branch: Option<String>,
}

pub trait RecentProjectsService: Send + Sync + 'static {
    fn recent(&self) -> Vec<RecentProject>;
}

// ---------------------------------------------------------------------------
// Version control
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Branch {
    pub name: String,
    pub is_remote: bool,
    /// Commits ahead/behind its upstream.
    pub ahead: u32,
    pub behind: u32,
}

pub trait VcsService: Send + Sync + 'static {
    fn current_branch(&self) -> BoxFuture<'static, Result<Branch>>;
    fn branches(&self) -> BoxFuture<'static, Result<Vec<Branch>>>;
    fn checkout(&self, name: &str) -> BoxFuture<'static, Result<()>>;
}

// ---------------------------------------------------------------------------
// Run configurations
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunConfiguration {
    pub id: String,
    pub name: String,
    /// e.g. `cargo`, `gradle`, `npm`, `pytest`.
    pub kind: String,
    pub command: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunState {
    Idle,
    Running { debug: bool },
    Finished { exit_code: i32 },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RunEvent {
    StateChanged { id: String, state: RunState },
    Output { id: String, line: String },
}

pub trait RunService: Send + Sync + 'static {
    fn configurations(&self) -> Vec<RunConfiguration>;
    fn state(&self, id: &str) -> RunState;
    fn start(&self, id: &str, debug: bool) -> Result<()>;
    fn stop(&self, id: &str);
    fn subscribe(&self) -> async_channel::Receiver<RunEvent>;
}

// ---------------------------------------------------------------------------
// Status: language servers and background tasks
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ServerHealth {
    Starting,
    Ready,
    Crashed,
}

#[derive(Debug, Clone, PartialEq)]
pub struct LanguageServerStatus {
    pub name: String,
    pub language: String,
    pub health: ServerHealth,
    pub memory_mb: u32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TaskProgress {
    pub id: u64,
    /// Message id of the task's title, e.g. `task-indexing`.
    pub title_key: String,
    /// 0.0..=1.0; `None` for indeterminate.
    pub fraction: Option<f32>,
    pub done: bool,
}

pub trait StatusService: Send + Sync + 'static {
    fn language_servers(&self) -> Vec<LanguageServerStatus>;
    fn subscribe_tasks(&self) -> async_channel::Receiver<TaskProgress>;
}

// ---------------------------------------------------------------------------
// Bundle
// ---------------------------------------------------------------------------

/// Every service a workspace window needs, behind trait objects.
#[derive(Clone)]
pub struct Services {
    pub project: std::sync::Arc<dyn ProjectService>,
    pub recent: std::sync::Arc<dyn RecentProjectsService>,
    pub vcs: std::sync::Arc<dyn VcsService>,
    pub run: std::sync::Arc<dyn RunService>,
    pub status: std::sync::Arc<dyn StatusService>,
}

impl Services {
    /// All-mock services for the demo project. `config` controls latency,
    /// failure rate and the size of the synthetic stress tree.
    pub fn mock(config: mock::MockConfig) -> Self {
        use std::sync::Arc;
        Self {
            project: Arc::new(mock::MockProjectService::demo(config.clone())),
            recent: Arc::new(mock::MockRecentProjects::new(config.seed)),
            vcs: Arc::new(mock::MockVcsService::new(config.clone())),
            run: Arc::new(mock::MockRunService::new()),
            status: Arc::new(mock::MockStatusService::new(config)),
        }
    }
}
