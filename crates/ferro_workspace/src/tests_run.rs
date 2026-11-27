//! Headless tests for Stage 8 in the window, on mock services: the Run tool
//! window (tabs per run, console, filter, test tree), ▶ runnables and Run
//! at Caret, the Build tool window, and terminal input.

use std::time::{Duration, Instant};

use ferro_services::{
    BuildTask, RunState, Runnable, RunnableKind, Services, TerminalSnapshot, line_text,
    mock::MockConfig,
};
use ferro_settings::{Paths, Settings};
use gpui_kit::{
    AnyWindowHandle, AppContext as _, Bounds, Entity, Point, Render, TestAppContext, WindowBounds,
    WindowOptions, px, size,
};

use crate::{
    AppState,
    panels::{
        build_tools::{BuildRow, BuildToolsView},
        run::{RunTab, RunView},
        terminal::{CellPos, TerminalSelection, TerminalView},
    },
    requests::{RunBuildTask, RunRunnable},
    tool_window::ToolWindowKind,
    workspace::Workspace,
};

fn setup(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    cx.update(|cx| {
        gpui_kit::init(cx);
        ferro_ui::init(cx);
        let root = std::env::temp_dir().join(format!("ferro-run-test-{}", std::process::id()));
        let settings = Settings {
            locale: ferro_i18n::Locale::En,
            ..Settings::default()
        };
        AppState::init(settings, Paths::under(&root), false, cx);
        crate::init(cx);
    });
}

fn options() -> WindowOptions {
    WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(Bounds {
            origin: Point::default(),
            size: size(px(1400.), px(900.)),
        })),
        ..Default::default()
    }
}

fn open_view<V: Render + 'static>(
    cx: &mut TestAppContext,
    build: impl FnOnce(Services, &mut gpui_kit::Window, &mut gpui_kit::Context<V>) -> V + 'static,
) -> (AnyWindowHandle, Entity<V>, Services) {
    setup(cx);
    let services = Services::mock(MockConfig::instant());
    let for_view = services.clone();
    let (window, view) = cx.update(|cx| {
        gpui_kit::open_window(options(), cx, move |window, cx| {
            cx.new(|cx| build(for_view, window, cx))
        })
        .expect("open view window")
    });
    cx.run_until_parked();
    (window, view, services)
}

fn open_workspace(cx: &mut TestAppContext) -> (AnyWindowHandle, Entity<Workspace>) {
    setup(cx);
    let services = Services::mock(MockConfig::instant());
    let opened = cx.update(|cx| {
        gpui_kit::open_window(options(), cx, move |window, cx| {
            Workspace::build(services, window, cx)
        })
        .expect("open workspace window")
    });
    cx.run_until_parked();
    opened
}

fn with<V: 'static, R>(
    cx: &mut TestAppContext,
    window: AnyWindowHandle,
    view: &Entity<V>,
    f: impl FnOnce(&mut V, &mut gpui_kit::Window, &mut gpui_kit::Context<V>) -> R,
) -> R {
    let result = cx
        .update_window(window, |_, window, cx| {
            view.update(cx, |v, cx| f(v, window, cx))
        })
        .expect("window is open");
    cx.run_until_parked();
    result
}

/// Mock runs print on a real thread; wait for them.
fn wait_until(cx: &mut TestAppContext, what: &str, done: impl FnMut(&mut TestAppContext) -> bool) {
    wait_for(cx, what, Duration::from_secs(10), done)
}

