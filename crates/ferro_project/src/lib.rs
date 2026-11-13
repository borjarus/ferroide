//! The files of a real project (plan §8, Stage 4).
//!
//! [`FsProjectService`] implements the `ProjectService` contract on the file
//! system, so the views built on mocks in Stages 1–2 work on real projects
//! unchanged:
//!
//! - directory listings respect `.gitignore`, `.ignore` and
//!   `.git/info/exclude` (the `ignore` crate) and never show `.git`;
//! - a background scan builds a [`FileIndex`] of every file for Quick Open
//!   and Find in Files ([`FsSearchService`]);
//! - a [`notify`] watcher per root, with a short debounce, reports changes
//!   made outside the IDE well under the 200 ms budget;
//! - deleting moves to the system trash, saving is atomic
//!   (`ferro_text::file::write_atomic`);
//! - one window can hold several root directories (the equivalent of
//!   IntelliJ modules).

use std::{
    collections::HashMap,
    fs,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, RwLock, Weak},
    time::Duration,
};

use anyhow::{Context as _, Result, anyhow, bail};
use ferro_services::{Entry, EntryKind, ProjectEvent, ProjectService};
use futures::{FutureExt as _, future::BoxFuture};

mod index;
mod search;
mod watch;

pub use index::{FileIndex, IndexedFile};
pub use search::FsSearchService;
pub use watch::{DEFAULT_DEBOUNCE, FileWatcher, IgnoreFilter};

/// Options of a [`FsProjectService`].
#[derive(Debug, Clone)]
pub struct ProjectOptions {
    /// Show files matched by `.gitignore` in listings (they are still left
    /// out of the index and search).
    pub show_ignored: bool,
    /// Start file watchers. Tests that only list files turn it off.
    pub watch: bool,
    /// How long the watcher waits for a burst of changes to settle.
    pub debounce: Duration,
}

impl Default for ProjectOptions {
    fn default() -> Self {
        Self {
            show_ignored: false,
            watch: true,
            debounce: DEFAULT_DEBOUNCE,
        }
    }
}

/// A project on the file system.
#[derive(Clone)]
pub struct FsProjectService {
    inner: Arc<Inner>,
}

struct Inner {
    roots: RwLock<Vec<PathBuf>>,
    options: ProjectOptions,
    subscribers: Mutex<Vec<async_channel::Sender<ProjectEvent>>>,
    watchers: Mutex<HashMap<PathBuf, FileWatcher>>,
    filters: RwLock<HashMap<PathBuf, Arc<IgnoreFilter>>>,
    index: FileIndex,
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|e| e.into_inner())
}

impl FsProjectService {
    /// Open the project rooted at `root`. The scan of its files starts in
    /// the background; listings work immediately.
    pub fn open(root: impl Into<PathBuf>) -> Result<Self> {
        Self::open_with(vec![root.into()], ProjectOptions::default())
    }

    /// Open a project with several root directories.
    pub fn open_with(roots: Vec<PathBuf>, options: ProjectOptions) -> Result<Self> {
        if roots.is_empty() {
            bail!("a project needs at least one root directory");
        }
        let this = Self {
            inner: Arc::new(Inner {
                roots: RwLock::new(Vec::new()),
                options,
                subscribers: Mutex::new(Vec::new()),
                watchers: Mutex::new(HashMap::new()),
                filters: RwLock::new(HashMap::new()),
                index: FileIndex::new(),
            }),
        };
        for root in roots {
            this.attach_root(root)?;
        }
        Ok(this)
    }

    /// The index of every (not ignored) file, filled by the background scan.
    pub fn index(&self) -> &FileIndex {
        &self.inner.index
    }

    /// Delete without the trash. The UI uses [`ProjectService::delete`].
    pub fn delete_permanently(&self, path: &Path) -> Result<()> {
        let path = path.to_path_buf();
        self.inner.check_inside(&path)?;
        let meta = fs::symlink_metadata(&path).with_context(|| path.display().to_string())?;
        if meta.is_dir() {
            fs::remove_dir_all(&path)
        } else {
            fs::remove_file(&path)
        }
        .with_context(|| format!("deleting {}", path.display()))?;
        self.inner.apply(vec![ProjectEvent::Removed(path)]);
        Ok(())
    }

