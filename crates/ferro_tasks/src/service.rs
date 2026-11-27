//! [`TaskRunService`]: the project's [`RunService`].
//!
//! Each start runs on its own thread: before-launch configurations one
//! after another, then either the members of a compound or the process,
//! whose output lines and test results become [`RunEvent`]s. State lives
//! behind one mutex; events are sent while holding it, so every
//! subscriber sees them in the order they happened. A condition variable
//! signals state changes to compounds waiting for their members.
//!
//! Runs are identified by configuration id, and each start gets a new
//! generation: [`RunService::stop`] reports `Finished { exit_code: 130 }`
//! at once and kills the process tree, and anything the old thread still
//! produces afterwards is dropped, so a restart right after a stop gets a
//! clean console.
//!
//! Configurations are the saved ones ([`ConfigStore`]), then temporary
//! ones (▶ icons, build tool tasks, failed test reruns — the most recent
//! [`RunOptions::max_temporary`]), then — only while nothing is saved —
//! defaults derived from the build tools found, so the run selector is
//! never empty. A background poll (every two seconds, ending with the
//! service) notices edits of `.ferroide/run/` made outside the IDE.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, OnceLock, Weak};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Result, anyhow, bail};
use ferro_services::{
    BuildProject, BuildTask, RunConfiguration, RunEvent, RunService, RunState, Runnable,
    ServiceKind, ServiceNode, ServiceState, TestNode, TestStatus,
};
use futures::future::BoxFuture;

use crate::build_tools;
use crate::exec;
use crate::process::{self, ProcessControl};
use crate::runnables;
use crate::store::{ConfigStore, sanitize_id};
use crate::testing::{Detected, TestCase, rerun_command};

/// How deep the build tool walk for the tool window goes.
const BUILD_DEPTH: usize = 6;
/// How deep the walk for default configurations and templates goes.
const DEFAULTS_DEPTH: usize = 3;
/// How often a running test tree is republished at most.
const TREE_INTERVAL: Duration = Duration::from_millis(150);
/// How often `.ferroide/run/` is checked for outside edits.
const POLL_INTERVAL: Duration = Duration::from_secs(2);
/// Nested before-launch / compound levels allowed (cycles end here).
const MAX_NESTING: u32 = 8;

/// Dim (SGR 2) console text, for the lines the runner adds itself.
fn dim(text: &str) -> String {
    format!("\x1b[2m{text}\x1b[0m")
}

/// Settings of a [`TaskRunService`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunOptions {
    /// Project roots; configurations live in the first one.
    pub roots: Vec<PathBuf>,
    /// Temporary configurations kept (oldest dropped first).
    pub max_temporary: usize,
    /// Variables set for every run, below a configuration's own.
    pub env: Vec<(String, String)>,
}

impl Default for RunOptions {
    fn default() -> Self {
        Self {
            roots: Vec::new(),
            max_temporary: 5,
            env: Vec::new(),
        }
    }
}

/// The results of a configuration's last test run.
struct TestRecord {
    tree: Vec<TestNode>,
    cases: Vec<TestCase>,
    detected: Detected,
    config: RunConfiguration,
    cwd: PathBuf,
}

struct Active {
    generation: u64,
    control: Arc<ProcessControl>,
}

#[derive(Default)]
struct State {
    temporaries: Vec<RunConfiguration>,
    runs: HashMap<String, RunState>,
    active: HashMap<String, Active>,
    /// Members started by a running compound.
    members: HashMap<String, Vec<String>>,
    generation: u64,
    tests: HashMap<String, TestRecord>,
    last_test_run: Option<String>,
    working_dirs: HashMap<String, PathBuf>,
    subscribers: Vec<async_channel::Sender<RunEvent>>,
}

struct Shared {
    root: PathBuf,
    options: RunOptions,
    store: ConfigStore,
    defaults: OnceLock<(Vec<RunConfiguration>, Vec<RunConfiguration>)>,
    state: Mutex<State>,
    changed: Condvar,
}

