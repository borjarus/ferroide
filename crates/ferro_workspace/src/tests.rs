//! Headless UI tests for the Stage 1 Definition of Done: the dock, tool
//! windows, editor tabs and splits, the theme-switch budget and layout
//! persistence — exercised through the real views on mock services.

use std::time::Duration;

use ferro_services::{Services, mock::MockConfig};
use ferro_settings::{Paths, Settings, themes};
use gpui_kit::{
    AnyWindowHandle, AppContext as _, Bounds, Entity, Point, TestAppContext, WindowBounds,
    WindowOptions,
    component::{
        Placement,
        dock::{DockAreaState, DockPlacement, PaneRef},
    },
    px, size,
};

use crate::{AppState, actions, tool_window::ToolWindowKind, workspace::Workspace};

fn setup(cx: &mut TestAppContext) {
    cx.update(|cx| {
        gpui_kit::init(cx);
        ferro_ui::init(cx);
        let root = std::env::temp_dir().join(format!("ferro-ws-test-{}", std::process::id()));
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

fn visible(cx: &mut TestAppContext, ws: &Entity<Workspace>, kind: ToolWindowKind) -> bool {
    cx.read(|cx| ws.read(cx).is_tool_window_visible(kind, cx))
}

fn dock_open(cx: &mut TestAppContext, ws: &Entity<Workspace>, placement: DockPlacement) -> bool {
    cx.read(|cx| ws.read(cx).dock_area().read(cx).is_dock_open(placement))
}

#[gpui_kit::test]
fn every_catalog_action_is_a_registered_gpui_action(cx: &mut TestAppContext) {
    setup(cx);
    let missing = cx.read(actions::missing_actions);
    assert!(
        missing.is_empty(),
        "catalog actions without a GPUI type: {missing:?}"
    );
}

#[gpui_kit::test]
fn default_layout_matches_intellij(cx: &mut TestAppContext) {
    let (_, ws) = open(cx);
    assert!(visible(cx, &ws, ToolWindowKind::Project));
    assert!(!visible(cx, &ws, ToolWindowKind::Terminal));
    assert!(dock_open(cx, &ws, DockPlacement::Left));
    assert!(!dock_open(cx, &ws, DockPlacement::Bottom));
    assert!(!dock_open(cx, &ws, DockPlacement::Right));
    let editors = cx.read(|cx| ws.read(cx).editors.len());
    assert_eq!(editors, 3);
}

#[gpui_kit::test]
fn stripe_toggles_hide_show_and_switch_tool_windows(cx: &mut TestAppContext) {
    let (window, ws) = open(cx);

    // Project is visible: its stripe hides it, and shows it again.
    with(cx, window, &ws, |ws, w, cx| {
        ws.toggle_tool_window(ToolWindowKind::Project, w, cx)
    });
    assert!(!visible(cx, &ws, ToolWindowKind::Project));
    assert!(!dock_open(cx, &ws, DockPlacement::Left));
    with(cx, window, &ws, |ws, w, cx| {
        ws.toggle_tool_window(ToolWindowKind::Project, w, cx)
    });
    assert!(visible(cx, &ws, ToolWindowKind::Project));

    // Terminal opens the bottom dock; Run takes its place in the same dock.
    with(cx, window, &ws, |ws, w, cx| {
        ws.toggle_tool_window(ToolWindowKind::Terminal, w, cx)
    });
    assert!(dock_open(cx, &ws, DockPlacement::Bottom));
    assert!(visible(cx, &ws, ToolWindowKind::Terminal));
    with(cx, window, &ws, |ws, w, cx| {
        ws.toggle_tool_window(ToolWindowKind::Run, w, cx)
    });
    assert!(visible(cx, &ws, ToolWindowKind::Run));
    assert!(!visible(cx, &ws, ToolWindowKind::Terminal));
    with(cx, window, &ws, |ws, w, cx| {
        ws.toggle_tool_window(ToolWindowKind::Run, w, cx)
    });
    assert!(!dock_open(cx, &ws, DockPlacement::Bottom));

    // A right-side tool window.
    with(cx, window, &ws, |ws, w, cx| {
        ws.toggle_tool_window(ToolWindowKind::Database, w, cx)
    });
    assert!(dock_open(cx, &ws, DockPlacement::Right));
    assert!(visible(cx, &ws, ToolWindowKind::Database));
}

#[gpui_kit::test]
fn hide_all_tool_windows_restores_what_it_hid(cx: &mut TestAppContext) {
    let (window, ws) = open(cx);
    with(cx, window, &ws, |ws, w, cx| {
        ws.toggle_tool_window(ToolWindowKind::Git, w, cx)
    });
    assert!(dock_open(cx, &ws, DockPlacement::Left) && dock_open(cx, &ws, DockPlacement::Bottom));

    with(cx, window, &ws, |ws, w, cx| ws.hide_all_tool_windows(w, cx));
    for p in [
        DockPlacement::Left,
        DockPlacement::Bottom,
        DockPlacement::Right,
    ] {
        assert!(!dock_open(cx, &ws, p), "{p:?} should be hidden");
    }
    with(cx, window, &ws, |ws, w, cx| ws.hide_all_tool_windows(w, cx));
    assert!(dock_open(cx, &ws, DockPlacement::Left));
    assert!(dock_open(cx, &ws, DockPlacement::Bottom));
    assert!(!dock_open(cx, &ws, DockPlacement::Right));
}

#[gpui_kit::test]
fn opening_files_reuses_tabs_and_closing_respects_pins(cx: &mut TestAppContext) {
    let (window, ws) = open(cx);
    let root = cx.read(|cx| ws.read(cx).services.project.roots()[0].clone());

    with(cx, window, &ws, |ws, w, cx| {
        ws.open_file(root.join("README.md"), w, cx)
    });
    with(cx, window, &ws, |ws, w, cx| {
        ws.open_file(root.join("README.md"), w, cx)
    });
    let count = cx.read(|cx| ws.read(cx).editors.len());
    assert_eq!(count, 4, "the same file must not open twice");

    // The opened file is the active editor; pin it, then close everything.
    with(cx, window, &ws, |ws, w, cx| ws.toggle_pin(w, cx));
    with(cx, window, &ws, |ws, w, cx| ws.close_all_tabs(w, cx));
    let left: Vec<String> = cx.read(|cx| {
        ws.read(cx)
            .editors
            .iter()
            .map(|e| e.read(cx).file_name().to_string())
            .collect()
    });
    assert_eq!(
        left,
        vec!["README.md".to_string()],
        "only the pinned tab survives"
    );

    // Close Tab refuses the pinned tab; unpinning allows it.
    with(cx, window, &ws, |ws, w, cx| ws.close_tab(w, cx));
    assert_eq!(cx.read(|cx| ws.read(cx).editors.len()), 1);
    with(cx, window, &ws, |ws, w, cx| ws.toggle_pin(w, cx));
    with(cx, window, &ws, |ws, w, cx| ws.close_tab(w, cx));
    assert_eq!(cx.read(|cx| ws.read(cx).editors.len()), 0);
}

#[gpui_kit::test]
fn the_tab_limit_closes_the_least_recently_used_tab(cx: &mut TestAppContext) {
    let (window, ws) = open(cx);
    cx.update(|cx| AppState::update_settings(cx, |s| s.editor.tab_limit = 3));
    let root = cx.read(|cx| ws.read(cx).services.project.roots()[0].clone());
    for rel in ["README.md", "src/config.rs", "tests/smoke.rs"] {
        with(cx, window, &ws, |ws, w, cx| {
            ws.open_file(root.join(rel), w, cx)
        });
    }
    let names: Vec<String> = cx.read(|cx| {
        ws.read(cx)
            .editors
            .iter()
            .map(|e| e.read(cx).file_name().to_string())
            .collect()
    });
    assert_eq!(names, vec!["README.md", "config.rs", "smoke.rs"]);
}

#[gpui_kit::test]
fn split_right_creates_a_second_tab_group(cx: &mut TestAppContext) {
    let (window, ws) = open(cx);
    let groups = |cx: &mut TestAppContext| {
        cx.read(|cx| {
            let area = ws.read(cx).dock_area().read(cx);
            let mut count = 0;
            area.layout(DockPlacement::Center)
                .unwrap()
                .root()
                .walk(&mut |node| {
                    if matches!(node.kind(), PaneRef::Tabs { .. }) {
                        count += 1;
                    }
                });
            count
        })
    };
    assert_eq!(groups(cx), 1);
    with(cx, window, &ws, |ws, w, cx| {
        ws.split(Placement::Right, w, cx)
    });
    assert_eq!(groups(cx), 2);
    assert_eq!(cx.read(|cx| ws.read(cx).editors.len()), 4);

    // Next/previous tab cycles within the active group without panicking.
    with(cx, window, &ws, |ws, w, cx| ws.cycle_tab(true, w, cx));
    with(cx, window, &ws, |ws, w, cx| ws.cycle_tab(false, w, cx));
}

#[gpui_kit::test]
fn maximize_zooms_and_restores(cx: &mut TestAppContext) {
    let (window, ws) = open(cx);
    with(cx, window, &ws, |ws, w, cx| {
        ws.toggle_tool_window(ToolWindowKind::Problems, w, cx)
    });
    with(cx, window, &ws, |ws, w, cx| ws.toggle_maximize(w, cx));
    assert!(cx.read(|cx| ws.read(cx).dock_area().read(cx).is_zoomed()));
    with(cx, window, &ws, |ws, w, cx| ws.toggle_maximize(w, cx));
    assert!(!cx.read(|cx| ws.read(cx).dock_area().read(cx).is_zoomed()));
}

#[gpui_kit::test]
fn every_theme_switch_fits_the_50ms_budget(cx: &mut TestAppContext) {
    let (_window, _ws) = open(cx);
    for name in themes::ALL {
        let took = cx.update(|cx| ferro_ui::theme::apply_theme(name, cx));
        assert_eq!(cx.read(ferro_ui::theme::current_theme), name);
        assert!(took < Duration::from_millis(50), "{name} took {took:?}");
    }
}

#[gpui_kit::test]
fn the_layout_survives_a_dump_and_restore(cx: &mut TestAppContext) {
    let (window, ws) = open(cx);
    with(cx, window, &ws, |ws, w, cx| {
        ws.toggle_tool_window(ToolWindowKind::Terminal, w, cx)
    });
    with(cx, window, &ws, |ws, w, cx| {
        ws.split(Placement::Bottom, w, cx)
    });
    let state: DockAreaState = cx.read(|cx| ws.read(cx).dock_area().read(cx).dump(cx));
    let json = serde_json::to_value(&state).unwrap();

    // Load the same state into the dock and check what comes back.
    let restored: DockAreaState = serde_json::from_value(json).unwrap();
    with(cx, window, &ws, |ws, w, cx| {
        let area = ws.dock_area().clone();
        area.update(cx, |area, cx| area.load(restored, w, cx))
            .unwrap();
    });
    let again: DockAreaState = cx.read(|cx| ws.read(cx).dock_area().read(cx).dump(cx));
    assert_eq!(
        serde_json::to_value(&state).unwrap(),
        serde_json::to_value(&again).unwrap(),
        "a restored layout dumps identically"
    );
    assert!(dock_open(cx, &ws, DockPlacement::Bottom));
}