    fn attach_root(&self, root: PathBuf) -> Result<PathBuf> {
        let root = canonical(&root)?;
        if !root.is_dir() {
            bail!("{} is not a directory", root.display());
        }
        {
            let mut roots = self.inner.roots.write().unwrap_or_else(|e| e.into_inner());
            if roots
                .iter()
                .any(|r| root.starts_with(r) || r.starts_with(&root))
            {
                bail!("{} overlaps a folder of the project", root.display());
            }
            roots.push(root.clone());
        }
        let filter = Arc::new(IgnoreFilter::new(&root));
        self.inner
            .filters
            .write()
            .unwrap_or_else(|e| e.into_inner())
            .insert(root.clone(), filter.clone());
        self.inner.index.scan_root(root.clone());
        if self.inner.options.watch {
            // Registering a recursive watch walks the whole tree (one inotify
            // watch per directory on Linux), so it must not delay the first
            // listing.
            let weak: Weak<Inner> = Arc::downgrade(&self.inner);
            let watched = root.clone();
            let spawned = std::thread::Builder::new()
                .name("ferro-watch-setup".into())
                .spawn(move || start_watcher(weak, watched, filter));
            if let Err(err) = spawned {
                tracing::warn!("cannot watch {}: {err}", root.display());
            }
        }
        Ok(root)
    }

    /// Whether every root has a running file watcher.
    pub fn is_watching(&self) -> bool {
        let watchers = lock(&self.inner.watchers);
        self.inner.roots().iter().all(|r| watchers.contains_key(r))
    }
}

fn start_watcher(weak: Weak<Inner>, root: PathBuf, filter: Arc<IgnoreFilter>) {
    let Some(debounce) = weak.upgrade().map(|inner| inner.options.debounce) else {
        return;
    };
    let sink = weak.clone();
    let watcher = FileWatcher::new(&root, filter, debounce, move |events| {
        if let Some(inner) = sink.upgrade() {
            inner.apply(events);
        }
    });
    match watcher {
        Ok(watcher) => {
            if let Some(inner) = weak.upgrade()
                && inner.roots().contains(&root)
            {
                lock(&inner.watchers).insert(root, watcher);
            }
        }
        // The project still works, it just does not see outside changes.
        Err(err) => tracing::warn!("cannot watch {}: {err:#}", root.display()),
    }
}

impl Inner {
    fn roots(&self) -> Vec<PathBuf> {
        self.roots.read().unwrap_or_else(|e| e.into_inner()).clone()
    }

    fn root_of(&self, path: &Path) -> Option<PathBuf> {
        self.roots().into_iter().find(|r| path.starts_with(r))
    }

    fn check_inside(&self, path: &Path) -> Result<()> {
        if self.root_of(path).is_none() {
            bail!("{} is outside the project", path.display());
        }
        Ok(())
    }

    /// Update the index and notify subscribers.
    fn apply(&self, events: Vec<ProjectEvent>) {
        if events.is_empty() {
            return;
        }
        {
            let filters = self.filters.read().unwrap_or_else(|e| e.into_inner());
            let ignored = |path: &Path| {
                filters
                    .iter()
                    .find(|(root, _)| path.starts_with(root))
                    .is_none_or(|(_, filter)| filter.is_ignored(path))
            };
            for event in &events {
                self.index.apply(event, |path| self.root_of(path), ignored);
            }
        }
        let mut subscribers = lock(&self.subscribers);
        subscribers.retain(|tx| !tx.is_closed());
        for event in events {
            for tx in subscribers.iter() {
                let _ = tx.try_send(event.clone());
            }
        }
    }
}

/// `path` made absolute and free of `..`, without resolving a final symlink
/// on platforms where that would surprise (Windows `\\?\` prefixes).
fn canonical(path: &Path) -> Result<PathBuf> {
    let canonical = fs::canonicalize(path).with_context(|| path.display().to_string())?;
    Ok(strip_verbatim(canonical))
}

