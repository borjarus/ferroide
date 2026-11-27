//! [`PtyTerminalService`]: terminal sessions on real PTYs behind the
//! `TerminalService` contract.

use std::{
    path::PathBuf,
    sync::{
        Arc, Mutex, MutexGuard, OnceLock,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};

use ferro_services::{
    NewTerminal, ShellInfo, TerminalEvent, TerminalService, TerminalSession, TerminalSnapshot,
};

use crate::{
    TerminalCore,
    core::CoreEvent,
    pty::{self, Control, PtyHandle},
    shell,
};

/// Size of a new session until the view reports its own.
const INITIAL_COLS: u16 = 80;
const INITIAL_ROWS: u16 = 24;

/// How long a command for a new session waits for the shell's first prompt
/// when the shell reports it (shell integration), and when it does not.
const PROMPT_WAIT_INTEGRATED: Duration = Duration::from_secs(5);
const PROMPT_WAIT_PLAIN: Duration = Duration::from_millis(800);

/// How sessions start.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TerminalOptions {
    /// The default working directory (the first project root).
    pub cwd: PathBuf,
    /// A [`ShellInfo::id`] or a program path, from the settings; `None` is
    /// the system's default shell.
    pub default_shell: Option<String>,
    /// Lines of scrollback per session.
    pub scrollback: usize,
    /// Start shells with OSC 133 shell integration.
    pub shell_integration: bool,
    /// Where the integration scripts are written.
    pub integration_dir: PathBuf,
    /// Extra environment variables for every shell.
    pub env: Vec<(String, String)>,
}

impl Default for TerminalOptions {
    fn default() -> Self {
        Self {
            cwd: std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
            default_shell: None,
            scrollback: 10_000,
            shell_integration: true,
            integration_dir: std::env::temp_dir().join("ferroide-shell"),
            env: Vec::new(),
        }
    }
}

type Subscribers = Mutex<Vec<async_channel::Sender<TerminalEvent>>>;

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn broadcast(subscribers: &Subscribers, event: TerminalEvent) {
    lock(subscribers).retain(|tx| tx.try_send(event.clone()).is_ok());
}

/// A session's state shared between the service and its I/O thread.
pub(crate) struct SessionShared {
    pub id: u64,
    pub title: String,
    core: Mutex<TerminalCore>,
    /// A `Changed` was sent and no snapshot taken since: further changes
    /// need no event (a flood of output sends few events).
    dirty: AtomicBool,
    subscribers: Arc<Subscribers>,
    /// A command to type once the shell is ready, and until when to wait
    /// for its prompt.
    pending_command: Mutex<Option<(String, Instant)>>,
}

impl SessionShared {
    fn changed(&self) {
        if !self.dirty.swap(true, Ordering::AcqRel) {
            broadcast(&self.subscribers, TerminalEvent::Changed(self.id));
        }
    }

    fn send(&self, events: Vec<CoreEvent>) {
        for event in events {
            let event = match event {
                CoreEvent::Title(title) => TerminalEvent::TitleChanged {
                    id: self.id,
                    title: title.unwrap_or_else(|| self.title.clone()),
                },
                CoreEvent::Bell => TerminalEvent::Bell(self.id),
                CoreEvent::CommandFinished(exit_code) => TerminalEvent::CommandFinished {
                    id: self.id,
                    exit_code,
                },
            };
            broadcast(&self.subscribers, event);
        }
    }

    /// Output of the shell; returns the emulator's replies for the PTY.
    pub fn on_output(&self, bytes: &[u8]) -> Vec<u8> {
        let output = lock(&self.core).advance(bytes);
        self.changed();
        self.send(output.events);
        output.replies
    }

    /// Timed work: ending a synchronized update the program left open, and
    /// typing the pending command once the prompt is there. Returns bytes
    /// for the PTY.
    pub fn on_tick(&self) -> Vec<u8> {
        let now = Instant::now();
        let mut core = lock(&self.core);
        let mut bytes = Vec::new();
        if core.sync_deadline().is_some_and(|d| d <= now) {
            let output = core.flush_sync();
            drop(core);
            self.changed();
            self.send(output.events);
            bytes.extend(output.replies);
            core = lock(&self.core);
        }
        let ready = core.prompt_ready();
        drop(core);
        let mut pending = lock(&self.pending_command);
        if pending
            .as_ref()
            .is_some_and(|(_, deadline)| ready || *deadline <= now)
            && let Some((command, _)) = pending.take()
        {
            bytes.extend(command.as_bytes());
            bytes.push(b'\r');
        }
        bytes
    }

