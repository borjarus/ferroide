//! One language server process: launch, `initialize`, document
//! synchronization, server-to-client requests, file watching and shutdown.

use std::{
    collections::{HashMap, HashSet, VecDeque},
    io::{self, BufRead as _, BufReader, Read, Write},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::{
        Arc, Mutex, RwLock,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use globset::{GlobBuilder, GlobMatcher};
use lsp_types::{
    FileChangeType, MessageType, ServerCapabilities, TextDocumentSyncCapability,
    TextDocumentSyncKind, TextDocumentSyncSaveOptions,
};
use serde_json::{Value, json};

use crate::{
    position::{Encoding, incremental_change},
    rpc::{Connection, Handler, Incoming, RpcError, codes},
    uri::{path_to_uri, uri_to_path},
};

/// How long a server may take to answer `initialize` (jdtls and
/// rust-analyzer on a cold cache are slow).
pub const INITIALIZE_TIMEOUT: Duration = Duration::from_secs(60);
/// Lines of a server's stderr kept for crash reports.
const STDERR_TAIL: usize = 40;

/// What to start and where.
#[derive(Debug, Clone, PartialEq)]
pub struct ServerConfig {
    /// The manifest id, e.g. `rust-analyzer`.
    pub id: String,
    /// The Language Pack id the server was started for, e.g. `rust`.
    pub language: String,
    pub command: PathBuf,
    pub args: Vec<String>,
    pub env: Vec<(String, String)>,
    /// The workspace root (the folder with the pack's project marker).
    pub root: PathBuf,
    pub initialization_options: Option<Value>,
    /// Answers to `workspace/configuration`, by section (`"rust-analyzer"`,
    /// `"python.analysis"`, …).
    pub settings: Value,
}

/// The streams of a started server.
pub struct Transport {
    pub reader: Box<dyn Read + Send>,
    pub writer: Box<dyn Write + Send>,
    pub stderr: Option<Box<dyn Read + Send>>,
    pub process: Option<Child>,
}

/// Starts servers. [`ProcessLauncher`] runs real processes; tests run
/// servers in-process.
pub trait Launcher: Send + Sync + 'static {
    fn launch(&self, config: &ServerConfig) -> io::Result<Transport>;
}

/// Runs the configured command with piped stdio, without a console window
/// on Windows.
pub struct ProcessLauncher;

impl Launcher for ProcessLauncher {
    fn launch(&self, config: &ServerConfig) -> io::Result<Transport> {
        let mut command = Command::new(&config.command);
        command
            .args(&config.args)
            .current_dir(&config.root)
            .envs(config.env.iter().map(|(k, v)| (k, v)))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt as _;
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            command.creation_flags(CREATE_NO_WINDOW);
        }
        let mut child = command.spawn()?;
        let missing = || io::Error::other("no stdio pipe");
        Ok(Transport {
            reader: Box::new(child.stdout.take().ok_or_else(missing)?),
            writer: Box::new(child.stdin.take().ok_or_else(missing)?),
            stderr: child
                .stderr
                .take()
                .map(|e| Box::new(e) as Box<dyn Read + Send>),
            process: Some(child),
        })
    }
}

/// Something a server did on its own.
#[derive(Debug, Clone)]
pub enum ServerEvent {
    Diagnostics {
        uri: String,
        diagnostics: Vec<lsp_types::Diagnostic>,
    },
    /// Work done progress began or ended (indexing, building).
    Busy(bool),
    Message {
        level: MessageType,
        text: String,
    },
    /// `workspace/*/refresh`.
    Refresh,
    /// The process ended or closed its output.
    Exited,
}

pub type EventSink = Arc<dyn Fn(ServerEvent) + Send + Sync>;

/// A file watcher a server registered (`workspace/didChangeWatchedFiles`).
struct Watcher {
    registration: String,
    matcher: GlobMatcher,
    /// Paths are matched relative to this folder (`RelativePattern`).
    base: Option<PathBuf>,
    /// LSP `WatchKind` bits: 1 create, 2 change, 4 delete.
    kind: u8,
}

