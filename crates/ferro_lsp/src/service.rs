//! [`LspLanguageService`]: the `LanguageService` of projects on disk, backed
//! by language servers with the tree-sitter service as a fallback.
//!
//! - **Servers on demand.** A file's Language Pack names its servers; each
//!   starts when the first request for one of its files arrives, in the
//!   folder holding the pack's outermost project marker (`Cargo.toml`,
//!   `go.mod`, …) inside the project root. Several servers can serve one
//!   file (Python: basedpyright + ruff); a feature goes to the servers whose
//!   manifest lists it, or to all of them when none does.
//! - **Documents** are synchronized lazily: every request carries the
//!   editor's text, and the server gets `didOpen` or one incremental
//!   `didChange` before the request.
//! - **Governor** (plan §6.3): at most `max_servers` run at once (the least
//!   recently used one stops), servers idle for `idle_timeout` stop and
//!   restart on the next request, a crashed server restarts at most three
//!   times in three minutes. Power save mode skips inlay hints, semantic
//!   tokens and code lens.
//! - **Safe mode**: nothing starts in an untrusted project.
//! - **Fallback**: with no server (untrusted, not installed, crashed for
//!   good, or a language without one) the tree-sitter service answers.

use std::{
    collections::{BTreeMap, HashMap},
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex, RwLock, Weak,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};

use ferro_languages::{
    LanguagePack, LanguageRegistry, SyntaxLanguageService, manifest::LanguageServerManifest,
};
use ferro_services::{
    CodeLens, CompletionItem, Diagnostic, DocumentLink, GotoKind, HierarchyDirection,
    HierarchyItem, HoverInfo, InlayHint, LanguageEvent, LanguageServerStatus, LanguageService,
    Location, ProjectEvent, QuickFix, Runnable, SemanticToken, ServerHealth, SignatureHelp, Symbol,
    TextEdit, TextPos, TextRange, WorkspaceEdit, fuzzy,
};
use futures::{
    FutureExt as _,
    future::{BoxFuture, Shared, join_all},
};
use lsp_types::{self as lsp, request as req};
use serde_json::{Value, json};

use crate::{
    convert::{self, Texts},
    install,
    position::{Encoding, Lines},
    rpc::RpcError,
    server::{EventSink, LanguageServer, Launcher, ServerConfig, ServerEvent, Transport},
    templates,
    uri::{lsp_uri, normalize, path_to_uri, uri_to_path},
};

const COMPLETION_TIMEOUT: Duration = Duration::from_secs(5);
const QUICK_TIMEOUT: Duration = Duration::from_secs(5);
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(10);
const LONG_TIMEOUT: Duration = Duration::from_secs(30);
const RESOLVE_TIMEOUT: Duration = Duration::from_millis(800);
/// Completion items resolved eagerly (documentation, auto-import edits).
const RESOLVED_ITEMS: usize = 10;
const MAX_RESTARTS: usize = 3;
const RESTART_WINDOW: Duration = Duration::from_secs(180);

/// Per-server overrides from `settings.toml` (`[lsp.servers.<id>]`).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ServerSettings {
    pub enabled: Option<bool>,
    pub command: Option<String>,
    pub args: Option<Vec<String>>,
    /// Answers to `workspace/configuration`.
    pub settings: Option<Value>,
    pub initialization_options: Option<Value>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct LspOptions {
    pub max_servers: usize,
    pub idle_timeout: Duration,
    pub power_save: bool,
    /// `<data>/servers`: where the installer puts servers.
    pub install_dir: Option<PathBuf>,
    pub servers: HashMap<String, ServerSettings>,
}

impl Default for LspOptions {
    fn default() -> Self {
        Self {
            max_servers: 6,
            idle_timeout: Duration::from_secs(30 * 60),
            power_save: false,
            install_dir: None,
            servers: HashMap::new(),
        }
    }
}

/// Starts real processes, looking commands up in the install folder and on
/// `PATH`.
pub struct ProcessLauncher {
    pub install_dir: Option<PathBuf>,
}

impl Launcher for ProcessLauncher {
    fn launch(&self, config: &ServerConfig) -> std::io::Result<Transport> {
        let command = config.command.to_string_lossy();
        let resolved = install::resolve_command(&command, &config.id, self.install_dir.as_deref())
            .ok_or_else(|| {
                std::io::Error::new(
                    std::io::ErrorKind::NotFound,
                    format!("{command} is not installed"),
                )
            })?;
        let config = ServerConfig {
            command: resolved,
            ..config.clone()
        };
        crate::server::ProcessLauncher.launch(&config)
    }
}

/// Lists the project's root folders.
pub type RootsFn = Arc<dyn Fn() -> Vec<PathBuf> + Send + Sync>;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct Key {
    id: String,
    root: PathBuf,
}

impl Key {
    fn label(&self) -> String {
        format!("{}@{}", self.id, self.root.display())
    }
}

type Ready = Shared<BoxFuture<'static, Result<(), String>>>;

enum SlotState {
    Running {
        server: Arc<LanguageServer>,
        ready: Ready,
    },
    Stopped,
    Failed(String),
}

struct Slot {
    key: Key,
    language: String,
    generation: u64,
    state: SlotState,
    restarts: Vec<Instant>,
    last_used: Instant,
}

impl Slot {
    fn running(&self) -> Option<&Arc<LanguageServer>> {
        match &self.state {
            SlotState::Running { server, .. } if server.is_running() => Some(server),
            _ => None,
        }
    }
}

/// A code action offered by [`LanguageService::code_actions`], kept until
/// the user picks one.
struct StoredAction {
    server: Arc<LanguageServer>,
    action: lsp::CodeActionOrCommand,
}

/// Diagnostics of one file, per server: converted, and as the server sent
/// them (for the code action context).
type FileDiagnostics = BTreeMap<String, (Vec<Diagnostic>, Vec<lsp::Diagnostic>)>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Feature {
    Completion,
    Hover,
    SignatureHelp,
    Definition,
    References,
    Rename,
    Diagnostics,
    Formatting,
    CodeActions,
    InlayHints,
    Symbols,
    SemanticTokens,
    CodeLens,
    Links,
    Hierarchy,
}

impl Feature {
    /// Names a manifest's `features` may use for it.
    fn names(self) -> &'static [&'static str] {
        match self {
            Feature::Completion => &["completion"],
            Feature::Hover => &["hover"],
            Feature::SignatureHelp => &["signature_help", "signature"],
            Feature::Definition => &["definition", "navigation"],
            Feature::References => &["references"],
            Feature::Rename => &["rename"],
            Feature::Diagnostics => &["diagnostics"],
            Feature::Formatting => &["formatting"],
            Feature::CodeActions => &["code_actions", "quick_fixes"],
            Feature::InlayHints => &["inlay_hints"],
            Feature::Symbols => &["symbols", "document_symbols", "workspace_symbols"],
            Feature::SemanticTokens => &["semantic_tokens"],
            Feature::CodeLens => &["code_lens"],
            Feature::Links => &["links", "document_links"],
            Feature::Hierarchy => &["hierarchy", "call_hierarchy", "type_hierarchy"],
        }
    }

    fn supported(self, server: &LanguageServer) -> bool {
        let caps = server.capabilities();
        fn on<T>(c: Option<lsp::OneOf<bool, T>>) -> bool {
            match c {
                Some(lsp::OneOf::Left(b)) => b,
                Some(lsp::OneOf::Right(_)) => true,
                None => false,
            }
        }
        match self {
            Feature::Completion => caps.completion_provider.is_some(),
            Feature::Hover => !matches!(
                caps.hover_provider,
                None | Some(lsp::HoverProviderCapability::Simple(false))
            ),
            Feature::SignatureHelp => caps.signature_help_provider.is_some(),
            Feature::Definition => on(caps.definition_provider),
            Feature::References => on(caps.references_provider),
            Feature::Rename => on(caps.rename_provider),
            // Push diagnostics need no capability.
            Feature::Diagnostics => true,
            Feature::Formatting => {
                on(caps.document_formatting_provider)
                    || on(caps.document_range_formatting_provider)
                    || server.has_registration("textDocument/formatting")
            }
            Feature::CodeActions => !matches!(
                caps.code_action_provider,
                None | Some(lsp::CodeActionProviderCapability::Simple(false))
            ),
            Feature::InlayHints => on(caps.inlay_hint_provider),
            Feature::Symbols => on(caps.document_symbol_provider),
            Feature::SemanticTokens => caps.semantic_tokens_provider.is_some(),
            Feature::CodeLens => caps.code_lens_provider.is_some(),
            Feature::Links => caps.document_link_provider.is_some(),
            // Checked per direction (call or type hierarchy).
            Feature::Hierarchy => true,
        }
    }
}