    /// When [`Self::on_tick`] has work next.
    pub fn next_deadline(&self) -> Option<Instant> {
        let sync = lock(&self.core).sync_deadline();
        let command = lock(&self.pending_command).as_ref().map(|(_, d)| *d);
        match (sync, command) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        }
    }

    /// The shell exited (`None`: code unknown).
    pub fn on_exit(&self, code: Option<i32>) {
        lock(&self.core).set_exited(code.unwrap_or(-1));
        lock(&self.pending_command).take();
        self.changed();
        broadcast(
            &self.subscribers,
            TerminalEvent::Exited { id: self.id, code },
        );
    }

    fn snapshot(&self) -> TerminalSnapshot {
        let core = lock(&self.core);
        // Cleared under the lock: output after this sends a new event.
        self.dirty.store(false, Ordering::Release);
        core.snapshot()
    }
}

struct Session {
    shared: Arc<SessionShared>,
    /// `None` when the shell could not be started.
    pty: Option<PtyHandle>,
}

/// Terminal sessions running shells on pseudoterminals (ConPTY on
/// Windows), emulated by `alacritty_terminal`.
pub struct PtyTerminalService {
    options: TerminalOptions,
    shells: OnceLock<Vec<ShellInfo>>,
    sessions: Mutex<Vec<Session>>,
    next_id: AtomicU64,
    subscribers: Arc<Subscribers>,
}

impl PtyTerminalService {
    pub fn new(options: TerminalOptions) -> Self {
        Self {
            options,
            shells: OnceLock::new(),
            sessions: Mutex::new(Vec::new()),
            next_id: AtomicU64::new(1),
            subscribers: Arc::new(Mutex::new(Vec::new())),
        }
    }

    fn detected_shells(&self) -> &[ShellInfo] {
        self.shells.get_or_init(crate::detect_shells)
    }

    fn shared(&self, id: u64) -> Option<Arc<SessionShared>> {
        lock(&self.sessions)
            .iter()
            .find(|s| s.shared.id == id)
            .map(|s| s.shared.clone())
    }

    fn send(&self, id: u64, control: Control) {
        if let Some(session) = lock(&self.sessions).iter().find(|s| s.shared.id == id)
            && let Some(pty) = &session.pty
        {
            pty.send(control);
        }
    }

    fn new_shared(
        &self,
        title: String,
        pending_command: Option<(String, Instant)>,
    ) -> Arc<SessionShared> {
        Arc::new(SessionShared {
            id: self.next_id.fetch_add(1, Ordering::Relaxed),
            title,
            core: Mutex::new(TerminalCore::new(
                INITIAL_COLS,
                INITIAL_ROWS,
                self.options.scrollback,
            )),
            dirty: AtomicBool::new(false),
            subscribers: self.subscribers.clone(),
            pending_command: Mutex::new(pending_command),
        })
    }

    /// The lowest free "Local (n)" title, as IntelliJ does.
    fn free_title(sessions: &[Session]) -> String {
        (1..)
            .map(|n| {
                if n == 1 {
                    "Local".to_string()
                } else {
                    format!("Local ({n})")
                }
            })
            .find(|t| !sessions.iter().any(|s| &s.shared.title == t))
            .unwrap_or_default()
    }

