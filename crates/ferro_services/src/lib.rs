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

pub mod debug;
pub mod fuzzy;
pub mod language;
pub mod mock;
pub mod search;
pub mod terminal;

pub use debug::*;
pub use language::*;
pub use search::*;
pub use terminal::*;

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
    /// The file's raw bytes, for detecting its encoding. The default reads
    /// the text as UTF-8.
    fn read_bytes(&self, path: &Path) -> BoxFuture<'static, Result<Vec<u8>>> {
        let read = self.read_file(path);
        Box::pin(async move { read.await.map(String::into_bytes) })
    }
    /// Replace the file's content (atomically in a real file system),
    /// creating it if needed.
    fn write_file(&self, path: &Path, bytes: Vec<u8>) -> BoxFuture<'static, Result<()>>;
    /// Added/removed/changed notifications.
    fn subscribe(&self) -> async_channel::Receiver<ProjectEvent>;
    /// Create an empty file, or a directory, at `path`.
    fn create(&self, path: &Path, kind: EntryKind) -> BoxFuture<'static, Result<()>>;
    /// Rename or move `from` to `to`.
    fn rename(&self, from: &Path, to: &Path) -> BoxFuture<'static, Result<()>>;
    fn delete(&self, path: &Path) -> BoxFuture<'static, Result<()>>;
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum ChangeKind {
    Modified,
    Added,
    Deleted,
    Renamed,
    Untracked,
    Conflicted,
}

impl ChangeKind {
    /// The one-letter code git and IntelliJ show.
    pub fn letter(self) -> &'static str {
        match self {
            ChangeKind::Modified => "M",
            ChangeKind::Added => "A",
            ChangeKind::Deleted => "D",
            ChangeKind::Renamed => "R",
            ChangeKind::Untracked => "U",
            ChangeKind::Conflicted => "C",
        }
    }
}

/// A changed file in the working tree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileChange {
    pub path: PathBuf,
    /// Relative to the repository root, `/`-separated.
    pub relative: String,
    pub kind: ChangeKind,
    /// The changelist it belongs to (IntelliJ), e.g. `Changes`.
    pub changelist: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Commit {
    /// Full hash; views show the first 7–8 characters.
    pub id: String,
    /// Parent ids, first parent first. Two or more for a merge.
    pub parents: Vec<String>,
    pub author: String,
    /// Seconds since the Unix epoch.
    pub timestamp: i64,
    /// First line.
    pub subject: String,
    pub body: String,
    /// Branch and tag names pointing at it.
    pub refs: Vec<String>,
    /// Relative paths of the files it changed.
    pub files: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LineChangeKind {
    Added,
    Modified,
    /// Lines were deleted just above `lines.start`.
    Deleted,
}

/// A changed range of the working copy against `HEAD`, for the gutter.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LineChange {
    /// 0-based, half-open. Empty for deletions.
    pub lines: std::ops::Range<u32>,
    pub kind: LineChangeKind,
}

/// The two sides of a change.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileDiff {
    pub relative: String,
    pub old_text: String,
    pub new_text: String,
    /// The same change as a unified patch (`diff --git` format).
    pub patch: String,
}

/// A file with a merge conflict: the common base and both sides.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MergeConflict {
    pub path: PathBuf,
    pub relative: String,
    pub base: String,
    /// Our side (the current branch).
    pub ours: String,
    /// Their side (the branch being merged).
    pub theirs: String,
    pub ours_label: String,
    pub theirs_label: String,
}

pub trait VcsService: Send + Sync + 'static {
    fn current_branch(&self) -> BoxFuture<'static, Result<Branch>>;
    fn branches(&self) -> BoxFuture<'static, Result<Vec<Branch>>>;
    fn checkout(&self, name: &str) -> BoxFuture<'static, Result<()>>;
    /// Uncommitted changes.
    fn changes(&self) -> BoxFuture<'static, Result<Vec<FileChange>>>;
    /// Commits reachable from all branches, newest first.
    fn log(&self, limit: usize) -> BoxFuture<'static, Result<Vec<Commit>>>;
    /// The working copy of `path` against `HEAD`.
    fn diff(&self, path: &Path) -> BoxFuture<'static, Result<FileDiff>>;
    /// Changed line ranges of `text` (the editor's current text) against
    /// `HEAD`. Synchronous, for the gutter.
    fn line_changes(&self, path: &Path, text: &str) -> Vec<LineChange>;
    fn conflicts(&self) -> BoxFuture<'static, Result<Vec<MergeConflict>>>;
    /// Mark the conflicts of `path` resolved, with `text` as the merged file.
    fn resolve_conflict(&self, path: &Path, text: &str) -> BoxFuture<'static, Result<()>>;
    /// Commit the given files; returns the new commit id.
    fn commit(&self, message: &str, paths: &[PathBuf]) -> BoxFuture<'static, Result<String>>;
    /// Clone `url` into `directory`; returns the project root.
    fn clone_repository(&self, url: &str, directory: &Path) -> BoxFuture<'static, Result<PathBuf>>;
}