/// Run configurations, processes, build tool tasks and test results of a
/// project.
pub struct TaskRunService {
    shared: Arc<Shared>,
}

impl TaskRunService {
    pub fn new(options: RunOptions) -> Result<Self> {
        let root = options
            .roots
            .first()
            .cloned()
            .ok_or_else(|| anyhow!("a run service needs a project root"))?;
        let shared = Arc::new(Shared {
            store: ConfigStore::new(&root),
            root,
            options,
            defaults: OnceLock::new(),
            state: Mutex::new(State::default()),
            changed: Condvar::new(),
        });
        shared.store.refresh();
        let weak = Arc::downgrade(&shared);
        thread::Builder::new()
            .name("run-config-poll".into())
            .spawn(move || poll(weak))?;
        Ok(Self { shared })
    }
}

fn poll(weak: Weak<Shared>) {
    loop {
        thread::sleep(POLL_INTERVAL);
        let Some(shared) = weak.upgrade() else { return };
        shared.saved();
    }
}

impl Shared {
    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn broadcast_locked(&self, state: &mut State, event: RunEvent) {
        state
            .subscribers
            .retain(|tx| tx.try_send(event.clone()).is_ok() || !tx.is_closed());
    }

    fn broadcast(&self, event: RunEvent) {
        let mut state = self.lock();
        self.broadcast_locked(&mut state, event);
    }

    /// Saved configurations; announces outside edits.
    fn saved(&self) -> Vec<RunConfiguration> {
        let (configs, changed) = self.store.refresh();
        if changed {
            self.broadcast(RunEvent::ConfigurationsChanged);
        }
        configs
    }

    /// (defaults, templates) from the build tools near the root.
    fn derived(&self) -> &(Vec<RunConfiguration>, Vec<RunConfiguration>) {
        self.defaults.get_or_init(|| {
            let projects = build_tools::discover(&self.options.roots, DEFAULTS_DEPTH);
            (
                build_tools::default_configurations(&projects, &self.root),
                build_tools::templates(&projects, &self.root),
            )
        })
    }

    fn configurations(&self) -> Vec<RunConfiguration> {
        let mut all = self.saved();
        let saved_any = !all.is_empty();
        let temporaries: Vec<RunConfiguration> =
            self.lock().temporaries.iter().rev().cloned().collect();
        for t in temporaries {
            if !all.iter().any(|c| c.id == t.id) {
                all.push(t);
            }
        }
        if !saved_any {
            for d in &self.derived().0 {
                if !all.iter().any(|c| c.id == d.id) {
                    all.push(d.clone());
                }
            }
        }
        all
    }

    fn find(&self, id: &str) -> Option<RunConfiguration> {
        self.configurations().into_iter().find(|c| c.id == id)
    }

    fn add_temporary(&self, config: RunConfiguration) {
        let mut state = self.lock();
        state.temporaries.retain(|c| c.id != config.id);
        state.temporaries.push(config);
        let max = self.options.max_temporary.max(1);
        while state.temporaries.len() > max {
            // Drop the oldest that is not running.
            let runs = &state.runs;
            let Some(i) = state
                .temporaries
                .iter()
                .position(|c| !matches!(runs.get(&c.id), Some(RunState::Running { .. })))
            else {
                break;
            };
            state.temporaries.remove(i);
        }
        self.broadcast_locked(&mut state, RunEvent::ConfigurationsChanged);
    }

    fn set_state(&self, state: &mut State, id: &str, run: RunState) {
        state.runs.insert(id.to_string(), run);
        self.broadcast_locked(
            state,
            RunEvent::StateChanged {
                id: id.to_string(),
                state: run,
            },
        );
        self.changed.notify_all();
    }

    fn state_of(&self, id: &str) -> RunState {
        self.lock().runs.get(id).copied().unwrap_or(RunState::Idle)
    }

