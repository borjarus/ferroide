//! Debugging: breakpoints, threads, call stacks and variables. Stage 10
//! backs this with DAP; until then the mock serves a paused session.

use std::path::{Path, PathBuf};

use crate::Result;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Breakpoint {
    pub path: PathBuf,
    /// 0-based.
    pub line: u32,
    pub enabled: bool,
    pub condition: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StackFrame {
    /// e.g. `demo_app::greet`.
    pub name: String,
    pub path: Option<PathBuf>,
    /// 0-based.
    pub line: u32,
    /// Library frames are dimmed and collapsed by default.
    pub is_library: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DebugThread {
    pub id: u64,
    pub name: String,
    pub paused: bool,
    pub frames: Vec<StackFrame>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Variable {
    pub name: String,
    pub value: String,
    pub type_name: String,
    pub children: Vec<Variable>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DebugState {
    NotStarted,
    Running,
    Paused,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DebugEvent {
    BreakpointsChanged,
    StateChanged(DebugState),
}

pub trait DebugService: Send + Sync + 'static {
    fn state(&self) -> DebugState;
    fn breakpoints(&self) -> Vec<Breakpoint>;
    fn breakpoints_in(&self, path: &Path) -> Vec<Breakpoint> {
        self.breakpoints()
            .into_iter()
            .filter(|b| b.path == path)
            .collect()
    }
    /// Add a breakpoint at `line`, or remove the one there.
    fn toggle_breakpoint(&self, path: &Path, line: u32);
    fn set_breakpoint_enabled(&self, path: &Path, line: u32, enabled: bool);
    fn threads(&self) -> Vec<DebugThread>;
    /// Variables visible in frame `frame` of thread `thread`.
    fn variables(&self, thread: u64, frame: usize) -> Result<Vec<Variable>>;
    /// Debug console output so far.
    fn console(&self) -> Vec<String>;
    /// Evaluate an expression in the console.
    fn evaluate(&self, expression: &str) -> Result<String>;
    fn subscribe(&self) -> async_channel::Receiver<DebugEvent>;
}
