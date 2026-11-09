//! Headless tests for Quick Open, the Command Palette and Find in Files,
//! through the real views on mock services.

use std::{
    path::PathBuf,
    time::{Duration, Instant},
};

use ferro_services::{FileMatch, Services, TextPos, mock::MockConfig};
use ferro_settings::{Paths, Settings};
use gpui_kit::{
    AnyWindowHandle, AppContext as _, Bounds, Entity, Point, TestAppContext, WindowBounds,
    WindowOptions, component::dock::DockPlacement, px, size,
};

use crate::{
    AppState,
    find_in_files::{self, FindInFiles, Row},
    quick_open::{self, QuickOpen, QuickOpenItem, QuickOpenMode, Status},
    workspace::Workspace,
};

fn setup(cx: &mut TestAppContext) {
    cx.update(|cx| {
        gpui_kit::init(cx);
        ferro_ui::init(cx);
        let root = std::env::temp_dir().join(format!("ferro-search-test-{}", std::process::id()));
        let settings = Settings {
            locale: ferro_i18n::Locale::En,
            ..Settings::default()
        };
        AppState::init(settings, Paths::under(&root), false, cx);
        crate::init(cx);
    });
}

fn open(cx: &mut TestAppContext) -> (AnyWindowHandle, Entity<Workspace>) {
    setup(cx);
    let services = Services::mock(MockConfig::instant());
    let opened = cx.update(|cx| {
        gpui_kit::open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(Bounds {
                    origin: Point::default(),
                    size: size(px(1400.), px(900.)),
                })),
                ..Default::default()
            },
            cx,
            move |window, cx| Workspace::build(services, window, cx),
        )
        .expect("open workspace window")
    });
    cx.run_until_parked();
    opened
}

fn with<R>(
    cx: &mut TestAppContext,
    window: AnyWindowHandle,
    ws: &Entity<Workspace>,
    f: impl FnOnce(&mut Workspace, &mut gpui_kit::Window, &mut gpui_kit::Context<Workspace>) -> R,
) -> R {
    let result = cx
        .update_window(window, |_, window, cx| {
            ws.update(cx, |ws, cx| f(ws, window, cx))
        })
        .expect("window is open");
    cx.run_until_parked();
    result
}

fn view_do<V: 'static, R>(
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

/// Let debounce timers expire and the background work finish.
fn settle(cx: &mut TestAppContext) {
    cx.executor().advance_clock(Duration::from_millis(500));
    cx.run_until_parked();
}

fn quick_open(
    cx: &mut TestAppContext,
    window: AnyWindowHandle,
    ws: &Entity<Workspace>,
    query: &str,
) -> Entity<QuickOpen> {
    let view = with(cx, window, ws, |ws, w, cx| {
        quick_open::open_view(ws, QuickOpenMode::Files, w, cx)
    });
    view_do(cx, window, &view, |v, w, cx| v.set_query(query, w, cx));
    settle(cx);
    view
}

fn items(cx: &mut TestAppContext, view: &Entity<QuickOpen>) -> Vec<QuickOpenItem> {
    cx.read(|cx| view.read(cx).items().to_vec())
}

fn root(cx: &mut TestAppContext, ws: &Entity<Workspace>) -> PathBuf {
    cx.read(|cx| ws.read(cx).services.project.roots()[0].clone())
}

fn active_path(cx: &mut TestAppContext, ws: &Entity<Workspace>) -> Option<PathBuf> {
    cx.read(|cx| {
        ws.read(cx)
            .active_editor_entity()
            .map(|e| e.read(cx).path().to_path_buf())
    })
}

#[gpui_kit::test]
fn prefixes_switch_modes_live(cx: &mut TestAppContext) {
    let (window, ws) = open(cx);
    let view = quick_open(cx, window, &ws, "main");
    assert_eq!(cx.read(|cx| view.read(cx).mode()), QuickOpenMode::Files);
    for (query, mode) in [
        (">theme", QuickOpenMode::Commands),
        ("@ma", QuickOpenMode::SymbolsInFile),
        ("#Conf", QuickOpenMode::SymbolsInWorkspace),
        (":3", QuickOpenMode::GoToLine),
        ("%TODO", QuickOpenMode::Text),
        ("lib", QuickOpenMode::Files),
    ] {
        view_do(cx, window, &view, |v, w, cx| v.set_query(query, w, cx));
        settle(cx);
        assert_eq!(cx.read(|cx| view.read(cx).mode()), mode, "{query}");
        assert_eq!(cx.read(|cx| view.read(cx).status().clone()), Status::Ready);
        assert!(!items(cx, &view).is_empty(), "{query} found nothing");
    }
    // Text mode used the search service: every item is a text match.
    view_do(cx, window, &view, |v, w, cx| v.set_query("%TODO", w, cx));
    settle(cx);
    assert!(
        items(cx, &view)
            .iter()
            .all(|i| matches!(i, QuickOpenItem::Text { .. }))
    );
}

