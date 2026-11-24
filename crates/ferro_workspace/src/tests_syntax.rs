//! Headless tests for Stage 5 in the window: Language Packs in the editor
//! (highlighting, detection, comment syntax), the syntax layer of an editor
//! tab (Expand / Shrink Selection, Auto-Indent Lines, matching brackets,
//! folds) and the tree-sitter language service of projects on disk.

use std::{
    fs,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use ferro_services::{RunnableKind, TextPos};
use ferro_settings::{Paths, Settings};
use gpui_kit::{
    AnyWindowHandle, AppContext as _, Entity, TestAppContext,
    component::highlighter::SyntaxHighlighter,
};

use crate::{
    AppState,
    editor::{EditorCommand, EditorPanel},
    languages, projects,
    workspace::Workspace,
};

fn setup(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    cx.update(|cx| {
        gpui_kit::init(cx);
        ferro_ui::init(cx);
        let root = std::env::temp_dir().join(format!("ferro-syntax-test-{}", std::process::id()));
        let settings = Settings {
            locale: ferro_i18n::Locale::En,
            ..Settings::default()
        };
        AppState::init(settings, Paths::under(&root), false, cx);
        crate::init(cx);
    });
}

fn write(root: &Path, rel: &str, text: &str) -> PathBuf {
    let path = root.join(rel);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(&path, text).unwrap();
    path
}

fn wait_until(
    cx: &mut TestAppContext,
    what: &str,
    mut done: impl FnMut(&mut TestAppContext) -> bool,
) {
    let started = Instant::now();
    loop {
        cx.run_until_parked();
        if done(cx) {
            return;
        }
        assert!(
            started.elapsed() < Duration::from_secs(10),
            "timed out waiting for {what}"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn open_project(cx: &mut TestAppContext, root: &Path) -> (AnyWindowHandle, Entity<Workspace>) {
    let opened = cx
        .update(|cx| projects::open_project(vec![root.to_path_buf()], cx))
        .expect("the project opens");
    cx.run_until_parked();
    opened
}

fn with_editor<R>(
    cx: &mut TestAppContext,
    window: AnyWindowHandle,
    editor: &Entity<EditorPanel>,
    f: impl FnOnce(&mut EditorPanel, &mut gpui_kit::Window, &mut gpui_kit::Context<EditorPanel>) -> R,
) -> R {
    let result = cx
        .update_window(window, |_, window, cx| {
            editor.update(cx, |e, cx| f(e, window, cx))
        })
        .expect("window is open");
    cx.run_until_parked();
    result
}

/// Open a file and wait until its syntax tree is built.
fn open_file(
    cx: &mut TestAppContext,
    window: AnyWindowHandle,
    ws: &Entity<Workspace>,
    path: PathBuf,
) -> Entity<EditorPanel> {
    cx.update_window(window, |_, w, cx| {
        ws.update(cx, |ws, cx| ws.open_file(path, w, cx))
    })
    .unwrap();
    cx.run_until_parked();
    let editor = cx
        .read(|cx| ws.read(cx).active_editor_entity())
        .expect("an active editor");
    wait_until(cx, "the file and its syntax tree", |cx| {
        cx.read(|cx| {
            let e = editor.read(cx);
            e.is_loaded() && (e.language() == languages::PLAIN_TEXT || e.syntax_parses() > 0)
        })
    });
    editor
}

fn text(cx: &mut TestAppContext, editor: &Entity<EditorPanel>) -> String {
    cx.read(|cx| editor.read(cx).text(cx).expect("loaded"))
}

fn selected(cx: &mut TestAppContext, editor: &Entity<EditorPanel>) -> String {
    let text = text(cx, editor);
    let range = cx.read(|cx| editor.read(cx).selection(cx)).unwrap();
    text[range].to_string()
}

fn run(
    cx: &mut TestAppContext,
    window: AnyWindowHandle,
    ws: &Entity<Workspace>,
    command: EditorCommand,
) {
    cx.update_window(window, |_, w, cx| {
        ws.update(cx, |ws, cx| ws.editor_command(command, w, cx))
    })
    .unwrap();
    cx.run_until_parked();
}

#[gpui_kit::test]
fn every_pack_compiles_in_the_editor_highlighter(cx: &mut TestAppContext) {
    setup(cx);
    let registry = languages::registry();
    let mut checked = 0;
    for pack in registry.packs() {
        // GPUI Kit joins injections, locals and highlights into one query; a
        // query it cannot compile falls back to plain text.
        let highlighter = SyntaxHighlighter::new(pack.id());
        assert_eq!(
            highlighter.language().as_ref(),
            pack.id(),
            "{} falls back",
            pack.id()
        );
        for alias in &pack.manifest().aliases {
            assert_eq!(
                SyntaxHighlighter::new(alias).language().as_ref(),
                pack.id(),
                "alias {alias}"
            );
        }
        checked += 1;
    }
    assert!(checked >= 15);
}

#[gpui_kit::test]
fn files_open_in_their_language(cx: &mut TestAppContext) {
    setup(cx);
    let dir = tempfile::tempdir().unwrap();
    let tool = write(
        dir.path(),
        "bin/tool",
        "#!/usr/bin/env python3\nprint('hi')\n",
    );
    let config = write(
        dir.path(),
        "conf/app.txt",
        "# vim: set ft=yaml :\nkey: value\n",
    );
    let header = write(dir.path(), "src/api.hpp", "class Api {};\n");
    let (window, ws) = open_project(cx, dir.path());
    for (path, id, name) in [
        (tool, "python", "Python"),
        (config, "yaml", "YAML"),
        (header, "cpp", "C++"),
    ] {
        let editor = open_file(cx, window, &ws, path);
        let (language, display) = cx.read(|cx| {
            let e = editor.read(cx);
            (e.language().to_string(), e.language_name().to_string())
        });
        assert_eq!((language.as_str(), display.as_str()), (id, name));
    }
}

#[gpui_kit::test]
fn comments_follow_the_pack(cx: &mut TestAppContext) {
    setup(cx);
    let dir = tempfile::tempdir().unwrap();
    let lua = write(dir.path(), "init.lua", "local x = 1\n");
    let (window, ws) = open_project(cx, dir.path());
    let editor = open_file(cx, window, &ws, lua);
    with_editor(cx, window, &editor, |e, w, cx| {
        e.go_to(TextPos::new(0, 3), w, cx)
    });
    run(cx, window, &ws, EditorCommand::ToggleLineComment);
    assert_eq!(text(cx, &editor), "-- local x = 1\n");
}

#[gpui_kit::test]
fn expand_and_shrink_selection(cx: &mut TestAppContext) {
    setup(cx);
    let dir = tempfile::tempdir().unwrap();
    let source = "fn main() {\n    let total = add(1, 2);\n}\n";
    let main = write(dir.path(), "src/main.rs", source);
    let (window, ws) = open_project(cx, dir.path());
    let editor = open_file(cx, window, &ws, main);
    let caret = source.find("add").unwrap() + 1;
    with_editor(cx, window, &editor, |e, w, cx| {
        e.select(caret..caret, w, cx)
    });

    run(cx, window, &ws, EditorCommand::ExpandSelection);
    assert_eq!(selected(cx, &editor), "add");
    run(cx, window, &ws, EditorCommand::ExpandSelection);
    assert_eq!(selected(cx, &editor), "add(1, 2)");
    run(cx, window, &ws, EditorCommand::ExpandSelection);
    run(cx, window, &ws, EditorCommand::ShrinkSelection);
    assert_eq!(selected(cx, &editor), "add(1, 2)");
    run(cx, window, &ws, EditorCommand::ShrinkSelection);
    assert_eq!(selected(cx, &editor), "add");
    run(cx, window, &ws, EditorCommand::ShrinkSelection);
    assert_eq!(
        cx.read(|cx| editor.read(cx).selection(cx)),
        Some(caret..caret)
    );

    // Moving the caret forgets the history.
    run(cx, window, &ws, EditorCommand::ExpandSelection);
    with_editor(cx, window, &editor, |e, w, cx| e.select(0..0, w, cx));
    run(cx, window, &ws, EditorCommand::ShrinkSelection);
    assert_eq!(cx.read(|cx| editor.read(cx).selection(cx)), Some(0..0));
}

#[gpui_kit::test]
fn auto_indent_lines_reindents_from_the_tree(cx: &mut TestAppContext) {
    setup(cx);
    let dir = tempfile::tempdir().unwrap();
    let messy = "fn main() {\nlet x = 1;\n        if x > 0 {\nrun();\n    }\n}\n";
    let path = write(dir.path(), "src/main.rs", messy);
    let (window, ws) = open_project(cx, dir.path());
    let editor = open_file(cx, window, &ws, path);
    let len = messy.len();
    with_editor(cx, window, &editor, |e, w, cx| e.select(0..len, w, cx));
    run(cx, window, &ws, EditorCommand::AutoIndentLines);
    assert_eq!(
        text(cx, &editor),
        "fn main() {\n    let x = 1;\n    if x > 0 {\n        run();\n    }\n}\n"
    );
}

#[gpui_kit::test]
fn projects_on_disk_get_tree_sitter_intelligence(cx: &mut TestAppContext) {
    setup(cx);
    let dir = tempfile::tempdir().unwrap();
    let main = write(
        dir.path(),
        "src/main.rs",
        "struct Config;\n\nfn main() {}\n\n#[test]\nfn parses() {}\n",
    );
    write(dir.path(), "src/util.py", "def helper():\n    pass\n");
    let (window, ws) = open_project(cx, dir.path());
    let editor = open_file(cx, window, &ws, main.clone());
    wait_until(cx, "the outline", |cx| {
        cx.read(|cx| !editor.read(cx).analysis().symbols.is_empty())
    });
    let (symbols, runnables) = cx.read(|cx| {
        let analysis = editor.read(cx).analysis();
        (
            analysis
                .symbols
                .iter()
                .map(|s| s.name.clone())
                .collect::<Vec<_>>(),
            analysis
                .runnables
                .iter()
                .map(|r| (r.line, r.kind))
                .collect::<Vec<_>>(),
        )
    });
    assert_eq!(symbols, ["Config", "main", "parses"]);
    assert_eq!(
        runnables,
        [(2, RunnableKind::Main), (5, RunnableKind::Test)]
    );
    assert_eq!(cx.read(|cx| editor.read(cx).problem_counts()), (0, 0));

    // A syntax error shows up as a problem.
    with_editor(cx, window, &editor, |e, w, cx| {
        e.replace_text("struct Config;\n\nfn main() { let = ; }\n".into(), w, cx)
    });
    wait_until(cx, "the syntax error", |cx| {
        // Analysis runs after a typing pause (simulated clock).
        cx.executor()
            .advance_clock(crate::editor::ANALYSIS_DEBOUNCE);
        cx.read(|cx| editor.read(cx).problem_counts().0 > 0)
    });

    // Go to Symbol finds definitions across the project.
    let language = cx.read(|cx| ws.read(cx).services.language.clone());
    wait_until(cx, "the file index", |_| {
        smol::block_on(language.workspace_symbols("helper"))
            .unwrap()
            .iter()
            .any(|s| s.name == "helper")
    });
}
