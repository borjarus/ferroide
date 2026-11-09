//! Headless tests for the Stage 2 tool windows, on mock services, plus
//! performance checks with data far larger than a typical project.

use std::{path::PathBuf, time::Instant};

use ferro_services::{
    AnsiColor, Diagnostic, EntryKind, Services, Severity, TextPos, TextRange,
    mock::{MockConfig, SyntheticTree},
};
use ferro_settings::{Paths, Settings};
use gpui_kit::{
    AnyWindowHandle, AppContext as _, Bounds, Entity, Focusable as _, Point, Render,
    TestAppContext, WindowBounds, WindowOptions, px, size,
};

use crate::{
    AppState,
    panels::{
        debug::DebugView, notifications::NotificationsView, problems::ProblemsView,
        project::ProjectTree, run::RunView, services::ServicesView, structure::StructureView,
        terminal::TerminalView, todo::TodoView,
    },
    tool_window::ToolWindowKind,
    workspace::Workspace,
};

fn setup(cx: &mut TestAppContext) {
    cx.update(|cx| {
        gpui_kit::init(cx);
        ferro_ui::init(cx);
        let root = std::env::temp_dir().join(format!("ferro-panels-test-{}", std::process::id()));
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

/// Open `build`'s view as the root of its own window.
fn open_view<V: Render + 'static>(
    cx: &mut TestAppContext,
    config: MockConfig,
    build: impl FnOnce(Services, &mut gpui_kit::Window, &mut gpui_kit::Context<V>) -> V + 'static,
) -> (AnyWindowHandle, Entity<V>, Services) {
    setup(cx);
    let services = Services::mock(config);
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

fn root_of(services: &Services) -> PathBuf {
    services.project.roots()[0].clone()
}

// ---------------------------------------------------------------------------
// Project tree
// ---------------------------------------------------------------------------

#[gpui_kit::test]
fn project_tree_keyboard_navigation(cx: &mut TestAppContext) {
    let (window, tree, services) = open_view(cx, MockConfig::instant(), |s, _, cx| {
        ProjectTree::new(s, cx)
    });
    let root = root_of(&services);
    let rows = cx.read(|cx| tree.read(cx).row_count());
    assert!(rows > 5, "root and its children are listed: {rows}");

    with(cx, window, &tree, |t, w, cx| {
        t.handle_key("home", None, w, cx);
        assert_eq!(t.selected(), Some(root.as_path()));
        // Right on an expanded directory moves to its first child.
        t.handle_key("right", None, w, cx);
        assert_eq!(t.selected().and_then(|p| p.parent()), Some(root.as_path()));
        t.handle_key("end", None, w, cx);
        t.handle_key("home", None, w, cx);
        assert_eq!(t.selected(), Some(root.as_path()));
        // Left collapses the expanded root.
        t.handle_key("left", None, w, cx);
        assert_eq!(t.row_count(), 1);
        t.handle_key("right", None, w, cx);
    });
    assert!(cx.read(|cx| tree.read(cx).row_count()) > 5);
}

#[gpui_kit::test]
fn project_tree_speed_search_selects_main_rs(cx: &mut TestAppContext) {
    let (window, tree, services) = open_view(cx, MockConfig::instant(), |s, _, cx| {
        ProjectTree::new(s, cx)
    });
    let root = root_of(&services);
    with(cx, window, &tree, |t, _, cx| {
        t.expand(&root.join("src"), cx)
    });
    with(cx, window, &tree, |t, w, cx| {
        for c in ["m", "a", "i", "n"] {
            assert!(t.handle_key(c, Some(c), w, cx));
        }
        assert_eq!(t.search_query(), Some("main"));
        assert_eq!(
            t.selected(),
            Some(root.join("src").join("main.rs").as_path())
        );
        // Esc ends the search but keeps the selection.
        t.handle_key("escape", None, w, cx);
        assert_eq!(t.search_query(), None);
        assert_eq!(
            t.selected(),
            Some(root.join("src").join("main.rs").as_path())
        );
    });
}

#[gpui_kit::test]
fn project_tree_file_operations_refresh_rows(cx: &mut TestAppContext) {
    let (window, tree, services) = open_view(cx, MockConfig::instant(), |s, _, cx| {
        ProjectTree::new(s, cx)
    });
    let root = root_of(&services);
    let names = |cx: &mut TestAppContext| cx.read(|cx| tree.read(cx).row_names());

    with(cx, window, &tree, |t, _, cx| {
        t.create_directory(root.clone(), "assets", cx)
    });
    assert!(names(cx).contains(&"assets".to_string()));

    with(cx, window, &tree, |t, _, cx| {
        t.rename_entry(root.join("README.md"), "READ.md", cx)
    });
    let after = names(cx);
    assert!(after.contains(&"READ.md".to_string()) && !after.contains(&"README.md".to_string()));
    assert_eq!(
        cx.read(|cx| tree.read(cx).selected().map(PathBuf::from)),
        Some(root.join("READ.md"))
    );

    with(cx, window, &tree, |t, _, cx| {
        t.delete_entry(root.join("READ.md"), cx)
    });
    assert!(!names(cx).contains(&"READ.md".to_string()));

    // A failing operation reports its error under the tree.
    with(cx, window, &tree, |t, _, cx| {
        t.rename_entry(root.join("nope.rs"), "x.rs", cx)
    });
    assert!(cx.read(|cx| tree.read(cx).message().is_some()));
}

#[gpui_kit::test]
fn project_tree_drag_and_drop_moves_files(cx: &mut TestAppContext) {
    let (window, tree, services) = open_view(cx, MockConfig::instant(), |s, _, cx| {
        ProjectTree::new(s, cx)
    });
    let root = root_of(&services);
    assert!(!ProjectTree::can_move(
        &root.join("src"),
        &root.join("src").join("x")
    ));
    assert!(!ProjectTree::can_move(&root.join("Cargo.toml"), &root));

    let moved = with(cx, window, &tree, |t, _, cx| {
        t.move_entry(&root.join("Cargo.toml"), &root.join("docs"), cx)
    });
    assert!(moved);
    let target = root.join("docs").join("Cargo.toml");
    assert_eq!(
        cx.read(|cx| tree.read(cx).selected().map(PathBuf::from)),
        Some(target.clone())
    );
    let names = cx.read(|cx| tree.read(cx).row_names());
    assert_eq!(names.iter().filter(|n| *n == "Cargo.toml").count(), 1);
    assert!(cx.read(|cx| tree.read(cx).is_expanded(&root.join("docs"))));
}

#[gpui_kit::test]
fn project_tree_with_a_million_nodes_stays_fast(cx: &mut TestAppContext) {
    let config = MockConfig {
        stress_files: 1_000_000,
        ..MockConfig::instant()
    };
    let (window, tree, services) =
        open_view(cx, config.clone(), |s, _, cx| ProjectTree::new(s, cx));
    let stress = root_of(&services).join("stress");
    let synthetic = SyntheticTree::new(stress.clone(), config.seed, config.stress_files);

    // List every directory of the synthetic tree once (what browsing would
    // fetch lazily), then expand everything.
    let mut dirs = vec![stress.clone()];
    let mut listed = Vec::new();
    while let Some(dir) = dirs.pop() {
        let entries = synthetic.children(&dir).unwrap();
        dirs.extend(
            entries
                .iter()
                .filter(|e| e.kind == EntryKind::Directory)
                .map(|e| e.path.clone()),
        );
        listed.push((dir, entries));
    }
    let (rows, rebuild) = with(cx, window, &tree, |t, _, _| {
        for (dir, entries) in listed {
            t.preload_children(dir, entries);
        }
        let started = Instant::now();
        t.rebuild_rows();
        (t.row_count(), started.elapsed())
    });
    assert!(rows > 1_000_000, "expected over a million rows, got {rows}");

    // Collapse and re-expand the whole stress subtree (incremental splice).
    let toggle = with(cx, window, &tree, |t, _, cx| {
        let started = Instant::now();
        t.collapse(&stress, cx);
        t.expand(&stress, cx);
        started.elapsed()
    });
    // Render one screenful from the middle of the list.
    let render = with(cx, window, &tree, |t, _, cx| {
        let middle = t.row_count() / 2;
        let started = Instant::now();
        let n = t.render_range(middle..middle + 60, cx);
        assert_eq!(n, 60);
        started.elapsed()
    });
    println!(
        "project tree: {rows} rows, rebuild {rebuild:?}, collapse+expand {toggle:?}, render 60 rows {render:?}"
    );
    // Generous budgets: debug build, shared CI machines.
    assert!(rebuild.as_secs_f64() < 5.0, "rebuild took {rebuild:?}");
    assert!(toggle.as_secs_f64() < 5.0, "toggle took {toggle:?}");
    assert!(render.as_millis() < 250, "render took {render:?}");
}

// ---------------------------------------------------------------------------
// Problems, Terminal, Run, Debug
// ---------------------------------------------------------------------------

#[gpui_kit::test]
fn problems_group_by_file_and_filter_by_severity(cx: &mut TestAppContext) {
    let (window, view, _) = open_view(cx, MockConfig::instant(), |s, _, cx| {
        ProblemsView::new(s, cx)
    });
    let (groups, shown) = cx.read(|cx| (view.read(cx).group_count(), view.read(cx).shown_count()));
    assert!(cx.read(|cx| view.read(cx).is_loaded()));
    assert!(groups >= 2, "diagnostics of several files: {groups}");
    with(cx, window, &view, |v, _, cx| {
        v.toggle_severity(Severity::Warning, cx)
    });
    let filtered = cx.read(|cx| view.read(cx).shown_count());
    assert!(
        filtered < shown,
        "hiding warnings hides some: {filtered} vs {shown}"
    );
    with(cx, window, &view, |v, _, cx| {
        v.toggle_severity(Severity::Warning, cx)
    });
    assert_eq!(cx.read(|cx| view.read(cx).shown_count()), shown);
}

#[gpui_kit::test]
fn problems_view_handles_20k_diagnostics(cx: &mut TestAppContext) {
    let (window, view, services) = open_view(cx, MockConfig::instant(), |s, _, cx| {
        ProblemsView::new(s, cx)
    });
    let root = root_of(&services);
    let diagnostics: Vec<Diagnostic> = (0..20_000u32)
        .map(|i| Diagnostic {
            path: root.join(format!("src/module_{}.rs", i % 500)),
            range: TextRange::on_line(i / 500, 0, 4),
            severity: [Severity::Error, Severity::Warning, Severity::Info][i as usize % 3],
            message: format!("synthetic problem {i}"),
            source: "mock".into(),
            code: None,
        })
        .collect();
    let took = with(cx, window, &view, |v, _, cx| {
        let started = Instant::now();
        v.set_diagnostics(diagnostics);
        v.toggle_sort(cx);
        cx.notify();
        started.elapsed()
    });
    println!("problems: 20,000 diagnostics grouped and sorted in {took:?}");
    assert_eq!(cx.read(|cx| view.read(cx).group_count()), 500);
    assert!(took.as_secs_f64() < 2.0, "took {took:?}");
}

#[gpui_kit::test]
fn terminal_renders_sessions_with_ansi_colors(cx: &mut TestAppContext) {
    let (window, view, _) = open_view(cx, MockConfig::instant(), |s, _, cx| {
        TerminalView::new(s, cx)
    });
    let titles = cx.read(|cx| view.read(cx).session_titles());
    assert!(titles.len() >= 2, "{titles:?}");
    let colored = cx.read(|cx| {
        view.read(cx)
            .active_session()
            .unwrap()
            .lines
            .iter()
            .flatten()
            .any(|span| span.fg != AnsiColor::Default)
    });
    assert!(colored, "cargo output carries ANSI colors");
    cx.read(|cx| {
        let green = crate::panels::terminal::ansi_color(AnsiColor::Green, cx);
        let red = crate::panels::terminal::ansi_color(AnsiColor::Red, cx);
        assert_ne!(green, red);
    });

    with(cx, window, &view, |v, _, cx| v.new_session(cx));
    let after = cx.read(|cx| view.read(cx).session_titles());
    assert_eq!(after.len(), titles.len() + 1);
    let active = cx.read(|cx| view.read(cx).active_session().unwrap().id);
    with(cx, window, &view, |v, _, cx| v.close_session(active, cx));
    assert_eq!(
        cx.read(|cx| view.read(cx).session_titles().len()),
        titles.len()
    );
}

#[gpui_kit::test]
fn run_tool_window_summarizes_and_filters_tests(cx: &mut TestAppContext) {
    let (window, view, _) = open_view(cx, MockConfig::instant(), |s, _, cx| RunView::new(s, cx));
    assert_eq!(cx.read(|cx| view.read(cx).summary()), (2, 1, 1));
    let all = cx.read(|cx| view.read(cx).row_count());
    assert_eq!(all, 7);
    // The failing test is the last row; its output explains the failure.
    with(cx, window, &view, |v, _, cx| v.select(all - 1, cx));
    assert!(cx.read(|cx| {
        view.read(cx)
            .selected_output()
            .iter()
            .any(|l| l.contains("panicked"))
    }));
    with(cx, window, &view, |v, _, cx| v.set_hide_passed(true, cx));
    assert_eq!(cx.read(|cx| view.read(cx).row_count()), 5);
}

#[gpui_kit::test]
fn debug_view_expands_variables_and_tracks_breakpoints(cx: &mut TestAppContext) {
    let (window, view, services) = open_view(cx, MockConfig::instant(), |s, w, cx| {
        DebugView::new(s, w, cx)
    });
    assert!(!cx.read(|cx| view.read(cx).variable_rows().is_empty()));
    // `main`'s frame holds the structured `config`.
    with(cx, window, &view, |v, w, cx| v.select_frame(1, 1, w, cx));
    let top = cx.read(|cx| view.read(cx).variable_rows().len());
    assert!(top > 0, "the paused frame has variables");
    let expandable = cx.read(|cx| {
        view.read(cx)
            .variables()
            .iter()
            .position(|v| !v.children.is_empty())
    });
    let ix = expandable.expect("a structured variable");
    with(cx, window, &view, |v, _, cx| {
        v.toggle_variable(vec![ix], cx)
    });
    assert!(cx.read(|cx| view.read(cx).variable_rows().len()) > top);

    let before = cx.read(|cx| view.read(cx).breakpoint_count());
    services
        .debug
        .toggle_breakpoint(&root_of(&services).join("src/config.rs"), 3);
    cx.run_until_parked();
    assert_eq!(cx.read(|cx| view.read(cx).breakpoint_count()), before + 1);

    with(cx, window, &view, |v, _, cx| {
        v.evaluate("1+2", cx);
        v.evaluate("no_such_variable", cx);
    });
    let console = cx.read(|cx| view.read(cx).console_lines().to_vec());
    let n = console.len();
    assert_eq!(console[n - 3].text.as_ref(), "3");
    assert!(console[n - 1].error);
}

// ---------------------------------------------------------------------------
// Structure, TODO, Notifications, Services; focus
// ---------------------------------------------------------------------------

#[gpui_kit::test]
fn structure_follows_the_active_editor(cx: &mut TestAppContext) {
    let (window, ws) = open_workspace(cx);
    let root = cx.read(|cx| ws.read(cx).services.project.roots()[0].clone());
    let structure = cx
        .update_window(window, |_, w, cx| {
            ws.update(cx, |ws, cx| {
                ws.tool_window_body::<StructureView>(ToolWindowKind::Structure, w, cx)
            })
        })
        .unwrap()
        .expect("structure tool window");
    cx.update_window(window, |_, _, cx| {
        ws.update(cx, |ws, cx| {
            crate::panels::structure::active_editor_changed(ws, cx)
        })
    })
    .unwrap();
    cx.run_until_parked();
    let first = cx.read(|cx| structure.read(cx).file().cloned());
    assert!(first.is_some());

    cx.update_window(window, |_, w, cx| {
        ws.update(cx, |ws, cx| ws.open_file(root.join("src/lib.rs"), w, cx))
    })
    .unwrap();
    cx.run_until_parked();
    cx.update_window(window, |_, _, cx| {
        ws.update(cx, |ws, cx| {
            crate::panels::structure::active_editor_changed(ws, cx)
        })
    })
    .unwrap();
    cx.run_until_parked();
    assert_eq!(
        cx.read(|cx| structure.read(cx).file().cloned()),
        Some(root.join("src/lib.rs"))
    );
    assert!(cx.read(|cx| structure.read(cx).is_loaded()));
    let names = cx.read(|cx| structure.read(cx).row_names());
    assert!(names.iter().any(|n| n.contains("greet")), "{names:?}");
}

#[gpui_kit::test]
fn todo_notifications_and_services_show_their_data(cx: &mut TestAppContext) {
    let (_, todo, _) = open_view(cx, MockConfig::instant(), |s, _, cx| TodoView::new(s, cx));
    let (todos, fixmes) = cx.read(|cx| todo.read(cx).counts());
    assert!(todos + fixmes > 0);
    assert!(cx.read(|cx| todo.read(cx).file_count()) > 0);

    let (window, notes, _) = open_view(cx, MockConfig::instant(), |s, _, cx| {
        NotificationsView::new(s, cx)
    });
    assert!(cx.read(|cx| notes.read(cx).unread()) > 0);
    with(cx, window, &notes, |v, _, cx| v.mark_all_read(cx));
    assert_eq!(cx.read(|cx| notes.read(cx).unread()), 0);
    with(cx, window, &notes, |v, _, cx| v.clear_all(cx));
    assert!(cx.read(|cx| notes.read(cx).entries().is_empty()));

    let (window, services_view, services) = open_view(cx, MockConfig::instant(), |s, _, cx| {
        ServicesView::new(s, cx)
    });
    let names = cx.read(|cx| services_view.read(cx).row_names());
    assert!(names.contains(&"Docker".to_string()), "{names:?}");
    // Row 1 is the first run configuration ("demo-app").
    with(cx, window, &services_view, |v, _, cx| {
        v.select(1, cx);
        v.start_stop(cx);
    });
    assert!(matches!(
        services.run.state("app"),
        ferro_services::RunState::Running { .. }
    ));
    with(cx, window, &services_view, |v, _, cx| v.start_stop(cx));
    assert!(!matches!(
        services.run.state("app"),
        ferro_services::RunState::Running { .. }
    ));
    // Let the mock's run thread see the stop and exit while the services
    // (and their event channels) are still alive.
    std::thread::sleep(std::time::Duration::from_millis(600));
    cx.run_until_parked();
}

#[gpui_kit::test]
fn showing_a_tool_window_focuses_its_list(cx: &mut TestAppContext) {
    let (window, ws) = open_workspace(cx);
    cx.update_window(window, |_, w, cx| {
        ws.update(cx, |ws, cx| {
            ws.toggle_tool_window(ToolWindowKind::Problems, w, cx)
        })
    })
    .unwrap();
    cx.run_until_parked();
    let focused = cx
        .update_window(window, |_, w, cx| {
            let problems = ws.update(cx, |ws, cx| {
                ws.tool_window_body::<ProblemsView>(ToolWindowKind::Problems, w, cx)
            });
            problems.is_some_and(|p| p.read(cx).focus_handle(cx).is_focused(w))
        })
        .unwrap();
    assert!(focused, "the Problems list has keyboard focus");
    let _ = TextPos::default();
}
