//! A debug session paused at a breakpoint in `greet`.

use std::{
    path::{Path, PathBuf},
    sync::Mutex,
};

use anyhow::{anyhow, bail};

use super::{join_rel, lock, norm};
use crate::{
    Breakpoint, DebugEvent, DebugService, DebugState, DebugThread, Result, StackFrame, Variable,
};

pub struct MockDebugService {
    root: PathBuf,
    breakpoints: Mutex<Vec<Breakpoint>>,
    console: Mutex<Vec<String>>,
    subscribers: Mutex<Vec<async_channel::Sender<DebugEvent>>>,
}

fn var(name: &str, value: &str, type_name: &str, children: Vec<Variable>) -> Variable {
    Variable {
        name: name.into(),
        value: value.into(),
        type_name: type_name.into(),
        children,
    }
}

impl MockDebugService {
    pub fn new(root: PathBuf) -> Self {
        let bp = |rel: &str, line| Breakpoint {
            path: join_rel(&root, rel),
            line,
            enabled: true,
            condition: None,
        };
        Self {
            breakpoints: Mutex::new(vec![bp("src/main.rs", 4), bp("src/lib.rs", 9)]),
            console: Mutex::new(vec![
                "Launching target/debug/demo-app".into(),
                "Process 4242 launched".into(),
                "Hit breakpoint at src/lib.rs:10 in demo_app::greet".into(),
            ]),
            subscribers: Mutex::new(Vec::new()),
            root,
        }
    }

    fn broadcast(&self, event: DebugEvent) {
        lock(&self.subscribers).retain(|tx| tx.try_send(event.clone()).is_ok() || !tx.is_closed());
    }

    fn frame_variables(frame: usize) -> Vec<Variable> {
        let names = var(
            "names",
            "len = 2",
            "Vec<String>",
            vec![
                var("[0]", "\"Ada\"", "String", Vec::new()),
                var("[1]", "\"Grace\"", "String", Vec::new()),
            ],
        );
        let config = var(
            "config",
            "Config { names: [..], retries: 3 }",
            "demo_app::config::Config",
            vec![names, var("retries", "3", "u32", Vec::new())],
        );
        match frame {
            0 => vec![var("name", "\"Ada\"", "&str", Vec::new())],
            1 => vec![config, var("name", "\"Ada\"", "&String", Vec::new())],
            _ => Vec::new(),
        }
    }
}

/// `a op b` for integers, the only arithmetic the mock console knows.
fn arithmetic(expression: &str) -> Option<i64> {
    let ops = ['+', '-', '*', '/'];
    let ix = expression
        .char_indices()
        .skip(1)
        .find(|(_, c)| ops.contains(c))
        .map(|(i, _)| i);
    let Some(ix) = ix else {
        return expression.trim().parse().ok();
    };
    let (a, b) = (
        expression[..ix].trim().parse::<i64>().ok()?,
        arithmetic(&expression[ix + 1..])?,
    );
    match &expression[ix..ix + 1] {
        "+" => a.checked_add(b),
        "-" => a.checked_sub(b),
        "*" => a.checked_mul(b),
        _ => a.checked_div(b),
    }
}

impl DebugService for MockDebugService {
    fn state(&self) -> DebugState {
        DebugState::Paused
    }

    fn breakpoints(&self) -> Vec<Breakpoint> {
        lock(&self.breakpoints).clone()
    }

    fn breakpoints_in(&self, path: &Path) -> Vec<Breakpoint> {
        let path = norm(path);
        lock(&self.breakpoints)
            .iter()
            .filter(|b| norm(&b.path) == path)
            .cloned()
            .collect()
    }

    fn toggle_breakpoint(&self, path: &Path, line: u32) {
        {
            let mut bps = lock(&self.breakpoints);
            let n = norm(path);
            match bps
                .iter()
                .position(|b| norm(&b.path) == n && b.line == line)
            {
                Some(ix) => {
                    bps.remove(ix);
                }
                None => bps.push(Breakpoint {
                    path: path.to_path_buf(),
                    line,
                    enabled: true,
                    condition: None,
                }),
            }
        }
        self.broadcast(DebugEvent::BreakpointsChanged);
    }

    fn set_breakpoint_enabled(&self, path: &Path, line: u32, enabled: bool) {
        let changed = {
            let n = norm(path);
            let mut bps = lock(&self.breakpoints);
            match bps
                .iter_mut()
                .find(|b| norm(&b.path) == n && b.line == line)
            {
                Some(b) if b.enabled != enabled => {
                    b.enabled = enabled;
                    true
                }
                _ => false,
            }
        };
        if changed {
            self.broadcast(DebugEvent::BreakpointsChanged);
        }
    }

