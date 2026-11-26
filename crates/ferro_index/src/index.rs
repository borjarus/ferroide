//! The persistent project index and the search service built on it.
//!
//! At startup the index loads the files, symbols and TODO comments stored in
//! its database, so Quick Open, Go to Symbol and the TODO view answer before
//! the project scan finishes. Once the scan is done, a background thread
//! compares every file's size and modification time with the database,
//! re-indexes what changed, forgets what disappeared, and then follows the
//! project watcher's events. Indexing pauses while the user types.

use std::{
    collections::{HashMap, HashSet},
    fs,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, MutexGuard, OnceLock, RwLock, Weak},
    thread,
    time::{Duration, Instant, UNIX_EPOCH},
};

use anyhow::Result;
use ferro_languages::LanguageRegistry;
use ferro_project::{FsProjectService, IndexedFile};
use ferro_services::{
    FileMatch, ProjectEvent, ProjectService as _, SearchEvent, SearchService, Symbol, TextMatch,
    TextQuery, TodoItem, TodoKind,
};
use futures::{FutureExt as _, future::BoxFuture};
use regex::Regex;

use crate::{
    db::{Db, FileData, FileRow, TodoRow},
    fuzzy::{self, Haystack, PathMatcher, Query},
    text::{self, Candidate, FileMask, MAX_SEARCH_FILE},
    trigram::{self, AUTO_THRESHOLD, TrigramMode},
};

/// Files larger than this are not parsed for symbols (generated or
/// minified code).
const MAX_SYMBOL_FILE: u64 = 1024 * 1024;
/// Files stored per database transaction.
const BATCH: usize = 256;
/// Progress is reported every this many files.
const PROGRESS_EVERY: usize = 200;
/// TODO comments reported at most.
const MAX_TODOS: usize = 10_000;

/// How a project index is set up.
#[derive(Debug, Clone)]
pub struct IndexOptions {
    /// The database file; `None` keeps the index in memory (tests).
    pub db_path: Option<PathBuf>,
    pub trigrams: TrigramMode,
    /// `Auto` turns trigrams on above this many bytes of files.
    pub trigram_threshold: u64,
    /// How long indexing waits after the last keystroke.
    pub typing_pause: Duration,
}

impl Default for IndexOptions {
    fn default() -> Self {
        Self {
            db_path: None,
            trigrams: TrigramMode::Auto,
            trigram_threshold: AUTO_THRESHOLD,
            typing_pause: Duration::from_millis(500),
        }
    }
}

/// The database file of a project: `<data>/index/<name>-<hash>.sqlite`,
/// one per set of root folders.
pub fn db_path_for(data_dir: &Path, roots: &[PathBuf]) -> PathBuf {
    // FNV-1a: stable across runs and platforms, unlike `DefaultHasher`.
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for root in roots {
        for byte in root.to_string_lossy().bytes().chain([0]) {
            hash ^= u64::from(byte);
            hash = hash.wrapping_mul(0x0100_0000_01b3);
        }
    }
    let name: String = roots
        .first()
        .and_then(|r| r.file_name())
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { '_' })
        .take(40)
        .collect();
    data_dir
        .join("index")
        .join(format!("{name}-{hash:016x}.sqlite"))
}

/// What the index knows about a file.
#[derive(Debug, Clone)]
struct Record {
    id: i64,
    root: PathBuf,
    relative: String,
    size: u64,
    mtime: i64,
    trigrams: bool,
}

/// Symbol names prepared for matching.
struct SymbolTable {
    symbols: Vec<Symbol>,
    haystacks: Vec<Haystack>,
}

#[derive(Default)]
struct State {
    records: HashMap<PathBuf, Record>,
    by_id: HashMap<i64, PathBuf>,
    symbols: HashMap<PathBuf, Vec<Symbol>>,
    todos: HashMap<PathBuf, Vec<TodoRow>>,
    symbol_table: Option<Arc<SymbolTable>>,
    /// The file list stored by the previous session, used until the scan
    /// of this one finishes.
    persisted: Arc<Vec<IndexedFile>>,
    /// Files changed on disk and not yet indexed again.
    dirty: HashSet<PathBuf>,
    /// The first comparison with the disk has finished.
    ready: bool,
    trigrams: bool,
    progress: Option<(usize, usize)>,
}