fn wait_for(
    cx: &mut TestAppContext,
    what: &str,
    limit: Duration,
    mut done: impl FnMut(&mut TestAppContext) -> bool,
) {
    let started = Instant::now();
    loop {
        cx.run_until_parked();
        if done(cx) {
            return;
        }
        assert!(started.elapsed() < limit, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[gpui_kit::test]
fn a_run_gets_a_tab_with_its_console_and_a_filter(cx: &mut TestAppContext) {
    let (window, view, services) = open_view(cx, RunView::new);
    // The mock's earlier test results are a tab of their own.
    assert_eq!(cx.read(|cx| view.read(cx).runs().len()), 1);

    services.run.start("app", false).unwrap();
    wait_until(cx, "the run to finish", |cx| {
        view.read_with(cx, |v, _| {
            v.runs()
                .iter()
                .any(|r| matches!(r.state, RunState::Finished { exit_code: 0 }))
        })
    });
    let (tab, name, lines) = cx.read(|cx| {
        let v = view.read(cx);
        let run = v.active_run().unwrap();
        (
            v.tab(),
            run.name.clone(),
            run.lines.iter().map(|l| l.text.clone()).collect::<Vec<_>>(),
        )
    });
    assert_eq!(tab, RunTab::Console);
    assert_eq!(name, "demo-app");
    assert!(lines.iter().any(|l| l == "Hello, FerroIDE"), "{lines:?}");
    assert!(lines.last().unwrap().contains("exit code 0"));

    with(cx, window, &view, |v, w, cx| {
        v.set_filter("compiling", w, cx)
    });
    let shown = cx.read(|cx| view.read(cx).visible_lines().len());
    assert_eq!(shown, 1, "the filter ignores case");
    with(cx, window, &view, |v, w, cx| v.set_filter("", w, cx));
    assert_eq!(
        cx.read(|cx| view.read(cx).visible_lines().len()),
        lines.len()
    );

    // Running it again reuses the tab and starts a fresh console.
    services.run.start("app", false).unwrap();
    wait_until(cx, "the second run to start", |cx| {
        view.read_with(cx, |v, _| {
            matches!(v.active_run().unwrap().state, RunState::Running { .. })
        })
    });
    assert_eq!(cx.read(|cx| view.read(cx).runs().len()), 2);
    services.run.stop("app");
    wait_until(cx, "the stop", |cx| {
        view.read_with(cx, |v, _| {
            v.active_run().unwrap().state == RunState::Finished { exit_code: 130 }
        })
    });
    with(cx, window, &view, |v, _, cx| v.close_run(1, cx));
    assert_eq!(cx.read(|cx| view.read(cx).runs().len()), 1);
}

#[gpui_kit::test]
fn a_runnable_runs_as_a_temporary_configuration_and_is_selected(cx: &mut TestAppContext) {
    let (window, ws) = open_workspace(cx);
    let root = cx.read(|cx| ws.read(cx).services.project.roots()[0].clone());
    let request = RunRunnable {
        path: root.join("src/lib.rs"),
        runnable: Runnable {
            line: 21,
            kind: RunnableKind::Test,
            label: "greets_by_name".into(),
        },
        debug: false,
    };
    with(cx, window, &ws, |ws, w, cx| {
        ws.run_runnable(&request, w, cx)
    });
    let (selected, temporary) = cx.read(|cx| {
        let ws = ws.read(cx);
        let config = &ws.run_configs[ws.selected_run];
        (config.name.clone(), config.temporary)
    });
    assert_eq!(selected, "greets_by_name");
    assert!(temporary);
    assert!(cx.read(|cx| ws.read(cx).is_tool_window_visible(ToolWindowKind::Run, cx)));
    let run = cx
        .update_window(window, |_, w, cx| {
            ws.update(cx, |ws, cx| {
                ws.tool_window_body::<RunView>(ToolWindowKind::Run, w, cx)
            })
        })
        .unwrap()
        .expect("run tool window");
    wait_until(cx, "the runnable's output", |cx| {
        run.read_with(cx, |v, _| {
            v.runs()
                .iter()
                .any(|r| r.name == "greets_by_name" && !r.lines.is_empty())
        })
    });
}

#[gpui_kit::test]
fn build_tasks_are_listed_and_run(cx: &mut TestAppContext) {
    let (window, ws) = open_workspace(cx);
    let build = cx
        .update_window(window, |_, w, cx| {
            ws.update(cx, |ws, cx| {
                ws.show_tool_window(ToolWindowKind::Build, w, cx);
                ws.tool_window_body::<BuildToolsView>(ToolWindowKind::Build, w, cx)
            })
        })
        .unwrap()
        .expect("build tool window");
    cx.run_until_parked();
    let (projects, rows) = cx.read(|cx| {
        let v = build.read(cx);
        assert!(v.is_loaded());
        (v.projects().len(), v.rows().len())
    });
    assert_eq!(projects, 2);
    assert_eq!(rows, 2 + 5 + 3);
    with(cx, window, &build, |v, _, cx| v.set_all_collapsed(true, cx));
    assert_eq!(cx.read(|cx| build.read(cx).rows().len()), 2);
    with(cx, window, &build, |v, _, cx| {
        v.set_all_collapsed(false, cx);
        v.select(1, cx);
    });
    assert_eq!(cx.read(|cx| build.read(cx).rows()[1]), BuildRow::Task(0, 0));
    let task: BuildTask = cx.read(|cx| build.read(cx).selected_task().unwrap().clone());
    assert_eq!(task.command, "cargo build");

    let request = RunBuildTask {
        task,
        in_terminal: false,
    };
    with(cx, window, &ws, |ws, w, cx| {
        ws.run_build_task(&request, w, cx)
    });
    let selected = cx.read(|cx| {
        let ws = ws.read(cx);
        ws.run_configs[ws.selected_run].command.clone()
    });
    assert_eq!(selected, "cargo build");
}

#[gpui_kit::test]
fn run_at_caret_falls_back_to_the_selected_configuration(cx: &mut TestAppContext) {
    let (window, ws) = open_workspace(cx);
    // No editor is open: Run at Caret runs the selected configuration.
    with(cx, window, &ws, |ws, w, cx| ws.run_at_caret(false, w, cx));
    let state = cx.read(|cx| ws.read(cx).run_state);
    assert!(matches!(state, RunState::Running { .. }), "{state:?}");
}

fn text_of(snapshot: &TerminalSnapshot) -> Vec<String> {
    snapshot.lines.iter().map(|l| line_text(l)).collect()
}

#[gpui_kit::test]
fn terminal_input_reaches_the_session_and_selection_copies(cx: &mut TestAppContext) {
    let (window, view, _) = open_view(cx, |s, _, cx| TerminalView::new(s, cx));
    with(cx, window, &view, |v, _, cx| v.new_session(cx));
    with(cx, window, &view, |v, _, cx| v.send(b"echo hi", cx));
    let lines = cx.read(|cx| text_of(view.read(cx).snapshot().unwrap()));
    assert!(lines.last().unwrap().ends_with("echo hi"), "{lines:?}");

    with(cx, window, &view, |v, _, cx| v.paste("ls\nexit", cx));
    let lines = cx.read(|cx| text_of(view.read(cx).snapshot().unwrap()));
    let n = lines.len();
    assert!(lines[n - 2].ends_with("echo hils"), "{lines:?}");
    assert!(lines[n - 1].ends_with("exit"));

    // Select "exit" on the last line.
    let last = n as i64 - 1;
    let col = lines[n - 1].chars().count() as u16 - 4;
    with(cx, window, &view, |v, _, cx| {
        v.set_selection(
            Some(TerminalSelection {
                anchor: CellPos {
                    line: last,
                    col: col + 3,
                },
                head: CellPos { line: last, col },
            }),
            cx,
        )
    });
    assert_eq!(
        cx.read(|cx| view.read(cx).selected_text()),
        Some("exit".to_string())
    );
    assert!(with(cx, window, &view, |v, _, cx| v.copy(cx)));
    assert_eq!(
        cx.read(|cx| cx.read_from_clipboard().and_then(|c| c.text())),
        Some("exit".to_string())
    );
}

#[gpui_kit::test]
fn terminal_view_drives_a_real_shell(cx: &mut TestAppContext) {
    let root = std::env::temp_dir().join(format!("ferro-pty-view-{}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    let options = ferro_terminal::TerminalOptions {
        cwd: root.clone(),
        integration_dir: root.join("shell"),
        ..Default::default()
    };
    let (window, view, _) = open_view(cx, move |mut s, _, cx| {
        s.terminal = std::sync::Arc::new(ferro_terminal::PtyTerminalService::new(options));
        TerminalView::new(s, cx)
    });
    with(cx, window, &view, |v, _, cx| v.new_session(cx));
    with(cx, window, &view, |v, _, cx| {
        v.send(b"echo ferro-pty-ok\r", cx)
    });
    wait_until(cx, "the shell's echo", |cx| {
        view.read_with(cx, |v, _| {
            v.snapshot().is_some_and(|s| {
                text_of(s)
                    .iter()
                    .filter(|l| l.contains("ferro-pty-ok"))
                    .count()
                    >= 2
            })
        })
    });
    let id = cx.read(|cx| view.read(cx).active_session().unwrap().id);
    with(cx, window, &view, |v, _, cx| v.close_session(id, cx));
    assert!(cx.read(|cx| view.read(cx).session_titles().is_empty()));
}

/// A real Cargo project: ▶ on a test runs `cargo test` through
/// `ferro_tasks`, and the Run tool window shows the test tree.
#[gpui_kit::test]
fn a_test_runnable_runs_cargo_and_fills_the_test_tree(cx: &mut TestAppContext) {
    if std::process::Command::new("cargo")
        .arg("--version")
        .output()
        .is_err()
    {
        return;
    }
    setup(cx);
    let dir = tempfile::tempdir().unwrap();
    let root = std::path::PathBuf::from(
        dir.path()
            .canonicalize()
            .unwrap()
            .to_string_lossy()
            .trim_start_matches(r"\\?\"),
    );
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(
        root.join("Cargo.toml"),
        "[package]\nname = \"demo\"\nedition = \"2021\"\n",
    )
    .unwrap();
    std::fs::write(
        root.join("src/lib.rs"),
        "#[cfg(test)]\nmod tests {\n    #[test]\n    fn adds() {\n        assert_eq!(1 + 1, 2);\n    }\n}\n",
    )
    .unwrap();
    let (window, ws) = cx.update(|cx| {
        let (services, _) = crate::projects::project_services(
            vec![root.clone()],
            crate::lsp::options(cx),
            ferro_index::IndexOptions::default(),
        )
        .unwrap();
        crate::open_workspace(services, cx).unwrap()
    });
    cx.run_until_parked();
    let request = RunRunnable {
        path: root.join("src/lib.rs"),
        runnable: Runnable {
            line: 3,
            kind: RunnableKind::Test,
            label: "adds".into(),
        },
        debug: false,
    };
    with(cx, window, &ws, |ws, w, cx| {
        ws.run_runnable(&request, w, cx)
    });
    let run = cx
        .update_window(window, |_, w, cx| {
            ws.update(cx, |ws, cx| {
                ws.tool_window_body::<RunView>(ToolWindowKind::Run, w, cx)
            })
        })
        .unwrap()
        .expect("run tool window");
    wait_for(cx, "cargo test", Duration::from_secs(180), |cx| {
        run.read_with(cx, |v, _| {
            v.active_run()
                .is_some_and(|r| matches!(r.state, RunState::Finished { .. }))
        })
    });
    let (state, summary, tab) = cx.read(|cx| {
        let v = run.read(cx);
        (v.active_run().unwrap().state, v.summary(), v.tab())
    });
    assert_eq!(state, RunState::Finished { exit_code: 0 });
    assert_eq!(summary, (1, 0, 0));
    assert_eq!(tab, RunTab::Tests);
}