/// Remove Windows' `\\?\` prefix that `canonicalize` adds, so paths match
/// the ones the user and other tools see.
fn strip_verbatim(path: PathBuf) -> PathBuf {
    #[cfg(windows)]
    {
        let text = path.to_string_lossy();
        if let Some(rest) = text.strip_prefix(r"\\?\UNC\") {
            return PathBuf::from(format!(r"\\{rest}"));
        }
        if let Some(rest) = text.strip_prefix(r"\\?\") {
            return PathBuf::from(rest);
        }
    }
    path
}

/// Directories first, then by name, case-insensitively (as the mock and
/// IntelliJ sort).
pub fn sort_entries(entries: &mut [Entry]) {
    entries.sort_by(|a, b| {
        a.kind
            .cmp(&b.kind)
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
            .then_with(|| a.name.cmp(&b.name))
    });
}

/// The direct children of `dir`.
pub fn list_dir(dir: &Path, show_ignored: bool) -> Result<Vec<Entry>> {
    let walker = ignore::WalkBuilder::new(dir)
        .max_depth(Some(1))
        .hidden(false)
        .parents(true)
        .ignore(!show_ignored)
        .git_ignore(!show_ignored)
        .git_exclude(!show_ignored)
        .git_global(false)
        .require_git(false)
        .follow_links(false)
        .filter_entry(|e| e.file_name() != ".git")
        .build();
    let mut entries = Vec::new();
    for item in walker {
        let item = match item {
            Ok(item) => item,
            // A broken .gitignore or an unreadable child: skip it.
            Err(err) => {
                tracing::debug!("listing {}: {err}", dir.display());
                continue;
            }
        };
        if item.depth() == 0 {
            if !item.file_type().is_some_and(|t| t.is_dir()) {
                bail!("{} is not a directory", dir.display());
            }
            continue;
        }
        let path = item.path().to_path_buf();
        // Follow symlinks for the kind, so a linked directory expands.
        let meta = fs::metadata(&path).or_else(|_| fs::symlink_metadata(&path));
        let (kind, size) = match meta {
            Ok(m) if m.is_dir() => (EntryKind::Directory, 0),
            Ok(m) => (EntryKind::File, m.len()),
            Err(_) => continue,
        };
        entries.push(Entry {
            name: item.file_name().to_string_lossy().into_owned(),
            path,
            kind,
            size,
        });
    }
    if entries.is_empty() && !dir.is_dir() {
        bail!("{} is not a directory", dir.display());
    }
    sort_entries(&mut entries);
    Ok(entries)
}

fn unblock<T: Send + 'static>(
    f: impl FnOnce() -> Result<T> + Send + 'static,
) -> BoxFuture<'static, Result<T>> {
    smol::unblock(f).boxed()
}