    fn threads(&self) -> Vec<DebugThread> {
        let frame = |name: &str, rel: Option<&str>, line, is_library| StackFrame {
            name: name.into(),
            path: rel.map(|r| join_rel(&self.root, r)),
            line,
            is_library,
        };
        vec![
            DebugThread {
                id: 1,
                name: "main".into(),
                paused: true,
                frames: vec![
                    frame("demo_app::greet", Some("src/lib.rs"), 9, false),
                    frame("demo_app::main", Some("src/main.rs"), 6, false),
                    frame("core::ops::function::FnOnce::call_once", None, 250, true),
                    frame("std::rt::lang_start", None, 199, true),
                ],
            },
            DebugThread {
                id: 2,
                name: "tokio-runtime-worker".into(),
                paused: false,
                frames: Vec::new(),
            },
        ]
    }

    fn variables(&self, thread: u64, frame: usize) -> Result<Vec<Variable>> {
        let threads = self.threads();
        let t = threads
            .iter()
            .find(|t| t.id == thread)
            .ok_or_else(|| anyhow!("no thread {thread}"))?;
        if !t.paused {
            bail!("thread {} is running", t.name);
        }
        if frame >= t.frames.len() {
            bail!("no frame {frame}");
        }
        Ok(Self::frame_variables(frame))
    }

    fn console(&self) -> Vec<String> {
        lock(&self.console).clone()
    }

    fn evaluate(&self, expression: &str) -> Result<String> {
        let expr = expression.trim();
        let result = match expr {
            "" => Err(anyhow!("nothing to evaluate")),
            "name" => Ok("\"Ada\"".to_string()),
            "config.retries" | "retries" => Ok("3".to_string()),
            "config.names" => Ok("[\"Ada\", \"Grace\"]".to_string()),
            "config.names.len()" => Ok("2".to_string()),
            _ => arithmetic(expr)
                .map(|n| n.to_string())
                .ok_or_else(|| anyhow!("cannot evaluate `{expr}` in the mock")),
        };
        let mut console = lock(&self.console);
        console.push(format!("> {expr}"));
        match &result {
            Ok(value) => console.push(value.clone()),
            Err(err) => console.push(format!("error: {err}")),
        }
        result
    }

    fn subscribe(&self) -> async_channel::Receiver<DebugEvent> {
        let (tx, rx) = async_channel::unbounded();
        lock(&self.subscribers).push(tx);
        rx
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn debug() -> (MockDebugService, PathBuf) {
        let root = PathBuf::from("/p/demo-app");
        (MockDebugService::new(root.clone()), root)
    }

    #[test]
    fn toggling_a_breakpoint_notifies_subscribers() {
        let (debug, root) = debug();
        let events = debug.subscribe();
        let main = root.join("src").join("main.rs");
        assert_eq!(debug.breakpoints_in(&main).len(), 1);
        debug.toggle_breakpoint(&main, 7);
        assert_eq!(events.try_recv(), Ok(DebugEvent::BreakpointsChanged));
        assert_eq!(debug.breakpoints_in(&main).len(), 2);
        debug.toggle_breakpoint(&main, 7);
        assert_eq!(debug.breakpoints_in(&main).len(), 1);

        debug.set_breakpoint_enabled(&main, 4, false);
        assert!(!debug.breakpoints_in(&main)[0].enabled);
        while events.try_recv().is_ok() {}
        debug.set_breakpoint_enabled(&main, 4, false);
        assert!(events.try_recv().is_err(), "no event without a change");
    }

    #[test]
    fn the_paused_thread_has_frames_and_variables() {
        let (debug, _) = debug();
        let threads = debug.threads();
        assert!(threads[0].paused && threads[0].frames.len() >= 3);
        assert!(threads[0].frames.iter().any(|f| f.is_library));
        let vars = debug.variables(1, 1).unwrap();
        let config = vars.iter().find(|v| v.name == "config").unwrap();
        assert_eq!(config.children[0].children.len(), 2);
        assert!(debug.variables(2, 0).is_err(), "running thread");
        assert!(debug.variables(1, 99).is_err());
    }

    #[test]
    fn the_console_evaluates_simple_expressions() {
        let (debug, _) = debug();
        assert_eq!(debug.evaluate("name").unwrap(), "\"Ada\"");
        assert_eq!(debug.evaluate("1 + 2 * 3").unwrap(), "7");
        assert_eq!(debug.evaluate("-4+1").unwrap(), "-3");
        assert!(debug.evaluate("launch_missiles()").is_err());
        let console = debug.console();
        assert!(console.iter().any(|l| l == "> name"));
        assert!(console.last().unwrap().starts_with("error:"));
    }
}