#[gpui_kit::test]
fn files_query_puts_main_rs_first_and_opens_it(cx: &mut TestAppContext) {
    let (window, ws) = open(cx);
    // Close src/main.rs so it is not "recent".
    let root = root(cx, &ws);
    with(cx, window, &ws, |ws, w, cx| ws.close_all_tabs(w, cx));
    let view = quick_open(cx, window, &ws, "main");
    match &items(cx, &view)[0] {
        QuickOpenItem::File {
            relative,
            positions,
            ..
        } => {
            assert_eq!(relative.as_ref(), "src/main.rs");
            assert!(!positions.is_empty());
        }
        other => panic!("unexpected first item {other:?}"),
    }
    view_do(cx, window, &view, |v, w, cx| v.confirm(false, w, cx));
    assert_eq!(active_path(cx, &ws), Some(root.join("src/main.rs")));
    let dialog_open = cx
        .update_window(window, |_, w, cx| {
            use gpui_kit::component::WindowExt as _;
            w.has_active_dialog(cx)
        })
        .unwrap();
    assert!(!dialog_open, "confirming closes the popup");
}

#[gpui_kit::test]
fn recently_used_files_and_commands_come_first(cx: &mut TestAppContext) {
    let (window, ws) = open(cx);
    let root = root(cx, &ws);
    // Default layout opens main.rs, lib.rs, Cargo.toml; activate them in order.
    for rel in ["Cargo.toml", "src/lib.rs", "src/main.rs"] {
        with(cx, window, &ws, |ws, w, cx| {
            ws.open_file(root.join(rel), w, cx)
        });
    }
    let view = quick_open(cx, window, &ws, "");
    let order: Vec<String> = items(cx, &view)
        .iter()
        .filter_map(|i| match i {
            QuickOpenItem::File { relative, .. } => Some(relative.to_string()),
            _ => None,
        })
        .collect();
    // The previous file first, the active one last.
    assert_eq!(order, vec!["src/lib.rs", "Cargo.toml", "src/main.rs"]);

    // Run a command; next time it leads the palette.
    view_do(cx, window, &view, |v, w, cx| {
        v.set_query(">terminal", w, cx)
    });
    view_do(cx, window, &view, |v, w, cx| v.confirm(false, w, cx));
    let view = quick_open(cx, window, &ws, ">");
    match &items(cx, &view)[0] {
        QuickOpenItem::Command { id, recent, .. } => {
            assert!(id.contains("Terminal"), "{id}");
            assert!(*recent);
        }
        other => panic!("unexpected {other:?}"),
    }
}

#[gpui_kit::test]
fn command_mode_lists_shortcuts_and_runs_commands(cx: &mut TestAppContext) {
    let (window, ws) = open(cx);
    let view = quick_open(cx, window, &ws, ">toggle terminal");
    let first = items(cx, &view)[0].clone();
    match &first {
        QuickOpenItem::Command { id, shortcut, .. } => {
            assert_eq!(*id, "ferro::ToggleTerminal");
            // VS Code is the default scheme.
            assert_eq!(shortcut.as_deref(), Some("Ctrl+`"));
        }
        other => panic!("unexpected {other:?}"),
    }
    let all = quick_open(cx, window, &ws, ">");
    let count = items(cx, &all).len();
    assert_eq!(
        count,
        ferro_keymap::ACTIONS
            .iter()
            .filter(|a| a.searchable)
            .count()
    );
    let view = quick_open(cx, window, &ws, ">toggle terminal");
    assert!(!cx.read(|cx| {
        ws.read(cx)
            .dock_area()
            .read(cx)
            .is_dock_open(DockPlacement::Bottom)
    }));
    view_do(cx, window, &view, |v, w, cx| v.confirm(false, w, cx));
    assert!(
        cx.read(|cx| ws
            .read(cx)
            .dock_area()
            .read(cx)
            .is_dock_open(DockPlacement::Bottom)),
        "running Toggle Terminal opens the bottom dock"
    );
}

