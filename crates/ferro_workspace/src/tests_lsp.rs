//! Headless tests for Stage 6 in the window: a project on disk served by
//! the in-process fake language server (`ferro_lsp::testing`) — pushed
//! diagnostics reaching the editor and the status bar, Go to Definition,
//! Find Usages in the locations list, Format Document, saving, edits to
//! files that are not open, and safe mode.

use std::{
    fs,
    path::{Path, PathBuf},
    sync::{Arc, atomic::Ordering},
    time::{Duration, Instant},
};

use ferro_lsp::testing::{FakeLauncher, FakeState};
use ferro_services::{ServerHealth, TextEdit, TextPos, TextRange};
use ferro_settings::{Paths, Settings};
use gpui_kit::{AnyWindowHandle, AppContext as _, Entity, TestAppContext};

use crate::{
    AppState,
    editor::{EditorCommand, EditorPanel},
    projects,
    workspace::Workspace,
};

const SOURCE: &str = "fn main() {\n    greet(\"x\"); bad\n}\nfn greet(name: &str) {}\n";

fn setup(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    cx.update(|cx| {
        gpui_kit::init(cx);
        ferro_ui::init(cx);
        let root = std::env::temp_dir().join(format!("ferro-lsp-test-{}", std::process::id()));
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
        // Analysis and refreshes wait on timers of the simulated clock.
        cx.executor().advance_clock(Duration::from_millis(100));
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

struct Opened {
    window: AnyWindowHandle,
    ws: Entity<Workspace>,
    state: Arc<FakeState>,
    root: PathBuf,
    _dir: tempfile::TempDir,
}

fn open(cx: &mut TestAppContext, trusted: bool) -> Opened {
    setup(cx);
    let dir = tempfile::tempdir().unwrap();
    let root = PathBuf::from(
        dir.path()
            .canonicalize()
            .unwrap()
            .to_string_lossy()
            .trim_start_matches(r"\\?\"),
    );
    write(&root, "Cargo.toml", "[package]\nname = \"demo\"\n");
    write(&root, "src/main.rs", SOURCE);
    write(&root, "src/other.rs", "fn other() {}\n");
    let state = Arc::new(FakeState::default());
    let launcher = Arc::new(FakeLauncher {
        state: state.clone(),
    });
    let (window, ws) = cx.update(|cx| {
        let (services, language) =
            projects::project_services_with(vec![root.clone()], crate::lsp::options(cx), launcher)
                .unwrap();
        crate::lsp::register(&language, cx);
        services.language.clone().set_trusted(trusted);
        crate::open_workspace(services, cx).unwrap()
    });
    cx.run_until_parked();
    Opened {
        window,
        ws,
        state,
        root,
        _dir: dir,
    }
}

fn open_file(cx: &mut TestAppContext, o: &Opened, path: PathBuf) -> Entity<EditorPanel> {
    cx.update_window(o.window, |_, w, cx| {
        o.ws.update(cx, |ws, cx| ws.open_file(path, w, cx))
    })
    .unwrap();
    let editor = cx
        .read(|cx| o.ws.read(cx).active_editor_entity())
        .expect("an active editor");
    wait_until(cx, "the file", |cx| {
        cx.read(|cx| editor.read(cx).is_loaded())
    });
    editor
}

fn with_editor<R>(
    cx: &mut TestAppContext,
    o: &Opened,
    editor: &Entity<EditorPanel>,
    f: impl FnOnce(&mut EditorPanel, &mut gpui_kit::Window, &mut gpui_kit::Context<EditorPanel>) -> R,
) -> R {
    let result = cx
        .update_window(o.window, |_, window, cx| {
            editor.update(cx, |e, cx| f(e, window, cx))
        })
        .unwrap();
    cx.run_until_parked();
    result
}

fn command(cx: &mut TestAppContext, o: &Opened, command: EditorCommand) {
    cx.update_window(o.window, |_, w, cx| {
        o.ws.update(cx, |ws, cx| ws.editor_command(command, w, cx))
    })
    .unwrap();
    cx.run_until_parked();
}

#[gpui_kit::test]
fn the_inner_loop_runs_on_a_language_server(cx: &mut TestAppContext) {
    let o = open(cx, true);
    let main = o.root.join("src").join("main.rs");
    let editor = open_file(cx, &o, main.clone());

    // didOpen → publishDiagnostics → event → the editor re-reads them.
    wait_until(cx, "pushed diagnostics", |cx| {
        cx.read(|cx| {
            editor
                .read(cx)
                .analysis()
                .diagnostics
                .iter()
                .any(|d| d.source == "fake")
        })
    });
    assert_eq!(cx.read(|cx| editor.read(cx).problem_counts()), (0, 1));
    let servers = cx.read(|cx| o.ws.read(cx).services.status.language_servers());
    assert_eq!(servers.len(), 1);
    assert_eq!(servers[0].name, "rust-analyzer");
    assert_eq!(servers[0].health, ServerHealth::Ready);
    assert_eq!(servers[0].language, "Rust");

    // Go to Definition from the call jumps to `fn greet`.
    with_editor(cx, &o, &editor, |e, w, cx| {
        e.go_to(TextPos::new(1, 6), w, cx)
    });
    command(cx, &o, EditorCommand::GoToDefinition);
    wait_until(cx, "the jump", |cx| {
        cx.read(|cx| editor.read(cx).caret(cx).map(|c| c.line)) == Some(3)
    });

    // Find Usages lists the call and the definition.
    command(cx, &o, EditorCommand::FindUsages);
    wait_until(cx, "the usages", |cx| {
        cx.read(|cx| {
            o.ws.read(cx)
                .locations
                .as_ref()
                .and_then(|v| v.upgrade())
                .is_some()
        })
    });
    let view = cx.read(|cx| o.ws.read(cx).locations.clone().unwrap().upgrade().unwrap());
    let lines: Vec<u32> = cx.read(|cx| {
        view.read(cx)
            .items()
            .iter()
            .map(|i| i.location.range.start.line)
            .collect()
    });
    assert_eq!(lines, [1, 3]);
    cx.update_window(o.window, |_, w, cx| {
        use gpui_kit::component::WindowExt as _;
        w.close_dialog(cx)
    })
    .unwrap();

    // Format Document applies the server's edits.
    with_editor(cx, &o, &editor, |e, w, cx| {
        e.replace_text("fn main() {   \n}  \n".into(), w, cx)
    });
    command(cx, &o, EditorCommand::FormatDocument);
    wait_until(cx, "the formatting", |cx| {
        cx.read(|cx| editor.read(cx).text(cx)).as_deref() == Some("fn main() {\n}\n")
    });

    // Saving notifies the server.
    with_editor(cx, &o, &editor, |e, _, cx| e.save(cx).detach());
    wait_until(cx, "didSave", |_| !o.state.saved.lock().unwrap().is_empty());

    // Edits to a file that is not open are written to disk.
    let other = o.root.join("src").join("other.rs");
    cx.update_window(o.window, |_, w, cx| {
        o.ws.update(cx, |ws, cx| {
            ws.apply_workspace_edit(
                &[(
                    other.clone(),
                    vec![TextEdit {
                        range: TextRange::on_line(0, 3, 8),
                        new_text: "renamed".into(),
                    }],
                )],
                true,
                w,
                cx,
            )
        })
    })
    .unwrap();
    wait_until(cx, "the file on disk", |_| {
        fs::read_to_string(&other).unwrap() == "fn renamed() {}\n"
    });
}

#[gpui_kit::test]
fn an_untrusted_project_starts_no_server_until_trusted(cx: &mut TestAppContext) {
    let o = open(cx, false);
    let main = o.root.join("src").join("main.rs");
    let editor = open_file(cx, &o, main);
    wait_until(cx, "the outline from tree-sitter", |cx| {
        cx.read(|cx| !editor.read(cx).analysis().symbols.is_empty())
    });
    assert_eq!(o.state.launches.load(Ordering::SeqCst), 0);
    assert!(
        cx.read(|cx| o.ws.read(cx).services.status.language_servers())
            .is_empty()
    );

    let services = cx.read(|cx| o.ws.read(cx).services.clone());
    cx.update(|cx| projects::trust(&services, cx));
    // Trust refreshes the open editors, which start the server.
    wait_until(cx, "the server", |_| {
        o.state.launches.load(Ordering::SeqCst) == 1
    });
    wait_until(cx, "pushed diagnostics", |cx| {
        cx.read(|cx| {
            editor
                .read(cx)
                .analysis()
                .diagnostics
                .iter()
                .any(|d| d.source == "fake")
        })
    });
}