/// File-name matching data for one file list, rebuilt when the list changes.
struct FilesCache {
    files: Arc<Vec<IndexedFile>>,
    matcher: PathMatcher,
}

struct Inner {
    project: FsProjectService,
    registry: Arc<LanguageRegistry>,
    db: Db,
    options: IndexOptions,
    state: RwLock<State>,
    files_cache: Mutex<Option<FilesCache>>,
    paused_until: Mutex<Instant>,
    subscribers: Mutex<Vec<async_channel::Sender<SearchEvent>>>,
    /// Dropped with the index, which stops the worker.
    _alive: async_channel::Sender<()>,
}

/// The index of a project and the [`SearchService`] that answers from it.
#[derive(Clone)]
pub struct ProjectIndex {
    inner: Arc<Inner>,
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|e| e.into_inner())
}

fn mtime(meta: &fs::Metadata) -> i64 {
    meta.modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_nanos() as i64)
}

fn todo_regex() -> &'static Regex {
    static REGEX: OnceLock<Regex> = OnceLock::new();
    REGEX.get_or_init(|| Regex::new(r"\b(TODO|FIXME)\b:?\s*(.*)").expect("a valid pattern"))
}

/// TODO and FIXME comments of a file.
pub fn find_todos(text: &str) -> Vec<TodoRow> {
    if !text.contains("TODO") && !text.contains("FIXME") {
        return Vec::new();
    }
    text.lines()
        .enumerate()
        .filter_map(|(ix, line)| {
            let caps = todo_regex().captures(line)?;
            Some(TodoRow {
                line: ix as u32,
                kind: if &caps[1] == "TODO" {
                    TodoKind::Todo
                } else {
                    TodoKind::Fixme
                },
                text: caps[2].trim().trim_end_matches("*/").trim().to_string(),
            })
        })
        .collect()
}

/// How results show a file: relative to its root, prefixed with the root's
/// name when the project has several roots.
fn display_path(root: &Path, relative: &str, multi_root: bool) -> String {
    if multi_root {
        let name = root
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        format!("{name}/{relative}")
    } else {
        relative.to_string()
    }
}

impl ProjectIndex {
    /// Open the index of `project` and start indexing in the background.
    pub fn open(
        project: FsProjectService,
        registry: Arc<LanguageRegistry>,
        options: IndexOptions,
    ) -> Result<Self> {
        let db = match &options.db_path {
            Some(path) => Db::open(path).or_else(|err| {
                tracing::warn!("the index stays in memory: {err:#}");
                Db::open_in_memory()
            })?,
            None => Db::open_in_memory()?,
        };
        let mut state = State::default();
        let started = Instant::now();
        let loaded = db.load()?;
        let mut persisted = Vec::with_capacity(loaded.files.len());
        let mut symbols = loaded.symbols;
        let mut todos = loaded.todos;
        let roots = project.roots();
        for row in loaded.files {
            // A file of a root that is no longer part of the project is
            // forgotten by the first comparison with the disk.
            if roots.contains(&row.root) {
                persisted.push(IndexedFile {
                    path: row.path.clone(),
                    root: row.root.clone(),
                    relative: row.relative.clone(),
                });
            }
            if let Some(s) = symbols.remove(&row.id) {
                state.symbols.insert(row.path.clone(), s);
            }
            if let Some(t) = todos.remove(&row.id) {
                state.todos.insert(row.path.clone(), t);
            }
            state.by_id.insert(row.id, row.path.clone());
            state.records.insert(
                row.path,
                Record {
                    id: row.id,
                    root: row.root,
                    relative: row.relative,
                    size: row.size,
                    mtime: row.mtime,
                    trigrams: row.trigrams,
                },
            );
        }
        persisted.sort_by(|a, b| (&a.root, &a.relative).cmp(&(&b.root, &b.relative)));
        state.persisted = Arc::new(persisted);
        state.trigrams = db.meta("trigrams").as_deref() == Some("1");
        tracing::info!(
            target: "ferro::index",
            "loaded {} indexed files in {:.0} ms",
            state.records.len(),
            started.elapsed().as_secs_f64() * 1000.0
        );

        let (alive, alive_rx) = async_channel::bounded(1);
        let events = project.subscribe();
        let inner = Arc::new(Inner {
            project,
            registry,
            db,
            options,
            state: RwLock::new(state),
            files_cache: Mutex::new(None),
            paused_until: Mutex::new(Instant::now()),
            subscribers: Mutex::new(Vec::new()),
            _alive: alive,
        });
        let weak = Arc::downgrade(&inner);
        thread::Builder::new()
            .name("ferro-index".into())
            .spawn(move || worker(weak, events, alive_rx))?;
        Ok(Self { inner })
    }