/// State the reader thread updates.
struct Shared {
    settings: Value,
    root: PathBuf,
    watchers: Mutex<Vec<Watcher>>,
    /// Dynamically registered methods, by registration id.
    registrations: Mutex<HashMap<String, String>>,
    /// Edits the server applied through `workspace/applyEdit` (collected
    /// while a command runs).
    applied_edits: Mutex<Vec<lsp_types::WorkspaceEdit>>,
    progress: Mutex<HashSet<String>>,
    stderr_tail: Mutex<VecDeque<String>>,
}

struct Document {
    version: i32,
    text: String,
}

pub struct LanguageServer {
    config: ServerConfig,
    conn: Arc<Connection>,
    shared: Arc<Shared>,
    capabilities: RwLock<ServerCapabilities>,
    encoding: RwLock<Encoding>,
    initialized: AtomicBool,
    process: Mutex<Option<Child>>,
    pid: Option<u32>,
    documents: Mutex<HashMap<String, Document>>,
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// The `section` (dotted) of `settings`, or all of them.
pub fn settings_section(settings: &Value, section: Option<&str>) -> Value {
    let Some(section) = section.filter(|s| !s.is_empty()) else {
        return settings.clone();
    };
    if let Some(value) = settings.get(section) {
        return value.clone();
    }
    let mut value = settings;
    for part in section.split('.') {
        match value.get(part) {
            Some(next) => value = next,
            None => return Value::Null,
        }
    }
    value.clone()
}

impl LanguageServer {
    /// Launch the process and connect to it. Call
    /// [`LanguageServer::initialize`] before anything else.
    pub fn launch(
        config: ServerConfig,
        launcher: &dyn Launcher,
        sink: EventSink,
    ) -> io::Result<Arc<Self>> {
        let transport = launcher.launch(&config)?;
        let shared = Arc::new(Shared {
            settings: config.settings.clone(),
            root: config.root.clone(),
            watchers: Mutex::new(Vec::new()),
            registrations: Mutex::new(HashMap::new()),
            applied_edits: Mutex::new(Vec::new()),
            progress: Mutex::new(HashSet::new()),
            stderr_tail: Mutex::new(VecDeque::new()),
        });
        if let Some(stderr) = transport.stderr {
            let (shared, name) = (shared.clone(), config.id.clone());
            std::thread::Builder::new()
                .name(format!("lsp-stderr-{name}"))
                .spawn(move || {
                    for line in BufReader::new(stderr).lines() {
                        let Ok(line) = line else { break };
                        tracing::debug!(target: "lsp", "{name}: {line}");
                        let mut tail = lock(&shared.stderr_tail);
                        if tail.len() == STDERR_TAIL {
                            tail.pop_front();
                        }
                        tail.push_back(line);
                    }
                })?;
        }
        let handler = handler(shared.clone(), sink.clone());
        let on_close = {
            let sink = sink.clone();
            Box::new(move || sink(ServerEvent::Exited))
        };
        let conn = Connection::start(
            &config.id,
            transport.reader,
            transport.writer,
            handler,
            on_close,
        );
        let pid = transport.process.as_ref().map(Child::id);
        Ok(Arc::new(Self {
            config,
            conn,
            shared,
            capabilities: RwLock::new(ServerCapabilities::default()),
            encoding: RwLock::new(Encoding::Utf16),
            initialized: AtomicBool::new(false),
            process: Mutex::new(transport.process),
            pid,
            documents: Mutex::new(HashMap::new()),
        }))
    }

    pub fn config(&self) -> &ServerConfig {
        &self.config
    }

    pub fn id(&self) -> &str {
        &self.config.id
    }

    pub fn connection(&self) -> &Arc<Connection> {
        &self.conn
    }

    pub fn pid(&self) -> Option<u32> {
        self.pid
    }

    pub fn is_running(&self) -> bool {
        !self.conn.is_closed()
    }

    pub fn is_initialized(&self) -> bool {
        self.initialized.load(Ordering::SeqCst)
    }

    pub fn is_busy(&self) -> bool {
        !lock(&self.shared.progress).is_empty()
    }

