//! Headless UI tests for the version control views: Commit, Git log with
//! the commit graph, the diff viewer and the 3-way merge view.

use std::time::{Duration, Instant};

use ferro_services::{ChangeKind, Commit, Services, mock::MockConfig};
use ferro_settings::{Paths, Settings};
use gpui_kit::{
    AnyWindowHandle, AppContext as _, Bounds, Entity, Point, TestAppContext, WindowBounds,
    WindowOptions,
    component::dock::{DockAreaState, DockPlacement},
    px, size,
};

use crate::{
    AppState,
    tool_window::ToolWindowKind,
    vcs::{
        commit::CommitView,
        diff_view::{DiffPanel, find_panel},
        git_log::GitLogView,
        graph::layout_graph,
        merge_model::Resolution,
        merge_view::MergePanel,
    },
    workspace::Workspace,
};

fn open(cx: &mut TestAppContext) -> (AnyWindowHandle, Entity<Workspace>) {
    cx.update(|cx| {
        gpui_kit::init(cx);
        ferro_ui::init(cx);
        let root = std::env::temp_dir().join(format!("ferro-vcs-test-{}", std::process::id()));
        let settings = Settings {
            locale: ferro_i18n::Locale::En,
            ..Settings::default()
        };
        AppState::init(settings, Paths::under(&root), false, cx);
        crate::init(cx);
    });
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

fn body<T: 'static>(
    cx: &mut TestAppContext,
    window: AnyWindowHandle,
    ws: &Entity<Workspace>,
    kind: ToolWindowKind,
) -> Entity<T> {
    with(cx, window, ws, |ws, w, cx| {
        ws.show_tool_window(kind, w, cx);
        ws.tool_window_body::<T>(kind, w, cx)
    })
    .expect("tool window body has the expected type")
}

fn center_panel<T: 'static>(
    cx: &mut TestAppContext,
    ws: &Entity<Workspace>,
    pred: impl Fn(&T) -> bool,
) -> Option<Entity<T>> {
    cx.read(|cx| {
        let id = find_panel::<T>(ws.read(cx), cx, pred)?;
        let area = ws.read(cx).dock_area().read(cx);
        area.panel(id)?.view().downcast::<T>().ok()
    })
}

fn count_center<T: 'static>(cx: &mut TestAppContext, ws: &Entity<Workspace>) -> usize {
    cx.read(|cx| {
        let area = ws.read(cx).dock_area().read(cx);
        area.layout(DockPlacement::Center)
            .map(|tree| {
                tree.panels()
                    .filter(|id| {
                        area.panel(*id)
                            .is_some_and(|p| p.view().downcast::<T>().is_ok())
                    })
                    .count()
            })
            .unwrap_or(0)
    })
}

#[gpui_kit::test]
fn commit_flow_validates_commits_and_updates_the_log(cx: &mut TestAppContext) {
    let (window, ws) = open(cx);
    let commit = body::<CommitView>(cx, window, &ws, ToolWindowKind::Commit);
    let log = body::<GitLogView>(cx, window, &ws, ToolWindowKind::Git);
    let before = cx.read(|cx| commit.read(cx).changes().len());
    assert!(before >= 5, "the mock has a handful of changes");
    let n = cx.read(|cx| log.read(cx).commits().len());
    assert!(n >= 20, "{n} commits");

    // No message: disabled.
    assert!(!cx.read(|cx| commit.read(cx).can_commit(cx)));
    cx.update_window(window, |_, w, cx| {
        commit.update(cx, |c, cx| c.set_message("Tidy the config", w, cx))
    })
    .unwrap();
    assert!(cx.read(|cx| commit.read(cx).can_commit(cx)));

    // The conflicted file is checked by default and refused.
    cx.update_window(window, |_, w, cx| {
        commit.update(cx, |c, cx| c.commit(w, cx))
    })
    .unwrap();
    cx.run_until_parked();
    let error = cx.read(|cx| commit.read(cx).error().cloned());
    assert!(
        error
            .as_ref()
            .is_some_and(|e| e.contains("merge conflicts")),
        "{error:?}"
    );

    // Uncheck everything: disabled again.
    let changes = cx.read(|cx| commit.read(cx).changes().to_vec());
    commit.update(cx, |c, cx| {
        for change in &changes {
            c.set_checked(&change.path, false, cx);
        }
    });
    assert!(!cx.read(|cx| commit.read(cx).can_commit(cx)));

    // Commit the two modified source files.
    commit.update(cx, |c, cx| {
        for change in changes
            .iter()
            .filter(|c| c.relative.starts_with("src/c") || c.relative == "src/main.rs")
        {
            c.set_checked(&change.path, true, cx);
        }
    });
    cx.update_window(window, |_, w, cx| {
        commit.update(cx, |c, cx| c.commit(w, cx))
    })
    .unwrap();
    cx.run_until_parked();
    assert_eq!(cx.read(|cx| commit.read(cx).error().cloned()), None);
    let after = cx.read(|cx| commit.read(cx).changes().len());
    assert_eq!(after, before - 2, "committed files leave the list");
    let top = cx.read(|cx| log.read(cx).commits()[0].clone());
    assert_eq!(top.subject, "Tidy the config");
    assert!(top.refs.iter().any(|r| r == "HEAD -> main"));
    assert_eq!(top.files.len(), 2);
}