impl ProjectService for FsProjectService {
    fn name(&self) -> String {
        let roots = self.inner.roots();
        roots
            .first()
            .and_then(|r| r.file_name())
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "project".into())
    }

    fn roots(&self) -> Vec<PathBuf> {
        self.inner.roots()
    }

    /// The first root's README, if it has one.
    fn initial_files(&self) -> Vec<PathBuf> {
        let Some(root) = self.inner.roots().into_iter().next() else {
            return Vec::new();
        };
        ["README.md", "README", "readme.md", "README.txt"]
            .iter()
            .map(|name| root.join(name))
            .find(|path| path.is_file())
            .into_iter()
            .collect()
    }

    fn children(&self, dir: &Path) -> BoxFuture<'static, Result<Vec<Entry>>> {
        let dir = dir.to_path_buf();
        let show_ignored = self.inner.options.show_ignored;
        unblock(move || list_dir(&dir, show_ignored))
    }

    fn read_file(&self, path: &Path) -> BoxFuture<'static, Result<String>> {
        let path = path.to_path_buf();
        unblock(move || {
            let bytes = fs::read(&path).with_context(|| format!("reading {}", path.display()))?;
            String::from_utf8(bytes).map_err(|_| anyhow!("{} is not UTF-8 text", path.display()))
        })
    }

    fn read_bytes(&self, path: &Path) -> BoxFuture<'static, Result<Vec<u8>>> {
        let path = path.to_path_buf();
        unblock(move || fs::read(&path).with_context(|| format!("reading {}", path.display())))
    }

    fn write_file(&self, path: &Path, bytes: Vec<u8>) -> BoxFuture<'static, Result<()>> {
        let path = path.to_path_buf();
        let inner = self.inner.clone();
        unblock(move || {
            let existed = path.exists();
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent)
                    .with_context(|| format!("creating {}", parent.display()))?;
            }
            ferro_text::file::write_atomic(&path, &bytes)
                .with_context(|| format!("saving {}", path.display()))?;
            inner.apply(vec![if existed {
                ProjectEvent::Changed(path)
            } else {
                ProjectEvent::Added(path)
            }]);
            Ok(())
        })
    }

    fn subscribe(&self) -> async_channel::Receiver<ProjectEvent> {
        let (tx, rx) = async_channel::unbounded();
        lock(&self.inner.subscribers).push(tx);
        rx
    }

    fn create(&self, path: &Path, kind: EntryKind) -> BoxFuture<'static, Result<()>> {
        let path = path.to_path_buf();
        let inner = self.inner.clone();
        unblock(move || {
            inner.check_inside(&path)?;
            if fs::symlink_metadata(&path).is_ok() {
                bail!("{} already exists", path.display());
            }
            match kind {
                EntryKind::Directory => fs::create_dir_all(&path),
                EntryKind::File => {
                    if let Some(parent) = path.parent() {
                        fs::create_dir_all(parent)?;
                    }
                    fs::OpenOptions::new()
                        .write(true)
                        .create_new(true)
                        .open(&path)
                        .map(drop)
                }
            }
            .with_context(|| format!("creating {}", path.display()))?;
            inner.apply(vec![ProjectEvent::Added(path)]);
            Ok(())
        })
    }

    fn rename(&self, from: &Path, to: &Path) -> BoxFuture<'static, Result<()>> {
        let (from, to) = (from.to_path_buf(), to.to_path_buf());
        let inner = self.inner.clone();
        unblock(move || {
            inner.check_inside(&from)?;
            inner.check_inside(&to)?;
            if to.starts_with(&from) && to != from {
                bail!("cannot move {} into itself", from.display());
            }
            // A case-only rename on a case-insensitive file system is the same file.
            let same_file =
                from.to_string_lossy().to_lowercase() == to.to_string_lossy().to_lowercase();
            if fs::symlink_metadata(&to).is_ok() && !same_file {
                bail!("{} already exists", to.display());
            }
            if let Some(parent) = to.parent() {
                fs::create_dir_all(parent)?;
            }
            fs::rename(&from, &to)
                .with_context(|| format!("moving {} to {}", from.display(), to.display()))?;
            inner.apply(vec![ProjectEvent::Removed(from), ProjectEvent::Added(to)]);
            Ok(())
        })
    }

    fn delete(&self, path: &Path) -> BoxFuture<'static, Result<()>> {
        let path = path.to_path_buf();
        let inner = self.inner.clone();
        unblock(move || {
            inner.check_inside(&path)?;
            if inner.roots().contains(&path) {
                bail!("a root folder is removed from the project, not deleted");
            }
            trash::delete(&path)
                .map_err(|err| anyhow!("could not move {} to the trash: {err}", path.display()))?;
            inner.apply(vec![ProjectEvent::Removed(path)]);
            Ok(())
        })
    }

    fn add_root(&self, path: &Path) -> Result<PathBuf> {
        let root = self.attach_root(path.to_path_buf())?;
        self.inner.apply(vec![ProjectEvent::RootsChanged]);
        Ok(root)
    }

    fn remove_root(&self, path: &Path) -> Result<()> {
        {
            let mut roots = self.inner.roots.write().unwrap_or_else(|e| e.into_inner());
            if roots.len() == 1 {
                bail!("the last folder of a project cannot be removed");
            }
            let before = roots.len();
            roots.retain(|r| r != path);
            if roots.len() == before {
                bail!("{} is not a folder of the project", path.display());
            }
        }
        lock(&self.inner.watchers).remove(path);
        self.inner
            .filters
            .write()
            .unwrap_or_else(|e| e.into_inner())
            .remove(path);
        self.inner.index.remove_root(path);
        self.inner.apply(vec![ProjectEvent::RootsChanged]);
        Ok(())
    }
}

#[cfg(test)]
mod tests;
