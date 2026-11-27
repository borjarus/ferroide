//! Integration tests running real processes on this machine: scripts for
//! output, exit codes, stopping, before-launch and compounds, and a real
//! `cargo test` on a tiny crate for the test tree and rerunning failed
//! tests. Waits poll the event stream with a deadline.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use ferro_services::{RunConfiguration, RunEvent, RunService, RunState, TestNode, TestStatus};
use ferro_tasks::{RunOptions, TaskRunService};

const TIMEOUT: Duration = Duration::from_secs(60);

fn service(root: &Path, env: Vec<(String, String)>) -> TaskRunService {
    TaskRunService::new(RunOptions {
        roots: vec![root.to_path_buf()],
        max_temporary: 5,
        env,
    })
    .unwrap()
}

/// Write an executable script `name` and return the command running it.
fn script(dir: &Path, name: &str, windows: &str, unix: &str) -> String {
    if cfg!(windows) {
        std::fs::write(
            dir.join(format!("{name}.cmd")),
            windows.replace('\n', "\r\n"),
        )
        .unwrap();
    } else {
        let path = dir.join(name);
        std::fs::write(&path, format!("#!/bin/sh\n{unix}")).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
    }
    format!("./{name}")
}

fn config(id: &str, command: &str) -> RunConfiguration {
    RunConfiguration {
        id: id.into(),
        name: id.into(),
        kind: "shell".into(),
        command: command.into(),
        ..Default::default()
    }
}

/// Events, collected while waiting.
struct Events {
    rx: async_channel::Receiver<RunEvent>,
    seen: Vec<RunEvent>,
}

impl Events {
    fn new(run: &TaskRunService) -> Self {
        Self {
            rx: run.subscribe(),
            seen: Vec::new(),
        }
    }