/// LSP `languageId` of a Language Pack.
fn language_id(pack: &str) -> &str {
    match pack {
        "tsx" => "typescriptreact",
        "bash" => "shellscript",
        other => other,
    }
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

struct Inner {
    registry: Arc<LanguageRegistry>,
    fallback: SyntaxLanguageService,
    roots: RootsFn,
    launcher: Arc<dyn Launcher>,
    options: RwLock<LspOptions>,
    trusted: AtomicBool,
    slots: Mutex<Vec<Slot>>,
    generation: AtomicU64,
    diagnostics: Mutex<HashMap<PathBuf, FileDiagnostics>>,
    /// Paths as the editor spells them, by normalized path.
    paths: Mutex<HashMap<PathBuf, PathBuf>>,
    subscribers: Mutex<Vec<async_channel::Sender<LanguageEvent>>>,
    actions: Mutex<HashMap<u64, StoredAction>>,
    next_action: AtomicU64,
}

/// The language-server-backed [`LanguageService`].
pub struct LspLanguageService {
    inner: Arc<Inner>,
}

impl LspLanguageService {
    pub fn new(
        registry: Arc<LanguageRegistry>,
        fallback: SyntaxLanguageService,
        roots: RootsFn,
        launcher: Arc<dyn Launcher>,
        options: LspOptions,
        trusted: bool,
    ) -> Self {
        Self {
            inner: Arc::new(Inner {
                registry,
                fallback,
                roots,
                launcher,
                options: RwLock::new(options),
                trusted: AtomicBool::new(trusted),
                slots: Mutex::new(Vec::new()),
                generation: AtomicU64::new(1),
                diagnostics: Mutex::new(HashMap::new()),
                paths: Mutex::new(HashMap::new()),
                subscribers: Mutex::new(Vec::new()),
                actions: Mutex::new(HashMap::new()),
                next_action: AtomicU64::new(1),
            }),
        }
    }

    pub fn set_options(&self, options: LspOptions) {
        let power_save = options.power_save;
        *self
            .inner
            .options
            .write()
            .unwrap_or_else(|e| e.into_inner()) = options;
        self.inner.enforce_limits(None);
        if power_save {
            self.inner.emit(LanguageEvent::RefreshRequested);
        }
    }

    pub fn options(&self) -> LspOptions {
        self.inner.options().clone()
    }

    /// Servers that could not start or keep crashing, with the reason
    /// (e.g. how to install a missing one).
    pub fn problems(&self) -> Vec<(String, String)> {
        lock(&self.inner.slots)
            .iter()
            .filter_map(|s| match &s.state {
                SlotState::Failed(reason) => Some((s.key.id.clone(), reason.clone())),
                _ => None,
            })
            .collect()
    }

    /// Stop every server (they restart on the next request).
    pub fn stop_all(&self) {
        self.inner.stop_all();
    }

    /// The servers a file's language would use, and whether each is found.
    pub fn servers_of(&self, path: &Path) -> Vec<(LanguageServerManifest, bool)> {
        let Some(pack) = self.inner.registry.detect_path(path).cloned() else {
            return Vec::new();
        };
        let options = self.inner.options().clone();
        pack.manifest()
            .language_servers
            .iter()
            .map(|m| {
                let command = options
                    .servers
                    .get(&m.id)
                    .and_then(|s| s.command.clone())
                    .unwrap_or_else(|| m.command.clone());
                let found =
                    install::resolve_command(&command, &m.id, options.install_dir.as_deref())
                        .is_some();
                (m.clone(), found)
            })
            .collect()
    }

    /// Install a server into the install folder (blocking), then let it
    /// start again.
    pub fn install(&self, server: &LanguageServerManifest) -> anyhow::Result<()> {
        let dir = self
            .inner
            .options()
            .install_dir
            .clone()
            .ok_or_else(|| anyhow::anyhow!("no folder to install language servers into"))?;
        install::install(server, &dir)?;
        lock(&self.inner.slots).retain(|s| s.key.id != server.id);
        self.inner.emit(LanguageEvent::ServersChanged);
        Ok(())
    }
}

impl Drop for Inner {
    fn drop(&mut self) {
        self.stop_all();
    }
}

impl Inner {
    fn options(&self) -> std::sync::RwLockReadGuard<'_, LspOptions> {
        self.options.read().unwrap_or_else(|e| e.into_inner())
    }

    fn emit(&self, event: LanguageEvent) {
        lock(&self.subscribers).retain(|tx| tx.try_send(event.clone()).is_ok() || !tx.is_closed());
    }

    fn remember_path(&self, path: &Path) -> PathBuf {
        let key = normalize(path);
        lock(&self.paths)
            .entry(key.clone())
            .or_insert_with(|| path.to_path_buf());
        key
    }

    fn editor_path(&self, normalized: &Path) -> PathBuf {
        lock(&self.paths)
            .get(normalized)
            .cloned()
            .unwrap_or_else(|| normalized.to_path_buf())
    }

    fn pack(&self, path: &Path, text: &str) -> Option<Arc<LanguagePack>> {
        let head: Vec<&str> = text.lines().take(5).collect();
        let tail: Vec<&str> = text.lines().rev().take(5).collect();
        let probe = format!(
            "{}\n{}",
            head.join("\n"),
            tail.into_iter().rev().collect::<Vec<_>>().join("\n")
        );
        self.registry.detect(path, Some(&probe)).cloned()
    }

    /// The folder a server for `path` runs in: the outermost folder with a
    /// project marker inside the project root, else the project root.
    fn root_for(&self, path: &Path, markers: &[String]) -> PathBuf {
        let roots = (self.roots)();
        let project = roots
            .iter()
            .filter(|r| path.starts_with(r))
            .max_by_key(|r| r.as_os_str().len())
            .cloned();
        let mut found = None;
        let mut dir = path.parent();
        while let Some(d) = dir {
            if markers.iter().any(|m| d.join(m).exists()) {
                found = Some(d.to_path_buf());
                if project.is_none() {
                    break;
                }
            }
            if project.as_deref() == Some(d) {
                break;
            }
            dir = d.parent();
        }
        found
            .or(project)
            .or_else(|| path.parent().map(Path::to_path_buf))
            .unwrap_or_default()
    }

    /// The servers of a pack routed `feature`, enabled in the settings.
    fn candidates(&self, pack: &LanguagePack, feature: Feature) -> Vec<LanguageServerManifest> {
        let options = self.options();
        let enabled: Vec<&LanguageServerManifest> = pack
            .manifest()
            .language_servers
            .iter()
            .filter(|m| {
                options
                    .servers
                    .get(&m.id)
                    .and_then(|s| s.enabled)
                    .unwrap_or(true)
            })
            .collect();
        let lists = |m: &LanguageServerManifest| {
            m.features
                .iter()
                .any(|f| feature.names().contains(&f.as_str()))
        };
        let any_lists = enabled.iter().any(|m| lists(m));
        enabled
            .into_iter()
            .filter(|m| !any_lists || lists(m))
            .cloned()
            .collect()
    }

    fn config(&self, key: &Key, language: &str, manifest: &LanguageServerManifest) -> ServerConfig {
        let options = self.options();
        let overrides = options
            .servers
            .get(&manifest.id)
            .cloned()
            .unwrap_or_default();
        ServerConfig {
            id: manifest.id.clone(),
            language: language.to_string(),
            command: PathBuf::from(
                overrides
                    .command
                    .unwrap_or_else(|| manifest.command.clone()),
            ),
            args: overrides.args.unwrap_or_else(|| manifest.args.clone()),
            env: Vec::new(),
            root: key.root.clone(),
            initialization_options: overrides.initialization_options,
            settings: overrides.settings.unwrap_or(Value::Null),
        }
    }

    fn start(
        self: &Arc<Self>,
        slot: &mut Slot,
        manifest: &LanguageServerManifest,
    ) -> Result<Ready, String> {
        let generation = self.generation.fetch_add(1, Ordering::SeqCst);
        let config = self.config(&slot.key, &slot.language, manifest);
        let sink: EventSink = {
            let (weak, key) = (Arc::downgrade(self), slot.key.clone());
            Arc::new(move |event| {
                if let Some(inner) = Weak::upgrade(&weak) {
                    inner.on_event(&key, generation, event);
                }
            })
        };
        let server = LanguageServer::launch(config, &*self.launcher, sink).map_err(|err| {
            if err.kind() == std::io::ErrorKind::NotFound {
                let hint = match &self.options().install_dir {
                    Some(dir) => match install::plan(manifest, dir) {
                        install::InstallPlan::Steps(steps) => format!(
                            "{} is not installed (Install Language Server runs: {})",
                            manifest.command,
                            steps
                                .iter()
                                .map(|s| s.display())
                                .collect::<Vec<_>>()
                                .join(" && ")
                        ),
                        install::InstallPlan::Manual(hint) => hint,
                    },
                    None => format!("{} is not installed", manifest.command),
                };
                tracing::info!("{hint}");
                hint
            } else {
                tracing::warn!("cannot start {}: {err}", manifest.id);
                format!("cannot start {}: {err}", manifest.id)
            }
        })?;
        tracing::info!("started {} in {}", manifest.id, slot.key.root.display());
        let ready = server
            .clone()
            .initialize()
            .map(|r| r.map_err(|e| e.to_string()))
            .boxed()
            .shared();
        slot.generation = generation;
        slot.state = SlotState::Running {
            server,
            ready: ready.clone(),
        };
        Ok(ready)
    }

    /// The running, initialized server for `key`, starting (or restarting
    /// after a crash) as needed. `None` when it cannot run.
    async fn ensure(
        self: &Arc<Self>,
        key: Key,
        language: &str,
        manifest: &LanguageServerManifest,
    ) -> Option<Arc<LanguageServer>> {
        let (server, ready, generation) = {
            let mut slots = lock(&self.slots);
            let index = match slots.iter().position(|s| s.key == key) {
                Some(index) => index,
                None => {
                    slots.push(Slot {
                        key: key.clone(),
                        language: language.to_string(),
                        generation: 0,
                        state: SlotState::Stopped,
                        restarts: Vec::new(),
                        last_used: Instant::now(),
                    });
                    slots.len() - 1
                }
            };
            let slot = &mut slots[index];
            slot.last_used = Instant::now();
            let running = match &slot.state {
                SlotState::Running { server, ready } if server.is_running() => {
                    Some((server.clone(), ready.clone()))
                }
                SlotState::Failed(_) => return None,
                _ => None,
            };
            match running {
                Some((server, ready)) => (server, ready, slot.generation),
                None => {
                    if matches!(slot.state, SlotState::Running { .. }) {
                        // The process is gone: a crash.
                        let now = Instant::now();
                        slot.restarts
                            .retain(|t| now.duration_since(*t) < RESTART_WINDOW);
                        if slot.restarts.len() >= MAX_RESTARTS {
                            tracing::warn!("{} keeps crashing; not restarted", key.id);
                            slot.state = SlotState::Failed(format!(
                                "{} crashed {MAX_RESTARTS} times",
                                key.id
                            ));
                            drop(slots);
                            self.emit(LanguageEvent::ServersChanged);
                            return None;
                        }
                        slot.restarts.push(now);
                        tracing::info!("restarting {} after a crash", key.id);
                    }
                    let started = self.start(slot, manifest);
                    let generation = slot.generation;
                    let result = match started {
                        Ok(ready) => match &slot.state {
                            SlotState::Running { server, .. } => Ok((server.clone(), ready)),
                            _ => Err(String::new()),
                        },
                        Err(message) => {
                            slot.state = SlotState::Failed(message);
                            Err(String::new())
                        }
                    };
                    drop(slots);
                    self.enforce_limits(Some(&key));
                    self.emit(LanguageEvent::ServersChanged);
                    match result {
                        Ok((server, ready)) => (server, ready, generation),
                        Err(_) => return None,
                    }
                }
            }
        };
        match ready.await {
            Ok(()) => {
                self.stop_idle(&key);
                Some(server)
            }
            Err(message) => {
                // What the server said before it gave up, e.g. a rustup
                // proxy without the component.
                let message = match server.stderr_tail().last() {
                    Some(line) => format!("{message}: {line}"),
                    None => message,
                };
                tracing::warn!("{} failed to initialize: {message}", key.id);
                let mut slots = lock(&self.slots);
                if let Some(slot) = slots
                    .iter_mut()
                    .find(|s| s.key == key && s.generation == generation)
                {
                    server.kill();
                    slot.state = SlotState::Failed(message);
                }
                drop(slots);
                self.emit(LanguageEvent::ServersChanged);
                None
            }
        }
    }

    fn stop_slot(&self, slot: &mut Slot) -> Option<Key> {
        if let SlotState::Running { server, .. } =
            std::mem::replace(&mut slot.state, SlotState::Stopped)
        {
            tracing::info!("stopping {}", slot.key.label());
            server.stop();
            return Some(slot.key.clone());
        }
        None
    }

    /// Keep at most `max_servers` running: stop the least recently used.
    fn enforce_limits(&self, keep: Option<&Key>) {
        let max = self.options().max_servers.max(1);
        let mut stopped = Vec::new();
        {
            let mut slots = lock(&self.slots);
            loop {
                let running: Vec<usize> = (0..slots.len())
                    .filter(|&i| slots[i].running().is_some())
                    .collect();
                if running.len() <= max {
                    break;
                }
                let Some(&victim) = running
                    .iter()
                    .filter(|&&i| Some(&slots[i].key) != keep)
                    .min_by_key(|&&i| slots[i].last_used)
                else {
                    break;
                };
                stopped.extend(self.stop_slot(&mut slots[victim]));
            }
        }
        self.after_stop(stopped);
    }

    fn stop_idle(&self, keep: &Key) {
        let timeout = self.options().idle_timeout;
        let mut stopped = Vec::new();
        {
            let mut slots = lock(&self.slots);
            for slot in slots.iter_mut() {
                if &slot.key != keep
                    && slot.running().is_some()
                    && slot.last_used.elapsed() > timeout
                {
                    stopped.extend(self.stop_slot(slot));
                }
            }
        }
        self.after_stop(stopped);
    }

    fn stop_all(&self) {
        let stopped: Vec<Key> = lock(&self.slots)
            .iter_mut()
            .filter_map(|slot| self.stop_slot(slot))
            .collect();
        self.after_stop(stopped);
    }

    fn after_stop(&self, stopped: Vec<Key>) {
        if stopped.is_empty() {
            return;
        }
        for key in &stopped {
            self.clear_diagnostics(key);
        }
        self.emit(LanguageEvent::ServersChanged);
    }

    fn clear_diagnostics(&self, key: &Key) {
        let label = key.label();
        let mut changed = Vec::new();
        lock(&self.diagnostics).retain(|path, per_server| {
            if per_server.remove(&label).is_some() {
                changed.push(path.clone());
            }
            !per_server.is_empty()
        });
        for path in changed {
            self.emit(LanguageEvent::DiagnosticsChanged(self.editor_path(&path)));
        }
    }

    fn server_of(&self, key: &Key, generation: u64) -> Option<Arc<LanguageServer>> {
        lock(&self.slots)
            .iter()
            .find(|s| &s.key == key && s.generation == generation)
            .and_then(|s| match &s.state {
                SlotState::Running { server, .. } => Some(server.clone()),
                _ => None,
            })
    }

    /// Runs on a server's reader thread.
    fn on_event(&self, key: &Key, generation: u64, event: ServerEvent) {
        match event {
            ServerEvent::Diagnostics { uri, diagnostics } => {
                let Some(path) = uri_to_path(&uri) else {
                    return;
                };
                let server = self.server_of(key, generation);
                let Some(server) = server else { return };
                let text = server
                    .document_text(&path)
                    .or_else(|| std::fs::read_to_string(&path).ok())
                    .unwrap_or_default();
                self.store_diagnostics(&server, key, &path, &text, diagnostics);
            }
            ServerEvent::Busy(_) => self.emit(LanguageEvent::ServersChanged),
            ServerEvent::Message { level, text } => {
                if level == lsp::MessageType::ERROR || level == lsp::MessageType::WARNING {
                    tracing::warn!("{}: {text}", key.id);
                } else {
                    tracing::info!("{}: {text}", key.id);
                }
            }
            ServerEvent::Refresh => self.emit(LanguageEvent::RefreshRequested),
            ServerEvent::Exited => {
                let crashed = lock(&self.slots).iter().any(|s| {
                    &s.key == key
                        && s.generation == generation
                        && matches!(s.state, SlotState::Running { .. })
                });
                if crashed {
                    tracing::warn!("{} exited unexpectedly", key.label());
                    self.clear_diagnostics(key);
                    self.emit(LanguageEvent::ServersChanged);
                }
            }
        }
    }

    fn store_diagnostics(
        &self,
        server: &LanguageServer,
        key: &Key,
        path: &Path,
        text: &str,
        raw: Vec<lsp::Diagnostic>,
    ) {
        let normalized = normalize(path);
        let editor_path = self.editor_path(&normalized);
        let converted = convert::diagnostics(
            &editor_path,
            server.id(),
            &Lines::new(text),
            server.encoding(),
            raw.clone(),
        );
        {
            let mut all = lock(&self.diagnostics);
            let per_server = all.entry(normalized.clone()).or_default();
            if converted.is_empty() {
                per_server.remove(&key.label());
            } else {
                per_server.insert(key.label(), (converted, raw));
            }
            if per_server.is_empty() {
                all.remove(&normalized);
            }
        }
        self.emit(LanguageEvent::DiagnosticsChanged(editor_path));
    }

    /// Servers for a request on `path`, with the document synchronized.
    /// The flag says whether any language server covers the file at all
    /// (false: the tree-sitter fallback answers).
    async fn servers_for(
        self: &Arc<Self>,
        path: &Path,
        text: &str,
        feature: Feature,
    ) -> (bool, Vec<Arc<LanguageServer>>) {
        if !self.trusted.load(Ordering::SeqCst) {
            return (false, Vec::new());
        }
        let Some(pack) = self.pack(path, text) else {
            return (false, Vec::new());
        };
        self.remember_path(path);
        let root = self.root_for(path, &pack.manifest().project_markers);
        let mut covered = false;
        let mut out = Vec::new();
        for manifest in self.candidates(&pack, feature) {
            let key = Key {
                id: manifest.id.clone(),
                root: root.clone(),
            };
            let Some(server) = self.ensure(key, pack.id(), &manifest).await else {
                continue;
            };
            covered = true;
            if feature.supported(&server) {
                server.sync_document(path, language_id(pack.id()), text);
                out.push(server);
            }
        }
        (covered, out)
    }

    fn running_servers(&self) -> Vec<Arc<LanguageServer>> {
        lock(&self.slots)
            .iter()
            .filter_map(|s| s.running().filter(|s| s.is_initialized()).cloned())
            .collect()
    }

    fn stored_diagnostics(&self, path: &Path) -> Vec<Diagnostic> {
        let mut out: Vec<Diagnostic> = lock(&self.diagnostics)
            .get(&normalize(path))
            .map(|per_server| per_server.values().flat_map(|(d, _)| d.clone()).collect())
            .unwrap_or_default();
        for d in &mut out {
            d.path = path.to_path_buf();
        }
        out.sort_by_key(|a| a.range.start);
        out
    }

    fn raw_diagnostics(
        &self,
        path: &Path,
        server: &LanguageServer,
        range: lsp::Range,
    ) -> Vec<lsp::Diagnostic> {
        let all = lock(&self.diagnostics);
        let Some(per_server) = all.get(&normalize(path)) else {
            return Vec::new();
        };
        per_server
            .iter()
            .filter(|(label, _)| label.starts_with(&format!("{}@", server.id())))
            .flat_map(|(_, (_, raw))| raw.iter())
            .filter(|d| d.range.start <= range.end && range.start <= d.range.end)
            .cloned()
            .collect()
    }
}