    fn output(&self, id: &str, generation: u64, line: String, error: bool) {
        let mut state = self.lock();
        if state
            .active
            .get(id)
            .is_some_and(|a| a.generation == generation)
        {
            self.broadcast_locked(
                &mut state,
                RunEvent::Output {
                    id: id.to_string(),
                    line,
                    error,
                },
            );
        }
    }

    fn start(self: &Arc<Self>, id: &str, debug: bool) -> Result<()> {
        let config = self
            .find(id)
            .ok_or_else(|| anyhow!("unknown run configuration {id}"))?;
        let control = ProcessControl::new();
        let generation = {
            let mut state = self.lock();
            if matches!(state.runs.get(id), Some(RunState::Running { .. })) {
                bail!("{} is already running", config.name);
            }
            state.generation += 1;
            let generation = state.generation;
            state.active.insert(
                id.to_string(),
                Active {
                    generation,
                    control: control.clone(),
                },
            );
            state.members.remove(id);
            self.set_state(&mut state, id, RunState::Running { debug });
            generation
        };
        let shared = self.clone();
        let run_id = id.to_string();
        let spawned = thread::Builder::new()
            .name(format!("run-{id}"))
            .spawn(move || {
                let code = shared.execute(&run_id, generation, &config, debug, &control, 0);
                let mut state = shared.lock();
                let current = state
                    .active
                    .get(&run_id)
                    .is_some_and(|a| a.generation == generation);
                if current && !control.is_stopped() {
                    shared.set_state(&mut state, &run_id, RunState::Finished { exit_code: code });
                }
            });
        if let Err(e) = spawned {
            let mut state = self.lock();
            self.set_state(&mut state, id, RunState::Finished { exit_code: -1 });
            bail!("cannot start a run thread: {e}");
        }
        Ok(())
    }

    /// Run `config` on this thread: before-launch steps, then the
    /// compound's members or the process. Returns the exit code.
    fn execute(
        self: &Arc<Self>,
        id: &str,
        generation: u64,
        config: &RunConfiguration,
        debug: bool,
        control: &Arc<ProcessControl>,
        depth: u32,
    ) -> i32 {
        if depth > MAX_NESTING {
            self.output(
                id,
                generation,
                format!(
                    "{} nests before-launch or compound configurations too deeply",
                    config.name
                ),
                true,
            );
            return -1;
        }
        for before in &config.before_launch {
            if control.is_stopped() {
                return 130;
            }
            let Some(step) = self.find(before) else {
                self.output(
                    id,
                    generation,
                    format!("Before launch: unknown run configuration {before}; launch abandoned"),
                    true,
                );
                return -1;
            };
            self.output(
                id,
                generation,
                dim(&format!("Before launch: {}", step.name)),
                false,
            );
            let code = self.execute(id, generation, &step, false, control, depth + 1);
            if code != 0 {
                if !control.is_stopped() {
                    self.output(
                        id,
                        generation,
                        format!(
                            "Before launch: {} failed with exit code {code}; launch abandoned",
                            step.name
                        ),
                        true,
                    );
                }
                return code;
            }
        }
        if control.is_stopped() {
            return 130;
        }
        if config.is_compound() {
            return self.run_compound(id, generation, config, debug, control);
        }
        self.run_process(id, generation, config, control, depth == 0)
    }