#[gpui_kit::test]
fn go_to_line_previews_and_moves_the_caret(cx: &mut TestAppContext) {
    let (window, ws) = open(cx);
    let caret = |cx: &mut TestAppContext| {
        cx.read(|cx| {
            ws.read(cx)
                .active_editor_entity()
                .unwrap()
                .read(cx)
                .caret(cx)
        })
    };
    let view = quick_open(cx, window, &ws, ":5");
    assert!(matches!(items(cx, &view)[0], QuickOpenItem::Line { .. }));
    assert_eq!(caret(cx), Some(TextPos::new(4, 0)), "live preview");
    view_do(cx, window, &view, |v, w, cx| v.set_query(":4:5", w, cx));
    view_do(cx, window, &view, |v, w, cx| v.confirm(false, w, cx));
    assert_eq!(caret(cx), Some(TextPos::new(3, 4)));

    // Invalid and out-of-range input explain themselves.
    let view = quick_open(cx, window, &ws, ":abc");
    assert!(matches!(items(cx, &view)[0], QuickOpenItem::Message(_)));
    view_do(cx, window, &view, |v, w, cx| v.set_query(":99999", w, cx));
    assert!(matches!(items(cx, &view)[0], QuickOpenItem::Message(_)));
}

#[gpui_kit::test]
fn go_to_line_12_in_a_longer_file(cx: &mut TestAppContext) {
    let (window, ws) = open(cx);
    let root = root(cx, &ws);
    with(cx, window, &ws, |ws, w, cx| {
        ws.open_file(root.join("src/lib.rs"), w, cx)
    });
    let view = quick_open(cx, window, &ws, ":12");
    view_do(cx, window, &view, |v, w, cx| v.confirm(false, w, cx));
    let caret = cx.read(|cx| {
        ws.read(cx)
            .active_editor_entity()
            .unwrap()
            .read(cx)
            .caret(cx)
    });
    assert_eq!(caret, Some(TextPos::new(11, 0)));
}

#[gpui_kit::test]
fn symbol_modes_find_greet(cx: &mut TestAppContext) {
    let (window, ws) = open(cx);
    let root = root(cx, &ws);
    with(cx, window, &ws, |ws, w, cx| {
        ws.open_file(root.join("src/lib.rs"), w, cx)
    });
    let view = quick_open(cx, window, &ws, "@greet");
    let found = items(cx, &view);
    let greet = found
        .iter()
        .find(|i| matches!(i, QuickOpenItem::Symbol { name, .. } if name.as_ref() == "greet"))
        .cloned()
        .expect("greet in lib.rs");

    let view = quick_open(cx, window, &ws, "#greet");
    assert!(
        items(cx, &view)
            .iter()
            .any(|i| matches!(i, QuickOpenItem::Symbol { name, .. } if name.as_ref() == "greet"))
    );

    // Confirming a symbol puts the caret on it.
    let view = quick_open(cx, window, &ws, "@greet");
    view_do(cx, window, &view, |v, w, cx| v.confirm(false, w, cx));
    let QuickOpenItem::Symbol { position, .. } = greet else {
        unreachable!()
    };
    let caret = cx.read(|cx| {
        ws.read(cx)
            .active_editor_entity()
            .unwrap()
            .read(cx)
            .caret(cx)
    });
    assert_eq!(caret, Some(position));
}

#[gpui_kit::test]
fn failures_show_an_error_state(cx: &mut TestAppContext) {
    setup(cx);
    let services = Services::mock(MockConfig {
        failure_rate: 1.0,
        ..MockConfig::instant()
    });
    let (window, ws) = cx.update(|cx| {
        gpui_kit::open_window(WindowOptions::default(), cx, move |window, cx| {
            Workspace::build(services, window, cx)
        })
        .unwrap()
    });
    cx.run_until_parked();
    let view = quick_open(cx, window, &ws, "main");
    assert!(matches!(
        cx.read(|cx| view.read(cx).status().clone()),
        Status::Failed(_)
    ));
    assert!(items(cx, &view).is_empty());
}

fn find(
    cx: &mut TestAppContext,
    window: AnyWindowHandle,
    ws: &Entity<Workspace>,
    replace: bool,
    query: &str,
) -> Entity<FindInFiles> {
    let view = with(cx, window, ws, |ws, w, cx| {
        find_in_files::open_view(ws, replace, w, cx)
    });
    view_do(cx, window, &view, |v, w, cx| v.set_query(query, w, cx));
    settle(cx);
    view
}