    fn try_open(&self, options: NewTerminal) -> anyhow::Result<TerminalSession> {
        let requested = options
            .shell
            .as_deref()
            .or(self.options.default_shell.as_deref());
        let shell = shell::resolve_shell(requested, self.detected_shells()).ok_or_else(|| {
            anyhow::anyhow!("shell not found: {}", requested.unwrap_or("default"))
        })?;
        let cwd = options
            .cwd
            .filter(|d| d.is_dir())
            .or_else(|| Some(self.options.cwd.clone()).filter(|d| d.is_dir()))
            .or_else(|| std::env::current_dir().ok())
            .unwrap_or_else(|| PathBuf::from("."));
        let launch = shell::launch(&shell, &self.options);
        let wait = if launch.integrated {
            PROMPT_WAIT_INTEGRATED
        } else {
            PROMPT_WAIT_PLAIN
        };
        let pending = options
            .command
            .filter(|c| !c.trim().is_empty())
            .map(|c| (c, Instant::now() + wait));

        let mut sessions = lock(&self.sessions);
        let title = options.title.unwrap_or_else(|| Self::free_title(&sessions));
        let shared = self.new_shared(title, pending);
        let size = lock(&shared.core).window_size();
        let pty = pty::spawn(&launch, &cwd, size, shared.clone())?;
        let session = TerminalSession {
            id: shared.id,
            title: shared.title.clone(),
            lines: Vec::new(),
        };
        sessions.push(Session {
            shared,
            pty: Some(pty),
        });
        Ok(session)
    }
}

impl TerminalService for PtyTerminalService {
    fn sessions(&self) -> Vec<TerminalSession> {
        lock(&self.sessions)
            .iter()
            .map(|s| TerminalSession {
                id: s.shared.id,
                title: s.shared.title.clone(),
                lines: Vec::new(),
            })
            .collect()
    }

    /// Open a session with the default shell; when it cannot start, the
    /// session shows why and is marked exited.
    fn new_session(&self) -> TerminalSession {
        match self.try_open(NewTerminal::default()) {
            Ok(session) => session,
            Err(err) => {
                tracing::warn!("terminal: {err:#}");
                let mut sessions = lock(&self.sessions);
                let shared = self.new_shared(Self::free_title(&sessions), None);
                lock(&shared.core).advance(
                    format!("\x1b[31mThe shell could not be started: {err:#}\x1b[0m\r\n")
                        .as_bytes(),
                );
                let session = TerminalSession {
                    id: shared.id,
                    title: shared.title.clone(),
                    lines: Vec::new(),
                };
                sessions.push(Session {
                    shared: shared.clone(),
                    pty: None,
                });
                drop(sessions);
                shared.on_exit(None);
                session
            }
        }
    }

    fn open(&self, options: NewTerminal) -> ferro_services::Result<TerminalSession> {
        self.try_open(options)
    }

    fn close_session(&self, id: u64) {
        let removed = {
            let mut sessions = lock(&self.sessions);
            sessions
                .iter()
                .position(|s| s.shared.id == id)
                .map(|ix| sessions.remove(ix))
        };
        // Outside the lock: this ends the shell and waits for the thread.
        drop(removed);
    }

    fn shells(&self) -> Vec<ShellInfo> {
        self.detected_shells().to_vec()
    }

    fn write(&self, id: u64, bytes: &[u8]) {
        if let Some(shared) = self.shared(id) {
            // Typing returns the view to the bottom.
            let mut core = lock(&shared.core);
            if core.display_offset() != 0 {
                core.scroll(i32::MIN);
                drop(core);
                shared.changed();
            }
        }
        self.send(id, Control::Write(bytes.to_vec()));
    }

    fn resize(&self, id: u64, cols: u16, rows: u16) {
        let Some(shared) = self.shared(id) else {
            return;
        };
        let size = {
            let mut core = lock(&shared.core);
            if (core.cols(), core.rows()) == (cols.max(1), rows.max(1)) {
                return;
            }
            core.resize(cols, rows);
            core.window_size()
        };
        shared.changed();
        self.send(id, Control::Resize(size));
    }

    fn scroll(&self, id: u64, lines: i32) {
        if let Some(shared) = self.shared(id) {
            lock(&shared.core).scroll(lines);
            shared.changed();
        }
    }

    fn scroll_to_line(&self, id: u64, line: i64) {
        if let Some(shared) = self.shared(id) {
            lock(&shared.core).scroll_to_line(line);
            shared.changed();
        }
    }

    fn snapshot(&self, id: u64) -> Option<TerminalSnapshot> {
        Some(self.shared(id)?.snapshot())
    }

    fn subscribe(&self) -> async_channel::Receiver<TerminalEvent> {
        let (tx, rx) = async_channel::unbounded();
        lock(&self.subscribers).push(tx);
        rx
    }
}

impl Drop for PtyTerminalService {
    fn drop(&mut self) {
        // End every shell and join its thread.
        let sessions = std::mem::take(&mut *lock(&self.sessions));
        drop(sessions);
    }
}