    /// Whether the first comparison with the disk has finished.
    pub fn is_ready(&self) -> bool {
        self.inner.read().ready
    }

    /// Resolves once the first comparison with the disk has finished.
    pub async fn ready(&self) {
        let events = self.subscribe();
        while !self.is_ready() {
            if events.recv().await.is_err() {
                return;
            }
        }
    }

    /// Files indexed (stored in the database).
    pub fn indexed_files(&self) -> usize {
        self.inner.read().records.len()
    }

    /// Whether the trigram index is in use.
    pub fn uses_trigrams(&self) -> bool {
        self.inner.read().trigrams
    }

    /// Whether indexing is paused because the user is typing.
    pub fn is_paused(&self) -> bool {
        Instant::now() < *lock(&self.inner.paused_until)
    }

    /// Symbols whose name matches `query`, best first.
    pub fn search_symbols(&self, query: &str, limit: usize) -> Vec<Symbol> {
        let table = self.inner.symbol_table();
        if query.trim().is_empty() {
            return table.symbols.iter().take(limit).cloned().collect();
        }
        let query = Query::names(query);
        fuzzy::rank(&query, &table.haystacks, None, limit, |i| {
            table.symbols[i].name.as_str()
        })
        .0
        .into_iter()
        .map(|r| table.symbols[r.index].clone())
        .collect()
    }

    /// The function `SyntaxLanguageService` asks for Go to Symbol.
    pub fn symbol_search(&self) -> ferro_languages::SymbolSearch {
        let weak = Arc::downgrade(&self.inner);
        Arc::new(move |query, limit| {
            weak.upgrade()
                .map(|inner| ProjectIndex { inner }.search_symbols(query, limit))
                .unwrap_or_default()
        })
    }

    /// Files matching `query` (nucleo syntax), best first.
    pub fn match_files(&self, query: &str, limit: usize) -> Vec<FileMatch> {
        let files = self.inner.current_files();
        let multi_root = self.inner.project.roots().len() > 1;
        let mut cache = lock(&self.inner.files_cache);
        if cache
            .as_ref()
            .is_none_or(|c| !Arc::ptr_eq(&c.files, &files))
        {
            let shown = files
                .iter()
                .map(|f| display_path(&f.root, &f.relative, multi_root))
                .collect();
            *cache = Some(FilesCache {
                files: files.clone(),
                matcher: PathMatcher::new(shown),
            });
        }
        let cache = cache.as_mut().expect("just filled");
        if query.trim().is_empty() {
            return files
                .iter()
                .enumerate()
                .take(limit)
                .map(|(i, f)| FileMatch {
                    path: f.path.clone(),
                    relative: cache.matcher.text(i).to_string(),
                    score: 0,
                    positions: Vec::new(),
                })
                .collect();
        }
        cache
            .matcher
            .search(query, limit)
            .into_iter()
            .map(|r| FileMatch {
                path: files[r.index].path.clone(),
                relative: cache.matcher.text(r.index).to_string(),
                score: i64::from(r.score),
                positions: r.positions,
            })
            .collect()
    }