    fn run_compound(
        self: &Arc<Self>,
        id: &str,
        generation: u64,
        config: &RunConfiguration,
        debug: bool,
        control: &Arc<ProcessControl>,
    ) -> i32 {
        let mut started: Vec<String> = Vec::new();
        let mut failed_to_start = false;
        for member in &config.compound {
            if member == id {
                continue;
            }
            match self.start(member, debug) {
                Ok(()) => {
                    self.output(id, generation, dim(&format!("Started {member}")), false);
                    started.push(member.clone());
                }
                Err(e) => {
                    self.output(id, generation, format!("Cannot start {member}: {e}"), true);
                    failed_to_start = true;
                }
            }
        }
        self.lock().members.insert(id.to_string(), started.clone());
        let mut state = self.lock();
        loop {
            if control.is_stopped() {
                return 130;
            }
            let running = started
                .iter()
                .any(|m| matches!(state.runs.get(m), Some(RunState::Running { .. })));
            if !running {
                break;
            }
            state = self
                .changed
                .wait_timeout(state, Duration::from_millis(500))
                .map(|(s, _)| s)
                .unwrap_or_else(|e| e.into_inner().0);
        }
        let first_failure = started.iter().find_map(|m| match state.runs.get(m) {
            Some(RunState::Finished { exit_code }) if *exit_code != 0 => Some(*exit_code),
            _ => None,
        });
        first_failure.unwrap_or(if failed_to_start { -1 } else { 0 })
    }

    fn run_process(
        &self,
        id: &str,
        generation: u64,
        config: &RunConfiguration,
        control: &ProcessControl,
        collect_tests: bool,
    ) -> i32 {
        let prepared = match exec::prepare(config, &self.root, &self.options.env, collect_tests) {
            Ok(p) => p,
            Err(e) => {
                self.output(id, generation, format!("{e:#}"), true);
                return -1;
            }
        };
        let exec::Prepared {
            launch,
            display,
            tests,
        } = prepared;
        self.lock()
            .working_dirs
            .insert(id.to_string(), launch.cwd.clone());
        self.output(id, generation, dim(&display), false);
        let mut session = tests;
        if let Some(s) = &session {
            self.publish_tests(id, generation, s, config, &launch.cwd);
        }
        let mut last_tree = Instant::now();
        let mut dirty = false;
        let result = process::run(&launch, control, |line, error| match session.as_mut() {
            Some(s) => {
                let outcome = s.line(&line, error);
                match outcome.display {
                    Some(lines) => {
                        for l in lines {
                            self.output(id, generation, l, error);
                        }
                    }
                    None => self.output(id, generation, line, error),
                }
                dirty |= outcome.changed;
                if dirty && last_tree.elapsed() >= TREE_INTERVAL {
                    self.publish_tests(id, generation, s, config, &launch.cwd);
                    last_tree = Instant::now();
                    dirty = false;
                }
            }
            None => self.output(id, generation, line, error),
        });
        let code = match result {
            Ok(code) => code,
            Err(e) => {
                self.output(
                    id,
                    generation,
                    format!("Cannot run {}: {e}", launch.program.display()),
                    true,
                );
                -1
            }
        };
        if let Some(s) = session.as_mut() {
            s.finish(control.is_stopped());
            self.publish_tests(id, generation, s, config, &launch.cwd);
        }
        if control.is_stopped() { 130 } else { code }
    }

    fn publish_tests(
        &self,
        id: &str,
        generation: u64,
        session: &crate::testing::TestSession,
        config: &RunConfiguration,
        cwd: &Path,
    ) {
        let mut state = self.lock();
        if !state
            .active
            .get(id)
            .is_some_and(|a| a.generation == generation)
        {
            return;
        }
        state.tests.insert(
            id.to_string(),
            TestRecord {
                tree: session.tree(),
                cases: session.cases().to_vec(),
                detected: session.detected.clone(),
                config: config.clone(),
                cwd: cwd.to_path_buf(),
            },
        );
        state.last_test_run = Some(id.to_string());
        self.broadcast_locked(&mut state, RunEvent::Tests { id: id.to_string() });
    }

    fn stop(&self, id: &str) {
        let (control, members) = {
            let mut state = self.lock();
            if !matches!(state.runs.get(id), Some(RunState::Running { .. })) {
                return;
            }
            let control = state.active.get(id).map(|a| a.control.clone());
            let members = state.members.remove(id).unwrap_or_default();
            self.set_state(&mut state, id, RunState::Finished { exit_code: 130 });
            (control, members)
        };
        if let Some(control) = control {
            control.stop();
        }
        for member in members {
            self.stop(&member);
        }
    }
}