    /// Wait until `id` finished; its exit code.
    fn finished(&mut self, id: &str) -> i32 {
        let deadline = Instant::now() + TIMEOUT;
        loop {
            while let Ok(event) = self.rx.try_recv() {
                self.seen.push(event);
            }
            let code = self.seen.iter().find_map(|e| match e {
                RunEvent::StateChanged {
                    id: i,
                    state: RunState::Finished { exit_code },
                } if i == id => Some(*exit_code),
                _ => None,
            });
            if let Some(code) = code {
                return code;
            }
            assert!(
                Instant::now() < deadline,
                "{id} did not finish: {:#?}",
                self.seen
            );
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    fn wait_output(&mut self, id: &str, text: &str) {
        let deadline = Instant::now() + TIMEOUT;
        while !self.output(id).iter().any(|l| l.contains(text)) {
            while let Ok(event) = self.rx.try_recv() {
                self.seen.push(event);
            }
            assert!(
                Instant::now() < deadline,
                "no {text:?} from {id}: {:#?}",
                self.seen
            );
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    fn output(&self, id: &str) -> Vec<String> {
        self.seen
            .iter()
            .filter_map(|e| match e {
                RunEvent::Output { id: i, line, .. } if i == id => Some(line.clone()),
                _ => None,
            })
            .collect()
    }

    fn errors(&self, id: &str) -> Vec<String> {
        self.seen
            .iter()
            .filter_map(|e| match e {
                RunEvent::Output {
                    id: i,
                    line,
                    error: true,
                } if i == id => Some(line.clone()),
                _ => None,
            })
            .collect()
    }
}

#[test]
fn runs_a_process_and_reports_its_output_and_exit_code() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let command = script(
        root,
        "hello",
        "@echo off\necho hello %GREETING%\necho oops 1>&2\nexit /b 3\n",
        "echo hello $GREETING\necho oops 1>&2\nexit 3\n",
    );
    std::fs::write(root.join(".env"), "GREETING=from-env\n").unwrap();
    let run = service(root, Vec::new());
    let mut c = config("hello", &command);
    c.env_file = ".env".into();
    run.save_configuration(c).unwrap();
    let mut events = Events::new(&run);
    run.start("hello", false).unwrap();
    assert!(matches!(
        run.state("hello"),
        RunState::Running { debug: false }
    ));
    assert!(run.start("hello", false).is_err(), "already running");
    assert_eq!(events.finished("hello"), 3);
    let output = events.output("hello");
    assert!(output[0].starts_with("\x1b[2m"), "{output:?}");
    assert!(output[0].contains("hello"), "{output:?}");
    assert!(output.iter().any(|l| l == "hello from-env"), "{output:?}");
    assert!(events.errors("hello").iter().any(|l| l.trim() == "oops"));
    assert_eq!(run.state("hello"), RunState::Finished { exit_code: 3 });
    assert_eq!(run.working_directory("hello").as_deref(), Some(root));
    let services = run.services();
    assert_eq!(
        services[0].children[0].state,
        ferro_services::ServiceState::Failed
    );
}

#[test]
fn a_missing_program_is_an_error_line_and_exit_minus_one() {
    let dir = tempfile::tempdir().unwrap();
    let run = service(dir.path(), Vec::new());
    run.save_configuration(config("missing", "definitely-not-a-program-ferroide --x"))
        .unwrap();
    let mut events = Events::new(&run);
    run.start("missing", false).unwrap();
    assert_eq!(events.finished("missing"), -1);
    assert!(
        events
            .errors("missing")
            .iter()
            .any(|l| l.contains("cannot find program"))
    );
}

/// Size of `path`, 0 when missing.
fn size(path: &Path) -> u64 {
    std::fs::metadata(path).map(|m| m.len()).unwrap_or(0)
}

#[test]
fn stop_kills_the_whole_process_tree() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let beat = root.join("beat.txt");
    // The child, not the started script, keeps writing heartbeats.
    script(
        root,
        "child",
        "@echo off\ncd /d \"%~dp0\"\n:loop\necho x>>beat.txt\nping -n 2 127.0.0.1 >nul\ngoto loop\n",
        "while true; do echo x >> beat.txt; sleep 0.3; done\n",
    );
    let outer = script(
        root,
        "outer",
        // `/d`: no AutoRun commands (which may change the folder).
        "@echo off\necho started\ncmd /d /c \"%~dp0child.cmd\"\n",
        "echo started\n./child &\nwait\n",
    );
    let run = service(root, Vec::new());
    run.save_configuration(config("tree", &outer)).unwrap();
    let mut events = Events::new(&run);
    run.start("tree", false).unwrap();
    events.wait_output("tree", "started");
    let deadline = Instant::now() + TIMEOUT;
    while size(&beat) == 0 {
        assert!(Instant::now() < deadline, "the child never ran");
        std::thread::sleep(Duration::from_millis(50));
    }
    run.stop("tree");
    assert_eq!(events.finished("tree"), 130);
    assert_eq!(run.state("tree"), RunState::Finished { exit_code: 130 });
    // The heartbeat stops: unchanged for well over one beat interval.
    let deadline = Instant::now() + Duration::from_secs(20);
    let mut last = size(&beat);
    let mut stable_since = Instant::now();
    while stable_since.elapsed() < Duration::from_millis(3500) {
        assert!(Instant::now() < deadline, "the child is still running");
        std::thread::sleep(Duration::from_millis(100));
        let now = size(&beat);
        if now != last {
            last = now;
            stable_since = Instant::now();
        }
    }
    // A stopped run can start again at once.
    run.start("tree", false).unwrap();
    run.stop("tree");
}

#[test]
fn a_failing_before_launch_step_abandons_the_launch() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let fail = script(root, "fail", "@exit /b 4\n", "exit 4\n");
    let ok = script(root, "ok", "@echo built\n", "echo built\n");
    let main = script(
        root,
        "main",
        "@echo ran>ran.txt\n@echo main ran\n",
        "echo ran > ran.txt\necho main ran\n",
    );
    let run = service(root, Vec::new());
    run.save_configuration(config("fail", &fail)).unwrap();
    run.save_configuration(config("build", &ok)).unwrap();
    let mut m = config("main", &main);
    m.before_launch = vec!["build".into(), "fail".into()];
    run.save_configuration(m.clone()).unwrap();
    let mut events = Events::new(&run);
    run.start("main", false).unwrap();
    assert_eq!(events.finished("main"), 4);
    assert!(!root.join("ran.txt").exists());
    let output = events.output("main");
    assert!(output.iter().any(|l| l == "built"), "{output:?}");
    assert!(
        events
            .errors("main")
            .iter()
            .any(|l| l.contains("fail failed with exit code 4; launch abandoned")),
        "{output:?}"
    );

    m.before_launch = vec!["build".into()];
    run.save_configuration(m).unwrap();
    let mut events = Events::new(&run);
    run.start("main", false).unwrap();
    assert_eq!(events.finished("main"), 0);
    assert!(root.join("ran.txt").exists());
    assert!(events.output("main").iter().any(|l| l == "main ran"));
}

#[test]
fn a_compound_runs_its_members_and_reports_the_first_failure() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let a = script(root, "a", "@echo a\n", "echo a\n");
    let b = script(root, "b", "@echo b\n@exit /b 2\n", "echo b\nexit 2\n");
    let run = service(root, Vec::new());
    run.save_configuration(config("a", &a)).unwrap();
    run.save_configuration(config("b", &b)).unwrap();
    let mut both = config("both", "");
    both.compound = vec!["a".into(), "b".into()];
    run.save_configuration(both).unwrap();
    let mut events = Events::new(&run);
    run.start("both", true).unwrap();
    assert!(matches!(
        run.state("both"),
        RunState::Running { debug: true }
    ));
    assert_eq!(events.finished("both"), 2);
    assert_eq!(run.state("a"), RunState::Finished { exit_code: 0 });
    assert_eq!(run.state("b"), RunState::Finished { exit_code: 2 });
    assert!(events.output("a").iter().any(|l| l == "a"));
}

