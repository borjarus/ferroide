//! A file watcher with a debouncer.
//!
//! [`notify`] delivers raw events per operation; saving a file in another
//! editor can produce half a dozen (temporary file created, written, renamed
//! over the original, metadata touched). The watcher collects them until the
//! burst has been quiet for the debounce interval (capped at four times
//! that), then reports one [`ProjectEvent`] per path, decided by whether the
//! path exists when the burst settles:
//!
//! - it exists and was created during the burst → `Added`;
//! - it exists otherwise → `Changed`;
//! - it is gone and the burst started by creating it → nothing (a
//!   temporary file);
//! - it is gone otherwise → `Removed`.
//!
//! Paths inside `.git` and paths matched by the root's `.gitignore` are
//! dropped, so a build writing to `target/` does not flood the IDE.

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{
        Arc, RwLock,
        mpsc::{self, RecvTimeoutError},
    },
    thread,
    time::{Duration, Instant},
};

use anyhow::{Context as _, Result};
use ferro_services::ProjectEvent;
use ignore::gitignore::{Gitignore, GitignoreBuilder};
use notify::{
    EventKind, RecommendedWatcher, RecursiveMode, Watcher as _,
    event::{CreateKind, ModifyKind, RenameMode},
};

/// Short enough to keep the 200 ms "change visible" budget with room for
/// the platform's own latency.
pub const DEFAULT_DEBOUNCE: Duration = Duration::from_millis(50);

/// Which paths under a root are ignored: `.git` and `.gitignore` matches.
pub struct IgnoreFilter {
    root: PathBuf,
    matcher: RwLock<Gitignore>,
}

impl IgnoreFilter {
    pub fn new(root: &Path) -> Self {
        Self {
            root: root.to_path_buf(),
            matcher: RwLock::new(build_matcher(root)),
        }
    }

    /// Re-read `.gitignore` after it changed.
    pub fn reload(&self) {
        *self.matcher.write().unwrap_or_else(|e| e.into_inner()) = build_matcher(&self.root);
    }

    pub fn is_ignored(&self, path: &Path) -> bool {
        let Ok(relative) = path.strip_prefix(&self.root) else {
            return true;
        };
        if relative.components().any(|c| c.as_os_str() == ".git") {
            return true;
        }
        // Project settings reload live even when `.ferroide/` is ignored.
        if relative.starts_with(".ferroide") {
            return false;
        }
        if relative.as_os_str().is_empty() {
            return false;
        }
        let is_dir = path.is_dir();
        self.matcher
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .matched_path_or_any_parents(relative, is_dir)
            .is_ignore()
    }
}

fn build_matcher(root: &Path) -> Gitignore {
    let mut builder = GitignoreBuilder::new(root);
    for file in [".gitignore", ".ignore", ".git/info/exclude"] {
        let path = root.join(file);
        if path.is_file()
            && let Some(err) = builder.add(&path)
        {
            tracing::debug!("{}: {err}", path.display());
        }
    }
    builder.build().unwrap_or_else(|err| {
        tracing::warn!("ignoring the ignore files of {}: {err}", root.display());
        Gitignore::empty()
    })
}

/// Watches one directory tree; dropping it stops the watcher.
pub struct FileWatcher {
    // Dropping the watcher closes the channel, which ends the thread.
    _watcher: RecommendedWatcher,
}

#[derive(Default, Clone, Copy)]
struct Seen {
    created: bool,
    /// The burst's first event for this path was a creation.
    created_first: bool,
}

impl FileWatcher {
    /// Watch `root` recursively and call `sink` with each settled burst.
    pub fn new(
        root: &Path,
        filter: Arc<IgnoreFilter>,
        debounce: Duration,
        sink: impl Fn(Vec<ProjectEvent>) + Send + 'static,
    ) -> Result<Self> {
        let (tx, rx) = mpsc::channel::<notify::Result<notify::Event>>();
        let mut watcher = notify::recommended_watcher(move |event| {
            let _ = tx.send(event);
        })
        .context("starting the file watcher")?;
        watcher
            .watch(root, RecursiveMode::Recursive)
            .with_context(|| format!("watching {}", root.display()))?;

        thread::Builder::new()
            .name("ferro-watcher".into())
            .spawn(move || {
                while let Ok(first) = rx.recv() {
                    let mut burst: HashMap<PathBuf, Seen> = HashMap::new();
                    let mut order: Vec<PathBuf> = Vec::new();
                    collect(first, &mut burst, &mut order);
                    let started = Instant::now();
                    loop {
                        let left = (debounce * 4).saturating_sub(started.elapsed());
                        if left.is_zero() {
                            break;
                        }
                        match rx.recv_timeout(debounce.min(left)) {
                            Ok(event) => collect(event, &mut burst, &mut order),
                            Err(RecvTimeoutError::Timeout) => break,
                            Err(RecvTimeoutError::Disconnected) => return,
                        }
                    }
                    if order
                        .iter()
                        .any(|p| p.file_name().is_some_and(|n| n == ".gitignore"))
                    {
                        filter.reload();
                    }
                    let events = settle(&order, &burst, &filter);
                    if !events.is_empty() {
                        sink(events);
                    }
                }
            })
            .context("starting the watcher thread")?;
        Ok(Self { _watcher: watcher })
    }
}

fn collect(
    event: notify::Result<notify::Event>,
    burst: &mut HashMap<PathBuf, Seen>,
    order: &mut Vec<PathBuf>,
) {
    let event = match event {
        Ok(event) => event,
        Err(err) => {
            tracing::debug!("watcher: {err}");
            return;
        }
    };
    let created = |kind: &EventKind, ix: usize| match kind {
        EventKind::Create(CreateKind::Any | CreateKind::File | CreateKind::Folder) => true,
        EventKind::Modify(ModifyKind::Name(RenameMode::To)) => true,
        EventKind::Modify(ModifyKind::Name(RenameMode::Both)) => ix == 1,
        _ => false,
    };
    if matches!(event.kind, EventKind::Access(_)) {
        return;
    }
    for (ix, path) in event.paths.into_iter().enumerate() {
        let is_new = !burst.contains_key(&path);
        let seen = burst.entry(path.clone()).or_default();
        let created = created(&event.kind, ix);
        if is_new {
            order.push(path);
            seen.created_first = created;
        }
        seen.created |= created;
    }
}

fn settle(
    order: &[PathBuf],
    burst: &HashMap<PathBuf, Seen>,
    filter: &IgnoreFilter,
) -> Vec<ProjectEvent> {
    let mut events = Vec::new();
    for path in order {
        let seen = burst[path];
        let exists = path.symlink_metadata().is_ok();
        if filter.is_ignored(path) {
            continue;
        }
        let event = match (exists, seen.created, seen.created_first) {
            (true, true, _) => ProjectEvent::Added(path.clone()),
            (true, false, _) => ProjectEvent::Changed(path.clone()),
            (false, _, true) => continue,
            (false, _, false) => ProjectEvent::Removed(path.clone()),
        };
        events.push(event);
    }
    events
}