fn validate(config: &RunConfiguration) -> Result<()> {
    if config.id.trim().is_empty() {
        bail!("a run configuration needs an id");
    }
    if config.name.trim().is_empty() {
        bail!("a run configuration needs a name");
    }
    if !config.is_compound() && config.command.trim().is_empty() {
        bail!("a run configuration needs a command");
    }
    if config.compound.contains(&config.id) || config.before_launch.contains(&config.id) {
        bail!("{} cannot run itself", config.name);
    }
    Ok(())
}

impl RunService for TaskRunService {
    fn configurations(&self) -> Vec<RunConfiguration> {
        self.shared.configurations()
    }

    fn state(&self, id: &str) -> RunState {
        self.shared.state_of(id)
    }

    fn start(&self, id: &str, debug: bool) -> Result<()> {
        self.shared.start(id, debug)
    }

    fn stop(&self, id: &str) {
        self.shared.stop(id);
    }

    fn subscribe(&self) -> async_channel::Receiver<RunEvent> {
        let (tx, rx) = async_channel::unbounded();
        self.shared.lock().subscribers.push(tx);
        rx
    }

    fn save_configuration(&self, config: RunConfiguration) -> Result<()> {
        validate(&config)?;
        let original_id = config.id.clone();
        let mut config = config;
        if !config.temporary {
            config.id = sanitize_id(&config.id);
            if config.id.is_empty() {
                bail!("{original_id:?} is not a usable run configuration id");
            }
        }
        let clash = self.shared.configurations().into_iter().any(|c| {
            c.id != config.id
                && c.id != original_id
                && c.name == config.name
                && !c.id.starts_with("default:")
        });
        if clash {
            bail!("a run configuration named {} already exists", config.name);
        }
        if config.temporary {
            self.shared.add_temporary(config);
            return Ok(());
        }
        self.shared.store.save(&config)?;
        let mut state = self.shared.lock();
        state
            .temporaries
            .retain(|c| c.id != original_id && c.id != config.id);
        self.shared
            .broadcast_locked(&mut state, RunEvent::ConfigurationsChanged);
        Ok(())
    }

    fn delete_configuration(&self, id: &str) {
        self.shared.stop(id);
        let removed_temporary = {
            let mut state = self.shared.lock();
            let before = state.temporaries.len();
            state.temporaries.retain(|c| c.id != id);
            state.tests.remove(id);
            before != state.temporaries.len()
        };
        let removed_file = match self.shared.store.delete(id) {
            Ok(removed) => removed,
            Err(e) => {
                tracing::warn!("cannot delete run configuration {id}: {e:#}");
                false
            }
        };
        if removed_temporary || removed_file {
            self.shared.broadcast(RunEvent::ConfigurationsChanged);
        }
    }

    fn test_results(&self) -> Vec<TestNode> {
        let state = self.shared.lock();
        state
            .last_test_run
            .as_ref()
            .and_then(|id| state.tests.get(id))
            .map(|r| r.tree.clone())
            .unwrap_or_default()
    }

    fn test_results_for(&self, id: &str) -> Vec<TestNode> {
        self.shared
            .lock()
            .tests
            .get(id)
            .map(|r| r.tree.clone())
            .unwrap_or_default()
    }

    fn services(&self) -> Vec<ServiceNode> {
        let runs = self
            .configurations()
            .into_iter()
            .map(|c| {
                let state = match self.state(&c.id) {
                    RunState::Running { .. } => ServiceState::Running,
                    RunState::Finished { exit_code } if exit_code != 0 && exit_code != 130 => {
                        ServiceState::Failed
                    }
                    _ => ServiceState::Stopped,
                };
                let detail = if c.is_compound() {
                    c.compound.join(", ")
                } else {
                    c.command_line()
                };
                ServiceNode {
                    name: c.name,
                    kind: ServiceKind::RunConfiguration,
                    state,
                    detail: Some(detail),
                    children: Vec::new(),
                }
            })
            .collect();
        vec![ServiceNode {
            name: "Run Configurations".into(),
            kind: ServiceKind::Group,
            state: ServiceState::None,
            detail: None,
            children: runs,
        }]
    }