    /// The files Find in Files searches for `query`: scope, mask and (when
    /// enabled) the trigram index applied.
    pub fn text_candidates(&self, query: &TextQuery) -> Result<Vec<Candidate>> {
        let mask = FileMask::parse(query.path_filter.as_deref().unwrap_or(""))?;
        let scope_mask = match &query.scope {
            ferro_services::SearchScope::Mask(mask) => Some(FileMask::parse(mask)?),
            _ => None,
        };
        let files = self.inner.current_files();
        let multi_root = self.inner.project.roots().len() > 1;
        let narrowed = self.inner.trigram_files(query);
        Ok(files
            .iter()
            .filter(|f| query.scope.contains(&f.path))
            .filter(|f| narrowed.as_ref().is_none_or(|set| set.contains(&f.path)))
            .filter_map(|f| {
                let shown = display_path(&f.root, &f.relative, multi_root);
                let in_scope = scope_mask.as_ref().is_none_or(|m| m.matches(&shown));
                (in_scope && mask.matches(&shown)).then(|| Candidate {
                    path: f.path.clone(),
                    shown,
                })
            })
            .collect())
    }

    /// TODO comments of every indexed file, by path and line.
    pub fn todo_items(&self) -> Vec<TodoItem> {
        let multi_root = self.inner.project.roots().len() > 1;
        let state = self.inner.read();
        let mut files: Vec<(&PathBuf, &Record)> = state
            .todos
            .keys()
            .filter_map(|p| state.records.get(p).map(|r| (p, r)))
            .collect();
        files.sort_by(|a, b| (&a.1.root, &a.1.relative).cmp(&(&b.1.root, &b.1.relative)));
        let mut out = Vec::new();
        for (path, record) in files {
            let shown = display_path(&record.root, &record.relative, multi_root);
            for todo in &state.todos[path] {
                out.push(TodoItem {
                    path: path.clone(),
                    relative: shown.clone(),
                    line: todo.line,
                    kind: todo.kind,
                    text: todo.text.clone(),
                });
                if out.len() >= MAX_TODOS {
                    return out;
                }
            }
        }
        out
    }
}