    pub fn capabilities(&self) -> ServerCapabilities {
        self.capabilities
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    pub fn encoding(&self) -> Encoding {
        *self.encoding.read().unwrap_or_else(|e| e.into_inner())
    }

    /// The last lines the server wrote to stderr.
    pub fn stderr_tail(&self) -> Vec<String> {
        lock(&self.shared.stderr_tail).iter().cloned().collect()
    }

    /// Whether a method was registered dynamically (`client/registerCapability`).
    pub fn has_registration(&self, method: &str) -> bool {
        lock(&self.shared.registrations)
            .values()
            .any(|m| m == method)
    }

    /// The `initialize` handshake, then `initialized` and the settings.
    pub async fn initialize(self: Arc<Self>) -> Result<(), RpcError> {
        let root_uri = path_to_uri(&self.config.root);
        let name = self
            .config
            .root
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "root".into());
        let mut params = json!({
            "processId": std::process::id(),
            "clientInfo": {"name": "FerroIDE", "version": env!("CARGO_PKG_VERSION")},
            "locale": "en",
            "rootPath": self.config.root,
            "rootUri": root_uri,
            "workspaceFolders": [{"uri": root_uri, "name": name}],
            "capabilities": client_capabilities(),
            "trace": "off",
        });
        if let Some(options) = &self.config.initialization_options {
            params["initializationOptions"] = options.clone();
        }
        let result = self
            .conn
            .request_value("initialize", params, INITIALIZE_TIMEOUT)
            .await?;
        let capabilities: ServerCapabilities =
            serde_json::from_value(result.get("capabilities").cloned().unwrap_or_default())
                .map_err(|e| RpcError::Decode(e.to_string()))?;
        *self.encoding.write().unwrap_or_else(|e| e.into_inner()) =
            Encoding::from_kind(capabilities.position_encoding.as_ref());
        *self.capabilities.write().unwrap_or_else(|e| e.into_inner()) = capabilities;
        self.conn.notify("initialized", json!({}));
        if !self.config.settings.is_null() {
            self.conn.notify(
                "workspace/didChangeConfiguration",
                json!({ "settings": self.config.settings }),
            );
        }
        self.initialized.store(true, Ordering::SeqCst);
        Ok(())
    }

    fn sync_kind(&self) -> (bool, TextDocumentSyncKind) {
        match self.capabilities().text_document_sync {
            Some(TextDocumentSyncCapability::Kind(kind)) => {
                (kind != TextDocumentSyncKind::NONE, kind)
            }
            Some(TextDocumentSyncCapability::Options(options)) => (
                options.open_close.unwrap_or(false),
                options.change.unwrap_or(TextDocumentSyncKind::NONE),
            ),
            None => (false, TextDocumentSyncKind::NONE),
        }
    }

    /// Make the server's copy of the document equal to `text`: `didOpen`
    /// the first time, then one incremental (or full) `didChange`.
    pub fn sync_document(&self, path: &Path, language_id: &str, text: &str) {
        let (open_close, kind) = self.sync_kind();
        if !open_close && kind == TextDocumentSyncKind::NONE {
            return;
        }
        let uri = path_to_uri(path);
        let mut documents = lock(&self.documents);
        match documents.get_mut(&uri) {
            None => {
                self.conn.notify(
                    "textDocument/didOpen",
                    json!({"textDocument": {
                        "uri": uri, "languageId": language_id, "version": 0, "text": text,
                    }}),
                );
                documents.insert(
                    uri,
                    Document {
                        version: 0,
                        text: text.to_string(),
                    },
                );
            }
            Some(doc) if doc.text != text => {
                doc.version += 1;
                let change = if kind == TextDocumentSyncKind::INCREMENTAL {
                    serde_json::to_value(incremental_change(&doc.text, text, self.encoding()))
                        .unwrap_or_default()
                } else {
                    json!({ "text": text })
                };
                self.conn.notify(
                    "textDocument/didChange",
                    json!({
                        "textDocument": {"uri": uri, "version": doc.version},
                        "contentChanges": [change],
                    }),
                );
                doc.text = text.to_string();
            }
            Some(_) => {}
        }
    }

