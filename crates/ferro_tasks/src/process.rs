//! Running one process: output lines as they come, and stopping its whole
//! process tree.
//!
//! stdout and stderr are read on their own threads and split into lines;
//! `\r\n` ends a line like `\n`, and a line rewritten with bare `\r`s (a
//! progress bar) keeps only its last state. ANSI escape sequences are
//! passed through: the console renders them.
//!
//! Stopping kills the tree, not just the process, since build tools and
//! package managers run the real work in children (`npm` → `node`,
//! `cargo run` → the binary, `gradle` → its daemon client). On Windows
//! that is `taskkill /T /F`; on Unix the process starts its own process
//! group, which gets `SIGTERM` and, after a grace period, `SIGKILL` — via
//! the `kill` command, keeping the crate free of `unsafe`.
//!
//! When the process exits but a grandchild still holds its output pipes
//! open (a daemon it started), reading stops after a short quiet period
//! instead of waiting for that grandchild.

use std::io::Read;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::thread;
use std::time::Duration;
#[cfg(any(unix, test))]
use std::time::Instant;

/// How long `SIGTERM` gets before `SIGKILL` (Unix).
#[cfg(unix)]
const GRACE: Duration = Duration::from_secs(3);
/// How long output is still read after the process exited.
const DRAIN: Duration = Duration::from_millis(500);
/// A line longer than this is cut into pieces.
const MAX_LINE: usize = 1 << 20;

/// What to run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Launch {
    pub program: PathBuf,
    pub args: Vec<String>,
    pub cwd: PathBuf,
    /// Set on top of the inherited environment.
    pub env: Vec<(String, String)>,
}

/// Shared between the thread running a process and whoever stops it.
#[derive(Debug, Default)]
pub struct ProcessControl {
    pid: Mutex<Option<u32>>,
    stopped: AtomicBool,
    exited: Arc<AtomicBool>,
}

impl ProcessControl {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    /// Stop the current process tree, and any started later through this
    /// control.
    pub fn stop(&self) {
        self.stopped.store(true, Ordering::SeqCst);
        let pid = *self.pid.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(pid) = pid {
            kill_tree(pid, self.exited.clone());
        }
    }

    pub fn is_stopped(&self) -> bool {
        self.stopped.load(Ordering::SeqCst)
    }

    fn set_pid(&self, pid: Option<u32>) {
        *self.pid.lock().unwrap_or_else(|e| e.into_inner()) = pid;
    }
}

enum Message {
    Line(String, bool),
    Closed,
    Exited(i32),
}

/// Run `launch` to completion, calling `on_line(line, is_stderr)` for each
/// line of output on the calling thread. Returns the exit code (`-1` when
/// there is none; `128 + signal` for a signal on Unix), or the spawn error.
pub fn run(
    launch: &Launch,
    control: &ProcessControl,
    mut on_line: impl FnMut(String, bool),
) -> std::io::Result<i32> {
    if control.is_stopped() {
        return Ok(130);
    }
    let mut command = Command::new(&launch.program);
    command
        .args(&launch.args)
        .current_dir(&launch.cwd)
        .envs(launch.env.iter().map(|(k, v)| (k, v)))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    configure(&mut command);
    control.exited.store(false, Ordering::SeqCst);
    let mut child = command.spawn()?;
    control.set_pid(Some(child.id()));
    if control.is_stopped() {
        kill_tree(child.id(), control.exited.clone());
    }

    let (tx, rx) = mpsc::channel();
    let mut readers = 0;
    if let Some(stdout) = child.stdout.take() {
        readers += 1;
        spawn_reader(stdout, false, tx.clone());
    }
    if let Some(stderr) = child.stderr.take() {
        readers += 1;
        spawn_reader(stderr, true, tx.clone());
    }
    let exited = control.exited.clone();
    let waiter_tx = tx;
    thread::Builder::new()
        .name("run-wait".into())
        .spawn(move || {
            let code = match child.wait() {
                Ok(status) => exit_code(status),
                Err(e) => {
                    tracing::warn!("waiting for a run failed: {e}");
                    -1
                }
            };
            exited.store(true, Ordering::SeqCst);
            let _ = waiter_tx.send(Message::Exited(code));
        })?;

    let mut code = None;
    let mut open = readers;
    loop {
        let message = if code.is_some() {
            match rx.recv_timeout(DRAIN) {
                Ok(m) => m,
                Err(_) => break,
            }
        } else {
            match rx.recv() {
                Ok(m) => m,
                Err(_) => break,
            }
        };
        match message {
            Message::Line(line, error) => on_line(line, error),
            Message::Closed => open -= 1,
            Message::Exited(c) => code = Some(c),
        }
        if open == 0 && code.is_some() {
            break;
        }
    }
    control.set_pid(None);
    Ok(code.unwrap_or(-1))
}

#[cfg(windows)]
fn configure(command: &mut Command) {
    use std::os::windows::process::CommandExt;
    /// No console window for console programs started from the GUI.
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    command.creation_flags(CREATE_NO_WINDOW);
}

#[cfg(unix)]
fn configure(command: &mut Command) {
    use std::os::unix::process::CommandExt;
    command.process_group(0);
}

#[cfg(not(any(windows, unix)))]
fn configure(_command: &mut Command) {}

