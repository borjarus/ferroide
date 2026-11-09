//! Mock implementations of every service, driven by deterministic synthetic
//! data.
//!
//! The mocks are not toys: they deliberately include the cases real projects
//! produce rarely and UIs get wrong — 100,000-file trees, 200-character file
//! names, emoji and right-to-left text, nesting 40 levels deep, empty
//! directories, slow responses and failures.

use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex, MutexGuard, OnceLock,
        atomic::{AtomicU64, Ordering},
    },
    thread,
    time::Duration,
};

use anyhow::{anyhow, bail};
use futures::{FutureExt as _, future::BoxFuture};

use crate::{
    Entry, EntryKind, LanguageServerStatus, NotificationEntry, NotificationLevel, ProjectEvent,
    ProjectService, RecentProject, RecentProjectsService, Result, RunConfiguration, RunEvent,
    RunService, RunState, ServerHealth, ServiceKind, ServiceNode, ServiceState, StatusService,
    TaskProgress, TestNode, TestStatus,
};

mod debug;
mod diff;
mod language;
mod search;
mod terminal;
mod vcs;

pub use debug::MockDebugService;
pub use language::MockLanguageService;
pub use search::MockSearchService;
pub use terminal::MockTerminalService;
pub use vcs::{MockVcsService, conflict_markers};

/// "Now" for all mock timestamps (November 2026), so data is deterministic.
pub const MOCK_NOW: i64 = 1_794_200_000;

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|e| e.into_inner())
}

/// Rebuild a path with native separators, so `a/b` and `a\b` are one key.
fn norm(path: &Path) -> PathBuf {
    path.components().collect()
}

/// `root` joined with a `/`-separated relative path.
fn join_rel(root: &Path, relative: &str) -> PathBuf {
    relative
        .split('/')
        .filter(|s| !s.is_empty())
        .fold(root.to_path_buf(), |p, s| p.join(s))
}

/// `path` relative to `root`, `/`-separated.
fn rel_of(root: &Path, path: &Path) -> Option<String> {
    let rel = path.strip_prefix(root).ok()?;
    Some(
        rel.components()
            .map(|c| c.as_os_str().to_string_lossy())
            .collect::<Vec<_>>()
            .join("/"),
    )
}

/// Knobs shared by all mocks.
#[derive(Debug, Clone)]
pub struct MockConfig {
    pub seed: u64,
    /// Delay before every async answer.
    pub latency: Duration,
    /// Probability (0..=1) that an async call fails.
    pub failure_rate: f32,
    /// Approximate number of files in the synthetic `stress/` directory.
    /// Zero leaves it out.
    pub stress_files: usize,
}

impl Default for MockConfig {
    fn default() -> Self {
        Self {
            seed: 42,
            latency: Duration::from_millis(30),
            failure_rate: 0.0,
            stress_files: 100_000,
        }
    }
}

impl MockConfig {
    /// No latency, no failures, no stress tree: for unit tests.
    pub fn instant() -> Self {
        Self {
            latency: Duration::ZERO,
            stress_files: 0,
            ..Self::default()
        }
    }
}

/// SplitMix64: tiny, fast, deterministic. Good enough for synthetic data.
fn mix(mut x: u64) -> u64 {
    x = x.wrapping_add(0x9E3779B97F4A7C15);
    x = (x ^ (x >> 30)).wrapping_mul(0xBF58476D1CE4E5B9);
    x = (x ^ (x >> 27)).wrapping_mul(0x94D049BB133111EB);
    x ^ (x >> 31)
}

fn hash_str(seed: u64, s: &str) -> u64 {
    s.bytes().fold(mix(seed), |h, b| mix(h ^ b as u64))
}

/// Uniform float in 0..1 from a counter, for failure injection.
struct Dice {
    seed: u64,
    counter: AtomicU64,
}

impl Dice {
    fn new(seed: u64) -> Self {
        Self {
            seed,
            counter: AtomicU64::new(0),
        }
    }

    fn roll(&self) -> f32 {
        let n = self.counter.fetch_add(1, Ordering::Relaxed);
        (mix(self.seed ^ n) >> 40) as f32 / (1u64 << 24) as f32
    }
}