#[gpui_kit::test]
fn find_in_files_finds_todos_and_opens_the_match(cx: &mut TestAppContext) {
    let (window, ws) = open(cx);
    let view = find(cx, window, &ws, false, "TODO");
    let (results, rows) = cx.read(|cx| {
        let v = view.read(cx);
        (v.results().to_vec(), v.rows().to_vec())
    });
    assert!(!results.is_empty());
    assert!(
        results
            .iter()
            .all(|m| m.line_text.to_lowercase().contains("todo"))
    );
    assert!(matches!(rows[0], Row::File { .. }));
    let target = cx
        .read(|cx| view.read(cx).selected_match().cloned())
        .unwrap();

    // Down moves to the next match row, skipping headings; up comes back.
    view_do(cx, window, &view, |v, w, cx| v.move_selection(true, w, cx));
    view_do(cx, window, &view, |v, w, cx| v.move_selection(false, w, cx));
    assert_eq!(
        cx.read(|cx| view.read(cx).selected_match().cloned()),
        Some(target.clone())
    );

    view_do(cx, window, &view, |v, w, cx| v.open_selected(w, cx));
    assert_eq!(active_path(cx, &ws), Some(target.path.clone()));
    let caret = cx.read(|cx| {
        ws.read(cx)
            .active_editor_entity()
            .unwrap()
            .read(cx)
            .caret(cx)
    });
    assert_eq!(caret.map(|c| c.line), Some(target.line));
}

#[gpui_kit::test]
fn find_in_files_filters_toggles_and_reports_regex_errors(cx: &mut TestAppContext) {
    let (window, ws) = open(cx);
    let view = find(cx, window, &ws, false, "greet");
    let all = cx.read(|cx| view.read(cx).results().len());
    view_do(cx, window, &view, |v, w, cx| v.set_filter("src/", w, cx));
    settle(cx);
    let filtered = cx.read(|cx| view.read(cx).results().to_vec());
    assert!(!filtered.is_empty() && filtered.len() < all);
    assert!(filtered.iter().all(|m| m.relative.contains("src/")));

    view_do(cx, window, &view, |v, w, cx| v.toggle_whole_word(w, cx));
    settle(cx);
    let words = cx.read(|cx| view.read(cx).results().len());
    assert!(words <= filtered.len());

    view_do(cx, window, &view, |v, w, cx| {
        v.toggle_regex(w, cx);
        v.set_query("gr(e+)t", w, cx);
    });
    settle(cx);
    assert!(matches!(
        cx.read(|cx| view.read(cx).status().clone()),
        find_in_files::Status::Failed(_)
    ));

    view_do(cx, window, &view, |v, w, cx| v.set_query("", w, cx));
    assert_eq!(
        cx.read(|cx| view.read(cx).status().clone()),
        find_in_files::Status::Idle
    );
}

#[gpui_kit::test]
fn replace_all_rewrites_open_files_only(cx: &mut TestAppContext) {
    let (window, ws) = open(cx);
    let root = root(cx, &ws);
    with(cx, window, &ws, |ws, w, cx| {
        ws.open_file(root.join("src/lib.rs"), w, cx)
    });
    let view = find(cx, window, &ws, true, "greet");
    view_do(cx, window, &view, |v, w, cx| {
        v.set_replacement("welcome", w, cx)
    });
    let (count, files, skipped) = view_do(cx, window, &view, |v, w, cx| v.replace_all(w, cx));
    assert!(count >= 2, "lib.rs and main.rs mention greet");
    assert!(files >= 1);
    assert!(skipped >= 1, "web/src/app.ts is not open");
    let text = cx.read(|cx| {
        ws.read(cx)
            .editors
            .iter()
            .find(|e| e.read(cx).path() == root.join("src/lib.rs"))
            .and_then(|e| e.read(cx).text(cx))
            .unwrap()
    });
    assert!(
        text.contains("welcome") && !text.contains("greet("),
        "{text}"
    );
}

// ---------------------------------------------------------------------------
// Performance: 10× the typical project
// ---------------------------------------------------------------------------

/// Query the mock file search over a stress tree of `files` files; returns
/// the time of the second query (the first also builds the index).
fn time_find_files(files: usize) -> Duration {
    let config = MockConfig {
        stress_files: files,
        ..MockConfig::instant()
    };
    let services = Services::mock(config);
    let started = Instant::now();
    let first = smol::block_on(services.search.find_files("file0001", 50)).unwrap();
    let indexing = started.elapsed();
    let started = Instant::now();
    let hits = smol::block_on(services.search.find_files("mod12file7", 200)).unwrap();
    let query = started.elapsed();
    println!(
        "find_files over ~{files} files: first query (incl. index) {indexing:?}, next query {query:?}, {} hits",
        hits.len()
    );
    assert!(!first.is_empty() && !hits.is_empty());
    query
}

