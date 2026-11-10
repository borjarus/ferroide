//! Headless tests for the Stage 3 editor core in the window: saving in the
//! file's own encoding and line separators, the modified marker, changes
//! made on disk by other programs, files of the project service, and the
//! text commands (Ctrl+D, line moves, comments) with their shortcuts.

use std::{fs, path::PathBuf, sync::Arc};

use ferro_services::{Services, TextPos, mock::MockConfig, mock::MockProjectService};
use ferro_settings::{Paths, Settings};
use ferro_text::{encoding::TextEncoding, line_ending::LineEnding};
use gpui_kit::{
    AnyWindowHandle, AppContext as _, Bounds, Entity, Focusable as _, Point, TestAppContext,
    WindowBounds, WindowOptions, px, size,
};

use crate::{
    AppState,
    editor::{EditorCommand, EditorPanel, ExternalChange},
    workspace::Workspace,
};

fn setup(cx: &mut TestAppContext) {
    cx.update(|cx| {
        gpui_kit::init(cx);
        ferro_ui::init(cx);
        let root = std::env::temp_dir().join(format!("ferro-files-test-{}", std::process::id()));
        let settings = Settings {
            locale: ferro_i18n::Locale::En,
            ..Settings::default()
        };
        AppState::init(settings, Paths::under(&root), false, cx);
        crate::init(cx);
    });
}