fn text_doc(path: &Path) -> lsp::TextDocumentIdentifier {
    lsp::TextDocumentIdentifier { uri: lsp_uri(path) }
}

fn position_params(
    path: &Path,
    text: &str,
    pos: TextPos,
    encoding: Encoding,
) -> lsp::TextDocumentPositionParams {
    lsp::TextDocumentPositionParams {
        text_document: text_doc(path),
        position: Lines::new(text).to_lsp(pos, encoding),
    }
}

/// Log and drop errors of passive requests (hover, completion, …).
fn quietly<T: Default>(server: &LanguageServer, method: &str, result: Result<T, RpcError>) -> T {
    match result {
        Ok(value) => value,
        Err(err) => {
            if !err.is_stale() {
                tracing::debug!("{} {method}: {err}", server.id());
            }
            T::default()
        }
    }
}

fn open_text(servers: &[Arc<LanguageServer>]) -> impl Fn(&Path) -> Option<String> + Sync + '_ {
    move |path: &Path| servers.iter().find_map(|s| s.document_text(path))
}

fn formatting_options(pack: Option<&LanguagePack>) -> lsp::FormattingOptions {
    let indent = pack
        .map(|p| p.manifest().indent.clone())
        .unwrap_or_default();
    lsp::FormattingOptions {
        tab_size: indent.tab_size,
        insert_spaces: indent.unit != "\t",
        trim_trailing_whitespace: Some(true),
        insert_final_newline: Some(true),
        ..Default::default()
    }
}