fn find<'a>(nodes: &'a [TestNode], name: &str) -> Option<&'a TestNode> {
    nodes.iter().find_map(|n| {
        if n.name == name {
            Some(n)
        } else {
            find(&n.children, name)
        }
    })
}

fn leaves(nodes: &[TestNode]) -> Vec<String> {
    nodes
        .iter()
        .flat_map(|n| {
            if n.children.is_empty() {
                vec![n.name.clone()]
            } else {
                leaves(&n.children)
            }
        })
        .collect()
}

/// A real `cargo test` (slow-ish: compiles a tiny crate).
#[test]
fn cargo_test_builds_a_tree_and_reruns_the_failed_test() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("demo");
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(
        root.join("Cargo.toml"),
        "[package]\nname = \"demo\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[workspace]\n",
    )
    .unwrap();
    std::fs::write(
        root.join("src/lib.rs"),
        "pub fn add(a: i32, b: i32) -> i32 {\n    a + b\n}\n\n#[cfg(test)]\nmod tests {\n    use super::*;\n\n    #[test]\n    fn passes() {\n        assert_eq!(add(1, 2), 3);\n    }\n\n    #[test]\n    fn fails() {\n        assert_eq!(add(1, 1), 3, \"math is hard\");\n    }\n}\n",
    )
    .unwrap();
    let target: PathBuf = dir.path().join("target");
    let run = service(
        &root,
        vec![(
            "CARGO_TARGET_DIR".into(),
            target.to_string_lossy().into_owned(),
        )],
    );
    run.save_configuration(config("tests", "cargo test"))
        .unwrap();
    let mut events = Events::new(&run);
    run.start("tests", false).unwrap();
    assert_eq!(events.finished("tests"), 101);
    assert!(
        events
            .seen
            .iter()
            .any(|e| matches!(e, RunEvent::Tests { id } if id == "tests"))
    );

    let tree = run.test_results_for("tests");
    assert_eq!(run.test_results(), tree);
    let demo = find(&tree, "demo").expect("a suite for the crate");
    assert_eq!(demo.status, TestStatus::Failed);
    let passes = find(&tree, "passes").unwrap();
    assert_eq!(passes.status, TestStatus::Passed);
    assert_eq!(passes.location, Some((root.join("src").join("lib.rs"), 9)));
    let fails = find(&tree, "fails").unwrap();
    assert_eq!(fails.status, TestStatus::Failed);
    assert_eq!(fails.location, Some((root.join("src").join("lib.rs"), 14)));
    assert!(
        fails.output.iter().any(|l| l.contains("math is hard")),
        "{:?}",
        fails.output
    );

    let rerun = run.rerun_failed("tests").unwrap();
    assert_eq!(rerun, "failed:tests");
    let config = run
        .configurations()
        .into_iter()
        .find(|c| c.id == rerun)
        .unwrap();
    assert_eq!(config.name, "tests (failed)");
    assert!(config.temporary);
    assert_eq!(config.command, "cargo test -- --exact tests::fails");
    assert_eq!(events.finished(&rerun), 101);
    let tree = run.test_results_for(&rerun);
    assert_eq!(leaves(&tree), ["fails"]);
    assert_eq!(run.test_results(), tree);
    // The original run's tree is kept.
    assert!(find(&run.test_results_for("tests"), "passes").is_some());

    // A ▶ icon on one test.
    let runnable = ferro_services::Runnable {
        line: 9,
        kind: ferro_services::RunnableKind::Test,
        label: "passes".into(),
    };
    let id = run
        .run_runnable(&root.join("src").join("lib.rs"), &runnable, false)
        .unwrap();
    assert_eq!(events.finished(&id), 0);
    let config = run
        .configurations()
        .into_iter()
        .find(|c| c.id == id)
        .unwrap();
    assert_eq!(config.name, "test passes");
    assert_eq!(config.command, "cargo test --lib -- tests::passes --exact");
    assert_eq!(leaves(&run.test_results_for(&id)), ["passes"]);

    // The build tool window.
    let projects = smol::block_on(run.build_projects()).unwrap();
    assert_eq!(projects.len(), 1);
    assert_eq!(projects[0].name, "demo");
    assert!(
        projects[0]
            .tasks
            .iter()
            .any(|t| t.command == "cargo test" && t.is_test)
    );
}