/// Wait `latency`, then fail with probability `failure_rate`.
async fn simulate(latency: Duration, fail: bool, what: &'static str) -> Result<()> {
    if !latency.is_zero() {
        smol::Timer::after(latency).await;
    }
    if fail {
        bail!("simulated failure: {what}");
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Project
// ---------------------------------------------------------------------------

/// A demo project: a small, realistic Rust/TypeScript workspace plus an
/// optional, lazily generated `stress/` tree for performance work.
pub struct MockProjectService {
    root: PathBuf,
    /// Hand-written demo files, editable through create/rename/delete.
    files: Mutex<HashMap<PathBuf, String>>,
    /// Directories created explicitly (they show even when empty).
    dirs: Mutex<HashSet<PathBuf>>,
    tree: SyntheticTree,
    /// Relative paths of every file of the synthetic tree, built on first use.
    stress_index: OnceLock<Arc<Vec<String>>>,
    config: MockConfig,
    dice: Dice,
    subscribers: Mutex<Vec<async_channel::Sender<ProjectEvent>>>,
}

const DEMO_FILES: &[(&str, &str)] = &[
    (
        "Cargo.toml",
        include_str!("../fixtures/demo-app/Cargo.toml.txt"),
    ),
    ("README.md", include_str!("../fixtures/demo-app/README.md")),
    (
        "src/main.rs",
        include_str!("../fixtures/demo-app/src/main.rs.txt"),
    ),
    (
        "src/lib.rs",
        include_str!("../fixtures/demo-app/src/lib.rs.txt"),
    ),
    (
        "src/config.rs",
        include_str!("../fixtures/demo-app/src/config.rs.txt"),
    ),
    (
        "tests/smoke.rs",
        include_str!("../fixtures/demo-app/tests/smoke.rs.txt"),
    ),
    (
        "web/package.json",
        include_str!("../fixtures/demo-app/web/package.json"),
    ),
    (
        "web/src/app.ts",
        include_str!("../fixtures/demo-app/web/src/app.ts"),
    ),
    (".gitignore", "/target\nnode_modules/\n"),
    ("docs/.keep", ""),
    ("empty-dir/.keep", ""),
];

impl MockProjectService {
    pub fn demo(config: MockConfig) -> Self {
        let root = PathBuf::from(if cfg!(windows) {
            r"C:\Projects\demo-app"
        } else {
            "/home/dev/projects/demo-app"
        });
        let mut files: HashMap<PathBuf, String> = DEMO_FILES
            .iter()
            .map(|(rel, text)| (join_rel(&root, rel), text.to_string()))
            .collect();
        for (rel, text) in vcs::extra_demo_files(&root) {
            files.insert(join_rel(&root, &rel), text);
        }
        Self {
            tree: SyntheticTree::new(root.join("stress"), config.seed, config.stress_files),
            dice: Dice::new(config.seed),
            root,
            files: Mutex::new(files),
            dirs: Mutex::new(HashSet::new()),
            stress_index: OnceLock::new(),
            config,
            subscribers: Mutex::new(Vec::new()),
        }
    }

    /// The project root directory.
    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn config(&self) -> &MockConfig {
        &self.config
    }

    /// `path` relative to the root, `/`-separated.
    pub fn relative(&self, path: &Path) -> Option<String> {
        rel_of(&self.root, path)
    }

    /// The absolute path of a `/`-separated relative path.
    pub fn absolute(&self, relative: &str) -> PathBuf {
        join_rel(&self.root, relative)
    }

    fn is_stress(&self, path: &Path) -> bool {
        self.config.stress_files > 0 && path.starts_with(&self.tree.root)
    }

    /// Current text of a file: demo files and generated stress files.
    pub fn file_text(&self, path: &Path) -> Option<String> {
        if let Some(text) = lock(&self.files).get(&norm(path)) {
            return Some(text.clone());
        }
        (self.is_stress(path)).then(|| self.tree.file_content(path))
    }

    /// Every hand-written demo file (placeholders excluded) with its text,
    /// sorted by path.
    pub fn demo_files(&self) -> Vec<(PathBuf, String)> {
        let mut files: Vec<(PathBuf, String)> = lock(&self.files)
            .iter()
            .filter(|(p, _)| p.file_name().is_some_and(|n| n != ".keep"))
            .map(|(p, t)| (p.clone(), t.clone()))
            .collect();
        files.sort_by(|a, b| a.0.cmp(&b.0));
        files
    }

    /// Relative paths of every file in the synthetic `stress/` tree. Built
    /// once, on first use (about 0.1 s per 100,000 files), then shared.
    pub fn stress_files(&self) -> Arc<Vec<String>> {
        self.stress_index
            .get_or_init(|| {
                let mut out = Vec::new();
                if self.config.stress_files > 0 {
                    walk_tree(&self.tree, &self.tree.root, "stress", &mut out);
                }
                Arc::new(out)
            })
            .clone()
    }

    /// Every file of the project: demo files, then the stress tree.
    pub fn all_files(&self) -> Vec<PathBuf> {
        let mut files: Vec<PathBuf> = self.demo_files().into_iter().map(|(p, _)| p).collect();
        files.extend(self.stress_files().iter().map(|rel| self.absolute(rel)));
        files
    }

    fn should_fail(&self) -> bool {
        self.config.failure_rate > 0.0 && self.dice.roll() < self.config.failure_rate
    }

    /// Whether a demo file or directory (explicit or implied) exists.
    fn demo_exists(&self, path: &Path) -> bool {
        let path = norm(path);
        lock(&self.files).keys().any(|p| p.starts_with(&path))
            || lock(&self.dirs).iter().any(|p| p.starts_with(&path))
    }

    fn check_writable(&self, path: &Path) -> Result<()> {
        if self.is_stress(path) {
            bail!("{} is read-only in the mock", path.display());
        }
        if !path.starts_with(&self.root) || norm(path) == norm(&self.root) {
            bail!("{} is outside the project", path.display());
        }
        Ok(())
    }

    fn create_now(&self, path: &Path, kind: EntryKind) -> Result<()> {
        self.check_writable(path)?;
        if self.demo_exists(path) {
            bail!("{} already exists", path.display());
        }
        match kind {
            EntryKind::File => {
                lock(&self.files).insert(norm(path), String::new());
            }
            EntryKind::Directory => {
                lock(&self.dirs).insert(norm(path));
            }
        }
        self.emit(ProjectEvent::Added(path.to_path_buf()));
        Ok(())
    }

    fn rename_now(&self, from: &Path, to: &Path) -> Result<()> {
        self.check_writable(from)?;
        self.check_writable(to)?;
        if !self.demo_exists(from) {
            bail!("{} does not exist", from.display());
        }
        if self.demo_exists(to) {
            bail!("{} already exists", to.display());
        }
        if to.starts_with(from) {
            bail!("cannot move {} into itself", from.display());
        }
        let (from_n, to_n) = (norm(from), norm(to));
        let moved = |p: &PathBuf| -> Option<PathBuf> {
            p.strip_prefix(&from_n).ok().map(|rest| {
                if rest.as_os_str().is_empty() {
                    to_n.clone()
                } else {
                    to_n.join(rest)
                }
            })
        };
        {
            let mut files = lock(&self.files);
            let keys: Vec<PathBuf> = files
                .keys()
                .filter_map(|p| moved(p).map(|_| p.clone()))
                .collect();
            for key in keys {
                if let (Some(text), Some(new)) = (files.remove(&key), moved(&key)) {
                    files.insert(new, text);
                }
            }
        }
        {
            let mut dirs = lock(&self.dirs);
            let old: Vec<PathBuf> = dirs
                .iter()
                .filter(|p| moved(p).is_some())
                .cloned()
                .collect();
            for dir in old {
                dirs.remove(&dir);
                if let Some(new) = moved(&dir) {
                    dirs.insert(new);
                }
            }
        }
        self.emit(ProjectEvent::Removed(from.to_path_buf()));
        self.emit(ProjectEvent::Added(to.to_path_buf()));
        Ok(())
    }

    fn delete_now(&self, path: &Path) -> Result<()> {
        self.check_writable(path)?;
        if !self.demo_exists(path) {
            bail!("{} does not exist", path.display());
        }
        let path_n = norm(path);
        lock(&self.files).retain(|p, _| !p.starts_with(&path_n));
        lock(&self.dirs).retain(|p| !p.starts_with(&path_n));
        self.emit(ProjectEvent::Removed(path.to_path_buf()));
        Ok(())
    }

    /// Children of a directory of the hand-written part of the demo.
    fn demo_children(&self, dir: &Path) -> Vec<Entry> {
        let dir = norm(dir);
        let mut entries: HashMap<PathBuf, Entry> = HashMap::new();
        let files = lock(&self.files);
        let dirs = lock(&self.dirs);
        let all = files
            .iter()
            .map(|(p, t)| (p, Some(t.len())))
            .chain(dirs.iter().map(|p| (p, None)));
        for (path, size) in all {
            let Ok(rel) = path.strip_prefix(&dir) else {
                continue;
            };
            let mut parts = rel.components();
            let Some(first) = parts.next() else { continue };
            let child = dir.join(first);
            let is_dir = parts.next().is_some() || size.is_none();
            let name = first.as_os_str().to_string_lossy().into_owned();
            if name == ".keep" {
                continue;
            }
            entries.entry(child.clone()).or_insert(Entry {
                path: child,
                name,
                kind: if is_dir {
                    EntryKind::Directory
                } else {
                    EntryKind::File
                },
                size: if is_dir { 0 } else { size.unwrap_or(0) as u64 },
            });
        }
        if dir == norm(&self.root) && self.config.stress_files > 0 {
            let path = self.tree.root.clone();
            entries.insert(
                path.clone(),
                Entry {
                    path,
                    name: "stress".into(),
                    kind: EntryKind::Directory,
                    size: 0,
                },
            );
        }
        let mut entries: Vec<Entry> = entries.into_values().collect();
        sort_entries(&mut entries);
        entries
    }

    /// Run a file operation now (unless the dice say it fails, in which case
    /// nothing changes) and answer after the simulated latency.
    fn mutate(
        &self,
        op: impl FnOnce(&Self) -> Result<()>,
        what: &'static str,
    ) -> BoxFuture<'static, Result<()>> {
        let fail = self.should_fail();
        let result = if fail { Ok(()) } else { op(self) };
        let latency = self.config.latency;
        async move {
            simulate(latency, fail, what).await?;
            result
        }
        .boxed()
    }

    /// Notify subscribers, as a file watcher would. Used by the storybook.
    pub fn emit(&self, event: ProjectEvent) {
        let mut subscribers = self.subscribers.lock().unwrap_or_else(|e| e.into_inner());
        subscribers.retain(|tx| tx.try_send(event.clone()).is_ok() || !tx.is_closed());
    }
}

/// Collect the relative paths of every file under `dir`.
fn walk_tree(tree: &SyntheticTree, dir: &Path, prefix: &str, out: &mut Vec<String>) {
    for entry in tree.children(dir).unwrap_or_default() {
        let rel = format!("{prefix}/{}", entry.name);
        if entry.is_dir() {
            walk_tree(tree, &entry.path, &rel, out);
        } else {
            out.push(rel);
        }
    }
}

fn sort_entries(entries: &mut [Entry]) {
    entries.sort_by(|a, b| {
        a.kind
            .cmp(&b.kind)
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
}

impl ProjectService for MockProjectService {
    fn name(&self) -> String {
        "demo-app".into()
    }

    fn roots(&self) -> Vec<PathBuf> {
        vec![self.root.clone()]
    }

    fn children(&self, dir: &Path) -> BoxFuture<'static, Result<Vec<Entry>>> {
        let fail = self.should_fail();
        let latency = self.config.latency;
        let result = if dir.starts_with(&self.tree.root) {
            self.tree.children(dir)
        } else if dir.starts_with(&self.root) {
            Ok(self.demo_children(dir))
        } else {
            Err(anyhow!("{} is outside the project", dir.display()))
        };
        async move {
            simulate(latency, fail, "listing a directory").await?;
            result
        }
        .boxed()
    }

    fn read_file(&self, path: &Path) -> BoxFuture<'static, Result<String>> {
        let fail = self.should_fail();
        let latency = self.config.latency;
        let text = lock(&self.files).get(&norm(path)).cloned();
        let result = if let Some(text) = text {
            Ok(text)
        } else if path.starts_with(&self.tree.root) {
            Ok(self.tree.file_content(path))
        } else {
            Err(anyhow!("no such file: {}", path.display()))
        };
        async move {
            simulate(latency, fail, "reading a file").await?;
            result
        }
        .boxed()
    }

    fn create(&self, path: &Path, kind: EntryKind) -> BoxFuture<'static, Result<()>> {
        self.mutate(|this| this.create_now(path, kind), "creating a file")
    }

    fn rename(&self, from: &Path, to: &Path) -> BoxFuture<'static, Result<()>> {
        self.mutate(|this| this.rename_now(from, to), "renaming a file")
    }

    fn delete(&self, path: &Path) -> BoxFuture<'static, Result<()>> {
        self.mutate(|this| this.delete_now(path), "deleting a file")
    }

    fn subscribe(&self) -> async_channel::Receiver<ProjectEvent> {
        let (tx, rx) = async_channel::unbounded();
        self.subscribers
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(tx);
        rx
    }
}