    fn templates(&self) -> Vec<RunConfiguration> {
        self.shared.derived().1.clone()
    }

    fn run_runnable(&self, path: &Path, runnable: &Runnable, debug: bool) -> Result<String> {
        let config = runnables::configuration(path, runnable, &self.shared.root)?;
        let id = config.id.clone();
        self.shared.add_temporary(config);
        self.shared.start(&id, debug)?;
        Ok(id)
    }

    fn rerun_failed(&self, id: &str) -> Result<String> {
        let (config, line) = {
            let state = self.shared.lock();
            let record = state
                .tests
                .get(id)
                .ok_or_else(|| anyhow!("{id} has no test results"))?;
            let failed: Vec<&TestCase> = record
                .cases
                .iter()
                .filter(|c| c.status == TestStatus::Failed)
                .collect();
            if failed.is_empty() {
                bail!("no failed tests to run again");
            }
            let line = rerun_command(
                &record.detected,
                &record.config.command_line(),
                &failed,
                &record.cwd,
            )
            .ok_or_else(|| {
                anyhow!(
                    "the failed tests of {} cannot be selected",
                    record.config.name
                )
            })?;
            (record.config.clone(), line)
        };
        let base_id = config.id.trim_start_matches("failed:").to_string();
        let base_name = config.name.trim_end_matches(" (failed)").to_string();
        let rerun = RunConfiguration {
            id: format!("failed:{base_id}"),
            name: format!("{base_name} (failed)"),
            command: line,
            arguments: String::new(),
            temporary: true,
            ..config
        };
        let new_id = rerun.id.clone();
        if matches!(self.state(&new_id), RunState::Running { .. }) {
            bail!("{} is already running", rerun.name);
        }
        self.shared.add_temporary(rerun);
        self.shared.start(&new_id, false)?;
        Ok(new_id)
    }

    fn build_projects(&self) -> BoxFuture<'static, Result<Vec<BuildProject>>> {
        let roots = self.shared.options.roots.clone();
        let (tx, rx) = async_channel::bounded(1);
        let spawned = thread::Builder::new()
            .name("build-tools".into())
            .spawn(move || {
                let _ = tx.send_blocking(build_tools::discover(&roots, BUILD_DEPTH));
            });
        Box::pin(async move {
            spawned?;
            rx.recv()
                .await
                .map_err(|_| anyhow!("build tool discovery stopped"))
        })
    }

    fn run_task(&self, task: &BuildTask) -> Result<String> {
        let tool = crate::command::program_stem(task.command.split(' ').next().unwrap_or_default());
        let config = build_tools::task_configuration(task, &tool, &self.shared.root);
        let id = config.id.clone();
        self.shared.add_temporary(config);
        self.shared.start(&id, false)?;
        Ok(id)
    }

    fn working_directory(&self, id: &str) -> Option<PathBuf> {
        if let Some(dir) = self.shared.lock().working_dirs.get(id) {
            return Some(dir.clone());
        }
        self.shared
            .find(id)
            .map(|c| exec::working_directory(&c, &self.shared.root))
    }
}

