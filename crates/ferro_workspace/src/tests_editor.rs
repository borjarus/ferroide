//! Headless tests for the editor view layer (Stage 2): language providers,
//! diagnostics and problem navigation, breakpoints, runnables, breadcrumbs,
//! sticky lines, preview tabs, files outside the project, and a performance
//! check on a 50,000-line file.

use std::{path::PathBuf, time::Instant};

use ferro_services::{RunnableKind, Services, TextPos, mock::MockConfig};
use ferro_settings::{Paths, Settings};
use gpui_kit::{
    AnyWindowHandle, AppContext as _, Bounds, Entity, Point, TestAppContext, WindowBounds,
    WindowOptions,
    component::input::{CompletionProvider as _, Rope},
    px, size,
};
use lsp_types::{CompletionContext, CompletionResponse, CompletionTriggerKind};

use crate::{
    AppState,
    editor::{EditorCommand, EditorPanel, model, providers::LanguageProviders},
    requests::OpenLocation,
    workspace::Workspace,
};

fn setup(cx: &mut TestAppContext) {
    cx.update(|cx| {
        gpui_kit::init(cx);
        ferro_ui::init(cx);
        let root = std::env::temp_dir().join(format!("ferro-ed-test-{}", std::process::id()));
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

fn root(cx: &mut TestAppContext, ws: &Entity<Workspace>) -> PathBuf {
    cx.read(|cx| ws.read(cx).services.project.roots()[0].clone())
}

/// Open `rel` and return its editor once loaded and analyzed.
fn open_editor(
    cx: &mut TestAppContext,
    window: AnyWindowHandle,
    ws: &Entity<Workspace>,
    path: PathBuf,
) -> Entity<EditorPanel> {
    with(cx, window, ws, |ws, w, cx| ws.open_file(path, w, cx));
    let editor = cx
        .read(|cx| ws.read(cx).active_editor_entity())
        .expect("an active editor");
    assert!(cx.read(|cx| editor.read(cx).is_loaded()));
    editor
}

fn text(cx: &mut TestAppContext, editor: &Entity<EditorPanel>) -> String {
    cx.read(|cx| editor.read(cx).text(cx).expect("loaded"))
}

/// 0-based position of the first occurrence of `needle`, plus `offset`
/// characters.
fn find(text: &str, needle: &str, offset: u32) -> TextPos {
    for (line, line_text) in text.lines().enumerate() {
        if let Some(byte) = line_text.find(needle) {
            let column = line_text[..byte].chars().count() as u32;
            return TextPos::new(line as u32, column + offset);
        }
    }
    panic!("`{needle}` not found");
}

#[gpui_kit::test]
fn completion_items_arrive_for_a_prefix(cx: &mut TestAppContext) {
    let (window, ws) = open(cx);
    let path = root(cx, &ws).join("src/main.rs");
    let editor = open_editor(cx, window, &ws, path.clone());
    let source = text(cx, &editor);
    let language = cx.read(|cx| ws.read(cx).services.language.clone());
    let providers = LanguageProviders::new(path, language);

    // Ask at the end of `gre` in `greet(name)`.
    let offset = source.find("greet(name)").unwrap() + 3;
    let rope = Rope::from(source.as_str());
    let task = cx
        .update_window(window, |_, window, cx| {
            providers.completions(
                &rope,
                offset,
                CompletionContext {
                    trigger_kind: CompletionTriggerKind::INVOKED,
                    trigger_character: None,
                },
                window,
                cx,
            )
        })
        .unwrap();
    cx.run_until_parked();
    let response = cx
        .foreground_executor()
        .block_test(task)
        .expect("completions");
    let CompletionResponse::Array(items) = response else {
        panic!("expected an array");
    };
    assert!(
        items.iter().any(|i| i.label == "greet"),
        "{:?}",
        items.iter().map(|i| &i.label).collect::<Vec<_>>()
    );
}

#[gpui_kit::test]
fn diagnostics_populate_and_problem_navigation_moves_the_caret(cx: &mut TestAppContext) {
    let (window, ws) = open(cx);
    let path = root(cx, &ws).join("src/config.rs");
    let editor = open_editor(cx, window, &ws, path);
    let diagnostics = cx.read(|cx| editor.read(cx).analysis().diagnostics.clone());
    assert!(!diagnostics.is_empty(), "config.rs has mock diagnostics");
    let mut starts: Vec<TextPos> = diagnostics.iter().map(|d| d.range.start).collect();
    starts.sort();
    starts.dedup();

    with(cx, window, &ws, |ws, w, cx| {
        let editor = ws.active_editor_entity().unwrap();
        editor.update(cx, |e, cx| e.go_to(TextPos::new(0, 0), w, cx));
        ws.editor_command(EditorCommand::NextProblem, w, cx)
    });
    assert_eq!(cx.read(|cx| editor.read(cx).caret(cx)), Some(starts[0]));

    // Previous from the first wraps to the last.
    with(cx, window, &ws, |ws, w, cx| {
        ws.editor_command(EditorCommand::PreviousProblem, w, cx)
    });
    assert_eq!(
        cx.read(|cx| editor.read(cx).caret(cx)),
        starts.last().copied()
    );
    let (errors, warnings) = cx.read(|cx| editor.read(cx).problem_counts());
    assert!(
        errors + warnings > 0
            || diagnostics
                .iter()
                .all(|d| d.severity > ferro_services::Severity::Warning)
    );
}

#[gpui_kit::test]
fn toggling_a_breakpoint_updates_the_service_and_the_margin(cx: &mut TestAppContext) {
    let (window, ws) = open(cx);
    let path = root(cx, &ws).join("src/main.rs");
    let editor = open_editor(cx, window, &ws, path.clone());
    let debug = cx.read(|cx| ws.read(cx).services.debug.clone());
    // The mock starts with a breakpoint on main.rs line 4 (0-based).
    assert!(cx.read(|cx| editor.read(cx).breakpoints().iter().any(|b| b.line == 4)));

    with(cx, window, &ws, |ws, w, cx| {
        let editor = ws.active_editor_entity().unwrap();
        editor.update(cx, |e, cx| e.go_to(TextPos::new(2, 0), w, cx));
        ws.editor_command(EditorCommand::ToggleBreakpoint, w, cx)
    });
    assert!(debug.breakpoints_in(&path).iter().any(|b| b.line == 2));
    assert!(cx.read(|cx| editor.read(cx).breakpoints().iter().any(|b| b.line == 2)));

    // A change from elsewhere (the Debug tool window) reaches the margin.
    debug.toggle_breakpoint(&path, 2);
    cx.run_until_parked();
    assert!(!cx.read(|cx| editor.read(cx).breakpoints().iter().any(|b| b.line == 2)));
}

#[gpui_kit::test]
fn runnables_are_found_for_main_and_tests(cx: &mut TestAppContext) {
    let (window, ws) = open(cx);
    let root = root(cx, &ws);
    let main = open_editor(cx, window, &ws, root.join("src/main.rs"));
    let runnables = cx.read(|cx| main.read(cx).analysis().runnables.clone());
    assert!(runnables.iter().any(|r| r.kind == RunnableKind::Main));

    let lib = open_editor(cx, window, &ws, root.join("src/lib.rs"));
    let runnables = cx.read(|cx| lib.read(cx).analysis().runnables.clone());
    assert!(runnables.iter().any(|r| r.kind == RunnableKind::Test));
}

#[gpui_kit::test]
fn breadcrumbs_show_the_symbol_at_the_caret(cx: &mut TestAppContext) {
    let (window, ws) = open(cx);
    let path = root(cx, &ws).join("src/lib.rs");
    let editor = open_editor(cx, window, &ws, path);
    let source = text(cx, &editor);
    let inside = find(&source, "pub fn greet", 0);
    let inside = TextPos::new(inside.line + 1, 4);
    with(cx, window, &ws, |ws, w, cx| {
        let editor = ws.active_editor_entity().unwrap();
        editor.update(cx, |e, cx| e.go_to(inside, w, cx));
    });
    let names: Vec<String> = cx.read(|cx| {
        editor
            .read(cx)
            .breadcrumb_symbols(cx)
            .into_iter()
            .map(|s| s.name)
            .collect()
    });
    assert_eq!(names.last().map(String::as_str), Some("greet"), "{names:?}");
}

#[gpui_kit::test]
fn sticky_lines_follow_scrolling_in_real_outline(cx: &mut TestAppContext) {
    let (window, ws) = open(cx);
    let path = root(cx, &ws).join("src/lib.rs");
    let editor = open_editor(cx, window, &ws, path);
    let symbols = cx.read(|cx| editor.read(cx).analysis().symbols.clone());
    let source = text(cx, &editor);
    let test_fn = find(&source, "fn greets_by_name", 0).line;
    // Scrolled so the first visible line is inside the test function.
    let lines = model::sticky_lines(&symbols, test_fn, 3);
    assert!(lines.contains(&test_fn), "{lines:?} for test at {test_fn}");
    assert!(
        lines.len() >= 2,
        "the test module header is pinned too: {lines:?}"
    );
    assert!(model::sticky_lines(&symbols, 0, 3).is_empty());
}

#[gpui_kit::test]
fn parameter_info_opens_inside_a_call(cx: &mut TestAppContext) {
    let (window, ws) = open(cx);
    let path = root(cx, &ws).join("src/main.rs");
    let editor = open_editor(cx, window, &ws, path);
    let source = text(cx, &editor);
    let inside = find(&source, "greet(name)", 6);
    with(cx, window, &ws, |ws, w, cx| {
        let editor = ws.active_editor_entity().unwrap();
        editor.update(cx, |e, cx| e.go_to(inside, w, cx));
        ws.editor_command(EditorCommand::ParameterInfo, w, cx)
    });
    let help = cx.read(|cx| editor.read(cx).signature_help().cloned());
    let help = help.expect("signature help inside greet(…)");
    assert!(help.label.contains("greet"), "{}", help.label);
    with(cx, window, &ws, |ws, _, cx| {
        let editor = ws.active_editor_entity().unwrap();
        editor.update(cx, |e, cx| e.close_signature(cx));
    });
    assert!(cx.read(|cx| editor.read(cx).signature_help().is_none()));
}

#[gpui_kit::test]
fn preview_tabs_are_replaced_and_kept_by_typing(cx: &mut TestAppContext) {
    let (window, ws) = open(cx);
    let root = root(cx, &ws);
    let count = |cx: &mut TestAppContext| cx.read(|cx| ws.read(cx).editors.len());
    let before = count(cx);
    with(cx, window, &ws, |ws, w, cx| {
        ws.open_location(
            &OpenLocation::file(root.join("README.md")).preview(true),
            w,
            cx,
        )
    });
    assert_eq!(count(cx), before + 1);
    with(cx, window, &ws, |ws, w, cx| {
        ws.open_location(
            &OpenLocation::file(root.join("src/config.rs")).preview(true),
            w,
            cx,
        )
    });
    assert_eq!(
        count(cx),
        before + 1,
        "the second preview replaced the first"
    );
    let preview = cx.read(|cx| ws.read(cx).active_editor_entity()).unwrap();
    assert!(cx.read(|cx| preview.read(cx).is_preview()));
    assert_eq!(cx.read(|cx| preview.read(cx).file_name()), "config.rs");

    cx.simulate_input(window, "x");
    cx.run_until_parked();
    assert!(
        !cx.read(|cx| preview.read(cx).is_preview()),
        "typing keeps the tab"
    );
}

#[gpui_kit::test]
fn files_outside_the_project_open_from_disk(cx: &mut TestAppContext) {
    let (window, ws) = open(cx);
    let dir = std::env::temp_dir().join(format!("ferro-ed-outside-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("settings.toml");
    std::fs::write(&file, "[editor]\ntab_size = 2\n").unwrap();
    let editor = open_editor(cx, window, &ws, file);
    assert_eq!(text(cx, &editor), "[editor]\ntab_size = 2\n");

    let missing = dir.join("nope.toml");
    with(cx, window, &ws, |ws, w, cx| ws.open_file(missing, w, cx));
    let failed = cx.read(|cx| ws.read(cx).active_editor_entity()).unwrap();
    assert!(
        !cx.read(|cx| failed.read(cx).is_loaded()),
        "an error state, not a panic"
    );
}

/// A Rust file of `lines` lines: modules of impl blocks of functions.
fn generated_source(lines: usize) -> String {
    let mut out = String::with_capacity(lines * 32);
    let mut written = 0;
    let mut m = 0;
    while written < lines {
        out.push_str(&format!("mod m{m} {{\n    impl Thing{m} {{\n"));
        written += 2;
        for f in 0..20 {
            out.push_str(&format!(
                "        fn f{f}() {{\n            let value = {f};\n            println!(\"{{value}}\");\n        }}\n"
            ));
            written += 4;
        }
        out.push_str("    }\n}\n");
        written += 2;
        m += 1;
    }
    out
}

#[gpui_kit::test]
fn view_layer_computations_fit_the_frame_budget_on_50k_lines(cx: &mut TestAppContext) {
    let (window, ws) = open(cx);
    let source = generated_source(50_000);
    let line_count = source.lines().count();
    assert!(line_count >= 50_000);
    let path = root(cx, &ws).join("src/generated.rs");
    let language = cx.read(|cx| ws.read(cx).services.language.clone());
    let vcs = cx.read(|cx| ws.read(cx).services.vcs.clone());

    let started = Instant::now();
    let symbols = cx
        .foreground_executor()
        .block_test(language.document_symbols(&path, &source))
        .unwrap();
    let hints = cx
        .foreground_executor()
        .block_test(language.inlay_hints(&path, &source))
        .unwrap();
    let changes = vcs.line_changes(&path, &source);
    let line_hints = model::end_of_line_hints(&hints, &source);
    let gutter = model::gutter_ranges(&changes, &source);
    std::hint::black_box(&gutter);
    let analysis = started.elapsed();

    // What one frame computes: breadcrumbs, sticky lines, visible hints.
    let frames = 60;
    let started = Instant::now();
    for frame in 0..frames {
        let line = (frame * 811 % line_count) as u32;
        let chain = model::symbol_chain(&symbols, TextPos::new(line, 8));
        let sticky = model::sticky_lines(&symbols, line, 3);
        let first = line_hints.partition_point(|h| h.line < line);
        let visible = line_hints[first..]
            .iter()
            .take_while(|h| h.line < line + 60)
            .count();
        std::hint::black_box((chain, sticky, visible));
    }
    let per_frame = started.elapsed() / frames as u32;
    println!(
        "editor view layer on {line_count} lines: analysis {analysis:?} (once per edit, \
         background), per-frame view computations {per_frame:?}, {} symbols, {} hints",
        symbols.len(),
        hints.len()
    );
    // Debug build; the release budget is a 8.3 ms frame.
    assert!(
        per_frame.as_millis() < 16,
        "per-frame view computations took {per_frame:?}"
    );

    // And the editor itself opens and renders such a file (from disk).
    let dir = std::env::temp_dir().join(format!("ferro-ed-perf-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("generated.rs");
    std::fs::write(&file, &source).unwrap();
    let started = Instant::now();
    let editor = open_editor(cx, window, &ws, file);
    let opened = started.elapsed();
    let started = Instant::now();
    cx.update_window(window, |_, window, cx| {
        window.refresh();
        let _ = cx;
    })
    .unwrap();
    cx.run_until_parked();
    println!(
        "opened and analyzed a {line_count}-line file in {opened:?}, re-render {:?}",
        started.elapsed()
    );
    assert!(cx.read(|cx| editor.read(cx).is_loaded()));
    assert!(opened.as_secs() < 60, "opening took {opened:?}");
}