/// A lazily generated directory tree. Nothing is stored: the children of a
/// directory are a pure function of its path and the seed, so a
/// 1,000,000-file tree costs no memory until it is browsed.
pub struct SyntheticTree {
    root: PathBuf,
    seed: u64,
    /// Files per leaf-level directory and fan-out are derived from this.
    target_files: usize,
}

/// Names that break naive UIs.
const EXTREME_NAMES: &[&str] = &[
    "a",
    "README",
    "żółć-gęślą-jaźń.rs",
    "🦀 crab notes.md",
    "مرحبا.txt",
    "日本語のファイル名.ts",
    "file with    many    spaces.py",
    ".hidden",
    "UPPER_CASE_CONSTANTS.java",
    "x.y.z.min.js",
];

const EXTENSIONS: &[&str] = &[
    "rs", "ts", "py", "java", "go", "kt", "json", "md", "toml", "yaml",
];

impl SyntheticTree {
    pub fn new(root: PathBuf, seed: u64, target_files: usize) -> Self {
        Self {
            root,
            seed,
            target_files,
        }
    }

    fn depth(&self, dir: &Path) -> usize {
        dir.strip_prefix(&self.root)
            .map(|rel| rel.components().count())
            .unwrap_or(0)
    }

    /// Directory fan-out so that `fanout^3 * files_per_dir ≈ target`.
    fn shape(&self) -> (usize, usize) {
        let files_per_dir = 40usize;
        let leaves = (self.target_files / files_per_dir).max(1) as f64;
        let fanout = leaves.cbrt().ceil().max(1.0) as usize;
        (fanout, files_per_dir)
    }

