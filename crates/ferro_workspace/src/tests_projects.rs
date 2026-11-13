//! Headless tests for Stage 4 in the window: a project opened from disk,
//! trust and safe mode, project settings with live reload, root folders, and
//! the workspace state kept in the state database.

use std::{
    fs,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use ferro_services::{RunState, TextPos};
use ferro_settings::{Paths, Settings};
use gpui_kit::{AnyWindowHandle, AppContext as _, Entity, TestAppContext};

use crate::{
    AppState, panels::project::ProjectTree, projects, tool_window::ToolWindowKind,
    workspace::Workspace,
};

fn setup(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    cx.update(|cx| {
        gpui_kit::init(cx);
        ferro_ui::init(cx);
        let root = std::env::temp_dir().join(format!("ferro-projects-test-{}", std::process::id()));
        let settings = Settings {
            locale: ferro_i18n::Locale::En,
            ..Settings::default()
        };
        AppState::init(settings, Paths::under(&root), false, cx);
        crate::init(cx);
    });
}

fn write(root: &Path, rel: &str, text: &str) {
    let path = root.join(rel);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, text).unwrap();
}

fn sample() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), "README.md", "# Sample\n\nRead me.\n");
    write(
        dir.path(),
        "src/main.rs",
        "fn main() {\n    let x = 1;\n    println!(\"{x}\");\n}\n",
    );
    write(dir.path(), ".gitignore", "/target\n");
    write(dir.path(), "target/out.txt", "build output\n");
    dir
}