/// A typical large project (100k files), in every test run.
#[test]
fn find_files_over_100k_files() {
    let query = time_find_files(100_000);
    // Generous for unoptimized debug builds on a busy CI machine.
    assert!(query < Duration::from_secs(10), "query took {query:?}");
}

/// 10× that. Debug builds take ~30 s, so run it optimized:
/// `cargo test -p ferro_workspace --release -- --ignored million`.
#[test]
#[ignore = "10× stress test; run with --release --ignored"]
fn find_files_over_a_million_files() {
    let query = time_find_files(1_000_000);
    assert!(query < Duration::from_secs(10), "query took {query:?}");
}

#[gpui_kit::test]
fn handling_1000_results_fits_in_a_frame(cx: &mut TestAppContext) {
    let (window, ws) = open(cx);
    let root = root(cx, &ws);
    let matches: Vec<FileMatch> = (0..1_000)
        .map(|i| {
            let relative = format!("stress/module_{}/file_{i:04}.rs", i % 37);
            FileMatch {
                path: root.join(&relative),
                relative,
                score: 1_000 - i,
                positions: vec![7, 8, 9],
            }
        })
        .collect();
    let recent: Vec<PathBuf> = (0..50)
        .map(|i| root.join(format!("stress/x{i}.rs")))
        .collect();
    let started = Instant::now();
    let rows = quick_open::file_items(matches, &recent);
    let took = started.elapsed();
    println!("1,000 file results turned into rows in {took:?}");
    assert_eq!(rows.len(), 1_000);
    assert!(took < Duration::from_millis(16), "took {took:?}");

    // And a frame with the popup showing 200 results renders.
    let view = quick_open(cx, window, &ws, "s");
    let started = Instant::now();
    cx.update_window(window, |_, w, cx| {
        view.update(cx, |_, cx| cx.notify());
        w.refresh();
    })
    .unwrap();
    cx.run_until_parked();
    println!(
        "re-render with {} results: {:?}",
        items(cx, &view).len(),
        started.elapsed()
    );
}

#[gpui_kit::test]
fn keyboard_drives_the_popup(cx: &mut TestAppContext) {
    let (window, ws) = open(cx);
    let caret = |cx: &mut TestAppContext| {
        cx.read(|cx| {
            ws.read(cx)
                .active_editor_entity()
                .unwrap()
                .read(cx)
                .caret(cx)
        })
    };
    let before = caret(cx);
    // Esc after a live `:` preview puts the caret back.
    let _view = quick_open(cx, window, &ws, ":5");
    assert_eq!(caret(cx), Some(TextPos::new(4, 0)));
    cx.simulate_keystrokes(window, "escape");
    cx.run_until_parked();
    assert_eq!(caret(cx), before);

    // Arrows move the selection, Enter opens it.
    let view = quick_open(cx, window, &ws, "rs");
    cx.simulate_keystrokes(window, "down down up");
    cx.run_until_parked();
    assert_eq!(cx.read(|cx| view.read(cx).selected_index()), 1);
    let expected = match &items(cx, &view)[1] {
        QuickOpenItem::File { path, .. } => path.clone(),
        other => panic!("unexpected {other:?}"),
    };
    cx.simulate_keystrokes(window, "enter");
    cx.run_until_parked();
    assert_eq!(active_path(cx, &ws), Some(expected));
}

#[gpui_kit::test]
fn shift_shift_opens_search_everywhere_in_the_jetbrains_scheme(cx: &mut TestAppContext) {
    use gpui_kit::{Modifiers, VisualTestContext, component::WindowExt as _};
    let (window, _ws) = open(cx);
    let dialog_open = |cx: &mut TestAppContext| {
        cx.update_window(window, |_, w, cx| w.has_active_dialog(cx))
            .unwrap()
    };
    let double_tap = |cx: &mut TestAppContext| {
        let mut vcx = VisualTestContext::from_window(window, cx);
        for _ in 0..2 {
            vcx.simulate_modifiers_change(Modifiers::shift());
            vcx.simulate_modifiers_change(Modifiers::none());
        }
        vcx.run_until_parked();
    };
    // VS Code (default): nothing happens.
    double_tap(cx);
    assert!(!dialog_open(cx));
    cx.update(|cx| crate::app_state::set_keymap(ferro_keymap::KeymapPreset::JetBrains, cx));
    double_tap(cx);
    assert!(dialog_open(cx), "Shift Shift opens Search Everywhere");
}