impl Inner {
    fn read(&self) -> std::sync::RwLockReadGuard<'_, State> {
        self.state.read().unwrap_or_else(|e| e.into_inner())
    }

    fn write(&self) -> std::sync::RwLockWriteGuard<'_, State> {
        self.state.write().unwrap_or_else(|e| e.into_inner())
    }

    fn emit(&self, event: SearchEvent) {
        let mut subscribers = lock(&self.subscribers);
        subscribers.retain(|tx| !tx.is_closed());
        for tx in subscribers.iter() {
            let _ = tx.try_send(event.clone());
        }
    }

    /// The project's file list, or the stored one while the scan runs.
    fn current_files(&self) -> Arc<Vec<IndexedFile>> {
        let index = self.project.index();
        if index.is_scanning() {
            let persisted = self.read().persisted.clone();
            if !persisted.is_empty() {
                return persisted;
            }
        }
        index.snapshot()
    }

    fn symbol_table(&self) -> Arc<SymbolTable> {
        if let Some(table) = self.read().symbol_table.clone() {
            return table;
        }
        let mut state = self.write();
        if let Some(table) = state.symbol_table.clone() {
            return table;
        }
        let mut paths: Vec<&PathBuf> = state.symbols.keys().collect();
        paths.sort();
        let symbols: Vec<Symbol> = paths
            .into_iter()
            .flat_map(|p| state.symbols[p].iter().cloned())
            .collect();
        let haystacks = symbols.iter().map(|s| Haystack::new(&s.name)).collect();
        let table = Arc::new(SymbolTable { symbols, haystacks });
        state.symbol_table = Some(table.clone());
        table
    }

    /// The files that can contain `query` according to the trigram index,
    /// plus every file the index does not cover yet. `None` when the
    /// index is off or cannot narrow this query.
    fn trigram_files(&self, query: &TextQuery) -> Option<HashSet<PathBuf>> {
        {
            let state = self.read();
            if !state.trigrams || !state.ready {
                return None;
            }
        }
        let wanted = trigram::of_query(query)?;
        let ids = match self.db.files_with_trigrams(&wanted) {
            Ok(ids) => ids,
            Err(err) => {
                tracing::warn!("trigram lookup failed: {err:#}");
                return None;
            }
        };
        let state = self.read();
        let mut out: HashSet<PathBuf> = ids
            .into_iter()
            .filter_map(|id| state.by_id.get(&id).cloned())
            .collect();
        out.extend(state.dirty.iter().cloned());
        out.extend(
            state
                .records
                .iter()
                .filter(|(_, r)| !r.trigrams)
                .map(|(p, _)| p.clone()),
        );
        // Files the scan found but the index has not seen yet.
        for file in self.project.index().snapshot().iter() {
            if !state.records.contains_key(&file.path) {
                out.insert(file.path.clone());
            }
        }
        Some(out)
    }

    fn wait_while_paused(&self) {
        loop {
            let until = *lock(&self.paused_until);
            let now = Instant::now();
            if now >= until {
                return;
            }
            thread::sleep((until - now).min(Duration::from_millis(50)));
        }
    }

    /// Read and analyze one file.
    fn analyze(&self, path: &Path, size: u64, trigrams: bool) -> FileData {
        let mut data = FileData {
            trigrams: trigrams.then(Vec::new),
            ..FileData::default()
        };
        if size > MAX_SEARCH_FILE {
            return data;
        }
        let Ok(bytes) = fs::read(path) else {
            return data;
        };
        if bytes[..bytes.len().min(8192)].contains(&0) {
            return data;
        }
        let text = String::from_utf8_lossy(&bytes);
        data.todos = find_todos(&text);
        if size <= MAX_SYMBOL_FILE {
            data.symbols =
                ferro_languages::file_symbols(&self.registry, path, &text).unwrap_or_default();
        }
        if trigrams {
            data.trigrams = Some(trigram::of_text(&bytes));
        }
        data
    }

    /// Store analyzed files and publish them to searches.
    fn flush(&self, batch: &mut Vec<(FileRow, FileData)>) {
        if batch.is_empty() {
            return;
        }
        let ids = match self.db.store(batch) {
            Ok(ids) => ids,
            Err(err) => {
                tracing::warn!("storing the index failed: {err:#}");
                // Keep the data in memory with temporary ids.
                batch.iter().map(|(row, _)| -row.mtime.abs() - 1).collect()
            }
        };
        let mut state = self.write();
        for ((row, data), id) in batch.drain(..).zip(ids) {
            let path = row.path.clone();
            state.dirty.remove(&path);
            if let Some(old) = state.records.get(&path)
                && old.id != id
            {
                let old_id = old.id;
                state.by_id.remove(&old_id);
            }
            state.by_id.insert(id, path.clone());
            state.records.insert(
                path.clone(),
                Record {
                    id,
                    root: row.root,
                    relative: row.relative,
                    size: row.size,
                    mtime: row.mtime,
                    trigrams: data.trigrams.is_some(),
                },
            );
            if data.symbols.is_empty() {
                state.symbols.remove(&path);
            } else {
                state.symbols.insert(path.clone(), data.symbols);
            }
            if data.todos.is_empty() {
                state.todos.remove(&path);
            } else {
                state.todos.insert(path, data.todos);
            }
        }
        state.symbol_table = None;
        drop(state);
        self.emit(SearchEvent::Updated);
    }

    fn forget(&self, paths: &[PathBuf]) {
        if paths.is_empty() {
            return;
        }
        let mut state = self.write();
        let mut ids = Vec::new();
        for path in paths {
            state.dirty.remove(path);
            if let Some(record) = state.records.remove(path) {
                state.by_id.remove(&record.id);
                ids.push(record.id);
            }
            state.symbols.remove(path);
            state.todos.remove(path);
        }
        state.symbol_table = None;
        drop(state);
        if let Err(err) = self.db.remove(&ids) {
            tracing::warn!("removing files from the index failed: {err:#}");
        }
        self.emit(SearchEvent::Updated);
    }

    /// Analyze `files` (with their size and modification time), pausing
    /// while the user types. Returns false when the index was dropped.
    fn index_files(self: &Arc<Self>, files: Vec<(IndexedFile, u64, i64)>) -> bool {
        let total = files.len();
        let trigrams = self.read().trigrams;
        let mut batch = Vec::with_capacity(BATCH.min(total));
        for (done, (file, size, mtime)) in files.into_iter().enumerate() {
            self.wait_while_paused();
            if Arc::strong_count(self) == 1 {
                return false;
            }
            let data = self.analyze(&file.path, size, trigrams);
            batch.push((
                FileRow {
                    id: 0,
                    path: file.path,
                    root: file.root,
                    relative: file.relative,
                    size,
                    mtime,
                    trigrams,
                },
                data,
            ));
            if batch.len() >= BATCH {
                self.flush(&mut batch);
            }
            if total > PROGRESS_EVERY && done % PROGRESS_EVERY == 0 {
                self.write().progress = Some((done, total));
                self.emit(SearchEvent::Indexing { done, total });
            }
        }
        self.flush(&mut batch);
        if total > PROGRESS_EVERY {
            self.write().progress = None;
            self.emit(SearchEvent::Indexing { done: total, total });
        }
        true
    }

    /// Compare the scanned files with the index and bring it up to date.
    fn reconcile(self: &Arc<Self>) -> bool {
        let started = Instant::now();
        let files = self.project.index().snapshot();
        let mut stale = Vec::new();
        let mut present = HashSet::with_capacity(files.len());
        let mut total_bytes = 0u64;
        let mut stamps = Vec::with_capacity(files.len());
        for file in files.iter() {
            let Ok(meta) = fs::metadata(&file.path) else {
                continue;
            };
            total_bytes += meta.len();
            present.insert(file.path.clone());
            stamps.push((file.clone(), meta.len(), mtime(&meta)));
        }
        let trigrams = match self.options.trigrams {
            TrigramMode::Off => false,
            TrigramMode::On => true,
            TrigramMode::Auto => total_bytes > self.options.trigram_threshold,
        };
        {
            let mut state = self.write();
            if state.trigrams != trigrams {
                state.trigrams = trigrams;
                let _ = self
                    .db
                    .set_meta("trigrams", if trigrams { "1" } else { "0" });
            }
            for (file, size, mtime) in stamps {
                let fresh = state
                    .records
                    .get(&file.path)
                    .is_some_and(|r| r.size == size && r.mtime == mtime && r.trigrams == trigrams);
                if !fresh {
                    stale.push((file, size, mtime));
                }
            }
        }
        let gone: Vec<PathBuf> = self
            .read()
            .records
            .keys()
            .filter(|p| !present.contains(*p))
            .cloned()
            .collect();
        self.forget(&gone);
        tracing::info!(
            target: "ferro::index",
            "{} files: {} to index, {} gone ({:.0} ms to compare){}",
            files.len(),
            stale.len(),
            gone.len(),
            started.elapsed().as_secs_f64() * 1000.0,
            if trigrams { ", trigram index on" } else { "" }
        );
        let alive = self.index_files(stale);
        {
            let mut state = self.write();
            state.ready = true;
            state.persisted = Arc::new(Vec::new());
        }
        self.emit(SearchEvent::Updated);
        tracing::info!(
            target: "ferro::index",
            "index up to date in {:.1} s",
            started.elapsed().as_secs_f64()
        );
        alive
    }

    /// Follow changes on disk.
    fn apply(self: &Arc<Self>, events: Vec<ProjectEvent>) -> bool {
        if events.contains(&ProjectEvent::RootsChanged) {
            return self.reconcile();
        }
        let files = self.project.index().snapshot();
        let mut changed: Vec<PathBuf> = Vec::new();
        let mut removed: Vec<PathBuf> = Vec::new();
        for event in events {
            match event {
                ProjectEvent::Added(path) | ProjectEvent::Changed(path) => changed.push(path),
                ProjectEvent::Removed(path) => {
                    let state = self.read();
                    removed.extend(
                        state
                            .records
                            .keys()
                            .filter(|p| p.starts_with(&path))
                            .cloned(),
                    );
                }
                ProjectEvent::RootsChanged => {}
            }
        }
        self.forget(&removed);
        if changed.is_empty() {
            return true;
        }
        let mut work = Vec::new();
        let mut seen = HashSet::new();
        for file in files.iter() {
            if changed.iter().any(|c| file.path.starts_with(c))
                && seen.insert(file.path.clone())
                && let Ok(meta) = fs::metadata(&file.path)
            {
                work.push((file.clone(), meta.len(), mtime(&meta)));
            }
        }
        {
            let mut state = self.write();
            for (file, _, _) in &work {
                state.dirty.insert(file.path.clone());
            }
        }
        self.index_files(work)
    }
}