    /// The text the server has of `path`, if it is open there.
    pub fn document_text(&self, path: &Path) -> Option<String> {
        lock(&self.documents)
            .get(&path_to_uri(path))
            .map(|d| d.text.clone())
    }

    pub fn is_open(&self, path: &Path) -> bool {
        lock(&self.documents).contains_key(&path_to_uri(path))
    }

    pub fn open_documents(&self) -> usize {
        lock(&self.documents).len()
    }

    pub fn close_document(&self, path: &Path) {
        let uri = path_to_uri(path);
        if lock(&self.documents).remove(&uri).is_some() {
            self.conn.notify(
                "textDocument/didClose",
                json!({"textDocument": {"uri": uri}}),
            );
        }
    }

    pub fn save_document(&self, path: &Path, text: &str) {
        let uri = path_to_uri(path);
        if !lock(&self.documents).contains_key(&uri) {
            return;
        }
        let include_text = match self.capabilities().text_document_sync {
            Some(TextDocumentSyncCapability::Options(options)) => match options.save {
                Some(TextDocumentSyncSaveOptions::Supported(true)) => Some(false),
                Some(TextDocumentSyncSaveOptions::SaveOptions(save)) => {
                    Some(save.include_text.unwrap_or(false))
                }
                _ => None,
            },
            _ => None,
        };
        let Some(include_text) = include_text else {
            return;
        };
        let mut params = json!({"textDocument": {"uri": uri}});
        if include_text {
            params["text"] = Value::String(text.to_string());
        }
        self.conn.notify("textDocument/didSave", params);
    }

    /// Forward file changes matching the server's watchers.
    pub fn files_changed(&self, changes: &[(PathBuf, FileChangeType)]) {
        let watchers = lock(&self.shared.watchers);
        if watchers.is_empty() {
            return;
        }
        let events: Vec<Value> = changes
            .iter()
            .filter(|(path, change)| {
                let bit = match *change {
                    FileChangeType::CREATED => 1,
                    FileChangeType::CHANGED => 2,
                    _ => 4,
                };
                watchers.iter().any(|w| {
                    if w.kind & bit == 0 {
                        return false;
                    }
                    let candidate = match &w.base {
                        Some(base) => match path.strip_prefix(base) {
                            Ok(rel) => rel.to_path_buf(),
                            Err(_) => return false,
                        },
                        None => path.clone(),
                    };
                    let text = candidate.to_string_lossy().replace('\\', "/");
                    w.matcher.is_match(&text)
                })
            })
            .map(|(path, change)| json!({"uri": path_to_uri(path), "type": change}))
            .collect();
        drop(watchers);
        if !events.is_empty() {
            self.conn.notify(
                "workspace/didChangeWatchedFiles",
                json!({ "changes": events }),
            );
        }
    }

    pub fn watcher_count(&self) -> usize {
        lock(&self.shared.watchers).len()
    }

    /// Run `workspace/executeCommand`, returning the edits the server
    /// applied through `workspace/applyEdit` meanwhile.
    pub async fn execute_command(
        self: &Arc<Self>,
        command: &lsp_types::Command,
        timeout: Duration,
    ) -> Result<Vec<lsp_types::WorkspaceEdit>, RpcError> {
        lock(&self.shared.applied_edits).clear();
        self.conn
            .request_value(
                "workspace/executeCommand",
                json!({"command": command.command, "arguments": command.arguments.clone().unwrap_or_default()}),
                timeout,
            )
            .await?;
        Ok(std::mem::take(&mut *lock(&self.shared.applied_edits)))
    }

    /// Resident memory of the process in MB, where the platform tells.
    pub fn memory_mb(&self) -> Option<u32> {
        let pid = self.pid?;
        let statm = std::fs::read_to_string(format!("/proc/{pid}/statm")).ok()?;
        let pages: u64 = statm.split_whitespace().nth(1)?.parse().ok()?;
        Some((pages * 4096 / (1024 * 1024)) as u32)
    }

