//! The index of a project's files, for Quick Open and Find in Files.
//!
//! Each root is scanned in the background with the `ignore` crate's
//! parallel walker (respecting `.gitignore`); afterwards the watcher's events
//! keep the index current. Readers take a cheap snapshot (`Arc<Vec<_>>`);
//! writers copy on write.

use std::{
    path::{Path, PathBuf},
    sync::{Arc, Mutex, RwLock},
    thread,
    time::{Duration, Instant},
};

use ferro_services::ProjectEvent;

/// One file of the index.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexedFile {
    pub path: PathBuf,
    /// The root the file belongs to.
    pub root: PathBuf,
    /// Relative to its root, `/`-separated.
    pub relative: String,
}

#[derive(Default)]
struct State {
    files: Arc<Vec<IndexedFile>>,
    /// Roots whose scan is still running.
    scanning: Vec<PathBuf>,
    /// Woken when the last scan finishes: receivers see a closed channel.
    ready: Option<async_channel::Sender<()>>,
    ready_rx: Option<async_channel::Receiver<()>>,
    /// How long the last finished scan took.
    last_scan: Option<Duration>,
}

#[derive(Clone, Default)]
pub struct FileIndex {
    state: Arc<RwLock<State>>,
    /// Serializes scans so their results merge in order.
    scan_lock: Arc<Mutex<()>>,
}

fn relative(root: &Path, path: &Path) -> Option<String> {
    let rel = path.strip_prefix(root).ok()?;
    let text = rel.to_string_lossy();
    if text.is_empty() {
        return None;
    }
    Some(if std::path::MAIN_SEPARATOR == '/' {
        text.into_owned()
    } else {
        text.replace(std::path::MAIN_SEPARATOR, "/")
    })
}

/// Every file under `dir` that `.gitignore` does not exclude.
pub fn walk_files(root: &Path, dir: &Path) -> Vec<IndexedFile> {
    let files = Mutex::new(Vec::new());
    ignore::WalkBuilder::new(dir)
        .hidden(false)
        .parents(true)
        .git_global(false)
        .require_git(false)
        .follow_links(false)
        .filter_entry(|e| e.file_name() != ".git")
        .threads(
            thread::available_parallelism()
                .map_or(4, |n| n.get())
                .min(8),
        )
        .build_parallel()
        .run(|| {
            let files = &files;
            Box::new(move |item| {
                if let Ok(item) = item
                    && item.file_type().is_some_and(|t| t.is_file())
                    && let Some(rel) = relative(root, item.path())
                {
                    files
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .push(IndexedFile {
                            path: item.path().to_path_buf(),
                            root: root.to_path_buf(),
                            relative: rel,
                        });
                }
                ignore::WalkState::Continue
            })
        });
    let mut files = files.into_inner().unwrap_or_else(|e| e.into_inner());
    files.sort_by(|a, b| a.relative.cmp(&b.relative));
    files
}

impl FileIndex {
    pub fn new() -> Self {
        Self::default()
    }

    /// Scan `root` on a background thread and merge its files in.
    pub fn scan_root(&self, root: PathBuf) {
        {
            let mut state = self.write();
            state.scanning.push(root.clone());
            if state.ready.is_none() {
                let (tx, rx) = async_channel::bounded(1);
                state.ready = Some(tx);
                state.ready_rx = Some(rx);
            }
        }
        let this = self.clone();
        let spawned = thread::Builder::new()
            .name("ferro-scan".into())
            .spawn(move || this.scan_now(&root));
        if let Err(err) = spawned {
            tracing::error!("cannot start the project scan: {err}");
        }
    }

    fn scan_now(&self, root: &Path) {
        let _serial = self.scan_lock.lock().unwrap_or_else(|e| e.into_inner());
        let started = Instant::now();
        let found = walk_files(root, root);
        let elapsed = started.elapsed();
        tracing::info!(
            target: "ferro::project",
            "indexed {} files of {} in {:.0} ms",
            found.len(),
            root.display(),
            elapsed.as_secs_f64() * 1000.0
        );
        let mut state = self.write();
        let mut files: Vec<IndexedFile> = state
            .files
            .iter()
            .filter(|f| f.root != root)
            .cloned()
            .collect();
        files.extend(found);
        files.sort_by(|a, b| (&a.root, &a.relative).cmp(&(&b.root, &b.relative)));
        state.files = Arc::new(files);
        state.scanning.retain(|r| r != root);
        state.last_scan = Some(elapsed);
        if state.scanning.is_empty() {
            // Dropping the sender closes the channel and wakes every waiter.
            state.ready = None;
            state.ready_rx = None;
        }
    }

    pub fn remove_root(&self, root: &Path) {
        let mut state = self.write();
        let files: Vec<IndexedFile> = state
            .files
            .iter()
            .filter(|f| f.root != root)
            .cloned()
            .collect();
        state.files = Arc::new(files);
    }

    /// The files indexed so far.
    pub fn snapshot(&self) -> Arc<Vec<IndexedFile>> {
        self.read().files.clone()
    }

    pub fn is_scanning(&self) -> bool {
        !self.read().scanning.is_empty()
    }

    /// The duration of the last finished scan.
    pub fn last_scan(&self) -> Option<Duration> {
        self.read().last_scan
    }

    /// Resolves once no scan is running.
    pub async fn ready(&self) {
        let rx = self.read().ready_rx.clone();
        if let Some(rx) = rx {
            // Never sent to: returns when the sender is dropped.
            let _ = rx.recv().await;
        }
    }

    /// Keep the index current after a change on disk.
    ///
    /// `root_of` finds the root of a path and `ignored` tells whether the
    /// ignore files exclude it.
    pub fn apply(
        &self,
        event: &ProjectEvent,
        root_of: impl Fn(&Path) -> Option<PathBuf>,
        ignored: impl Fn(&Path) -> bool,
    ) {
        match event {
            ProjectEvent::Added(path) | ProjectEvent::Changed(path) => {
                let Some(root) = root_of(path) else { return };
                if ignored(path) {
                    return;
                }
                let added = if path.is_dir() {
                    walk_files(&root, path)
                } else if path.is_file() {
                    relative(&root, path)
                        .map(|rel| {
                            vec![IndexedFile {
                                path: path.clone(),
                                root: root.clone(),
                                relative: rel,
                            }]
                        })
                        .unwrap_or_default()
                } else {
                    Vec::new()
                };
                if added.is_empty() {
                    return;
                }
                let mut state = self.write();
                let files = Arc::make_mut(&mut state.files);
                for file in added {
                    if let Err(ix) = files.binary_search_by(|f| {
                        (&f.root, &f.relative).cmp(&(&file.root, &file.relative))
                    }) {
                        files.insert(ix, file);
                    }
                }
            }
            ProjectEvent::Removed(path) => {
                let mut state = self.write();
                if state.files.iter().any(|f| f.path.starts_with(path)) {
                    Arc::make_mut(&mut state.files).retain(|f| !f.path.starts_with(path));
                }
            }
            ProjectEvent::RootsChanged => {}
        }
    }

    fn read(&self) -> std::sync::RwLockReadGuard<'_, State> {
        self.state.read().unwrap_or_else(|e| e.into_inner())
    }

    fn write(&self) -> std::sync::RwLockWriteGuard<'_, State> {
        self.state.write().unwrap_or_else(|e| e.into_inner())
    }
}