impl Inner {
    async fn diagnostics(self: Arc<Self>, path: PathBuf, text: String) -> Vec<Diagnostic> {
        let (covered, servers) = self.servers_for(&path, &text, Feature::Diagnostics).await;
        if !covered {
            return self
                .fallback
                .diagnostics(&path, &text)
                .await
                .unwrap_or_default();
        }
        for server in servers {
            if server.capabilities().diagnostic_provider.is_none() {
                continue;
            }
            let request = server.connection().request_value(
                "textDocument/diagnostic",
                json!({"textDocument": {"uri": path_to_uri(&path)}}),
                DEFAULT_TIMEOUT,
            );
            let report = quietly(&server, "diagnostic", request.await);
            if report.get("kind").and_then(Value::as_str) == Some("full") {
                let items: Vec<lsp::Diagnostic> = report
                    .get("items")
                    .cloned()
                    .and_then(|i| serde_json::from_value(i).ok())
                    .unwrap_or_default();
                let key = Key {
                    id: server.id().to_string(),
                    root: server.config().root.clone(),
                };
                self.store_diagnostics(&server, &key, &path, &text, items);
            }
        }
        self.stored_diagnostics(&path)
    }

    async fn completions(
        self: Arc<Self>,
        path: PathBuf,
        text: String,
        pos: TextPos,
    ) -> Vec<CompletionItem> {
        let (_, servers) = self.servers_for(&path, &text, Feature::Completion).await;
        let pack = self.pack(&path, &text);
        let mut items = Vec::new();
        let lines = Lines::new(&text);
        for server in &servers {
            let encoding = server.encoding();
            let caps = server.capabilities();
            let options = caps.completion_provider.clone().unwrap_or_default();
            let before = lines
                .line(pos.line)
                .chars()
                .nth((pos.column as usize).wrapping_sub(1))
                .map(String::from);
            let trigger = before.filter(|c| {
                options
                    .trigger_characters
                    .as_ref()
                    .is_some_and(|t| t.contains(c))
            });
            let params = lsp::CompletionParams {
                text_document_position: position_params(&path, &text, pos, encoding),
                work_done_progress_params: Default::default(),
                partial_result_params: Default::default(),
                context: Some(lsp::CompletionContext {
                    trigger_kind: if trigger.is_some() {
                        lsp::CompletionTriggerKind::TRIGGER_CHARACTER
                    } else {
                        lsp::CompletionTriggerKind::INVOKED
                    },
                    trigger_character: trigger,
                }),
            };
            let response = server
                .connection()
                .request::<req::Completion>(params, COMPLETION_TIMEOUT)
                .await;
            let mut list = match quietly(server, "completion", response) {
                Some(lsp::CompletionResponse::Array(list)) => list,
                Some(lsp::CompletionResponse::List(list)) => list.items,
                None => Vec::new(),
            };
            list.sort_by(|a, b| {
                a.sort_text
                    .as_deref()
                    .unwrap_or(&a.label)
                    .cmp(b.sort_text.as_deref().unwrap_or(&b.label))
            });
            if options.resolve_provider == Some(true) {
                let resolved = join_all(list.iter().take(RESOLVED_ITEMS).map(|item| {
                    let request = server
                        .connection()
                        .request::<req::ResolveCompletionItem>(item.clone(), RESOLVE_TIMEOUT);
                    async move { request.await.ok() }
                }))
                .await;
                for (slot, resolved) in list.iter_mut().zip(resolved) {
                    if let Some(resolved) = resolved {
                        *slot = resolved;
                    }
                }
            }
            items.extend(
                list.into_iter()
                    .map(|item| convert::completion(item, &text, &lines, encoding, pos)),
            );
        }
        if servers.is_empty() {
            items = self
                .fallback
                .completions(&path, &text, pos)
                .await
                .unwrap_or_default();
        }
        if let Some(pack) = pack {
            let unit = pack.manifest().indent.unit.clone();
            items.extend(templates::completions(pack.id(), &text, pos, &unit));
        }
        items
    }

    async fn hover(
        self: Arc<Self>,
        path: PathBuf,
        text: String,
        pos: TextPos,
    ) -> Option<HoverInfo> {
        let (_, servers) = self.servers_for(&path, &text, Feature::Hover).await;
        let lines = Lines::new(&text);
        for server in &servers {
            let params = lsp::HoverParams {
                text_document_position_params: position_params(
                    &path,
                    &text,
                    pos,
                    server.encoding(),
                ),
                work_done_progress_params: Default::default(),
            };
            let response = server
                .connection()
                .request::<req::HoverRequest>(params, QUICK_TIMEOUT)
                .await;
            if let Some(hover) = quietly(server, "hover", response)
                .and_then(|h| convert::hover(h, &lines, server.encoding(), pos))
            {
                return Some(hover);
            }
        }
        None
    }

