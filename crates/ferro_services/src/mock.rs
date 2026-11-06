//! Mock implementations of every service, driven by deterministic synthetic
//! data.
//!
//! The mocks are not toys: they deliberately include the cases real projects
//! produce rarely and UIs get wrong — 100,000-file trees, 200-character file
//! names, emoji and right-to-left text, nesting 40 levels deep, empty
//! directories, slow responses and failures.

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
    thread,
    time::Duration,
};

use anyhow::{anyhow, bail};
use futures::{FutureExt as _, future::BoxFuture};

use crate::{
    Branch, Entry, EntryKind, LanguageServerStatus, ProjectEvent, ProjectService, RecentProject,
    RecentProjectsService, Result, RunConfiguration, RunEvent, RunService, RunState, ServerHealth,
    StatusService, TaskProgress, VcsService,
};

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
    files: HashMap<PathBuf, &'static str>,
    tree: SyntheticTree,
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
        let files = DEMO_FILES
            .iter()
            .map(|(rel, text)| (root.join(rel), *text))
            .collect();
        Self {
            tree: SyntheticTree::new(root.join("stress"), config.seed, config.stress_files),
            dice: Dice::new(config.seed),
            root,
            files,
            config,
            subscribers: Mutex::new(Vec::new()),
        }
    }

    fn should_fail(&self) -> bool {
        self.config.failure_rate > 0.0 && self.dice.roll() < self.config.failure_rate
    }

    /// Children of a directory of the hand-written part of the demo.
    fn demo_children(&self, dir: &Path) -> Vec<Entry> {
        let mut entries: HashMap<PathBuf, Entry> = HashMap::new();
        for (path, text) in &self.files {
            let Ok(rel) = path.strip_prefix(dir) else {
                continue;
            };
            let mut parts = rel.components();
            let Some(first) = parts.next() else { continue };
            let child = dir.join(first);
            let is_dir = parts.next().is_some();
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
                size: if is_dir { 0 } else { text.len() as u64 },
            });
        }
        if dir == self.root && self.config.stress_files > 0 {
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

    /// Notify subscribers, as a file watcher would. Used by the storybook.
    pub fn emit(&self, event: ProjectEvent) {
        let mut subscribers = self.subscribers.lock().unwrap_or_else(|e| e.into_inner());
        subscribers.retain(|tx| tx.try_send(event.clone()).is_ok() || !tx.is_closed());
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
        let result = if let Some(text) = self.files.get(path) {
            Ok(text.to_string())
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
// VCS
// ---------------------------------------------------------------------------

pub struct MockVcsService {
    current: Mutex<String>,
    branches: Vec<Branch>,
    config: MockConfig,
    dice: Dice,
}

impl MockVcsService {
    pub fn new(config: MockConfig) -> Self {
        let local = |name: &str, ahead, behind| Branch {
            name: name.into(),
            is_remote: false,
            ahead,
            behind,
        };
        let remote = |name: &str| Branch {
            name: name.into(),
            is_remote: true,
            ahead: 0,
            behind: 0,
        };
        Self {
            current: Mutex::new("main".into()),
            branches: vec![
                local("main", 0, 2),
                local("develop", 3, 0),
                local("feature/FERRO-101-dock-layout", 5, 1),
                local("fix/crash-on-empty-workspace", 1, 0),
                remote("origin/main"),
                remote("origin/develop"),
                remote("origin/release/1.0"),
            ],
            dice: Dice::new(config.seed ^ 0xbeef),
            config,
        }
    }
}

impl VcsService for MockVcsService {
    fn current_branch(&self) -> BoxFuture<'static, Result<Branch>> {
        let name = self
            .current
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        let branch = self
            .branches
            .iter()
            .find(|b| b.name == name)
            .cloned()
            .ok_or_else(|| anyhow!("branch {name} vanished"));
        let latency = self.config.latency;
        async move {
            simulate(latency, false, "").await?;
            branch
        }
        .boxed()
    }

    fn branches(&self) -> BoxFuture<'static, Result<Vec<Branch>>> {
        let fail = self.config.failure_rate > 0.0 && self.dice.roll() < self.config.failure_rate;
        let branches = self.branches.clone();
        let latency = self.config.latency;
        async move {
            simulate(latency, fail, "listing branches").await?;
            Ok(branches)
        }
        .boxed()
    }

    fn checkout(&self, name: &str) -> BoxFuture<'static, Result<()>> {
        let exists = self.branches.iter().any(|b| b.name == name && !b.is_remote);
        if exists {
            *self.current.lock().unwrap_or_else(|e| e.into_inner()) = name.to_string();
        }
        let name = name.to_string();
        let latency = self.config.latency * 3;
        async move {
            simulate(latency, false, "").await?;
            if exists {
                Ok(())
            } else {
                Err(anyhow!("no local branch {name}"))
            }
        }
        .boxed()
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
    configurations: Vec<RunConfiguration>,
    inner: Arc<Mutex<RunInner>>,
}

impl Default for MockRunService {
    fn default() -> Self {
        Self::new()
    }
}

impl MockRunService {
    pub fn new() -> Self {
        let config = |id: &str, name: &str, kind: &str, command: &str| RunConfiguration {
            id: id.into(),
            name: name.into(),
            kind: kind.into(),
            command: command.into(),
        };
        Self {
            configurations: vec![
                config("app", "demo-app", "cargo", "cargo run"),
                config("tests", "All tests", "cargo", "cargo nextest run"),
                config("web", "web: dev server", "npm", "npm run dev"),
                config("smoke", "smoke.rs", "cargo", "cargo test --test smoke"),
            ],
            inner: Arc::new(Mutex::new(RunInner {
                states: HashMap::new(),
                generation: HashMap::new(),
                subscribers: Vec::new(),
            })),
        }
    }

    fn broadcast(inner: &Mutex<RunInner>, event: RunEvent) {
        let mut inner = inner.lock().unwrap_or_else(|e| e.into_inner());
        if let RunEvent::StateChanged { id, state } = &event {
            inner.states.insert(id.clone(), *state);
        }
        inner
            .subscribers
            .retain(|tx| tx.try_send(event.clone()).is_ok() || !tx.is_closed());
    }
}

impl RunService for MockRunService {
    fn configurations(&self) -> Vec<RunConfiguration> {
        self.configurations.clone()
    }

    fn state(&self, id: &str) -> RunState {
        self.inner
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .states
            .get(id)
            .copied()
            .unwrap_or(RunState::Idle)
    }

    fn start(&self, id: &str, debug: bool) -> Result<()> {
        let config = self
            .configurations
            .iter()
            .find(|c| c.id == id)
            .ok_or_else(|| anyhow!("unknown run configuration {id}"))?
            .clone();
        if matches!(self.state(id), RunState::Running { .. }) {
            bail!("{} is already running", config.name);
        }
        let generation = {
            let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
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
                    let inner = inner.lock().unwrap_or_else(|e| e.into_inner());
                    inner.generation.get(&id) == Some(&generation)
                        && matches!(inner.states.get(&id), Some(RunState::Running { .. }))
                };
                let lines = [
                    format!("$ {}", config.command),
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
        self.inner
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .subscribers
            .push(tx);
        rx
    }
}

// ---------------------------------------------------------------------------
// Status
// ---------------------------------------------------------------------------

/// Reports two language servers and simulates one "Indexing" task per
/// subscriber.
pub struct MockStatusService {
    config: MockConfig,
    next_task: AtomicU64,
}

impl MockStatusService {
    pub fn new(config: MockConfig) -> Self {
        Self {
            config,
            next_task: AtomicU64::new(1),
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
        let run = MockRunService::new();
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
        let run = MockRunService::new();
        run.start("tests", true).unwrap();
        run.stop("tests");
        assert_eq!(run.state("tests"), RunState::Finished { exit_code: 130 });
    }

    #[test]
    fn vcs_checkout_switches_branch() {
        let vcs = MockVcsService::new(MockConfig::instant());
        assert_eq!(block(vcs.current_branch()).unwrap().name, "main");
        block(vcs.checkout("develop")).unwrap();
        assert_eq!(block(vcs.current_branch()).unwrap().name, "develop");
        assert!(block(vcs.checkout("origin/main")).is_err());
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
}