    pub fn children(&self, dir: &Path) -> Result<Vec<Entry>> {
        if !dir.starts_with(&self.root) {
            bail!("{} is outside the synthetic tree", dir.display());
        }
        let depth = self.depth(dir);
        let h = hash_str(self.seed, &dir.to_string_lossy());
        let (fanout, files_per_dir) = self.shape();
        let mut out = Vec::new();

        // A single deep chain hangs off the root, 40 levels down.
        let in_deep_chain = dir
            .strip_prefix(&self.root)
            .ok()
            .and_then(|rel| rel.components().next())
            .is_some_and(|c| c.as_os_str() == "deep");
        if in_deep_chain {
            if depth < 40 {
                out.push(dir_entry(dir, &format!("level-{:02}", depth)));
            } else {
                out.push(file_entry(dir, "bottom.txt", 12));
            }
            return Ok(out);
        }

        if depth == 0 {
            out.push(dir_entry(dir, "deep"));
            out.push(dir_entry(dir, "empty"));
            out.push(dir_entry(
                dir,
                &format!("very-long-directory-name-{}", "segment-".repeat(20)),
            ));
            for name in EXTREME_NAMES {
                out.push(file_entry(dir, name, mix(h ^ name.len() as u64) % 4096));
            }
        }
        if dir.ends_with("empty") {
            return Ok(Vec::new());
        }

        if depth < 3 {
            for i in 0..fanout {
                out.push(dir_entry(dir, &format!("module_{depth}_{i:03}")));
            }
        } else {
            for i in 0..files_per_dir {
                let fh = mix(h ^ i as u64);
                let ext = EXTENSIONS[(fh % EXTENSIONS.len() as u64) as usize];
                out.push(file_entry(dir, &format!("file_{i:04}.{ext}"), fh % 65_536));
            }
        }
        sort_entries(&mut out);
        Ok(out)
    }

    /// Generated contents: a believable source file, length from the hash.
    pub fn file_content(&self, path: &Path) -> String {
        let h = hash_str(self.seed, &path.to_string_lossy());
        let lines = 20 + (h % 400) as usize;
        let mut text = String::with_capacity(lines * 40);
        text.push_str("// Generated by FerroIDE mock services.\n");
        for i in 0..lines {
            text.push_str(&format!(
                "fn item_{i}() -> u64 {{ {} }}\n",
                mix(h ^ i as u64) % 1000
            ));
        }
        text
    }
}

fn dir_entry(parent: &Path, name: &str) -> Entry {
    Entry {
        path: parent.join(name),
        name: name.to_string(),
        kind: EntryKind::Directory,
        size: 0,
    }
}

fn file_entry(parent: &Path, name: &str, size: u64) -> Entry {
    Entry {
        path: parent.join(name),
        name: name.to_string(),
        kind: EntryKind::File,
        size,
    }
}

// ---------------------------------------------------------------------------
// Recent projects
// ---------------------------------------------------------------------------

pub struct MockRecentProjects {
    projects: Vec<RecentProject>,
}

impl MockRecentProjects {
    pub fn new(seed: u64) -> Self {
        let base = if cfg!(windows) {
            r"C:\Projects"
        } else {
            "/home/dev/projects"
        };
        let names: &[(&str, Option<&str>)] = &[
            ("demo-app", Some("main")),
            ("payments-service", Some("feature/PAY-1432-refunds")),
            ("ferroide", Some("main")),
            (
                "legacy-monolith-with-an-extremely-long-project-name-that-must-truncate",
                None,
            ),
            ("ml-experiments", Some("experiment/transformer-v2")),
            ("dotfiles", None),
            ("projekt-zażółć", Some("develop")),
        ];
        let projects = names
            .iter()
            .enumerate()
            .map(|(i, (name, branch))| RecentProject {
                name: name.to_string(),
                path: PathBuf::from(base).join(name),
                opened_days_ago: (mix(seed ^ i as u64) % 60) as u32 * (i as u32 > 0) as u32,
                branch: branch.map(str::to_string),
            })
            .collect();
        Self { projects }
    }