// ---------------------------------------------------------------------------
// Run configurations
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct RunConfiguration {
    pub id: String,
    pub name: String,
    /// e.g. `cargo`, `gradle`, `npm`, `pytest`.
    pub kind: String,
    pub command: String,
    /// Extra arguments passed to the program.
    pub arguments: String,
    /// Relative to the project root; empty is the root.
    pub working_directory: String,
    pub environment: Vec<(String, String)>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TestStatus {
    Passed,
    Failed,
    Ignored,
    Running,
    NotRun,
}

/// A test, or a suite of them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TestNode {
    pub name: String,
    pub status: TestStatus,
    pub duration_ms: u32,
    /// Output or failure message.
    pub output: Vec<String>,
    /// File and 0-based line.
    pub location: Option<(PathBuf, u32)>,
    pub children: Vec<TestNode>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ServiceKind {
    RunConfiguration,
    Docker,
    Database,
    Group,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ServiceState {
    Running,
    Stopped,
    Failed,
    /// For groups.
    None,
}

/// A node of the Services tool window.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServiceNode {
    pub name: String,
    pub kind: ServiceKind,
    pub state: ServiceState,
    /// e.g. `localhost:5432`, `Up 2 hours`.
    pub detail: Option<String>,
    pub children: Vec<ServiceNode>,
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
    /// Add or replace (by id) a run configuration.
    fn save_configuration(&self, config: RunConfiguration) -> Result<()>;
    fn delete_configuration(&self, id: &str);
    /// Results of the last test run, as a tree of suites.
    fn test_results(&self) -> Vec<TestNode>;
    /// The Services tool window tree.
    fn services(&self) -> Vec<ServiceNode>;
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum NotificationLevel {
    Info,
    Success,
    Warning,
    Error,
}

/// An entry of the Notifications tool window (the event log).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NotificationEntry {
    pub id: u64,
    pub level: NotificationLevel,
    pub title: String,
    pub body: String,
    /// Seconds since the Unix epoch.
    pub timestamp: i64,
    pub read: bool,
}

pub trait StatusService: Send + Sync + 'static {
    fn language_servers(&self) -> Vec<LanguageServerStatus>;
    fn subscribe_tasks(&self) -> async_channel::Receiver<TaskProgress>;
    fn notifications(&self) -> Vec<NotificationEntry>;
    fn mark_notifications_read(&self);
    fn clear_notifications(&self);
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
    pub language: std::sync::Arc<dyn LanguageService>,
    pub search: std::sync::Arc<dyn SearchService>,
    pub debug: std::sync::Arc<dyn DebugService>,
    pub terminal: std::sync::Arc<dyn TerminalService>,
}

impl Services {
    /// All-mock services for the demo project. `config` controls latency,
    /// failure rate and the size of the synthetic stress tree.
    pub fn mock(config: mock::MockConfig) -> Self {
        Self::mock_with_project(config).0
    }

    /// [`Services::mock`], also handing out the mock project so tests and
    /// the storybook can change files "on disk" behind the editor's back.
    pub fn mock_with_project(
        config: mock::MockConfig,
    ) -> (Self, std::sync::Arc<mock::MockProjectService>) {
        use std::sync::Arc;
        let project = Arc::new(mock::MockProjectService::demo(config.clone()));
        let root = project.root().to_path_buf();
        let mock_project = project.clone();
        let services = Self {
            recent: Arc::new(mock::MockRecentProjects::new(config.seed)),
            vcs: Arc::new(mock::MockVcsService::new(config.clone(), project.clone())),
            run: Arc::new(mock::MockRunService::new(root.clone())),
            language: Arc::new(mock::MockLanguageService::new(
                config.clone(),
                project.clone(),
            )),
            search: Arc::new(mock::MockSearchService::new(
                config.clone(),
                project.clone(),
            )),
            debug: Arc::new(mock::MockDebugService::new(root.clone())),
            terminal: Arc::new(mock::MockTerminalService::new(root)),
            status: Arc::new(mock::MockStatusService::new(config)),
            project,
        };
        (services, mock_project)
    }
}
