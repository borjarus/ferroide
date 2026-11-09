//! Static terminal sessions with colored output.

use std::{
    path::{Path, PathBuf},
    sync::{
        Mutex,
        atomic::{AtomicU64, Ordering},
    },
};

use super::lock;
use crate::{TerminalLine, TerminalService, TerminalSession, parse_ansi};

pub struct MockTerminalService {
    root: PathBuf,
    sessions: Mutex<Vec<TerminalSession>>,
    next_id: AtomicU64,
}

fn prompt(root: &Path, command: &str) -> TerminalLine {
    let text = if cfg!(windows) {
        format!("\x1b[32mPS {}>\x1b[0m {command}", root.display())
    } else {
        format!(
            "\x1b[1;32mdev@ferro\x1b[0m:\x1b[1;34m~/projects/{}\x1b[0m$ {command}",
            root.file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default()
        )
    };
    parse_ansi(&text)
}

impl MockTerminalService {
    pub fn new(root: PathBuf) -> Self {
        let build: Vec<TerminalLine> = std::iter::once(prompt(&root, "cargo build"))
            .chain(
                [
                    "\x1b[1;32m   Compiling\x1b[0m serde v1.0.228",
                    "\x1b[1;32m   Compiling\x1b[0m toml v0.9.8",
                    "\x1b[1;32m   Compiling\x1b[0m demo-app v0.1.0 (C:\\Projects\\demo-app)",
                    "\x1b[1;33mwarning\x1b[0m\x1b[1m: field `retries` is never read\x1b[0m",
                    "  \x1b[1;34m-->\x1b[0m src/config.rs:11:9",
                    "   \x1b[1;34m|\x1b[0m",
                    "\x1b[1;34m11\x1b[0m \x1b[1;34m|\x1b[0m     pub retries: u32,",
                    "   \x1b[1;34m|\x1b[0m         \x1b[1;33m^^^^^^^\x1b[0m",
                    "",
                    "\x1b[1;33mwarning\x1b[0m: `demo-app` (lib) generated 1 warning",
                    "\x1b[1;32m    Finished\x1b[0m `dev` profile [unoptimized + debuginfo] target(s) in 1.42s",
                ]
                .into_iter()
                .map(parse_ansi),
            )
            .chain(std::iter::once(prompt(&root, "")))
            .collect();
        let status: Vec<TerminalLine> = std::iter::once(prompt(&root, "git status"))
            .chain(
                [
                    "On branch main",
                    "You have unmerged paths.",
                    "",
                    "Changes to be committed:",
                    "\t\x1b[32mnew file:   web/src/app.ts\x1b[0m",
                    "",
                    "Unmerged paths:",
                    "\t\x1b[31mboth modified:   src/merge_me.rs\x1b[0m",
                    "",
                    "Changes not staged for commit:",
                    "\t\x1b[31mmodified:   README.md\x1b[0m",
                    "\t\x1b[31mmodified:   src/config.rs\x1b[0m",
                    "\t\x1b[31mmodified:   src/main.rs\x1b[0m",
                    "",
                    "Untracked files:",
                    "\t\x1b[31mdocs/notes.md\x1b[0m",
                ]
                .into_iter()
                .map(parse_ansi),
            )
            .chain(std::iter::once(prompt(&root, "")))
            .collect();
        Self {
            sessions: Mutex::new(vec![
                TerminalSession {
                    id: 1,
                    title: "Local".into(),
                    lines: build,
                },
                TerminalSession {
                    id: 2,
                    title: "Local (2)".into(),
                    lines: status,
                },
            ]),
            next_id: AtomicU64::new(3),
            root,
        }
    }
}

impl TerminalService for MockTerminalService {
    fn sessions(&self) -> Vec<TerminalSession> {
        lock(&self.sessions).clone()
    }

    fn new_session(&self) -> TerminalSession {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let mut sessions = lock(&self.sessions);
        // The lowest free "Local (n)" number, as IntelliJ does.
        let title = (1..)
            .map(|n| {
                if n == 1 {
                    "Local".to_string()
                } else {
                    format!("Local ({n})")
                }
            })
            .find(|t| !sessions.iter().any(|s| &s.title == t))
            .unwrap_or_default();
        let session = TerminalSession {
            id,
            title,
            lines: vec![prompt(&self.root, "")],
        };
        sessions.push(session.clone());
        session
    }

    fn close_session(&self, id: u64) {
        lock(&self.sessions).retain(|s| s.id != id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::AnsiColor;

    #[test]
    fn sessions_have_colored_output_and_can_be_opened_and_closed() {
        let terminal = MockTerminalService::new(PathBuf::from("/p/demo-app"));
        let sessions = terminal.sessions();
        assert_eq!(sessions.len(), 2);
        let spans: Vec<_> = sessions[0].lines.iter().flatten().collect();
        assert!(
            spans
                .iter()
                .any(|s| s.fg == AnsiColor::Yellow && s.text == "warning")
        );
        assert!(
            spans
                .iter()
                .any(|s| s.fg == AnsiColor::Green && s.text.contains("Finished"))
        );

        let new = terminal.new_session();
        assert_eq!(new.title, "Local (3)");
        assert_eq!(terminal.sessions().len(), 3);
        terminal.close_session(1);
        let again = terminal.new_session();
        assert_eq!(again.title, "Local");
        assert_ne!(again.id, new.id);
    }
}