    pub fn empty() -> Self {
        Self {
            projects: Vec::new(),
        }
    }
}

impl RecentProjectsService for MockRecentProjects {
    fn recent(&self) -> Vec<RecentProject> {
        self.projects.clone()
    }
}

// ---------------------------------------------------------------------------
// Run
// ---------------------------------------------------------------------------

struct RunInner {
    states: HashMap<String, RunState>,
    generation: HashMap<String, u64>,
    subscribers: Vec<async_channel::Sender<RunEvent>>,
}

/// Runs are simulated on a plain thread: a few lines of output, then exit.
pub struct MockRunService {
    root: PathBuf,
    configurations: Mutex<Vec<RunConfiguration>>,
    inner: Arc<Mutex<RunInner>>,
}

impl MockRunService {
    pub fn new(root: PathBuf) -> Self {
        let config =
            |id: &str, name: &str, kind: &str, command: &str, args: &str| RunConfiguration {
                id: id.into(),
                name: name.into(),
                kind: kind.into(),
                command: command.into(),
                arguments: args.into(),
                working_directory: String::new(),
                environment: vec![("RUST_LOG".into(), "info".into())],
            };
        let mut web = config(
            "web",
            "web: dev server",
            "npm",
            "npm run dev",
            "--port 5173",
        );
        web.working_directory = "web".into();
        web.environment = vec![("NODE_ENV".into(), "development".into())];
        Self {
            root,
            configurations: Mutex::new(vec![
                config(
                    "app",
                    "demo-app",
                    "cargo",
                    "cargo run",
                    "-- --names Ada,Grace",
                ),
                config("tests", "All tests", "cargo", "cargo nextest run", ""),
                web,
                config("smoke", "smoke.rs", "cargo", "cargo test --test smoke", ""),
            ]),
            inner: Arc::new(Mutex::new(RunInner {
                states: HashMap::new(),
                generation: HashMap::new(),
                subscribers: Vec::new(),
            })),
        }
    }

    fn broadcast(inner: &Mutex<RunInner>, event: RunEvent) {
        let mut inner = lock(inner);
        if let RunEvent::StateChanged { id, state } = &event {
            inner.states.insert(id.clone(), *state);
        }
        inner
            .subscribers
            .retain(|tx| tx.try_send(event.clone()).is_ok() || !tx.is_closed());
    }
}

fn test(name: &str, status: TestStatus, duration_ms: u32) -> TestNode {
    TestNode {
        name: name.into(),
        status,
        duration_ms,
        output: Vec::new(),
        location: None,
        children: Vec::new(),
    }
}

fn suite(name: &str, children: Vec<TestNode>) -> TestNode {
    let status = if children.iter().any(|c| c.status == TestStatus::Failed) {
        TestStatus::Failed
    } else if children.iter().all(|c| c.status == TestStatus::Ignored) {
        TestStatus::Ignored
    } else {
        TestStatus::Passed
    };
    TestNode {
        duration_ms: children.iter().map(|c| c.duration_ms).sum(),
        name: name.into(),
        status,
        output: Vec::new(),
        location: None,
        children,
    }
}

impl RunService for MockRunService {
    fn configurations(&self) -> Vec<RunConfiguration> {
        lock(&self.configurations).clone()
    }

    fn state(&self, id: &str) -> RunState {
        lock(&self.inner)
            .states
            .get(id)
            .copied()
            .unwrap_or(RunState::Idle)
    }

    fn start(&self, id: &str, debug: bool) -> Result<()> {
        let config = lock(&self.configurations)
            .iter()
            .find(|c| c.id == id)
            .cloned()
            .ok_or_else(|| anyhow!("unknown run configuration {id}"))?;
        if matches!(self.state(id), RunState::Running { .. }) {
            bail!("{} is already running", config.name);
        }
        let generation = {
            let mut inner = lock(&self.inner);
            let g = inner.generation.entry(id.to_string()).or_insert(0);
            *g += 1;
            *g
        };
        Self::broadcast(
            &self.inner,
            RunEvent::StateChanged {
                id: id.into(),
                state: RunState::Running { debug },
            },
        );
        let inner = self.inner.clone();
        let id = id.to_string();
        thread::Builder::new()
            .name(format!("mock-run-{id}"))
            .spawn(move || {
                let still_current = |inner: &Mutex<RunInner>| {
                    let inner = lock(inner);
                    inner.generation.get(&id) == Some(&generation)
                        && matches!(inner.states.get(&id), Some(RunState::Running { .. }))
                };
                let command = if config.arguments.is_empty() {
                    config.command.clone()
                } else {
                    format!("{} {}", config.command, config.arguments)
                };
                let lines = [
                    format!("$ {command}"),
                    "   Compiling demo-app v0.1.0".into(),
                    "    Finished `dev` profile in 1.42s".into(),
                    "     Running `target/debug/demo-app`".into(),
                    "Hello, FerroIDE".into(),
                ];
                for line in lines {
                    thread::sleep(Duration::from_millis(250));
                    if !still_current(&inner) {
                        return;
                    }
                    Self::broadcast(
                        &inner,
                        RunEvent::Output {
                            id: id.clone(),
                            line,
                        },
                    );
                }
                if still_current(&inner) {
                    Self::broadcast(
                        &inner,
                        RunEvent::StateChanged {
                            id,
                            state: RunState::Finished { exit_code: 0 },
                        },
                    );
                }
            })?;
        Ok(())
    }

    fn stop(&self, id: &str) {
        if matches!(self.state(id), RunState::Running { .. }) {
            Self::broadcast(
                &self.inner,
                RunEvent::StateChanged {
                    id: id.into(),
                    state: RunState::Finished { exit_code: 130 },
                },
            );
        }
    }