    /// `shutdown` and `exit`, then kill the process if it lingers. Runs on
    /// its own thread; returns at once.
    pub fn stop(self: &Arc<Self>) {
        let server = self.clone();
        let _ = std::thread::Builder::new()
            .name(format!("lsp-stop-{}", self.config.id))
            .spawn(move || {
                if server.is_running() {
                    let request =
                        server
                            .conn
                            .request_value("shutdown", Value::Null, Duration::from_secs(3));
                    let _ = smol::block_on(request);
                    server.conn.notify("exit", Value::Null);
                    std::thread::sleep(Duration::from_millis(300));
                }
                server.kill();
            });
    }

    /// End the process now.
    pub fn kill(&self) {
        self.conn.close();
        if let Some(mut child) = lock(&self.process).take() {
            if matches!(child.try_wait(), Ok(None)) {
                let _ = child.kill();
            }
            let _ = child.wait();
        }
    }
}

impl Drop for LanguageServer {
    fn drop(&mut self) {
        self.kill();
    }
}

fn handler(shared: Arc<Shared>, sink: EventSink) -> Handler {
    Arc::new(move |incoming| match incoming {
        Incoming::Notification { method, params } => {
            match method.as_str() {
                "textDocument/publishDiagnostics" => {
                    let uri = params
                        .get("uri")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string();
                    let diagnostics = params
                        .get("diagnostics")
                        .cloned()
                        .and_then(|d| serde_json::from_value(d).ok())
                        .unwrap_or_default();
                    sink(ServerEvent::Diagnostics { uri, diagnostics });
                }
                "$/progress" => {
                    let token = params
                        .get("token")
                        .map(|t| t.to_string())
                        .unwrap_or_default();
                    let kind = params
                        .pointer("/value/kind")
                        .and_then(Value::as_str)
                        .unwrap_or_default();
                    let mut progress = lock(&shared.progress);
                    let changed = match kind {
                        "begin" => progress.insert(token) && progress.len() == 1,
                        "end" => progress.remove(&token) && progress.is_empty(),
                        _ => false,
                    };
                    let busy = !progress.is_empty();
                    drop(progress);
                    if changed {
                        sink(ServerEvent::Busy(busy));
                    }
                }
                "window/showMessage" => {
                    if let Ok(message) =
                        serde_json::from_value::<lsp_types::ShowMessageParams>(params)
                    {
                        sink(ServerEvent::Message {
                            level: message.typ,
                            text: message.message,
                        });
                    }
                }
                "window/logMessage" => {
                    let text = params
                        .get("message")
                        .and_then(Value::as_str)
                        .unwrap_or_default();
                    tracing::debug!(target: "lsp", "{text}");
                }
                _ => {}
            }
            Ok(Value::Null)
        }
        Incoming::Request { method, params } => match method.as_str() {
            "workspace/configuration" => {
                let items = params
                    .get("items")
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default();
                Ok(Value::Array(
                    items
                        .iter()
                        .map(|item| {
                            settings_section(
                                &shared.settings,
                                item.get("section").and_then(Value::as_str),
                            )
                        })
                        .collect(),
                ))
            }
            "workspace/workspaceFolders" => {
                let uri = path_to_uri(&shared.root);
                let name = shared
                    .root
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default();
                Ok(json!([{"uri": uri, "name": name}]))
            }
            "client/registerCapability" => {
                let registrations = params
                    .get("registrations")
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default();
                for registration in registrations {
                    let id = registration
                        .get("id")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string();
                    let method = registration
                        .get("method")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string();
                    if method == "workspace/didChangeWatchedFiles" {
                        let watchers = registration
                            .pointer("/registerOptions/watchers")
                            .and_then(Value::as_array)
                            .cloned()
                            .unwrap_or_default();
                        let mut all = lock(&shared.watchers);
                        for watcher in watchers {
                            if let Some(w) = parse_watcher(&id, &watcher) {
                                all.push(w);
                            }
                        }
                    }
                    lock(&shared.registrations).insert(id, method);
                }
                Ok(Value::Null)
            }
            "client/unregisterCapability" => {
                let ids: Vec<String> = params
                    .get("unregisterations")
                    .or_else(|| params.get("unregistrations"))
                    .and_then(Value::as_array)
                    .map(|list| {
                        list.iter()
                            .filter_map(|u| u.get("id").and_then(Value::as_str))
                            .map(str::to_string)
                            .collect()
                    })
                    .unwrap_or_default();
                lock(&shared.watchers).retain(|w| !ids.contains(&w.registration));
                lock(&shared.registrations).retain(|id, _| !ids.contains(id));
                Ok(Value::Null)
            }
            "window/workDoneProgress/create" => Ok(Value::Null),
            "window/showMessageRequest" => {
                if let Ok(message) =
                    serde_json::from_value::<lsp_types::ShowMessageRequestParams>(params)
                {
                    sink(ServerEvent::Message {
                        level: message.typ,
                        text: message.message,
                    });
                }
                Ok(Value::Null)
            }
            "window/showDocument" => Ok(json!({"success": false})),
            "workspace/applyEdit" => {
                match params
                    .get("edit")
                    .cloned()
                    .and_then(|e| serde_json::from_value(e).ok())
                {
                    Some(edit) => {
                        lock(&shared.applied_edits).push(edit);
                        Ok(json!({"applied": true}))
                    }
                    None => Ok(json!({"applied": false, "failureReason": "invalid edit"})),
                }
            }
            "workspace/semanticTokens/refresh"
            | "workspace/inlayHint/refresh"
            | "workspace/codeLens/refresh"
            | "workspace/diagnostic/refresh" => {
                sink(ServerEvent::Refresh);
                Ok(Value::Null)
            }
            _ => Err((
                codes::METHOD_NOT_FOUND,
                format!("{method} is not supported"),
            )),
        },
    })
}

fn parse_watcher(registration: &str, watcher: &Value) -> Option<Watcher> {
    let (pattern, base) = match watcher.get("globPattern")? {
        Value::String(pattern) => (pattern.clone(), None),
        relative => {
            let base = relative.get("baseUri")?;
            let base = base.as_str().or_else(|| base.get("uri")?.as_str())?;
            (
                relative.get("pattern")?.as_str()?.to_string(),
                Some(uri_to_path(base)?),
            )
        }
    };
    // Absolute patterns are matched against slash-separated paths.
    let pattern = pattern.replace('\\', "/");
    let matcher = GlobBuilder::new(&pattern)
        .literal_separator(true)
        .backslash_escape(false)
        .build()
        .ok()?
        .compile_matcher();
    Some(Watcher {
        registration: registration.to_string(),
        matcher,
        base,
        kind: watcher.get("kind").and_then(Value::as_u64).unwrap_or(7) as u8,
    })
}

pub const SEMANTIC_TOKEN_TYPES: &[&str] = &[
    "namespace",
    "type",
    "class",
    "enum",
    "interface",
    "struct",
    "typeParameter",
    "parameter",
    "variable",
    "property",
    "enumMember",
    "event",
    "function",
    "method",
    "macro",
    "keyword",
    "modifier",
    "comment",
    "string",
    "number",
    "regexp",
    "operator",
    "decorator",
];

pub const SEMANTIC_TOKEN_MODIFIERS: &[&str] = &[
    "declaration",
    "definition",
    "readonly",
    "static",
    "deprecated",
    "abstract",
    "async",
    "modification",
    "documentation",
    "defaultLibrary",
];

/// What FerroIDE supports, as LSP client capabilities.
pub fn client_capabilities() -> Value {
    json!({
        "general": {
            "positionEncodings": ["utf-16", "utf-8", "utf-32"],
        },
        "workspace": {
            "applyEdit": true,
            "workspaceEdit": {
                "documentChanges": true,
                "resourceOperations": ["create", "rename", "delete"],
            },
            "didChangeConfiguration": {"dynamicRegistration": true},
            "didChangeWatchedFiles": {"dynamicRegistration": true, "relativePatternSupport": true},
            "symbol": {"dynamicRegistration": false},
            "executeCommand": {"dynamicRegistration": false},
            "workspaceFolders": true,
            "configuration": true,
            "semanticTokens": {"refreshSupport": true},
            "inlayHint": {"refreshSupport": true},
            "codeLens": {"refreshSupport": true},
            "diagnostics": {"refreshSupport": true},
        },
        "textDocument": {
            "synchronization": {"didSave": true, "willSave": false, "willSaveWaitUntil": false},
            "completion": {
                "completionItem": {
                    "snippetSupport": true,
                    "documentationFormat": ["markdown", "plaintext"],
                    "deprecatedSupport": true,
                    "labelDetailsSupport": true,
                    "insertReplaceSupport": false,
                    "resolveSupport": {"properties": ["documentation", "detail", "additionalTextEdits"]},
                },
                "contextSupport": true,
            },
            "hover": {"contentFormat": ["markdown", "plaintext"]},
            "signatureHelp": {
                "signatureInformation": {
                    "documentationFormat": ["markdown", "plaintext"],
                    "parameterInformation": {"labelOffsetSupport": true},
                    "activeParameterSupport": true,
                },
                "contextSupport": true,
            },
            "declaration": {"linkSupport": true},
            "definition": {"linkSupport": true},
            "typeDefinition": {"linkSupport": true},
            "implementation": {"linkSupport": true},
            "references": {},
            "documentHighlight": {},
            "documentSymbol": {"hierarchicalDocumentSymbolSupport": true},
            "codeAction": {
                "codeActionLiteralSupport": {"codeActionKind": {"valueSet": [
                    "", "quickfix", "refactor", "refactor.extract", "refactor.inline",
                    "refactor.rewrite", "source", "source.organizeImports", "source.fixAll",
                ]}},
                "isPreferredSupport": true,
                "disabledSupport": true,
                "dataSupport": true,
                "resolveSupport": {"properties": ["edit"]},
            },
            "codeLens": {},
            "documentLink": {"tooltipSupport": true},
            "formatting": {},
            "rangeFormatting": {},
            "onTypeFormatting": {},
            "rename": {"prepareSupport": true},
            "publishDiagnostics": {"relatedInformation": false, "versionSupport": true, "codeDescriptionSupport": true},
            "diagnostic": {"dynamicRegistration": false, "relatedDocumentSupport": false},
            "inlayHint": {"resolveSupport": {"properties": []}},
            "semanticTokens": {
                "requests": {"range": false, "full": {"delta": false}},
                "tokenTypes": SEMANTIC_TOKEN_TYPES,
                "tokenModifiers": SEMANTIC_TOKEN_MODIFIERS,
                "formats": ["relative"],
                "overlappingTokenSupport": false,
                "multilineTokenSupport": false,
            },
            "callHierarchy": {},
            "typeHierarchy": {},
        },
        "window": {
            "workDoneProgress": true,
            "showMessage": {},
            "showDocument": {"support": false},
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn configuration_sections_resolve_by_dotted_path() {
        let settings = json!({"python": {"analysis": {"typeCheckingMode": "strict"}}, "rust-analyzer": {"check": {"command": "clippy"}}});
        assert_eq!(
            settings_section(&settings, Some("python.analysis")),
            json!({"typeCheckingMode": "strict"})
        );
        assert_eq!(
            settings_section(&settings, Some("rust-analyzer"))["check"]["command"],
            "clippy"
        );
        assert_eq!(settings_section(&settings, Some("missing.x")), Value::Null);
        assert_eq!(settings_section(&settings, None), settings);
    }

    #[test]
    fn watchers_parse_plain_and_relative_patterns() {
        let plain = parse_watcher("1", &json!({"globPattern": "**/*.rs"})).unwrap();
        assert!(plain.matcher.is_match("/work/src/main.rs"));
        assert!(!plain.matcher.is_match("/work/src/main.go"));
        assert_eq!(plain.kind, 7);
        let base = if cfg!(windows) {
            "file:///C:/work"
        } else {
            "file:///work"
        };
        let relative = parse_watcher(
            "2",
            &json!({"globPattern": {"baseUri": base, "pattern": "Cargo.toml"}, "kind": 2}),
        )
        .unwrap();
        assert!(relative.base.is_some());
        assert!(relative.matcher.is_match("Cargo.toml"));
        assert!(!relative.matcher.is_match("sub/Cargo.toml"));
    }
}