fn worker(
    weak: Weak<Inner>,
    events: async_channel::Receiver<ProjectEvent>,
    alive: async_channel::Receiver<()>,
) {
    let Some(files) = weak.upgrade().map(|inner| inner.project.index().clone()) else {
        return;
    };
    let scanned = smol::block_on(smol::future::or(
        async {
            files.ready().await;
            true
        },
        async {
            let _ = alive.recv().await;
            false
        },
    ));
    if !scanned {
        return;
    }
    match weak.upgrade() {
        Some(inner) if inner.reconcile() => {}
        _ => return,
    }
    loop {
        let event = smol::block_on(smol::future::or(
            async { events.recv().await.ok() },
            async {
                let _ = alive.recv().await;
                None
            },
        ));
        let Some(event) = event else { return };
        let mut batch = vec![event];
        while let Ok(more) = events.try_recv() {
            batch.push(more);
        }
        let Some(inner) = weak.upgrade() else { return };
        if !inner.apply(batch) {
            return;
        }
    }
}

impl SearchService for ProjectIndex {
    fn find_files(
        &self,
        query: &str,
        limit: usize,
    ) -> BoxFuture<'static, ferro_services::Result<Vec<FileMatch>>> {
        let this = self.clone();
        let query = query.to_string();
        async move {
            // With no stored list, wait for the scan rather than show a
            // partial one.
            if this.inner.read().persisted.is_empty() {
                this.inner.project.index().ready().await;
            }
            Ok(smol::unblock(move || this.match_files(&query, limit)).await)
        }
        .boxed()
    }

    fn find_text(
        &self,
        query: TextQuery,
        limit: usize,
    ) -> BoxFuture<'static, ferro_services::Result<Vec<TextMatch>>> {
        let this = self.clone();
        async move {
            if query.text.is_empty() {
                return Ok(Vec::new());
            }
            text::query_regex(&query)?;
            this.inner.project.index().ready().await;
            smol::unblock(move || {
                let candidates = this.text_candidates(&query)?;
                text::search_files(&candidates, &query, limit)
            })
            .await
        }
        .boxed()
    }

    fn todos(&self) -> BoxFuture<'static, ferro_services::Result<Vec<TodoItem>>> {
        let this = self.clone();
        async move { Ok(this.todo_items()) }.boxed()
    }

    fn subscribe(&self) -> async_channel::Receiver<SearchEvent> {
        let (tx, rx) = async_channel::unbounded();
        if let Some((done, total)) = self.inner.read().progress {
            let _ = tx.try_send(SearchEvent::Indexing { done, total });
        }
        lock(&self.inner.subscribers).push(tx);
        rx
    }

    fn user_activity(&self) {
        *lock(&self.inner.paused_until) = Instant::now() + self.inner.options.typing_pause;
    }
}