    fn subscribe(&self) -> async_channel::Receiver<RunEvent> {
        let (tx, rx) = async_channel::unbounded();
        lock(&self.inner).subscribers.push(tx);
        rx
    }

    fn save_configuration(&self, config: RunConfiguration) -> Result<()> {
        if config.id.trim().is_empty() {
            bail!("a run configuration needs an id");
        }
        if config.name.trim().is_empty() {
            bail!("a run configuration needs a name");
        }
        if config.command.trim().is_empty() {
            bail!("a run configuration needs a command");
        }
        let mut configs = lock(&self.configurations);
        if configs
            .iter()
            .any(|c| c.id != config.id && c.name == config.name)
        {
            bail!("a run configuration named {} already exists", config.name);
        }
        match configs.iter_mut().find(|c| c.id == config.id) {
            Some(existing) => *existing = config,
            None => configs.push(config),
        }
        Ok(())
    }

    fn delete_configuration(&self, id: &str) {
        self.stop(id);
        lock(&self.configurations).retain(|c| c.id != id);
    }

    fn test_results(&self) -> Vec<TestNode> {
        let lib = self.root.join("src").join("lib.rs");
        let smoke = self.root.join("tests").join("smoke.rs");
        let mut by_name = test("greets_by_name", TestStatus::Passed, 3);
        by_name.location = Some((lib.clone(), 21));
        let mut strangers = test("greets_strangers", TestStatus::Passed, 1);
        strangers.location = Some((lib, 26));
        let mut unicode = test("greets_in_unicode", TestStatus::Ignored, 0);
        unicode.output = vec!["ignored: needs ICU data".into()];
        let mut prints = test("prints_greeting", TestStatus::Failed, 412);
        prints.location = Some((smoke, 3));
        prints.output = vec![
            "thread 'prints_greeting' panicked at tests/smoke.rs:9:10:".into(),
            "Unexpected stdout, failed var.contains(Hello, FerroIDE)".into(),
            "├── var: Hello, Ada!".into(),
            "└── var as str: Hello, Ada!".into(),
            "note: run with `RUST_BACKTRACE=1` environment variable to display a backtrace".into(),
        ];
        vec![suite(
            "demo-app",
            vec![
                suite("lib tests", vec![by_name, strangers, unicode]),
                suite("smoke", vec![prints]),
            ],
        )]
    }

    fn services(&self) -> Vec<ServiceNode> {
        let node = |name: &str, kind, state, detail: Option<&str>| ServiceNode {
            name: name.into(),
            kind,
            state,
            detail: detail.map(str::to_string),
            children: Vec::new(),
        };
        let group = |name: &str, children: Vec<ServiceNode>| ServiceNode {
            name: name.into(),
            kind: ServiceKind::Group,
            state: ServiceState::None,
            detail: None,
            children,
        };
        let runs = self
            .configurations()
            .into_iter()
            .map(|c| {
                let state = match self.state(&c.id) {
                    RunState::Running { .. } => ServiceState::Running,
                    RunState::Finished { exit_code } if exit_code != 0 && exit_code != 130 => {
                        ServiceState::Failed
                    }
                    _ => ServiceState::Stopped,
                };
                node(
                    &c.name,
                    ServiceKind::RunConfiguration,
                    state,
                    Some(&c.command),
                )
            })
            .collect();
        vec![
            group("Run Configurations", runs),
            group(
                "Docker",
                vec![
                    node(
                        "postgres:16",
                        ServiceKind::Docker,
                        ServiceState::Running,
                        Some("Up 2 hours · 5432"),
                    ),
                    node(
                        "redis:7",
                        ServiceKind::Docker,
                        ServiceState::Stopped,
                        Some("Exited (0) 3 days ago"),
                    ),
                ],
            ),
            group(
                "Databases",
                vec![node(
                    "demo@localhost",
                    ServiceKind::Database,
                    ServiceState::Running,
                    Some("PostgreSQL 16.4 · localhost:5432"),
                )],
            ),
        ]
    }
}

// ---------------------------------------------------------------------------
// Status
// ---------------------------------------------------------------------------

/// Reports two language servers, simulates one "Indexing" task per
/// subscriber and keeps an event log.
pub struct MockStatusService {
    config: MockConfig,
    next_task: AtomicU64,
    notifications: Mutex<Vec<NotificationEntry>>,
}

impl MockStatusService {
    pub fn new(config: MockConfig) -> Self {
        let entry =
            |id, level, title: &str, body: &str, minutes_ago: i64, read| NotificationEntry {
                id,
                level,
                title: title.into(),
                body: body.into(),
                timestamp: MOCK_NOW - minutes_ago * 60,
                read,
            };
        use NotificationLevel::*;
        Self {
            config,
            next_task: AtomicU64::new(1),
            notifications: Mutex::new(vec![
                entry(
                    6,
                    Info,
                    "Running on mock services",
                    "FerroIDE Stage 2 shows synthetic data; nothing is read from disk.",
                    1,
                    false,
                ),
                entry(
                    5,
                    Error,
                    "rust-analyzer crashed",
                    "The language server stopped unexpectedly and was restarted.",
                    4,
                    false,
                ),
                entry(
                    4,
                    Warning,
                    "Outdated dependency",
                    "toml 0.9.8 is available (Cargo.toml uses 0.9).",
                    12,
                    false,
                ),
                entry(
                    3,
                    Success,
                    "Pushed 2 commits to origin/main",
                    "main is up to date with origin/main.",
                    55,
                    true,
                ),
                entry(
                    2,
                    Success,
                    "Build succeeded",
                    "cargo build finished in 1.42 s.",
                    80,
                    true,
                ),
                entry(
                    1,
                    Info,
                    "Indexing finished",
                    "demo-app: 13 files indexed in 0.4 s.",
                    95,
                    true,
                ),
            ]),
        }
    }
}