    async fn signature_help(
        self: Arc<Self>,
        path: PathBuf,
        text: String,
        pos: TextPos,
    ) -> Option<SignatureHelp> {
        let (_, servers) = self.servers_for(&path, &text, Feature::SignatureHelp).await;
        for server in &servers {
            let params = lsp::SignatureHelpParams {
                context: None,
                text_document_position_params: position_params(
                    &path,
                    &text,
                    pos,
                    server.encoding(),
                ),
                work_done_progress_params: Default::default(),
            };
            let response = server
                .connection()
                .request::<req::SignatureHelpRequest>(params, QUICK_TIMEOUT)
                .await;
            if let Some(help) =
                quietly(server, "signatureHelp", response).and_then(convert::signature_help)
            {
                return Some(help);
            }
        }
        None
    }

    fn store_action(
        &self,
        server: &Arc<LanguageServer>,
        action: lsp::CodeActionOrCommand,
    ) -> String {
        let id = self.next_action.fetch_add(1, Ordering::SeqCst);
        let mut actions = lock(&self.actions);
        // Keep the latest few hundred; older menus are gone.
        if actions.len() > 512 {
            let oldest = id.saturating_sub(256);
            actions.retain(|k, _| *k > oldest);
        }
        actions.insert(
            id,
            StoredAction {
                server: server.clone(),
                action,
            },
        );
        id.to_string()
    }

    async fn code_actions(
        self: Arc<Self>,
        path: PathBuf,
        text: String,
        range: TextRange,
        only: Option<Vec<lsp::CodeActionKind>>,
    ) -> Vec<(Arc<LanguageServer>, lsp::CodeActionOrCommand)> {
        let (_, servers) = self.servers_for(&path, &text, Feature::CodeActions).await;
        let lines = Lines::new(&text);
        let mut out = Vec::new();
        for server in &servers {
            let lsp_range = lines.range_to_lsp(range, server.encoding());
            let params = lsp::CodeActionParams {
                text_document: text_doc(&path),
                range: lsp_range,
                context: lsp::CodeActionContext {
                    diagnostics: self.raw_diagnostics(&path, server, lsp_range),
                    only: only.clone(),
                    trigger_kind: Some(lsp::CodeActionTriggerKind::INVOKED),
                },
                work_done_progress_params: Default::default(),
                partial_result_params: Default::default(),
            };
            let response = server
                .connection()
                .request::<req::CodeActionRequest>(params, DEFAULT_TIMEOUT)
                .await;
            for action in quietly(server, "codeAction", response).unwrap_or_default() {
                if let lsp::CodeActionOrCommand::CodeAction(a) = &action
                    && a.disabled.is_some()
                {
                    continue;
                }
                out.push((server.clone(), action));
            }
        }
        out
    }

    /// The edits of a code action: its own, or resolved, then those its
    /// command applies.
    async fn action_edit(
        self: Arc<Self>,
        path: PathBuf,
        text: String,
        server: Arc<LanguageServer>,
        action: lsp::CodeActionOrCommand,
    ) -> Result<WorkspaceEdit, RpcError> {
        let (mut edit, command) = match action {
            lsp::CodeActionOrCommand::Command(command) => (None, Some(command)),
            lsp::CodeActionOrCommand::CodeAction(mut action) => {
                let resolvable = match server.capabilities().code_action_provider {
                    Some(lsp::CodeActionProviderCapability::Options(o)) => {
                        o.resolve_provider == Some(true)
                    }
                    _ => false,
                };
                if action.edit.is_none() && resolvable {
                    action = server
                        .connection()
                        .request::<req::CodeActionResolveRequest>(action, DEFAULT_TIMEOUT)
                        .await?;
                }
                (action.edit, action.command)
            }
        };
        let mut applied = Vec::new();
        if let Some(command) = command {
            applied = server.execute_command(&command, LONG_TIMEOUT).await?;
        }
        let servers = [server.clone()];
        let open = open_text(&servers);
        let mut texts = Texts::new(&path, &text, &open);
        let mut out = WorkspaceEdit::default();
        for e in edit.take().into_iter().chain(applied) {
            let converted = convert::workspace_edit(e, &mut texts, server.encoding());
            out.files.extend(converted.files);
        }
        Ok(out)
    }
}