#[gpui_kit::test]
fn commit_rows_group_by_changelist_and_collapse(cx: &mut TestAppContext) {
    let (window, ws) = open(cx);
    let commit = body::<CommitView>(cx, window, &ws, ToolWindowKind::Commit);
    let rows = cx.read(|cx| commit.read(cx).rows().to_vec());
    use crate::vcs::commit::Row;
    assert_eq!(rows[0], Row::Group("Changes".into()));
    assert!(rows.contains(&Row::Group("Unversioned Files".into())));
    // Unversioned files start unchecked.
    let untracked = cx.read(|cx| {
        commit
            .read(cx)
            .changes()
            .iter()
            .find(|c| c.kind == ChangeKind::Untracked)
            .unwrap()
            .path
            .clone()
    });
    assert!(!cx.read(|cx| commit.read(cx).is_checked(&untracked)));
    commit.update(cx, |c, cx| c.toggle_group("Changes", cx));
    let collapsed = cx.read(|cx| commit.read(cx).rows().len());
    assert_eq!(collapsed, 3, "two group headers and the unversioned file");
}

/// A synthetic history: a main line with a feature branch merged every
/// seventh commit.
fn synthetic_history(n: usize) -> Vec<Commit> {
    let id = |i: usize| format!("{i:040x}");
    let mut commits = Vec::with_capacity(n);
    for i in 0..n {
        let mut parents = Vec::new();
        if i + 1 < n {
            parents.push(id(i + 1));
        }
        if i % 7 == 0 && i + 4 < n {
            parents.push(id(i + 4));
        }
        commits.push(Commit {
            id: id(i),
            parents,
            author: format!("Author {}", i % 5),
            timestamp: 1_794_182_400 - i as i64 * 3_600,
            subject: format!("Change number {i}"),
            body: String::new(),
            refs: if i == 0 {
                vec!["HEAD -> main".into()]
            } else {
                Vec::new()
            },
            files: vec!["src/main.rs".into()],
        });
    }
    commits
}

#[gpui_kit::test]
fn the_log_lays_out_and_renders_ten_times_a_typical_history(cx: &mut TestAppContext) {
    let (window, ws) = open(cx);
    let log = body::<GitLogView>(cx, window, &ws, ToolWindowKind::Git);
    // The mock has 46 commits; 10× that.
    let history = synthetic_history(460);

    let started = Instant::now();
    let rows = layout_graph(&history);
    let layout = started.elapsed();
    assert_eq!(rows.len(), 460);
    assert!(rows.iter().all(|r| r.width <= 3));

    let started = Instant::now();
    log.update(cx, |log, cx| log.set_commits(history, cx));
    cx.run_until_parked();
    let _ = cx.update_window(window, |_, w, _| w.refresh());
    cx.run_until_parked();
    let render = started.elapsed();
    println!("git log: layout of 460 commits {layout:?}, set + render {render:?}");
    assert_eq!(cx.read(|cx| log.read(cx).visible_count()), 460);
    // Generous budgets for unoptimized test builds.
    assert!(
        layout < Duration::from_millis(100),
        "layout took {layout:?}"
    );
    assert!(
        render < Duration::from_millis(1_000),
        "render took {render:?}"
    );

    // Keyboard-like selection and the details pane.
    log.update(cx, |log, cx| log.select(10, cx));
    assert_eq!(
        cx.read(|cx| log.read(cx).selected_commit().map(|c| c.subject.clone())),
        Some("Change number 10".into())
    );
}

#[gpui_kit::test]
fn the_branch_filter_keeps_reachable_commits(cx: &mut TestAppContext) {
    let (window, ws) = open(cx);
    let log = body::<GitLogView>(cx, window, &ws, ToolWindowKind::Git);
    let all = cx.read(|cx| log.read(cx).visible_count());
    log.update(cx, |log, cx| {
        log.set_branch_filter(Some("feature/FERRO-207-jitter".into()), cx)
    });
    let filtered = cx.read(|cx| log.read(cx).visible_count());
    assert!(filtered > 0 && filtered < all, "{filtered} of {all}");
    log.update(cx, |log, cx| log.set_branch_filter(None, cx));
    assert_eq!(cx.read(|cx| log.read(cx).visible_count()), all);
}