impl StatusService for MockStatusService {
    fn language_servers(&self) -> Vec<LanguageServerStatus> {
        vec![
            LanguageServerStatus {
                name: "rust-analyzer".into(),
                language: "Rust".into(),
                health: ServerHealth::Ready,
                memory_mb: 412,
            },
            LanguageServerStatus {
                name: "vtsls".into(),
                language: "TypeScript".into(),
                health: ServerHealth::Starting,
                memory_mb: 96,
            },
        ]
    }

    fn subscribe_tasks(&self) -> async_channel::Receiver<TaskProgress> {
        let (tx, rx) = async_channel::unbounded();
        let id = self.next_task.fetch_add(1, Ordering::Relaxed);
        let step = if self.config.latency.is_zero() {
            Duration::ZERO
        } else {
            Duration::from_millis(60)
        };
        let _ = thread::Builder::new()
            .name("mock-indexing".into())
            .spawn(move || {
                for i in 0..=50u32 {
                    if !step.is_zero() {
                        thread::sleep(step);
                    }
                    let progress = TaskProgress {
                        id,
                        title_key: "task-indexing".into(),
                        fraction: Some(i as f32 / 50.0),
                        done: i == 50,
                    };
                    if tx.send_blocking(progress).is_err() {
                        return;
                    }
                }
            });
        rx
    }

    fn notifications(&self) -> Vec<NotificationEntry> {
        lock(&self.notifications).clone()
    }

    fn mark_notifications_read(&self) {
        for entry in lock(&self.notifications).iter_mut() {
            entry.read = true;
        }
    }