fn exit_code(status: std::process::ExitStatus) -> i32 {
    if let Some(code) = status.code() {
        return code;
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        if let Some(signal) = status.signal() {
            return 128 + signal;
        }
    }
    -1
}

fn spawn_reader(stream: impl Read + Send + 'static, error: bool, tx: mpsc::Sender<Message>) {
    let spawned = thread::Builder::new()
        .name(if error { "run-stderr" } else { "run-stdout" }.into())
        .spawn({
            let tx = tx.clone();
            move || {
                read_lines(stream, |line| {
                    let _ = tx.send(Message::Line(line, error));
                });
                let _ = tx.send(Message::Closed);
            }
        });
    if spawned.is_err() {
        let _ = tx.send(Message::Closed);
    }
}

/// Split a byte stream into lines (see the module docs).
pub fn read_lines(mut stream: impl Read, mut emit: impl FnMut(String)) {
    let mut buf: Vec<u8> = Vec::new();
    let mut chunk = [0u8; 8192];
    loop {
        let n = match stream.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => n,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(_) => break,
        };
        let start = buf.len();
        buf.extend_from_slice(&chunk[..n]);
        let mut from = 0;
        let mut search = start;
        while let Some(pos) = buf[search..].iter().position(|b| *b == b'\n') {
            let end = search + pos;
            emit(clean_line(&buf[from..end]));
            from = end + 1;
            search = from;
        }
        buf.drain(..from);
        if buf.len() > MAX_LINE {
            emit(clean_line(&buf));
            buf.clear();
        }
    }
    if !buf.is_empty() {
        emit(clean_line(&buf));
    }
}

/// A line without its `\r\n` ending, keeping what a terminal would show
/// after carriage returns.
fn clean_line(bytes: &[u8]) -> String {
    let bytes = bytes.strip_suffix(b"\r").unwrap_or(bytes);
    let text = String::from_utf8_lossy(bytes);
    if !text.contains('\r') {
        return text.into_owned();
    }
    text.split('\r')
        .rev()
        .find(|part| !part.is_empty())
        .unwrap_or_default()
        .to_string()
}

/// Kill process `pid` and its descendants. `exited` turns true when the
/// process has been reaped (ends the Unix grace period early).
pub fn kill_tree(pid: u32, exited: Arc<AtomicBool>) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        let _ = exited;
        let status = Command::new("taskkill")
            .args(["/T", "/F", "/PID", &pid.to_string()])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .creation_flags(0x0800_0000)
            .status();
        if let Err(e) = status {
            tracing::warn!(pid, "taskkill failed: {e}");
        }
    }
    #[cfg(unix)]
    {
        let signal = |name: &str| {
            Command::new("kill")
                .args([&format!("-{name}"), "--", &format!("-{pid}")])
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
        };
        if let Err(e) = signal("TERM") {
            tracing::warn!(pid, "kill failed: {e}");
            return;
        }
        let _ = thread::Builder::new()
            .name("run-kill".into())
            .spawn(move || {
                let deadline = Instant::now() + GRACE;
                while Instant::now() < deadline {
                    if exited.load(Ordering::SeqCst) {
                        return;
                    }
                    thread::sleep(Duration::from_millis(50));
                }
                let _ = signal("KILL");
            });
    }
    #[cfg(not(any(windows, unix)))]
    {
        let _ = (pid, exited);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(bytes: &[u8]) -> Vec<String> {
        let mut out = Vec::new();
        // Small reads, to cross line boundaries between chunks.
        struct Trickle<'a>(&'a [u8]);
        impl Read for Trickle<'_> {
            fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
                let n = self.0.len().min(3).min(buf.len());
                buf[..n].copy_from_slice(&self.0[..n]);
                self.0 = &self.0[n..];
                Ok(n)
            }
        }
        read_lines(Trickle(bytes), |l| out.push(l));
        out
    }

    #[test]
    fn splits_lines_and_keeps_the_last_progress_state() {
        assert_eq!(
            lines(b"one\r\ntwo\n\x1b[32mgreen\x1b[0m\n 10%\r 50%\r100%\r\nend\r\r\nlast"),
            ["one", "two", "\x1b[32mgreen\x1b[0m", "100%", "end", "last"]
        );
        assert_eq!(lines(b"\n\n"), ["", ""]);
        assert_eq!(lines("zażółć\n".as_bytes()), ["zażółć"]);
    }

    #[test]
    fn reports_spawn_errors() {
        let launch = Launch {
            program: PathBuf::from("definitely-not-a-program-ferroide"),
            args: Vec::new(),
            cwd: std::env::temp_dir(),
            env: Vec::new(),
        };
        assert!(run(&launch, &ProcessControl::default(), |_, _| {}).is_err());
    }

    #[test]
    fn stopped_control_does_not_start() {
        let control = ProcessControl::default();
        control.stop();
        let launch = Launch {
            program: PathBuf::from("definitely-not-a-program-ferroide"),
            args: Vec::new(),
            cwd: std::env::temp_dir(),
            env: Vec::new(),
        };
        let started = Instant::now();
        assert_eq!(run(&launch, &control, |_, _| {}).unwrap(), 130);
        assert!(started.elapsed() < Duration::from_secs(1));
    }
}