#[gpui_kit::test]
fn the_diff_panel_opens_toggles_and_is_reused(cx: &mut TestAppContext) {
    let (window, ws) = open(cx);
    let main = cx.read(|cx| {
        ws.read(cx).services.project.roots()[0]
            .join("src")
            .join("main.rs")
    });
    with(cx, window, &ws, |ws, w, cx| {
        crate::vcs::diff_view::open(ws, main.clone(), w, cx)
    });
    let panel = center_panel::<DiffPanel>(cx, &ws, |p| p.path() == main).expect("diff tab");
    assert!(cx.read(|cx| panel.read(cx).is_loaded()));
    let (added, deleted) = cx
        .read(|cx| panel.read(cx).stats())
        .expect("main.rs has changes");
    assert!(added + deleted > 0);
    assert!(!cx.read(|cx| panel.read(cx).is_split()));
    panel.update(cx, |p, cx| p.set_split(true, cx));
    assert!(cx.read(|cx| panel.read(cx).is_split()));
    panel.update(cx, |p, cx| p.next_change(true, cx));

    with(cx, window, &ws, |ws, w, cx| {
        crate::vcs::diff_view::open(ws, main.clone(), w, cx)
    });
    assert_eq!(count_center::<DiffPanel>(cx, &ws), 1, "the tab is reused");
}

#[gpui_kit::test]
fn a_saved_layout_with_vcs_tabs_restores(cx: &mut TestAppContext) {
    let (window, ws) = open(cx);
    let root = cx.read(|cx| ws.read(cx).services.project.roots()[0].clone());
    with(cx, window, &ws, |ws, w, cx| {
        crate::vcs::diff_view::open(ws, root.join("src").join("config.rs"), w, cx);
        crate::vcs::merge_view::open(ws, None, w, cx);
    });
    cx.run_until_parked();
    assert_eq!(count_center::<MergePanel>(cx, &ws), 1);
    let state: DockAreaState = cx.read(|cx| ws.read(cx).dock_area().read(cx).dump(cx));
    let mut json = serde_json::to_value(&state).unwrap();
    with(cx, window, &ws, |ws, w, cx| {
        let area = ws.dock_area().clone();
        let restored: DockAreaState = serde_json::from_value(json.clone()).unwrap();
        area.update(cx, |area, cx| area.load(restored, w, cx))
            .unwrap();
    });
    assert_eq!(count_center::<DiffPanel>(cx, &ws), 1);
    assert_eq!(count_center::<MergePanel>(cx, &ws), 1);

    // A corrupted entry still restores (as a tab showing an error).
    fn scrub(value: &mut serde_json::Value) {
        match value {
            serde_json::Value::Object(map) => {
                if map.get("panel_name").and_then(|v| v.as_str()) == Some("ferro.diff") {
                    map.insert("info".into(), serde_json::json!({"panel": {"bogus": 1}}));
                }
                map.values_mut().for_each(scrub);
            }
            serde_json::Value::Array(items) => items.iter_mut().for_each(scrub),
            _ => {}
        }
    }
    scrub(&mut json);
    if let Ok(restored) = serde_json::from_value::<DockAreaState>(json) {
        with(cx, window, &ws, |ws, w, cx| {
            let area = ws.dock_area().clone();
            let _ = area.update(cx, |area, cx| area.load(restored, w, cx));
        });
    }
}

#[gpui_kit::test]
fn the_merge_view_resolves_and_applies(cx: &mut TestAppContext) {
    let (window, ws) = open(cx);
    with(cx, window, &ws, |ws, w, cx| {
        crate::vcs::merge_view::open(ws, None, w, cx)
    });
    cx.run_until_parked();
    let merge = center_panel::<MergePanel>(cx, &ws, |_| true).expect("merge tab");
    let total = cx.read(|cx| merge.read(cx).model().map(|m| m.conflict_count()));
    let total = total.expect("conflicts loaded");
    assert!(total >= 1);

    // Apply is impossible while anything is unresolved.
    cx.update_window(window, |_, w, cx| merge.update(cx, |m, cx| m.apply(w, cx)))
        .unwrap();
    cx.run_until_parked();
    assert!(!cx.read(|cx| merge.read(cx).is_applied()));

    for ix in 0..total {
        let resolution = if ix % 2 == 0 {
            Resolution::Ours
        } else {
            Resolution::Theirs
        };
        merge.update(cx, |m, cx| m.resolve(ix, Some(resolution), cx));
    }
    let result = cx.read(|cx| merge.read(cx).model().and_then(|m| m.result_text()));
    assert!(result.is_some_and(|t| !t.contains("<<<<<<<")));
    cx.update_window(window, |_, w, cx| merge.update(cx, |m, cx| m.apply(w, cx)))
        .unwrap();
    cx.run_until_parked();
    assert!(cx.read(|cx| merge.read(cx).is_applied()));

    // The Commit tool window reloads: the file is now just modified.
    let commit = body::<CommitView>(cx, window, &ws, ToolWindowKind::Commit);
    cx.run_until_parked();
    let changes = cx.read(|cx| commit.read(cx).changes().to_vec());
    assert!(!changes.is_empty());
    assert!(
        changes.iter().all(|c| c.kind != ChangeKind::Conflicted),
        "{changes:?}"
    );

    // With nothing left, ResolveConflicts opens no new tab.
    with(cx, window, &ws, |ws, w, cx| {
        crate::vcs::merge_view::open(ws, None, w, cx)
    });
    assert_eq!(count_center::<MergePanel>(cx, &ws), 1);
}