fn open_with(cx: &mut TestAppContext, services: Services) -> (AnyWindowHandle, Entity<Workspace>) {
    setup(cx);
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

fn open(cx: &mut TestAppContext) -> (AnyWindowHandle, Entity<Workspace>) {
    open_with(cx, Services::mock(MockConfig::instant()))
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

fn dirty(cx: &mut TestAppContext, editor: &Entity<EditorPanel>) -> bool {
    cx.read(|cx| editor.read(cx).is_dirty())
}

/// Replace the whole text as the user would (undoable, marks the tab).
fn edit(cx: &mut TestAppContext, window: AnyWindowHandle, editor: &Entity<EditorPanel>, new: &str) {
    let new = new.to_string();
    with_editor(cx, window, editor, |e, w, cx| e.replace_text(new, w, cx));
}

fn focus(cx: &mut TestAppContext, window: AnyWindowHandle, editor: &Entity<EditorPanel>) {
    cx.update_window(window, |_, window, cx| {
        let handle = editor.read(cx).focus_handle(cx);
        handle.focus(window, cx);
    })
    .unwrap();
    cx.run_until_parked();
}

#[gpui_kit::test]
fn saving_keeps_the_encoding_and_line_separators(cx: &mut TestAppContext) {
    let (window, ws) = open(cx);
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("notes.txt");
    fs::write(&path, b"\xEF\xBB\xBFone\r\ntwo\r\n").unwrap();

    let editor = open_editor(cx, window, &ws, path.clone());
    assert_eq!(text(cx, &editor), "one\ntwo\n");
    let format = cx.read(|cx| editor.read(cx).disk().format);
    assert_eq!(format.line_ending, LineEnding::CrLf);
    assert_eq!(format.encoding, TextEncoding::UTF8_BOM);
    assert!(!dirty(cx, &editor));

    edit(cx, window, &editor, "one\ntwo\nthree\n");
    assert!(dirty(cx, &editor), "an edit marks the tab modified");

    with(cx, window, &ws, |ws, _, cx| ws.save_active(cx));
    assert_eq!(
        fs::read(&path).unwrap(),
        b"\xEF\xBB\xBFone\r\ntwo\r\nthree\r\n",
        "saved with the BOM and CRLF of the original"
    );
    assert!(!dirty(cx, &editor));
    // The atomic save left no temporary file behind.
    assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 1);

    // Undoing back to the saved text clears the marker again.
    edit(cx, window, &editor, "one\ntwo\nthree\nfour\n");
    edit(cx, window, &editor, "one\ntwo\nthree\n");
    assert!(!dirty(cx, &editor));
}

#[gpui_kit::test]
fn the_line_separator_and_encoding_can_be_changed(cx: &mut TestAppContext) {
    let (window, ws) = open(cx);
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("pl.txt");
    let polish = "Zażółć gęślą jaźń. Pchnąć w tę łódź jeża lub ośm skrzyń fig.\n\
                  Mężny bądź, chroń pułk twój i sześć flag.\n";
    let cp1250 = TextEncoding::new(encoding_rs::WINDOWS_1250, false);
    fs::write(&path, ferro_text::encoding::encode(polish, cp1250).unwrap()).unwrap();

    let editor = open_editor(cx, window, &ws, path.clone());
    assert_eq!(text(cx, &editor), polish, "legacy encoding detected");
    let detected = cx.read(|cx| editor.read(cx).disk().format.encoding);
    assert!(!detected.is_utf8());

    with_editor(cx, window, &editor, |e, _, cx| {
        e.save_with_encoding(TextEncoding::UTF8, cx)
    });
    assert_eq!(fs::read_to_string(&path).unwrap(), polish);

    with_editor(cx, window, &editor, |e, _, cx| {
        e.set_line_ending(LineEnding::CrLf, cx)
    });
    assert_eq!(
        fs::read_to_string(&path).unwrap(),
        polish.replace('\n', "\r\n")
    );

    // Reopening with the wrong encoding shows mojibake, and back.
    with_editor(cx, window, &editor, |e, w, cx| {
        e.reopen_with_encoding(TextEncoding::new(encoding_rs::WINDOWS_1252, false), w, cx)
    });
    assert_ne!(text(cx, &editor), polish);
    with_editor(cx, window, &editor, |e, w, cx| {
        e.reopen_with_encoding(TextEncoding::UTF8, w, cx)
    });
    assert_eq!(text(cx, &editor), polish);
}

#[gpui_kit::test]
fn changes_on_disk_reload_clean_tabs_and_ask_about_modified_ones(cx: &mut TestAppContext) {
    let (window, ws) = open(cx);
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("a.txt");
    fs::write(&path, "first\n").unwrap();
    let editor = open_editor(cx, window, &ws, path.clone());

    // Unmodified: reloads silently.
    fs::write(&path, "second!\n").unwrap();
    with_editor(cx, window, &editor, |e, w, cx| e.check_disk(w, cx));
    assert_eq!(text(cx, &editor), "second!\n");
    assert!(!dirty(cx, &editor));

    // Modified: the editor keeps its text and offers a choice.
    edit(cx, window, &editor, "mine\n");
    fs::write(&path, "theirs, longer\n").unwrap();
    with_editor(cx, window, &editor, |e, w, cx| e.check_disk(w, cx));
    assert_eq!(text(cx, &editor), "mine\n");
    assert!(matches!(
        cx.read(|cx| editor.read(cx).external_change().cloned()),
        Some(ExternalChange::Modified(_))
    ));
    assert_eq!(
        cx.read(|cx| editor.read(cx).compare_texts(cx)),
        Some(("theirs, longer\n".to_string(), "mine\n".to_string()))
    );
    // Compare opens a diff tab.
    with_editor(cx, window, &editor, |_, _, cx| {
        cx.emit(crate::editor::EditorEvent::CompareWithDisk)
    });
    let diffs = cx.read(|cx| {
        crate::vcs::diff_view::find_panel::<crate::vcs::diff_view::DiffPanel>(
            ws.read(cx),
            cx,
            |_| true,
        )
    });
    assert!(diffs.is_some(), "Compare opens the diff view");

    with_editor(cx, window, &editor, |e, w, cx| e.reload(w, cx));
    assert_eq!(text(cx, &editor), "theirs, longer\n");
    assert!(!dirty(cx, &editor));
    assert!(cx.read(|cx| editor.read(cx).external_change().is_none()));

    // Keep Mine: the next save overwrites the disk.
    edit(cx, window, &editor, "keep me\n");
    fs::write(&path, "overwritten soon\n").unwrap();
    with_editor(cx, window, &editor, |e, w, cx| e.check_disk(w, cx));
    with_editor(cx, window, &editor, |e, _, cx| e.keep_mine(cx));
    with(cx, window, &ws, |ws, _, cx| ws.save_active(cx));
    assert_eq!(fs::read_to_string(&path).unwrap(), "keep me\n");

    // Deleted: saving creates the file again.
    fs::remove_file(&path).unwrap();
    with_editor(cx, window, &editor, |e, w, cx| e.check_disk(w, cx));
    assert!(matches!(
        cx.read(|cx| editor.read(cx).external_change().cloned()),
        Some(ExternalChange::Deleted)
    ));
    edit(cx, window, &editor, "back\n");
    with(cx, window, &ws, |ws, _, cx| ws.save_active(cx));
    assert_eq!(fs::read_to_string(&path).unwrap(), "back\n");
}

#[gpui_kit::test]
fn project_files_save_through_the_project_service(cx: &mut TestAppContext) {
    let (services, project): (Services, Arc<MockProjectService>) =
        Services::mock_with_project(MockConfig::instant());
    let (window, ws) = open_with(cx, services);
    let path = project.root().join("src/lib.rs");
    let editor = open_editor(cx, window, &ws, path.clone());

    edit(cx, window, &editor, "pub fn changed() {}\n");
    with(cx, window, &ws, |ws, _, cx| ws.save_all(cx));
    assert_eq!(
        project.file_text(&path).as_deref(),
        Some("pub fn changed() {}\n")
    );
    assert!(!dirty(cx, &editor));

    // Another program changes it: the clean tab follows.
    project.modify_externally(&path, "pub fn external() {}\n");
    cx.run_until_parked();
    assert_eq!(text(cx, &editor), "pub fn external() {}\n");
}

#[gpui_kit::test]
fn closing_a_modified_tab_saves_it(cx: &mut TestAppContext) {
    let (window, ws) = open(cx);
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("close.txt");
    fs::write(&path, "before\n").unwrap();
    let editor = open_editor(cx, window, &ws, path.clone());
    edit(cx, window, &editor, "after\n");
    with(cx, window, &ws, |ws, w, cx| ws.close_tab(w, cx));
    assert_eq!(fs::read_to_string(&path).unwrap(), "after\n");
}

#[gpui_kit::test]
fn text_commands_edit_lines_and_comments(cx: &mut TestAppContext) {
    let (window, ws) = open(cx);
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("main.rs");
    fs::write(&path, "fn main() {\n    run();\n}\n").unwrap();
    let editor = open_editor(cx, window, &ws, path);
    let services = cx.read(|cx| ws.read(cx).services.clone());
    let run = |cx: &mut TestAppContext, command: EditorCommand| {
        let services = services.clone();
        with_editor(cx, window, &editor, |e, w, cx| {
            e.run_command(command, &services, w, cx)
        });
    };

    with_editor(cx, window, &editor, |e, w, cx| {
        e.go_to(TextPos::new(1, 6), w, cx)
    });
    run(cx, EditorCommand::ToggleLineComment);
    assert_eq!(text(cx, &editor), "fn main() {\n    // run();\n}\n");
    run(cx, EditorCommand::ToggleLineComment);
    assert_eq!(text(cx, &editor), "fn main() {\n    run();\n}\n");

    run(cx, EditorCommand::MoveLineUp);
    assert_eq!(text(cx, &editor), "    run();\nfn main() {\n}\n");
    run(cx, EditorCommand::MoveLineDown);
    assert_eq!(text(cx, &editor), "fn main() {\n    run();\n}\n");

    run(cx, EditorCommand::DuplicateLineDown);
    assert_eq!(
        text(cx, &editor),
        "fn main() {\n    run();\n    run();\n}\n"
    );
    // The caret followed the copy: deleting it removes the copy.
    assert_eq!(
        cx.read(|cx| editor.read(cx).caret(cx)).map(|p| p.line),
        Some(2)
    );
    run(cx, EditorCommand::DeleteLine);
    assert_eq!(text(cx, &editor), "fn main() {\n    run();\n}\n");

    run(cx, EditorCommand::ToggleBlockComment);
    assert!(text(cx, &editor).contains("/*  */"));
}

#[gpui_kit::test]
fn select_next_occurrence_adds_cursors_for_typing(cx: &mut TestAppContext) {
    let (window, ws) = open(cx);
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("occ.rs");
    fs::write(&path, "let a = 1;\nlet b = a + a;\nlet aa = 2;\n").unwrap();
    let editor = open_editor(cx, window, &ws, path);
    let services = cx.read(|cx| ws.read(cx).services.clone());
    focus(cx, window, &editor);
    with_editor(cx, window, &editor, |e, w, cx| {
        e.go_to(TextPos::new(0, 4), w, cx)
    });
    for _ in 0..3 {
        let services = services.clone();
        with_editor(cx, window, &editor, |e, w, cx| {
            e.run_command(EditorCommand::SelectNextOccurrence, &services, w, cx)
        });
    }
    // Three whole-word `a`s are selected (not `aa`); typing replaces all.
    cx.simulate_input(window, "x");
    cx.run_until_parked();
    assert_eq!(
        text(cx, &editor),
        "let x = 1;\nlet b = x + x;\nlet aa = 2;\n"
    );
}

#[gpui_kit::test]
fn editing_shortcuts_work_only_inside_the_editor(cx: &mut TestAppContext) {
    let (window, ws) = open(cx);
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("keys.rs");
    fs::write(&path, "fn a() {}\n").unwrap();
    let editor = open_editor(cx, window, &ws, path);
    focus(cx, window, &editor);
    let comment = if cfg!(target_os = "macos") {
        "cmd-/"
    } else {
        "ctrl-/"
    };
    cx.simulate_keystrokes(window, comment);
    cx.run_until_parked();
    assert_eq!(
        text(cx, &editor),
        "// fn a() {}\n",
        "VS Code's Ctrl+/ comments the line"
    );
    let save = if cfg!(target_os = "macos") {
        "cmd-s"
    } else {
        "ctrl-s"
    };
    cx.simulate_keystrokes(window, save);
    cx.run_until_parked();
    assert!(!dirty(cx, &editor), "Ctrl+S saves");
}
