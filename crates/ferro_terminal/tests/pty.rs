//! Real shells on a real PTY: start, type, read the screen, exit, close.
//! Each test skips (passes) when its shell is not installed.

use std::time::{Duration, Instant};

use ferro_services::{
    NewTerminal, ShellInfo, TerminalEvent, TerminalService, TerminalSnapshot, line_text,
};
use ferro_terminal::{PtyTerminalService, TerminalOptions, detect_shells};

const TIMEOUT: Duration = Duration::from_secs(30);

struct Harness {
    service: PtyTerminalService,
    events: async_channel::Receiver<TerminalEvent>,
    seen: Vec<TerminalEvent>,
    _dir: tempfile::TempDir,
}

impl Harness {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let service = PtyTerminalService::new(TerminalOptions {
            cwd: dir.path().to_path_buf(),
            integration_dir: dir.path().join("shell"),
            ..TerminalOptions::default()
        });
        let events = service.subscribe();
        Self {
            service,
            events,
            seen: Vec::new(),
            _dir: dir,
        }
    }

    fn drain(&mut self) {
        while let Ok(event) = self.events.try_recv() {
            self.seen.push(event);
        }
    }

    /// Poll the screen until `done` holds.
    fn wait_screen(&mut self, id: u64, what: &str, done: impl Fn(&TerminalSnapshot) -> bool) {
        let start = Instant::now();
        loop {
            self.drain();
            let snapshot = self.service.snapshot(id).expect("session");
            if done(&snapshot) {
                return;
            }
            if start.elapsed() > TIMEOUT {
                panic!(
                    "timed out waiting for {what}; screen:\n{}",
                    screen(&snapshot)
                );
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    /// Wait for an event matching `find`, returning it.
    fn wait_event(
        &mut self,
        id: u64,
        what: &str,
        find: impl Fn(&TerminalEvent) -> bool,
    ) -> TerminalEvent {
        let start = Instant::now();
        loop {
            self.drain();
            if let Some(ix) = self.seen.iter().position(&find) {
                return self.seen.remove(ix);
            }
            if start.elapsed() > TIMEOUT {
                let snapshot = self.service.snapshot(id).unwrap_or_default();
                panic!(
                    "timed out waiting for {what}; events {:?}; screen:\n{}",
                    self.seen,
                    screen(&snapshot)
                );
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }
}

fn screen(snapshot: &TerminalSnapshot) -> String {
    snapshot
        .lines
        .iter()
        .map(|l| line_text(l))
        .collect::<Vec<_>>()
        .join("\n")
}

/// A line that is exactly `text` (the output, not the echoed command).
fn has_line(snapshot: &TerminalSnapshot, text: &str) -> bool {
    snapshot
        .lines
        .iter()
        .any(|l| line_text(l).trim_end() == text)
}

fn find_shell(ids: &[&str]) -> Option<ShellInfo> {
    let shells = detect_shells();
    ids.iter()
        .find_map(|id| shells.iter().find(|s| s.id == *id).cloned())
}

fn open(h: &Harness, shell: &ShellInfo, command: Option<&str>) -> u64 {
    h.service
        .open(NewTerminal {
            shell: Some(shell.id.clone()),
            command: command.map(str::to_string),
            ..NewTerminal::default()
        })
        .expect("shell starts")
        .id
}

/// Echo, an exit code through shell integration, resize, exit.
fn echo_exit_code_resize_exit(ids: &[&str], fail: &str, fail_code: i32, exit: &str) {
    let Some(shell) = find_shell(ids) else {
        eprintln!("skipped: none of {ids:?} found");
        return;
    };
    let mut h = Harness::new();
    let id = open(&h, &shell, None);
    assert_eq!(h.service.sessions()[0].title, "Local");

    h.wait_event(id, "the first prompt", |e| *e == TerminalEvent::Changed(id));
    h.service.write(id, b"echo hello-ferro\r");
    h.wait_screen(id, "echo output", |s| has_line(s, "hello-ferro"));

    // A finished command with its exit code (shell integration).
    h.service.write(id, format!("{fail}\r").as_bytes());
    let event = h.wait_event(id, "CommandFinished", |e| {
        matches!(e, TerminalEvent::CommandFinished { exit_code: Some(c), .. } if *c == fail_code)
    });
    assert_eq!(
        event,
        TerminalEvent::CommandFinished {
            id,
            exit_code: Some(fail_code)
        }
    );
    let snapshot = h.service.snapshot(id).unwrap();
    assert!(
        snapshot
            .commands
            .iter()
            .any(|c| c.finished && c.exit_code == Some(fail_code)),
        "{:?}",
        snapshot.commands
    );

    h.service.resize(id, 100, 30);
    let snapshot = h.service.snapshot(id).unwrap();
    assert_eq!((snapshot.cols, snapshot.rows), (100, 30));
    h.service.write(id, b"echo after-resize\r");
    h.wait_screen(id, "output after resize", |s| has_line(s, "after-resize"));

    h.service.write(id, format!("{exit}\r").as_bytes());
    let event = h.wait_event(id, "Exited", |e| matches!(e, TerminalEvent::Exited { .. }));
    assert_eq!(event, TerminalEvent::Exited { id, code: Some(7) });
    let snapshot = h.service.snapshot(id).unwrap();
    assert_eq!(snapshot.exited, Some(7));
    // The session stays open, showing its output.
    assert_eq!(h.service.sessions().len(), 1);
    h.service.close_session(id);
    assert!(h.service.sessions().is_empty());
}

#[cfg(windows)]
#[test]
fn cmd_runs_commands_and_exits() {
    // cmd reports prompts but no exit codes; check the rest.
    let Some(shell) = find_shell(&["cmd"]) else {
        return;
    };
    let mut h = Harness::new();
    let id = open(&h, &shell, None);
    h.service.write(id, b"echo hello-ferro\r");
    h.wait_screen(id, "echo output", |s| has_line(s, "hello-ferro"));
    h.wait_screen(id, "prompt marks", |s| s.commands.len() >= 2);
    h.service.resize(id, 60, 10);
    assert_eq!(h.service.snapshot(id).unwrap().cols, 60);
    h.service.write(id, b"exit 7\r");
    let event = h.wait_event(id, "Exited", |e| matches!(e, TerminalEvent::Exited { .. }));
    assert_eq!(event, TerminalEvent::Exited { id, code: Some(7) });
}

#[cfg(windows)]
#[test]
fn powershell_reports_exit_codes() {
    echo_exit_code_resize_exit(&["pwsh", "powershell"], "cmd /c exit 5", 5, "exit 7");
}

#[cfg(windows)]
#[test]
fn git_bash_reports_exit_codes() {
    echo_exit_code_resize_exit(&["bash"], "(exit 4)", 4, "exit 7");
}

#[cfg(unix)]
#[test]
fn bash_reports_exit_codes() {
    echo_exit_code_resize_exit(&["bash"], "(exit 4)", 4, "exit 7");
}

#[cfg(unix)]
#[test]
fn sh_runs_commands_and_exits() {
    let Some(shell) = find_shell(&["sh"]) else {
        return;
    };
    let mut h = Harness::new();
    let id = open(&h, &shell, None);
    h.service.write(id, b"echo hello-ferro\r");
    h.wait_screen(id, "echo output", |s| has_line(s, "hello-ferro"));
    h.service.write(id, b"exit 7\r");
    let event = h.wait_event(id, "Exited", |e| matches!(e, TerminalEvent::Exited { .. }));
    assert_eq!(event, TerminalEvent::Exited { id, code: Some(7) });
}

#[test]
fn a_command_is_typed_once_the_shell_is_ready() {
    let Some(shell) = find_shell(&["pwsh", "powershell", "bash", "zsh", "sh", "cmd"]) else {
        return;
    };
    let mut h = Harness::new();
    let id = open(&h, &shell, Some("echo typed-by-ferro"));
    h.wait_screen(id, "the typed command's output", |s| {
        has_line(s, "typed-by-ferro")
    });
}

/// The shell's process id, printed and read from the screen.
fn shell_pid(h: &mut Harness, id: u64, shell: &ShellInfo) -> u32 {
    let print = if shell.id.contains("powershell") || shell.id == "pwsh" {
        "echo \"pid=$PID\"\r"
    } else {
        "echo \"pid=$$\"\r"
    };
    h.service.write(id, print.as_bytes());
    let mut pid = 0;
    h.wait_screen(id, "the pid", |s| {
        s.lines.iter().any(|l| {
            line_text(l)
                .trim_end()
                .strip_prefix("pid=")
                .is_some_and(|p| p.parse::<u32>().is_ok())
        })
    });
    for line in h.service.snapshot(id).unwrap().lines {
        if let Some(p) = line_text(&line).trim_end().strip_prefix("pid=")
            && let Ok(p) = p.parse()
        {
            pid = p;
        }
    }
    pid
}

fn process_alive(pid: u32) -> bool {
    if cfg!(windows) {
        let out = std::process::Command::new("tasklist")
            .args(["/FI", &format!("PID eq {pid}"), "/NH"])
            .output()
            .unwrap();
        String::from_utf8_lossy(&out.stdout).contains(&pid.to_string())
    } else {
        std::process::Command::new("kill")
            .args(["-0", &pid.to_string()])
            .status()
            .is_ok_and(|s| s.success())
    }
}

#[test]
fn closing_a_session_ends_the_shell() {
    let Some(shell) = find_shell(&["pwsh", "powershell", "bash", "sh"]) else {
        return;
    };
    let mut h = Harness::new();
    let id = open(&h, &shell, None);
    let pid = shell_pid(&mut h, id, &shell);
    assert!(process_alive(pid));
    h.service.close_session(id);
    assert!(h.service.snapshot(id).is_none());
    let start = Instant::now();
    while process_alive(pid) {
        assert!(start.elapsed() < TIMEOUT, "shell {pid} still runs");
        std::thread::sleep(Duration::from_millis(50));
    }
    // Dropping the service with a live session ends that shell too.
    let id = open(&h, &shell, None);
    let pid = shell_pid(&mut h, id, &shell);
    drop(h);
    let start = Instant::now();
    while process_alive(pid) {
        assert!(start.elapsed() < TIMEOUT, "shell {pid} still runs");
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[test]
fn titles_take_the_lowest_free_number_and_bad_shells_fail() {
    let Some(shell) = find_shell(&["cmd", "sh", "bash", "pwsh", "powershell"]) else {
        return;
    };
    let h = Harness::new();
    let a = open(&h, &shell, None);
    let b = open(&h, &shell, None);
    let titles: Vec<_> = h.service.sessions().into_iter().map(|s| s.title).collect();
    assert_eq!(titles, ["Local", "Local (2)"]);
    h.service.close_session(a);
    let c = open(&h, &shell, None);
    assert_eq!(
        h.service
            .sessions()
            .into_iter()
            .find(|s| s.id == c)
            .unwrap()
            .title,
        "Local"
    );
    assert!(
        h.service
            .open(NewTerminal {
                shell: Some("no-such-shell-xyz".into()),
                ..NewTerminal::default()
            })
            .is_err()
    );
    h.service.close_session(b);
    h.service.close_session(c);
}

#[test]
fn output_floods_send_few_change_events() {
    let Some(shell) = find_shell(&["pwsh", "powershell", "bash", "sh", "cmd"]) else {
        return;
    };
    let mut h = Harness::new();
    let id = open(&h, &shell, None);
    let flood = if shell.id == "cmd" {
        "for /L %i in (1,1,2000) do @echo line %i\r"
    } else if shell.id.contains("powershell") || shell.id == "pwsh" {
        "1..2000 | % { \"line $_\" }\r"
    } else {
        "i=1; while [ $i -le 2000 ]; do echo line $i; i=$((i+1)); done\r"
    };
    h.service.write(id, flood.as_bytes());
    // Without taking snapshots, at most one Changed is pending.
    std::thread::sleep(Duration::from_secs(2));
    h.drain();
    let changed = h
        .seen
        .iter()
        .filter(|e| **e == TerminalEvent::Changed(id))
        .count();
    assert_eq!(changed, 1, "{:?}", h.seen);
    h.wait_screen(id, "the end of the flood", |s| has_line(s, "line 2000"));
}