/// Run the app until `done`, giving real file-system threads time to work.
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
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn open_project(
    cx: &mut TestAppContext,
    roots: Vec<PathBuf>,
) -> (AnyWindowHandle, Entity<Workspace>) {
    let opened = cx
        .update(|cx| projects::open_project(roots, cx))
        .expect("the project opens");
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

fn root_of(cx: &mut TestAppContext, ws: &Entity<Workspace>) -> PathBuf {
    cx.read(|cx| ws.read(cx).services.project.roots()[0].clone())
}

fn project_tree(
    cx: &mut TestAppContext,
    window: AnyWindowHandle,
    ws: &Entity<Workspace>,
) -> Entity<ProjectTree> {
    with(cx, window, ws, |ws, w, cx| {
        ws.tool_window_body::<ProjectTree>(ToolWindowKind::Project, w, cx)
    })
    .expect("the Project tool window")
}

#[gpui_kit::test]
fn a_folder_opens_on_the_real_file_system(cx: &mut TestAppContext) {
    setup(cx);
    let dir = sample();
    let (window, ws) = open_project(cx, vec![dir.path().to_path_buf()]);
    let root = root_of(cx, &ws);

    // The project is remembered for the welcome screen.
    let recent = cx.read(|cx| AppState::global(cx).db.recent_projects().unwrap());
    assert_eq!(recent[0].path, root);

    // A new window shows the README and the tree without ignored files.
    let editors: Vec<PathBuf> = cx.read(|cx| {
        ws.read(cx)
            .editors
            .iter()
            .map(|e| e.read(cx).path().to_path_buf())
            .collect()
    });
    assert_eq!(editors, [root.join("README.md")]);
    let tree = project_tree(cx, window, &ws);
    wait_until(cx, "the project tree", |cx| {
        cx.read(|cx| tree.read(cx).row_names()).len() >= 4
    });
    let names = cx.read(|cx| tree.read(cx).row_names());
    assert!(names.iter().any(|n| n == "src"), "{names:?}");
    assert!(names.iter().any(|n| n == "README.md"), "{names:?}");
    assert!(!names.iter().any(|n| n == "target"), "{names:?}");

    // Quick Open searches the real files.
    let search = cx.read(|cx| ws.read(cx).services.search.clone());
    let found = smol::block_on(search.find_files("main", 10)).unwrap();
    assert_eq!(found[0].path, root.join("src/main.rs"));
}

#[gpui_kit::test]
fn an_untrusted_project_runs_nothing_until_trusted(cx: &mut TestAppContext) {
    setup(cx);
    let dir = sample();
    let (window, ws) = open_project(cx, vec![dir.path().to_path_buf()]);
    let root = root_of(cx, &ws);
    let trusted =
        |cx: &mut TestAppContext| cx.read(|cx| projects::is_trusted(&ws.read(cx).services, cx));
    assert!(!trusted(cx), "a new folder opens in safe mode");

    with(cx, window, &ws, |ws, w, cx| ws.start_run(false, w, cx));
    assert_eq!(cx.read(|cx| ws.read(cx).run_state), RunState::Idle);

    cx.update(|cx| {
        let services = ws.read(cx).services.clone();
        projects::trust(&services, cx);
    });
    assert!(trusted(cx));
    assert!(cx.read(|cx| {
        AppState::global(cx)
            .db
            .is_trusted(&root.join("src"))
            .unwrap()
    }));

    // Reopening a trusted folder does not ask again.
    cx.update_window(window, |_, w, _| w.remove_window())
        .unwrap();
    let (_, ws) = open_project(cx, vec![dir.path().to_path_buf()]);
    assert!(cx.read(|cx| projects::is_trusted(&ws.read(cx).services, cx)));
}

#[gpui_kit::test]
fn project_settings_override_the_editor_and_reload_live(cx: &mut TestAppContext) {
    setup(cx);
    let dir = sample();
    write(
        dir.path(),
        ".ferroide/settings.toml",
        "[editor]\ntab_size = 2\n",
    );
    let (_, ws) = open_project(cx, vec![dir.path().to_path_buf()]);
    let root = root_of(cx, &ws);
    let main = root.join("src/main.rs");
    let tab_size = |cx: &mut TestAppContext, path: &Path| {
        cx.read(|cx| AppState::editor_settings(path, cx).tab_size)
    };
    assert_eq!(tab_size(cx, &main), 2);
    // Files outside the project keep the global settings.
    assert_eq!(tab_size(cx, Path::new("/elsewhere/x.rs")), 4);

    write(&root, ".ferroide/settings.toml", "[editor]\ntab_size = 8\n");
    wait_until(cx, "the project settings to reload", |cx| {
        tab_size(cx, &main) == 8
    });
}

#[gpui_kit::test]
fn folders_can_be_added_to_and_removed_from_a_project(cx: &mut TestAppContext) {
    setup(cx);
    let dir = sample();
    let other = tempfile::tempdir().unwrap();
    write(other.path(), "lib/util.rs", "pub fn util() {}\n");
    let (window, ws) = open_project(cx, vec![dir.path().to_path_buf()]);
    let tree = project_tree(cx, window, &ws);
    let project = cx.read(|cx| ws.read(cx).services.project.clone());

    let added = project.add_root(other.path()).unwrap();
    let name = added.file_name().unwrap().to_string_lossy().into_owned();
    wait_until(cx, "the second root in the tree", |cx| {
        cx.read(|cx| tree.read(cx).row_names()).contains(&name)
    });

    project.remove_root(&added).unwrap();
    wait_until(cx, "the second root to leave the tree", |cx| {
        !cx.read(|cx| tree.read(cx).row_names()).contains(&name)
    });
}

#[gpui_kit::test]
fn layout_history_and_carets_survive_reopening(cx: &mut TestAppContext) {
    setup(cx);
    let dir = sample();
    let (window, ws) = open_project(cx, vec![dir.path().to_path_buf()]);
    let root = root_of(cx, &ws);
    let main = root.join("src/main.rs");

    with(cx, window, &ws, |ws, w, cx| {
        ws.open_location(
            &crate::requests::OpenLocation::at(main.clone(), TextPos::new(2, 4)),
            w,
            cx,
        )
    });
    let loaded = |cx: &mut TestAppContext, ws: &Entity<Workspace>| {
        cx.read(|cx| ws.read(cx).editors.iter().all(|e| e.read(cx).is_loaded()))
    };
    wait_until(cx, "the editors to load", |cx| loaded(cx, &ws));
    with(cx, window, &ws, |ws, w, cx| {
        ws.open_file(root.join("README.md"), w, cx)
    });
    assert!(cx.read(|cx| ws.read(cx).can_go_back()));
    cx.read(|cx| ws.read(cx).save_layout(cx));
    cx.update_window(window, |_, w, _| w.remove_window())
        .unwrap();
    cx.run_until_parked();

    let (_, ws) = open_project(cx, vec![dir.path().to_path_buf()]);
    wait_until(cx, "the restored editors to load", |cx| loaded(cx, &ws));
    let carets: Vec<(PathBuf, Option<TextPos>)> = cx.read(|cx| {
        ws.read(cx)
            .editors
            .iter()
            .map(|e| (e.read(cx).path().to_path_buf(), e.read(cx).caret(cx)))
            .collect()
    });
    assert!(
        carets.contains(&(main.clone(), Some(TextPos::new(2, 4)))),
        "{carets:?}"
    );
    assert!(cx.read(|cx| ws.read(cx).can_go_back()), "Back survives");
    let recent = cx.read(|cx| {
        AppState::global(cx)
            .db
            .recent_files(ws.read(cx).project_key())
            .unwrap()
    });
    assert!(recent.contains(&main), "{recent:?}");
}