impl Drop for TaskRunService {
    fn drop(&mut self) {
        // Leave no orphaned processes behind.
        let ids: Vec<String> = self
            .shared
            .lock()
            .runs
            .iter()
            .filter(|(_, s)| matches!(s, RunState::Running { .. }))
            .map(|(id, _)| id.clone())
            .collect();
        for id in ids {
            self.shared.stop(&id);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn service(root: &Path) -> TaskRunService {
        TaskRunService::new(RunOptions {
            roots: vec![root.to_path_buf()],
            ..Default::default()
        })
        .unwrap()
    }

    fn config(id: &str, name: &str, command: &str) -> RunConfiguration {
        RunConfiguration {
            id: id.into(),
            name: name.into(),
            command: command.into(),
            ..Default::default()
        }
    }

    #[test]
    fn needs_a_root() {
        assert!(TaskRunService::new(RunOptions::default()).is_err());
    }

    #[test]
    fn saves_validates_and_deletes() {
        let dir = tempfile::tempdir().unwrap();
        let run = service(dir.path());
        let events = run.subscribe();
        assert!(
            run.save_configuration(config("", "x", "cargo run"))
                .is_err()
        );
        assert!(
            run.save_configuration(config("a", "", "cargo run"))
                .is_err()
        );
        assert!(run.save_configuration(config("a", "x", "")).is_err());
        let mut compound = config("all", "All", "");
        compound.compound = vec!["a".into()];
        run.save_configuration(compound).unwrap();
        run.save_configuration(config("my app/dev", "App", "cargo run"))
            .unwrap();
        assert!(dir.path().join(".ferroide/run/my-app-dev.toml").is_file());
        assert!(
            run.save_configuration(config("other", "App", "cargo run"))
                .is_err()
        );
        // Replacing by id keeps the name.
        run.save_configuration(config("my-app-dev", "App", "cargo run --release"))
            .unwrap();
        let ids: Vec<_> = run.configurations().into_iter().map(|c| c.id).collect();
        assert_eq!(ids, ["all", "my-app-dev"]);
        assert!(matches!(
            events.try_recv(),
            Ok(RunEvent::ConfigurationsChanged)
        ));

        // A temporary one, then saved for good.
        let mut temp = config("temp:x", "Run x", "cargo run --bin x");
        temp.temporary = true;
        run.save_configuration(temp.clone()).unwrap();
        assert!(
            run.configurations()
                .iter()
                .any(|c| c.id == "temp:x" && c.temporary)
        );
        temp.temporary = false;
        run.save_configuration(temp).unwrap();
        let configs = run.configurations();
        assert!(configs.iter().any(|c| c.id == "temp-x" && !c.temporary));
        assert!(!configs.iter().any(|c| c.id == "temp:x"));

        run.delete_configuration("temp-x");
        assert!(!dir.path().join(".ferroide/run/temp-x.toml").exists());
        assert_eq!(run.configurations().len(), 2);
    }

    #[test]
    fn keeps_the_most_recent_temporaries() {
        let dir = tempfile::tempdir().unwrap();
        let run = TaskRunService::new(RunOptions {
            roots: vec![dir.path().to_path_buf()],
            max_temporary: 2,
            env: Vec::new(),
        })
        .unwrap();
        for i in 0..4 {
            let mut c = config(&format!("t{i}"), &format!("T{i}"), "x");
            c.temporary = true;
            run.save_configuration(c).unwrap();
        }
        let ids: Vec<_> = run.configurations().into_iter().map(|c| c.id).collect();
        assert_eq!(ids, ["t3", "t2"]);
    }

    #[test]
    fn lists_defaults_and_templates_until_something_is_saved() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("Cargo.toml"),
            "[package]\nname = \"demo\"\n",
        )
        .unwrap();
        std::fs::create_dir_all(dir.path().join("src")).unwrap();
        std::fs::write(dir.path().join("src/main.rs"), "fn main() {}").unwrap();
        let run = service(dir.path());
        let names: Vec<_> = run.configurations().into_iter().map(|c| c.name).collect();
        assert_eq!(names, ["test (demo)", "run (demo)"]);
        assert_eq!(run.templates()[0].command, "cargo run");
        run.save_configuration(config("app", "App", "cargo run"))
            .unwrap();
        assert_eq!(run.configurations().len(), 1);
        let services = run.services();
        assert_eq!(services.len(), 1);
        assert_eq!(services[0].children[0].name, "App");
        assert_eq!(services[0].children[0].state, ServiceState::Stopped);
        assert_eq!(run.working_directory("app").as_deref(), Some(dir.path()));
        assert!(run.start("nope", false).is_err());
    }
}