    fn clear_notifications(&self) {
        lock(&self.notifications).clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn block<T>(f: BoxFuture<'static, T>) -> T {
        smol::block_on(f)
    }

    #[test]
    fn demo_project_lists_directories_first() {
        let project = MockProjectService::demo(MockConfig::instant());
        let root = project.roots()[0].clone();
        let names: Vec<_> = block(project.children(&root))
            .unwrap()
            .into_iter()
            .map(|e| (e.kind, e.name))
            .collect();
        let first_file = names
            .iter()
            .position(|(k, _)| *k == EntryKind::File)
            .unwrap();
        assert!(
            names[..first_file]
                .iter()
                .all(|(k, _)| *k == EntryKind::Directory)
        );
        assert!(names.iter().any(|(_, n)| n == "src"));
        assert!(names.iter().any(|(_, n)| n == "Cargo.toml"));
        // `empty-dir` holds only a placeholder, which is hidden.
        let empty = block(project.children(&root.join("empty-dir"))).unwrap();
        assert!(empty.is_empty());
    }

    #[test]
    fn demo_files_are_readable_and_unknown_files_error() {
        let project = MockProjectService::demo(MockConfig::instant());
        let root = project.roots()[0].clone();
        let main = block(project.read_file(&root.join("src/main.rs"))).unwrap();
        assert!(main.contains("fn main"));
        assert!(block(project.read_file(&root.join("nope.rs"))).is_err());
    }

    #[test]
    fn synthetic_tree_is_deterministic_and_close_to_its_target_size() {
        let target = 100_000;
        let tree = SyntheticTree::new(PathBuf::from("/stress"), 7, target);
        let again = SyntheticTree::new(PathBuf::from("/stress"), 7, target);
        assert_eq!(
            tree.children(Path::new("/stress")).unwrap(),
            again.children(Path::new("/stress")).unwrap()
        );

        // Count every file reachable through the module directories.
        fn count(tree: &SyntheticTree, dir: &Path) -> usize {
            tree.children(dir)
                .unwrap()
                .iter()
                .map(|e| match e.kind {
                    EntryKind::File => 1,
                    EntryKind::Directory if e.name.starts_with("module_") => count(tree, &e.path),
                    EntryKind::Directory => 0,
                })
                .sum()
        }
        let total = count(&tree, Path::new("/stress"));
        assert!(
            (target..target * 2).contains(&total),
            "expected about {target} files, got {total}"
        );
    }

    #[test]
    fn synthetic_tree_contains_extreme_cases() {
        let tree = SyntheticTree::new(PathBuf::from("/s"), 1, 1000);
        let root = tree.children(Path::new("/s")).unwrap();
        assert!(root.iter().any(|e| e.name.contains('🦀')));
        assert!(root.iter().any(|e| e.name.len() > 150));
        let mut dir = PathBuf::from("/s/deep");
        let mut depth = 1;
        while let Some(next) = tree
            .children(&dir)
            .unwrap()
            .into_iter()
            .find(|e| e.is_dir())
        {
            dir = next.path;
            depth += 1;
        }
        assert!(depth >= 40, "deep chain is only {depth} levels");
        assert!(tree.children(Path::new("/s/empty")).unwrap().is_empty());
    }

    #[test]
    fn failure_rate_one_always_fails_and_latency_is_applied() {
        let config = MockConfig {
            failure_rate: 1.0,
            latency: Duration::from_millis(20),
            ..MockConfig::instant()
        };
        let project = MockProjectService::demo(config);
        let root = project.roots()[0].clone();
        let started = std::time::Instant::now();
        assert!(block(project.children(&root)).is_err());
        assert!(started.elapsed() >= Duration::from_millis(20));
    }

    #[test]
    fn run_service_reports_lifecycle() {
        let run = MockRunService::new(PathBuf::from("/p"));
        let events = run.subscribe();
        run.start("app", false).unwrap();
        assert!(run.start("app", false).is_err(), "double start must fail");
        assert_eq!(run.state("app"), RunState::Running { debug: false });
        let mut finished = false;
        while let Ok(event) = events.recv_blocking() {
            if let RunEvent::StateChanged {
                state: RunState::Finished { exit_code },
                ..
            } = event
            {
                assert_eq!(exit_code, 0);
                finished = true;
                break;
            }
        }
        assert!(finished);
        assert!(run.start("missing", false).is_err());
    }

    #[test]
    fn stopping_a_run_finishes_it_with_a_signal_code() {
        let run = MockRunService::new(PathBuf::from("/p"));
        run.start("tests", true).unwrap();
        run.stop("tests");
        assert_eq!(run.state("tests"), RunState::Finished { exit_code: 130 });
    }

    #[test]
    fn indexing_task_reaches_completion() {
        let status = MockStatusService::new(MockConfig::instant());
        let rx = status.subscribe_tasks();
        let last = std::iter::from_fn(|| rx.recv_blocking().ok())
            .last()
            .unwrap();
        assert!(last.done);
        assert_eq!(last.fraction, Some(1.0));
    }

    #[test]
    fn files_can_be_created_renamed_and_deleted() {
        let project = MockProjectService::demo(MockConfig::instant());
        let root = project.root().to_path_buf();
        let events = project.subscribe();

        let dir = root.join("src").join("ui");
        block(project.create(&dir, EntryKind::Directory)).unwrap();
        let src = block(project.children(&root.join("src"))).unwrap();
        assert!(
            src.iter().any(|e| e.name == "ui" && e.is_dir()),
            "empty dirs show"
        );

        let file = dir.join("view.rs");
        block(project.create(&file, EntryKind::File)).unwrap();
        assert!(
            block(project.create(&file, EntryKind::File)).is_err(),
            "exists"
        );
        assert_eq!(block(project.read_file(&file)).unwrap(), "");

        let renamed = root.join("src").join("widgets");
        block(project.rename(&dir, &renamed)).unwrap();
        assert!(block(project.read_file(&renamed.join("view.rs"))).is_ok());
        assert!(block(project.read_file(&file)).is_err());
        assert!(
            block(project.rename(&dir, &renamed)).is_err(),
            "source gone"
        );

        block(project.delete(&renamed)).unwrap();
        assert!(block(project.read_file(&renamed.join("view.rs"))).is_err());
        assert!(block(project.delete(&renamed)).is_err());

        let received: Vec<ProjectEvent> = std::iter::from_fn(|| events.try_recv().ok()).collect();
        assert_eq!(
            received,
            vec![
                ProjectEvent::Added(dir.clone()),
                ProjectEvent::Added(file),
                ProjectEvent::Removed(dir),
                ProjectEvent::Added(renamed.clone()),
                ProjectEvent::Removed(renamed),
            ]
        );
    }

    #[test]
    fn the_stress_tree_is_read_only_and_indexed() {
        let project = MockProjectService::demo(MockConfig {
            stress_files: 2_000,
            ..MockConfig::instant()
        });
        let target = project.root().join("stress").join("new.rs");
        let err = block(project.create(&target, EntryKind::File)).unwrap_err();
        assert!(err.to_string().contains("read-only"), "{err}");
        let stress = project.stress_files();
        assert!(stress.len() >= 2_000);
        assert!(stress.iter().all(|p| p.starts_with("stress/")));
        assert!(Arc::ptr_eq(&stress, &project.stress_files()), "built once");
        let all = project.all_files();
        assert!(all.iter().any(|p| p.ends_with("main.rs")));
        assert!(!all.iter().any(|p| p.ends_with(".keep")));
        let some = project.absolute(&stress[0]);
        assert!(project.file_text(&some).is_some());
    }

    #[test]
    fn run_configurations_can_be_saved_and_deleted() {
        let run = MockRunService::new(PathBuf::from("/p"));
        let mut config = run.configurations()[0].clone();
        config.arguments = "--verbose".into();
        run.save_configuration(config.clone()).unwrap();
        assert_eq!(run.configurations()[0].arguments, "--verbose");

        let new = RunConfiguration {
            id: "bench".into(),
            name: "Benchmarks".into(),
            kind: "cargo".into(),
            command: "cargo bench".into(),
            ..Default::default()
        };
        run.save_configuration(new).unwrap();
        assert_eq!(run.configurations().len(), 5);
        let clash = RunConfiguration {
            id: "other".into(),
            name: "Benchmarks".into(),
            command: "x".into(),
            ..Default::default()
        };
        assert!(run.save_configuration(clash).is_err(), "duplicate name");
        assert!(
            run.save_configuration(RunConfiguration::default()).is_err(),
            "empty"
        );
        run.delete_configuration("bench");
        assert_eq!(run.configurations().len(), 4);
    }

    #[test]
    fn test_results_and_services_have_the_expected_shape() {
        let run = MockRunService::new(PathBuf::from("/p"));
        let tree = run.test_results();
        assert_eq!(tree[0].status, TestStatus::Failed, "a failure bubbles up");
        let smoke = &tree[0].children[1].children[0];
        assert_eq!(smoke.status, TestStatus::Failed);
        assert!(!smoke.output.is_empty());
        assert!(smoke.location.is_some());

        let services = run.services();
        let names: Vec<_> = services.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, ["Run Configurations", "Docker", "Databases"]);
        assert_eq!(services[0].children.len(), 4);
        run.start("app", false).unwrap();
        assert_eq!(run.services()[0].children[0].state, ServiceState::Running);
        run.stop("app");
    }

    #[test]
    fn notifications_can_be_read_and_cleared() {
        let status = MockStatusService::new(MockConfig::instant());
        let all = status.notifications();
        assert!(all.len() >= 6);
        assert!(all.iter().any(|n| !n.read));
        assert!(all.windows(2).all(|w| w[0].timestamp >= w[1].timestamp));
        status.mark_notifications_read();
        assert!(status.notifications().iter().all(|n| n.read));
        status.clear_notifications();
        assert!(status.notifications().is_empty());
    }
}