impl LanguageService for LspLanguageService {
    fn diagnostics(
        &self,
        path: &Path,
        text: &str,
    ) -> BoxFuture<'static, ferro_services::Result<Vec<Diagnostic>>> {
        let (inner, path, text) = (self.inner.clone(), path.to_path_buf(), text.to_string());
        Box::pin(async move { Ok(inner.diagnostics(path, text).await) })
    }

    fn project_diagnostics(&self) -> BoxFuture<'static, ferro_services::Result<Vec<Diagnostic>>> {
        let inner = self.inner.clone();
        Box::pin(async move {
            let mut all: Vec<Diagnostic> = {
                let stored = lock(&inner.diagnostics);
                stored
                    .iter()
                    .flat_map(|(path, per_server)| {
                        let editor_path = inner.editor_path(path);
                        per_server.values().flat_map(move |(d, _)| {
                            let editor_path = editor_path.clone();
                            d.iter().cloned().map(move |mut d| {
                                d.path = editor_path.clone();
                                d
                            })
                        })
                    })
                    .collect()
            };
            all.extend(
                inner
                    .fallback
                    .project_diagnostics()
                    .await
                    .unwrap_or_default(),
            );
            all.sort_by(|a, b| a.path.cmp(&b.path).then(a.range.start.cmp(&b.range.start)));
            all.dedup();
            Ok(all)
        })
    }

    fn completions(
        &self,
        path: &Path,
        text: &str,
        pos: TextPos,
    ) -> BoxFuture<'static, ferro_services::Result<Vec<CompletionItem>>> {
        let (inner, path, text) = (self.inner.clone(), path.to_path_buf(), text.to_string());
        Box::pin(async move { Ok(inner.completions(path, text, pos).await) })
    }

    fn hover(
        &self,
        path: &Path,
        text: &str,
        pos: TextPos,
    ) -> BoxFuture<'static, ferro_services::Result<Option<HoverInfo>>> {
        let (inner, path, text) = (self.inner.clone(), path.to_path_buf(), text.to_string());
        Box::pin(async move { Ok(inner.hover(path, text, pos).await) })
    }

    fn signature_help(
        &self,
        path: &Path,
        text: &str,
        pos: TextPos,
    ) -> BoxFuture<'static, ferro_services::Result<Option<SignatureHelp>>> {
        let (inner, path, text) = (self.inner.clone(), path.to_path_buf(), text.to_string());
        Box::pin(async move { Ok(inner.signature_help(path, text, pos).await) })
    }

    fn code_actions(
        &self,
        path: &Path,
        text: &str,
        range: TextRange,
    ) -> BoxFuture<'static, ferro_services::Result<Vec<QuickFix>>> {
        let (inner, path, text) = (self.inner.clone(), path.to_path_buf(), text.to_string());
        Box::pin(async move {
            let actions = inner
                .clone()
                .code_actions(path.clone(), text.clone(), range, None)
                .await;
            let lines = Lines::new(&text);
            let uri = lsp_uri(&path);
            Ok(actions
                .into_iter()
                .map(|(server, action)| {
                    let (title, kind, preferred, edits) = match &action {
                        lsp::CodeActionOrCommand::Command(c) => {
                            (c.title.clone(), None, false, Vec::new())
                        }
                        lsp::CodeActionOrCommand::CodeAction(a) => {
                            let edits = a
                                .edit
                                .as_ref()
                                .and_then(|e| e.changes.as_ref())
                                .and_then(|c| c.get(&uri))
                                .map(|e| convert::text_edits(e.clone(), &lines, server.encoding()))
                                .unwrap_or_default();
                            (
                                a.title.clone(),
                                a.kind.clone(),
                                a.is_preferred == Some(true),
                                edits,
                            )
                        }
                    };
                    QuickFix {
                        title,
                        kind: convert::quick_fix_kind(kind.as_ref()),
                        edits,
                        preferred,
                        data: Some(inner.store_action(&server, action)),
                    }
                })
                .collect())
        })
    }

    fn resolve_quick_fix(
        &self,
        path: &Path,
        text: &str,
        fix: &QuickFix,
    ) -> BoxFuture<'static, ferro_services::Result<WorkspaceEdit>> {
        let stored = fix
            .data
            .as_ref()
            .and_then(|d| d.parse::<u64>().ok())
            .and_then(|id| {
                lock(&self.inner.actions)
                    .get(&id)
                    .map(|a| (a.server.clone(), a.action.clone()))
            });
        let Some((server, action)) = stored else {
            let edit = WorkspaceEdit {
                files: vec![ferro_services::FileEdit {
                    path: path.to_path_buf(),
                    edits: ferro_services::preview_edits(text, &fix.edits),
                }],
            };
            return Box::pin(async move { Ok(edit) });
        };
        let (inner, path, text) = (self.inner.clone(), path.to_path_buf(), text.to_string());
        Box::pin(async move { Ok(inner.action_edit(path, text, server, action).await?) })
    }

    fn inlay_hints(
        &self,
        path: &Path,
        text: &str,
    ) -> BoxFuture<'static, ferro_services::Result<Vec<InlayHint>>> {
        let (inner, path, text) = (self.inner.clone(), path.to_path_buf(), text.to_string());
        Box::pin(async move {
            if inner.options().power_save {
                return Ok(Vec::new());
            }
            let (_, servers) = inner.servers_for(&path, &text, Feature::InlayHints).await;
            let lines = Lines::new(&text);
            let mut hints = Vec::new();
            for server in &servers {
                let end = lsp::Position::new(lines.len() as u32, 0);
                let params = lsp::InlayHintParams {
                    text_document: text_doc(&path),
                    range: lsp::Range::new(lsp::Position::new(0, 0), end),
                    work_done_progress_params: Default::default(),
                };
                let response = server
                    .connection()
                    .request::<req::InlayHintRequest>(params, DEFAULT_TIMEOUT)
                    .await;
                hints.extend(
                    quietly(server, "inlayHint", response)
                        .unwrap_or_default()
                        .into_iter()
                        .map(|h| convert::inlay_hint(h, &lines, server.encoding())),
                );
            }
            Ok(hints)
        })
    }

    fn document_symbols(
        &self,
        path: &Path,
        text: &str,
    ) -> BoxFuture<'static, ferro_services::Result<Vec<Symbol>>> {
        let (inner, path, text) = (self.inner.clone(), path.to_path_buf(), text.to_string());
        Box::pin(async move {
            let (_, servers) = inner.servers_for(&path, &text, Feature::Symbols).await;
            let lines = Lines::new(&text);
            for server in &servers {
                let params = lsp::DocumentSymbolParams {
                    text_document: text_doc(&path),
                    work_done_progress_params: Default::default(),
                    partial_result_params: Default::default(),
                };
                let response = server
                    .connection()
                    .request::<req::DocumentSymbolRequest>(params, DEFAULT_TIMEOUT)
                    .await;
                if let Some(symbols) = quietly(server, "documentSymbol", response) {
                    let symbols =
                        convert::document_symbols(symbols, &path, &lines, server.encoding());
                    if !symbols.is_empty() {
                        return Ok(symbols);
                    }
                }
            }
            inner.fallback.document_symbols(&path, &text).await
        })
    }

    fn workspace_symbols(
        &self,
        query: &str,
    ) -> BoxFuture<'static, ferro_services::Result<Vec<Symbol>>> {
        let (inner, query) = (self.inner.clone(), query.to_string());
        Box::pin(async move {
            let servers: Vec<_> = inner
                .running_servers()
                .into_iter()
                .filter(|s| {
                    !matches!(
                        s.capabilities().workspace_symbol_provider,
                        None | Some(lsp::OneOf::Left(false))
                    )
                })
                .collect();
            let mut symbols = Vec::new();
            let open = open_text(&servers);
            let mut texts = Texts::new(Path::new(""), "", &open);
            for server in &servers {
                let params = lsp::WorkspaceSymbolParams {
                    query: query.clone(),
                    work_done_progress_params: Default::default(),
                    partial_result_params: Default::default(),
                };
                let response = server
                    .connection()
                    .request::<req::WorkspaceSymbolRequest>(params, DEFAULT_TIMEOUT)
                    .await;
                if let Some(found) = quietly(server, "workspace/symbol", response) {
                    symbols.extend(convert::workspace_symbols(
                        found,
                        &mut texts,
                        server.encoding(),
                    ));
                }
            }
            symbols.extend(
                inner
                    .fallback
                    .workspace_symbols(&query)
                    .await
                    .unwrap_or_default(),
            );
            let mut seen = std::collections::HashSet::new();
            symbols
                .retain(|s| seen.insert((s.name.clone(), normalize(&s.path), s.range.start.line)));
            let names: Vec<&str> = symbols.iter().map(|s| s.name.as_str()).collect();
            Ok(fuzzy::rank(&query, names, 200)
                .into_iter()
                .map(|(i, _)| symbols[i].clone())
                .collect())
        })
    }

    fn runnables(&self, path: &Path, text: &str) -> Vec<Runnable> {
        self.inner.fallback.runnables(path, text)
    }

    fn rename(
        &self,
        path: &Path,
        text: &str,
        pos: TextPos,
        new_name: &str,
    ) -> BoxFuture<'static, ferro_services::Result<WorkspaceEdit>> {
        let (inner, path, text, new_name) = (
            self.inner.clone(),
            path.to_path_buf(),
            text.to_string(),
            new_name.to_string(),
        );
        Box::pin(async move {
            let (_, servers) = inner.servers_for(&path, &text, Feature::Rename).await;
            let Some(server) = servers.first() else {
                return inner.fallback.rename(&path, &text, pos, &new_name).await;
            };
            let params = lsp::RenameParams {
                text_document_position: position_params(&path, &text, pos, server.encoding()),
                new_name,
                work_done_progress_params: Default::default(),
            };
            let edit = server
                .connection()
                .request::<req::Rename>(params, LONG_TIMEOUT)
                .await?
                .unwrap_or_default();
            let open = open_text(&servers);
            let mut texts = Texts::new(&path, &text, &open);
            Ok(convert::workspace_edit(edit, &mut texts, server.encoding()))
        })
    }

    fn goto(
        &self,
        path: &Path,
        text: &str,
        pos: TextPos,
        kind: GotoKind,
    ) -> BoxFuture<'static, ferro_services::Result<Vec<Location>>> {
        let (inner, path, text) = (self.inner.clone(), path.to_path_buf(), text.to_string());
        Box::pin(async move {
            let (_, servers) = inner.servers_for(&path, &text, Feature::Definition).await;
            for server in &servers {
                let caps = server.capabilities();
                let supported = match kind {
                    GotoKind::Definition => true,
                    GotoKind::Declaration => caps.declaration_provider.is_some(),
                    GotoKind::TypeDefinition => caps.type_definition_provider.is_some(),
                    GotoKind::Implementation => caps.implementation_provider.is_some(),
                };
                if !supported {
                    continue;
                }
                let params = lsp::GotoDefinitionParams {
                    text_document_position_params: position_params(
                        &path,
                        &text,
                        pos,
                        server.encoding(),
                    ),
                    work_done_progress_params: Default::default(),
                    partial_result_params: Default::default(),
                };
                let conn = server.connection();
                let response = match kind {
                    GotoKind::Definition => {
                        conn.request::<req::GotoDefinition>(params, DEFAULT_TIMEOUT)
                            .await
                    }
                    GotoKind::Declaration => {
                        conn.request::<req::GotoDeclaration>(params, DEFAULT_TIMEOUT)
                            .await
                    }
                    GotoKind::TypeDefinition => {
                        conn.request::<req::GotoTypeDefinition>(params, DEFAULT_TIMEOUT)
                            .await
                    }
                    GotoKind::Implementation => {
                        conn.request::<req::GotoImplementation>(params, LONG_TIMEOUT)
                            .await
                    }
                };
                if let Some(response) = quietly(server, "goto", response) {
                    let open = open_text(&servers);
                    let mut texts = Texts::new(&path, &text, &open);
                    let found = convert::goto_locations(response, &mut texts, server.encoding());
                    if !found.is_empty() {
                        return Ok(found);
                    }
                }
            }
            if kind == GotoKind::Definition {
                // Without a server: a definition of the word at the caret
                // from the tree-sitter index.
                let word = word_at(&text, pos);
                if !word.is_empty() {
                    let symbols = inner
                        .fallback
                        .workspace_symbols(&word)
                        .await
                        .unwrap_or_default();
                    return Ok(symbols
                        .into_iter()
                        .filter(|s| s.name == word)
                        .map(|s| Location {
                            line_text: std::fs::read_to_string(&s.path)
                                .ok()
                                .and_then(|t| {
                                    t.lines().nth(s.selection.line as usize).map(str::to_string)
                                })
                                .unwrap_or_default(),
                            range: TextRange::new(s.selection, s.selection),
                            path: s.path,
                        })
                        .collect());
                }
            }
            Ok(Vec::new())
        })
    }

    fn references(
        &self,
        path: &Path,
        text: &str,
        pos: TextPos,
        include_declaration: bool,
    ) -> BoxFuture<'static, ferro_services::Result<Vec<Location>>> {
        let (inner, path, text) = (self.inner.clone(), path.to_path_buf(), text.to_string());
        Box::pin(async move {
            let (_, servers) = inner.servers_for(&path, &text, Feature::References).await;
            let mut out = Vec::new();
            for server in &servers {
                let params = lsp::ReferenceParams {
                    text_document_position: position_params(&path, &text, pos, server.encoding()),
                    context: lsp::ReferenceContext {
                        include_declaration,
                    },
                    work_done_progress_params: Default::default(),
                    partial_result_params: Default::default(),
                };
                let found = server
                    .connection()
                    .request::<req::References>(params, LONG_TIMEOUT)
                    .await?
                    .unwrap_or_default();
                let open = open_text(&servers);
                let mut texts = Texts::new(&path, &text, &open);
                out.extend(
                    found
                        .into_iter()
                        .filter_map(|l| texts.location(&l.uri, l.range, server.encoding())),
                );
                if !out.is_empty() {
                    break;
                }
            }
            out.sort_by(|a, b| a.path.cmp(&b.path).then(a.range.start.cmp(&b.range.start)));
            out.dedup();
            Ok(out)
        })
    }

    fn format(
        &self,
        path: &Path,
        text: &str,
        range: Option<TextRange>,
    ) -> BoxFuture<'static, ferro_services::Result<Vec<TextEdit>>> {
        let (inner, path, text) = (self.inner.clone(), path.to_path_buf(), text.to_string());
        Box::pin(async move {
            let (_, servers) = inner.servers_for(&path, &text, Feature::Formatting).await;
            let pack = inner.pack(&path, &text);
            let options = formatting_options(pack.as_deref());
            let lines = Lines::new(&text);
            for server in &servers {
                let caps = server.capabilities();
                let ranged = !matches!(
                    caps.document_range_formatting_provider,
                    None | Some(lsp::OneOf::Left(false))
                );
                let response = match range {
                    Some(range) if ranged => {
                        let params = lsp::DocumentRangeFormattingParams {
                            text_document: text_doc(&path),
                            range: lines.range_to_lsp(range, server.encoding()),
                            options: options.clone(),
                            work_done_progress_params: Default::default(),
                        };
                        server
                            .connection()
                            .request::<req::RangeFormatting>(params, DEFAULT_TIMEOUT)
                            .await
                    }
                    _ => {
                        if matches!(
                            caps.document_formatting_provider,
                            None | Some(lsp::OneOf::Left(false))
                        ) && !server.has_registration("textDocument/formatting")
                        {
                            continue;
                        }
                        let params = lsp::DocumentFormattingParams {
                            text_document: text_doc(&path),
                            options: options.clone(),
                            work_done_progress_params: Default::default(),
                        };
                        server
                            .connection()
                            .request::<req::Formatting>(params, DEFAULT_TIMEOUT)
                            .await
                    }
                };
                if let Some(edits) = response? {
                    return Ok(convert::text_edits(edits, &lines, server.encoding()));
                }
            }
            Ok(Vec::new())
        })
    }

    fn format_on_type(
        &self,
        path: &Path,
        text: &str,
        pos: TextPos,
        ch: char,
    ) -> BoxFuture<'static, ferro_services::Result<Vec<TextEdit>>> {
        let (inner, path, text) = (self.inner.clone(), path.to_path_buf(), text.to_string());
        Box::pin(async move {
            let (_, servers) = inner.servers_for(&path, &text, Feature::Formatting).await;
            let pack = inner.pack(&path, &text);
            let lines = Lines::new(&text);
            for server in &servers {
                let Some(on_type) = server.capabilities().document_on_type_formatting_provider
                else {
                    continue;
                };
                let ch_text = ch.to_string();
                if on_type.first_trigger_character != ch_text
                    && !on_type
                        .more_trigger_character
                        .unwrap_or_default()
                        .contains(&ch_text)
                {
                    continue;
                }
                let params = lsp::DocumentOnTypeFormattingParams {
                    text_document_position: position_params(&path, &text, pos, server.encoding()),
                    ch: ch_text,
                    options: formatting_options(pack.as_deref()),
                };
                let response = server
                    .connection()
                    .request::<req::OnTypeFormatting>(params, QUICK_TIMEOUT)
                    .await;
                if let Some(edits) = quietly(server, "onTypeFormatting", response) {
                    return Ok(convert::text_edits(edits, &lines, server.encoding()));
                }
            }
            Ok(Vec::new())
        })
    }

    fn format_on_type_triggers(&self, path: &Path) -> Vec<char> {
        let Some(pack) = self.inner.registry.detect_path(path).cloned() else {
            return Vec::new();
        };
        let mut out = Vec::new();
        for slot in lock(&self.inner.slots).iter() {
            if slot.language != pack.id() {
                continue;
            }
            if let Some(on_type) = slot
                .running()
                .and_then(|s| s.capabilities().document_on_type_formatting_provider)
            {
                out.extend(on_type.first_trigger_character.chars().next());
                for more in on_type.more_trigger_character.unwrap_or_default() {
                    out.extend(more.chars().next());
                }
            }
        }
        out.sort();
        out.dedup();
        out
    }

    fn organize_imports(
        &self,
        path: &Path,
        text: &str,
    ) -> BoxFuture<'static, ferro_services::Result<Vec<TextEdit>>> {
        let (inner, path, text) = (self.inner.clone(), path.to_path_buf(), text.to_string());
        Box::pin(async move {
            let lines = Lines::new(&text);
            let whole = TextRange::new(TextPos::new(0, 0), TextPos::new(lines.len() as u32, 0));
            let actions = inner
                .clone()
                .code_actions(
                    path.clone(),
                    text.clone(),
                    whole,
                    Some(vec![lsp::CodeActionKind::SOURCE_ORGANIZE_IMPORTS]),
                )
                .await;
            let organize = actions.into_iter().find(|(_, a)| match a {
                lsp::CodeActionOrCommand::CodeAction(a) => a
                    .kind
                    .as_ref()
                    .is_some_and(|k| k.as_str().starts_with("source.organizeImports")),
                lsp::CodeActionOrCommand::Command(_) => true,
            });
            let Some((server, action)) = organize else {
                return Ok(Vec::new());
            };
            let edit = inner
                .action_edit(path.clone(), text, server, action)
                .await?;
            let target = normalize(&path);
            Ok(edit
                .files
                .into_iter()
                .filter(|f| normalize(&f.path) == target)
                .flat_map(|f| f.edits.into_iter().map(|e| e.edit))
                .collect())
        })
    }

    fn semantic_tokens(
        &self,
        path: &Path,
        text: &str,
    ) -> BoxFuture<'static, ferro_services::Result<Option<Vec<SemanticToken>>>> {
        let (inner, path, text) = (self.inner.clone(), path.to_path_buf(), text.to_string());
        Box::pin(async move {
            if inner.options().power_save {
                return Ok(None);
            }
            let (_, servers) = inner
                .servers_for(&path, &text, Feature::SemanticTokens)
                .await;
            let lines = Lines::new(&text);
            for server in &servers {
                let legend = match server.capabilities().semantic_tokens_provider {
                    Some(lsp::SemanticTokensServerCapabilities::SemanticTokensOptions(o)) => {
                        o.legend
                    }
                    Some(
                        lsp::SemanticTokensServerCapabilities::SemanticTokensRegistrationOptions(o),
                    ) => o.semantic_tokens_options.legend,
                    None => continue,
                };
                let params = lsp::SemanticTokensParams {
                    text_document: text_doc(&path),
                    work_done_progress_params: Default::default(),
                    partial_result_params: Default::default(),
                };
                let response = server
                    .connection()
                    .request::<req::SemanticTokensFullRequest>(params, DEFAULT_TIMEOUT)
                    .await;
                if let Some(lsp::SemanticTokensResult::Tokens(tokens)) =
                    quietly(server, "semanticTokens", response)
                {
                    return Ok(Some(convert::semantic_tokens(
                        &tokens.data,
                        &legend,
                        &lines,
                        server.encoding(),
                    )));
                }
            }
            Ok(None)
        })
    }

    fn code_lenses(
        &self,
        path: &Path,
        text: &str,
    ) -> BoxFuture<'static, ferro_services::Result<Vec<CodeLens>>> {
        let (inner, path, text) = (self.inner.clone(), path.to_path_buf(), text.to_string());
        Box::pin(async move {
            if inner.options().power_save {
                return Ok(Vec::new());
            }
            let (_, servers) = inner.servers_for(&path, &text, Feature::CodeLens).await;
            let lines = Lines::new(&text);
            let mut out = Vec::new();
            for server in &servers {
                let params = lsp::CodeLensParams {
                    text_document: text_doc(&path),
                    work_done_progress_params: Default::default(),
                    partial_result_params: Default::default(),
                };
                let response = server
                    .connection()
                    .request::<req::CodeLensRequest>(params, DEFAULT_TIMEOUT)
                    .await;
                let lenses = quietly(server, "codeLens", response).unwrap_or_default();
                let resolvable = server
                    .capabilities()
                    .code_lens_provider
                    .and_then(|o| o.resolve_provider)
                    == Some(true);
                let lenses = join_all(lenses.into_iter().take(100).map(|lens| {
                    let request = (lens.command.is_none() && resolvable).then(|| {
                        server
                            .connection()
                            .request::<req::CodeLensResolve>(lens.clone(), RESOLVE_TIMEOUT)
                    });
                    async move {
                        match request {
                            Some(request) => request.await.unwrap_or(lens),
                            None => lens,
                        }
                    }
                }))
                .await;
                out.extend(
                    lenses
                        .into_iter()
                        .filter_map(|l| convert::code_lens(l, &lines, server.encoding())),
                );
            }
            Ok(out)
        })
    }

    fn document_links(
        &self,
        path: &Path,
        text: &str,
    ) -> BoxFuture<'static, ferro_services::Result<Vec<DocumentLink>>> {
        let (inner, path, text) = (self.inner.clone(), path.to_path_buf(), text.to_string());
        Box::pin(async move {
            let (_, servers) = inner.servers_for(&path, &text, Feature::Links).await;
            let lines = Lines::new(&text);
            let mut out = Vec::new();
            for server in &servers {
                let params = lsp::DocumentLinkParams {
                    text_document: text_doc(&path),
                    work_done_progress_params: Default::default(),
                    partial_result_params: Default::default(),
                };
                let response = server
                    .connection()
                    .request::<req::DocumentLinkRequest>(params, DEFAULT_TIMEOUT)
                    .await;
                out.extend(
                    quietly(server, "documentLink", response)
                        .unwrap_or_default()
                        .into_iter()
                        .filter_map(|l| convert::document_link(l, &lines, server.encoding())),
                );
            }
            Ok(out)
        })
    }

    fn hierarchy(
        &self,
        path: &Path,
        text: &str,
        pos: TextPos,
        direction: HierarchyDirection,
    ) -> BoxFuture<'static, ferro_services::Result<Vec<HierarchyItem>>> {
        let (inner, path, text) = (self.inner.clone(), path.to_path_buf(), text.to_string());
        Box::pin(async move {
            let (_, servers) = inner.servers_for(&path, &text, Feature::Hierarchy).await;
            for server in &servers {
                let caps = server.capabilities();
                let encoding = server.encoding();
                let conn = server.connection();
                let open = open_text(&servers);
                let mut texts = Texts::new(&path, &text, &open);
                let position = position_params(&path, &text, pos, encoding);
                let items = match direction {
                    HierarchyDirection::Incoming | HierarchyDirection::Outgoing => {
                        if caps.call_hierarchy_provider.is_none() {
                            continue;
                        }
                        let params = lsp::CallHierarchyPrepareParams {
                            text_document_position_params: position,
                            work_done_progress_params: Default::default(),
                        };
                        let prepared = conn
                            .request::<req::CallHierarchyPrepare>(params, DEFAULT_TIMEOUT)
                            .await?
                            .unwrap_or_default();
                        let Some(item) = prepared.into_iter().next() else {
                            continue;
                        };
                        if direction == HierarchyDirection::Incoming {
                            let params = lsp::CallHierarchyIncomingCallsParams {
                                item,
                                work_done_progress_params: Default::default(),
                                partial_result_params: Default::default(),
                            };
                            conn.request::<req::CallHierarchyIncomingCalls>(params, LONG_TIMEOUT)
                                .await?
                                .unwrap_or_default()
                                .into_iter()
                                .map(|c| c.from)
                                .collect::<Vec<_>>()
                        } else {
                            let params = lsp::CallHierarchyOutgoingCallsParams {
                                item,
                                work_done_progress_params: Default::default(),
                                partial_result_params: Default::default(),
                            };
                            conn.request::<req::CallHierarchyOutgoingCalls>(params, LONG_TIMEOUT)
                                .await?
                                .unwrap_or_default()
                                .into_iter()
                                .map(|c| c.to)
                                .collect()
                        }
                        .into_iter()
                        .filter_map(|i| convert::hierarchy_item(i, &mut texts, encoding))
                        .collect::<Vec<_>>()
                    }
                    HierarchyDirection::Supertypes | HierarchyDirection::Subtypes => {
                        let params = lsp::TypeHierarchyPrepareParams {
                            text_document_position_params: position,
                            work_done_progress_params: Default::default(),
                        };
                        let prepared = conn
                            .request::<req::TypeHierarchyPrepare>(params, DEFAULT_TIMEOUT)
                            .await;
                        let Some(item) = quietly(server, "typeHierarchy", prepared)
                            .unwrap_or_default()
                            .into_iter()
                            .next()
                        else {
                            continue;
                        };
                        let found = if direction == HierarchyDirection::Supertypes {
                            let params = lsp::TypeHierarchySupertypesParams {
                                item,
                                work_done_progress_params: Default::default(),
                                partial_result_params: Default::default(),
                            };
                            conn.request::<req::TypeHierarchySupertypes>(params, LONG_TIMEOUT)
                                .await?
                        } else {
                            let params = lsp::TypeHierarchySubtypesParams {
                                item,
                                work_done_progress_params: Default::default(),
                                partial_result_params: Default::default(),
                            };
                            conn.request::<req::TypeHierarchySubtypes>(params, LONG_TIMEOUT)
                                .await?
                        };
                        found
                            .unwrap_or_default()
                            .into_iter()
                            .filter_map(|i| convert::type_hierarchy_item(i, &mut texts, encoding))
                            .collect()
                    }
                };
                return Ok(items);
            }
            Ok(Vec::new())
        })
    }

    fn document_closed(&self, path: &Path) {
        for server in self.inner.running_servers() {
            server.close_document(path);
        }
    }

    fn document_saved(&self, path: &Path, text: &str) {
        for server in self.inner.running_servers() {
            server.save_document(path, text);
        }
    }

    fn files_changed(&self, events: &[ProjectEvent]) {
        let changes: Vec<(PathBuf, lsp::FileChangeType)> = events
            .iter()
            .filter_map(|e| match e {
                ProjectEvent::Added(p) => Some((p.clone(), lsp::FileChangeType::CREATED)),
                ProjectEvent::Changed(p) => Some((p.clone(), lsp::FileChangeType::CHANGED)),
                ProjectEvent::Removed(p) => Some((p.clone(), lsp::FileChangeType::DELETED)),
                ProjectEvent::RootsChanged => None,
            })
            .collect();
        if changes.is_empty() {
            return;
        }
        for server in self.inner.running_servers() {
            server.files_changed(&changes);
        }
    }

    fn set_trusted(&self, trusted: bool) {
        let was = self.inner.trusted.swap(trusted, Ordering::SeqCst);
        if was && !trusted {
            self.inner.stop_all();
        }
        if was != trusted {
            // Failed slots get another chance once trusted.
            lock(&self.inner.slots).retain(|s| !matches!(s.state, SlotState::Failed(_)));
            self.inner.emit(LanguageEvent::ServersChanged);
            self.inner.emit(LanguageEvent::RefreshRequested);
        }
    }

    fn servers(&self) -> Vec<LanguageServerStatus> {
        let slots = lock(&self.inner.slots);
        slots
            .iter()
            .filter_map(|slot| {
                let language = self
                    .inner
                    .registry
                    .get(&slot.language)
                    .map(|p| p.name().to_string())
                    .unwrap_or_else(|| slot.language.clone());
                let (health, memory) = match &slot.state {
                    SlotState::Running { server, .. } if server.is_running() => (
                        if server.is_initialized() && !server.is_busy() {
                            ServerHealth::Ready
                        } else {
                            ServerHealth::Starting
                        },
                        server.memory_mb().unwrap_or(0),
                    ),
                    SlotState::Running { .. } => (ServerHealth::Crashed, 0),
                    SlotState::Failed(_) => (ServerHealth::Crashed, 0),
                    SlotState::Stopped => return None,
                };
                Some(LanguageServerStatus {
                    name: slot.key.id.clone(),
                    language,
                    health,
                    memory_mb: memory,
                })
            })
            .collect()
    }

    fn subscribe(&self) -> Option<async_channel::Receiver<LanguageEvent>> {
        let (tx, rx) = async_channel::unbounded();
        lock(&self.inner.subscribers).push(tx);
        Some(rx)
    }
}

/// The identifier at (or just before) `pos`.
fn word_at(text: &str, pos: TextPos) -> String {
    let line: Vec<char> = text
        .split('\n')
        .nth(pos.line as usize)
        .unwrap_or_default()
        .chars()
        .collect();
    let is_word = |c: &char| c.is_alphanumeric() || *c == '_';
    let caret = (pos.column as usize).min(line.len());
    let start = line[..caret]
        .iter()
        .rposition(|c| !is_word(c))
        .map_or(0, |i| i + 1);
    let end = line[caret..]
        .iter()
        .position(|c| !is_word(c))
        .map_or(line.len(), |i| caret + i);
    line[start..end].iter().collect()
}
