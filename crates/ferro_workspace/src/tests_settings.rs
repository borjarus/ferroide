//! Headless tests for the Settings window, the keymap page, the welcome
//! onboarding and the New File / Run Configurations / Clone / Rename
//! dialogs, on mock services.

use std::time::Instant;

use ferro_keymap::KeymapPreset;
use ferro_services::{Services, TextPos, mock::MockConfig};
use ferro_settings::{Paths, Settings, themes};
use gpui_kit::{
    AnyWindowHandle, AppContext as _, Bounds, Entity, Point, SharedString, TestAppContext,
    WindowBounds, WindowOptions,
    component::{WindowExt as _, dock::DockPlacement},
    px, size,
};

use crate::{
    AppState, app_state,
    chrome::display_keystroke,
    dialogs::{clone_repo, new_file, rename, run_config},
    keymap_page::{ActionRow, KeymapPage, RecordBinding},
    settings_window::{self, SettingKey, SettingValue, SettingsPage, SettingsWindow},
    workspace::Workspace,
};

fn setup(cx: &mut TestAppContext) {
    cx.update(|cx| {
        gpui_kit::init(cx);
        ferro_ui::init(cx);
        let root = std::env::temp_dir().join(format!("ferro-settings-test-{}", std::process::id()));
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

/// Update any view in the workspace window.
fn on<T: 'static, R>(
    cx: &mut TestAppContext,
    window: AnyWindowHandle,
    view: &Entity<T>,
    f: impl FnOnce(&mut T, &mut gpui_kit::Window, &mut gpui_kit::Context<T>) -> R,
) -> R {
    let result = cx
        .update_window(window, |_, window, cx| {
            view.update(cx, |v, cx| f(v, window, cx))
        })
        .expect("window is open");
    cx.run_until_parked();
    result
}

fn settings_window(cx: &mut TestAppContext, window: AnyWindowHandle) -> Entity<SettingsWindow> {
    cx.update_window(window, |_, window, cx| {
        cx.new(|cx| SettingsWindow::new(SettingsPage::Appearance, window, cx))
    })
    .unwrap()
}

fn keymap_page(cx: &mut TestAppContext, window: AnyWindowHandle) -> Entity<KeymapPage> {
    cx.update_window(window, |_, window, cx| {
        cx.new(|cx| KeymapPage::new(window, cx))
    })
    .unwrap()
}

fn shortcut(cx: &mut TestAppContext, action: &str) -> Option<String> {
    cx.read(|cx| AppState::shortcut(action, cx))
}

/// A built-in shortcut as written for Windows/Linux, in the form the current
/// platform uses: the macOS keymap variants replace `ctrl` with `cmd`.
fn platform_keys(keys: &str) -> String {
    if cfg!(target_os = "macos") {
        keys.replace("ctrl", "cmd")
    } else {
        keys.to_string()
    }
}

#[gpui_kit::test]
fn settings_change_only_on_apply_and_reset_reverts_the_draft(cx: &mut TestAppContext) {
    let (window, _ws) = open(cx);
    let view = settings_window(cx, window);
    on(cx, window, &view, |v, _, cx| {
        v.set(
            SettingKey::Theme,
            SettingValue::Choice(themes::FERRO_LIGHT.into()),
            cx,
        );
        v.set(SettingKey::SoftWrap, SettingValue::Bool(true), cx);
    });
    // Not applied yet.
    assert_eq!(cx.read(ferro_ui::theme::current_theme), themes::FERRO_DARK);
    assert!(!cx.read(|cx| AppState::settings(cx).editor.soft_wrap));
    assert!(cx.read(|cx| view.read(cx).is_dirty(cx)));

    // Reset reverts the draft.
    on(cx, window, &view, |v, _, cx| v.reset(cx));
    assert!(!cx.read(|cx| view.read(cx).is_dirty(cx)));
    assert_eq!(
        cx.read(|cx| view.read(cx).draft().appearance.theme.clone()),
        themes::FERRO_DARK
    );

    // Apply changes the IDE.
    on(cx, window, &view, |v, _, cx| {
        v.set(
            SettingKey::Theme,
            SettingValue::Choice(themes::FERRO_LIGHT.into()),
            cx,
        );
        v.set(SettingKey::SoftWrap, SettingValue::Bool(true), cx);
        v.apply(cx);
    });
    assert_eq!(cx.read(ferro_ui::theme::current_theme), themes::FERRO_LIGHT);
    assert!(cx.read(|cx| AppState::settings(cx).editor.soft_wrap));
    assert!(!cx.read(|cx| view.read(cx).is_dirty(cx)));

    // A window closed without Apply (Cancel) leaves everything as is.
    let other = settings_window(cx, window);
    on(cx, window, &other, |v, _, cx| {
        v.set(
            SettingKey::Theme,
            SettingValue::Choice(themes::HIGH_CONTRAST.into()),
            cx,
        )
    });
    drop(other);
    assert_eq!(cx.read(ferro_ui::theme::current_theme), themes::FERRO_LIGHT);
}

#[gpui_kit::test]
fn numbers_clamp_and_defaults_restore(cx: &mut TestAppContext) {
    let (window, _ws) = open(cx);
    let view = settings_window(cx, window);
    on(cx, window, &view, |v, _, cx| {
        v.set(SettingKey::TabSize, SettingValue::Number(99.0), cx);
        v.set(SettingKey::UiFontSize, SettingValue::Number(1.0), cx);
    });
    let draft = cx.read(|cx| view.read(cx).draft().clone());
    assert_eq!(draft.editor.tab_size, 16);
    assert_eq!(
        draft.appearance.ui_font_size,
        ferro_settings::MIN_UI_FONT_SIZE
    );
    on(cx, window, &view, |v, _, cx| {
        v.reset_to_default(SettingKey::TabSize, cx)
    });
    assert_eq!(cx.read(|cx| view.read(cx).draft().editor.tab_size), 4);
}

#[gpui_kit::test]
fn applying_language_and_scheme_takes_effect(cx: &mut TestAppContext) {
    let (window, _ws) = open(cx);
    let view = settings_window(cx, window);
    // The locale is process-global and other tests run in parallel and
    // compare English strings: switch, check and switch back in one
    // synchronous step, so no other test observes Polish.
    let applied_locale = on(cx, window, &view, |v, _, cx| {
        v.set(
            SettingKey::KeymapScheme,
            SettingValue::Choice("jetbrains".into()),
            cx,
        );
        v.set(SettingKey::Language, SettingValue::Choice("pl".into()), cx);
        v.apply(cx);
        let applied = ferro_i18n::locale();
        app_state::set_locale(ferro_i18n::Locale::En, cx);
        applied
    });
    assert_eq!(applied_locale, ferro_i18n::Locale::Pl);
    assert_eq!(
        shortcut(cx, "ferro::FindAction"),
        Some(platform_keys("ctrl-shift-a"))
    );
}

#[gpui_kit::test]
fn search_filters_settings_across_pages(cx: &mut TestAppContext) {
    let (window, _ws) = open(cx);
    let view = settings_window(cx, window);
    on(cx, window, &view, |v, w, cx| v.set_search("wrap", w, cx));
    assert_eq!(
        cx.read(|cx| view.read(cx).search_results(cx)),
        vec![SettingKey::SoftWrap]
    );
    on(cx, window, &view, |v, w, cx| v.set_search("FONT", w, cx));
    assert_eq!(
        cx.read(|cx| view.read(cx).search_results(cx)),
        vec![SettingKey::UiFontSize, SettingKey::EditorFontSize]
    );
    // The page name matches every setting on it.
    on(cx, window, &view, |v, w, cx| v.set_search("keymap", w, cx));
    let results = cx.read(|cx| view.read(cx).search_results(cx));
    assert!(results.contains(&SettingKey::VimMode) && results.contains(&SettingKey::KeymapScheme));
    on(cx, window, &view, |v, w, cx| {
        v.set_search("no such setting", w, cx)
    });
    assert!(cx.read(|cx| view.read(cx).search_results(cx)).is_empty());
}

#[gpui_kit::test]
fn opening_settings_shows_a_dialog_on_the_requested_page(cx: &mut TestAppContext) {
    let (window, ws) = open(cx);
    let view = with(cx, window, &ws, |_, w, cx| {
        settings_window::show(SettingsPage::Keymap, w, cx)
    });
    assert_eq!(cx.read(|cx| view.read(cx).page()), SettingsPage::Keymap);
    let open = cx
        .update_window(window, |_, w, cx| w.has_active_dialog(cx))
        .unwrap();
    assert!(open);
}

#[gpui_kit::test]
fn the_settings_file_opens_in_an_editor_tab(cx: &mut TestAppContext) {
    let (window, ws) = open(cx);
    with(cx, window, &ws, |ws, w, cx| {
        settings_window::open_settings_file(ws, w, cx)
    });
    let file = cx.read(|cx| AppState::global(cx).paths.settings_file());
    let open = cx.read(|cx| {
        ws.read(cx)
            .editors
            .iter()
            .any(|e| e.read(cx).path() == file)
    });
    assert!(open, "settings.toml must open in a tab");
}

#[gpui_kit::test]
fn a_user_binding_survives_a_scheme_switch_and_fires(cx: &mut TestAppContext) {
    let (window, ws) = open(cx);
    cx.update(|cx| {
        AppState::update_user_keymap(cx, |k| k.set("ferro::ToggleBottomDock", &["ctrl-e".into()]))
    });
    assert_eq!(
        shortcut(cx, "ferro::ToggleBottomDock").as_deref(),
        Some("ctrl-e")
    );
    cx.update(|cx| app_state::set_keymap(KeymapPreset::JetBrains, cx));
    assert_eq!(
        cx.read(|cx| AppState::settings(cx).keymap.base),
        KeymapPreset::JetBrains
    );
    assert_eq!(
        shortcut(cx, "ferro::ToggleBottomDock").as_deref(),
        Some("ctrl-e")
    );

    // The keystroke reaches GPUI.
    let open = |cx: &mut TestAppContext| {
        cx.read(|cx| {
            ws.read(cx)
                .dock_area()
                .read(cx)
                .is_dock_open(DockPlacement::Bottom)
        })
    };
    assert!(!open(cx));
    cx.simulate_keystrokes(window, "ctrl-e");
    cx.run_until_parked();
    assert!(open(cx), "ctrl-e must toggle the bottom dock");

    // Resetting the binding restores the scheme's (JetBrains has none).
    cx.update(|cx| AppState::update_user_keymap(cx, |k| k.reset("ferro::ToggleBottomDock")));
    assert_eq!(shortcut(cx, "ferro::ToggleBottomDock"), None);
}

#[gpui_kit::test]
fn the_keymap_page_marks_conflicts_and_filters_them(cx: &mut TestAppContext) {
    let (window, _ws) = open(cx);
    let page = keymap_page(cx, window);
    assert!(cx.read(|cx| page.read(cx).conflicts().is_empty()));
    // Ctrl+G is Go to Line in VS Code.
    cx.update(|cx| {
        AppState::update_user_keymap(cx, |k| k.set("ferro::QuickOpen", &["ctrl-g".into()]))
    });
    cx.run_until_parked();
    assert!(cx.read(|cx| page.read(cx).is_conflicting("ferro::GoToLine")));
    on(cx, window, &page, |p, _, cx| p.set_conflicts_only(true, cx));
    let ids = cx.read(|cx| page.read(cx).visible_ids());
    assert_eq!(
        ids,
        vec![
            SharedString::from("ferro::QuickOpen"),
            SharedString::from("ferro::GoToLine")
        ]
    );
    // Removing the binding clears the conflict.
    cx.update(|cx| AppState::update_user_keymap(cx, |k| k.set("ferro::QuickOpen", &[])));
    cx.run_until_parked();
    assert!(cx.read(|cx| page.read(cx).visible_ids()).is_empty());
    assert_eq!(shortcut(cx, "ferro::QuickOpen"), None);
}

#[gpui_kit::test]
fn the_keymap_page_searches_by_name_shortcut_and_pressed_keys(cx: &mut TestAppContext) {
    let (window, _ws) = open(cx);
    let page = keymap_page(cx, window);
    let ids = |cx: &mut TestAppContext| -> Vec<String> {
        cx.read(|cx| {
            page.read(cx)
                .visible_ids()
                .iter()
                .map(|s| s.to_string())
                .collect()
        })
    };
    on(cx, window, &page, |p, w, cx| p.set_query("terminal", w, cx));
    let found = ids(cx);
    assert!(found.contains(&"ferro::ToggleTerminal".into()));
    assert!(found.contains(&"ferro::NewTerminal".into()));
    assert!(!found.contains(&"ferro::QuickOpen".into()));

    // By the displayed shortcut.
    on(cx, window, &page, |p, w, cx| {
        let query = display_keystroke(&platform_keys("ctrl-shift-p")).to_lowercase();
        p.set_query(&query, w, cx)
    });
    assert_eq!(ids(cx), vec!["ferro::CommandPalette".to_string()]);

    // By pressed keys: the chord prefix Ctrl+K.
    on(cx, window, &page, |p, w, cx| {
        p.set_query("", w, cx);
        p.set_recording(true, w, cx);
        p.record(&platform_keys("ctrl-k"), cx);
    });
    let found = ids(cx);
    for id in [
        "ferro::OpenKeymapSettings",
        "ferro::QuickDocumentation",
        "ferro::CloseAllTabs",
    ] {
        assert!(
            found.contains(&id.to_string()),
            "{id} missing from {found:?}"
        );
    }
    assert!(!found.contains(&"ferro::QuickOpen".into()));
    on(cx, window, &page, |p, _, cx| {
        p.record(&platform_keys("ctrl-s"), cx)
    });
    assert_eq!(ids(cx), vec!["ferro::OpenKeymapSettings".to_string()]);
}

#[gpui_kit::test]
fn recording_a_binding_warns_about_conflicts_and_saves(cx: &mut TestAppContext) {
    let (window, _ws) = open(cx);
    let record = cx
        .update_window(window, |_, _, cx| {
            cx.new(|cx| RecordBinding::new("ferro::Run".into(), cx))
        })
        .unwrap();
    on(cx, window, &record, |r, _, cx| {
        r.record(&platform_keys("ctrl-p"), cx)
    });
    assert_eq!(
        cx.read(|cx| record.read(cx).conflicting_actions(cx)),
        vec!["ferro::QuickOpen".to_string()]
    );
    on(cx, window, &record, |r, _, cx| {
        r.clear(cx);
        r.record("ctrl-alt-r", cx);
        r.record("x", cx);
    });
    assert_eq!(cx.read(|cx| record.read(cx).keys()), "ctrl-alt-r x");
    assert!(
        cx.read(|cx| record.read(cx).conflicting_actions(cx))
            .is_empty()
    );
    on(cx, window, &record, |r, _, cx| r.save(cx));
    assert_eq!(shortcut(cx, "ferro::Run").as_deref(), Some("ctrl-alt-r x"));
    assert!(cx.read(|cx| AppState::global(cx).user_keymap.overrides("ferro::Run")));
}

#[gpui_kit::test]
fn new_file_validates_creates_and_opens(cx: &mut TestAppContext) {
    let (window, ws) = open(cx);
    let dialog = with(cx, window, &ws, new_file::show);
    // The active editor is src/main.rs, so the directory defaults to src.
    let root = cx.read(|cx| ws.read(cx).services.project.roots()[0].clone());
    on(cx, window, &dialog, |d, w, cx| {
        d.set_name("bad:name", w, cx);
        d.create(w, cx);
    });
    assert!(cx.read(|cx| dialog.read(cx).error().is_some()));

    on(cx, window, &dialog, |d, w, cx| {
        d.set_name("utils.rs", w, cx);
        d.create(w, cx);
    });
    cx.run_until_parked();
    let target = root.join("src").join("utils.rs");
    let opened = cx.read(|cx| {
        ws.read(cx)
            .editors
            .iter()
            .any(|e| e.read(cx).path() == target)
    });
    assert!(opened, "the new file opens in a tab");

    // Creating it again fails: the service now has it.
    let again = with(cx, window, &ws, new_file::show);
    on(cx, window, &again, |d, w, cx| {
        d.set_name("utils.rs", w, cx);
        d.create(w, cx);
    });
    cx.run_until_parked();
    assert!(cx.read(|cx| again.read(cx).error().is_some()));
}

#[gpui_kit::test]
fn run_configurations_are_edited_and_the_selector_refreshes(cx: &mut TestAppContext) {
    let (window, ws) = open(cx);
    let before = cx.read(|cx| ws.read(cx).run_configs.len());
    let dialog = with(cx, window, &ws, run_config::show);
    on(cx, window, &dialog, |d, w, cx| {
        d.add(w, cx);
        d.set_field(run_config::RunField::Name, "Bench", w, cx);
        d.set_field(run_config::RunField::Command, "cargo bench", w, cx);
        assert!(d.apply(cx));
    });
    let names: Vec<String> = cx.read(|cx| {
        ws.read(cx)
            .run_configs
            .iter()
            .map(|c| c.name.clone())
            .collect()
    });
    assert_eq!(names.len(), before + 1);
    assert!(names.contains(&"Bench".to_string()));

    // A missing command is refused; removing a configuration deletes it.
    on(cx, window, &dialog, |d, w, cx| {
        d.set_field(run_config::RunField::Command, "  ", w, cx);
        assert!(!d.apply(cx));
        assert!(d.error().is_some());
        d.remove(w, cx);
        assert!(d.apply(cx));
    });
    assert_eq!(cx.read(|cx| ws.read(cx).run_configs.len()), before);
}

#[gpui_kit::test]
fn rename_previews_both_files_and_applies_to_open_editors(cx: &mut TestAppContext) {
    let (window, ws) = open(cx);
    let (root, services) = cx.read(|cx| {
        let ws = ws.read(cx);
        (ws.services.project.roots()[0].clone(), ws.services.clone())
    });
    let lib = root.join("src").join("lib.rs");
    let text = cx
        .read(|cx| {
            ws.read(cx)
                .editors
                .iter()
                .find(|e| e.read(cx).path() == lib)
                .and_then(|e| e.read(cx).text(cx))
        })
        .expect("lib.rs is open in the default layout");
    let (line, column) = text
        .lines()
        .enumerate()
        .find_map(|(i, l)| l.find("fn greet").map(|c| (i as u32, c as u32 + 3)))
        .unwrap();
    let handle = ws.downgrade();
    let dialog = cx
        .update_window(window, |_, w, cx| {
            cx.new(|cx| {
                rename::RenameDialog::new(
                    handle,
                    services,
                    rename::RenameTarget {
                        path: lib.clone(),
                        text,
                        position: TextPos::new(line, column),
                        original: "greet".into(),
                    },
                    w,
                    cx,
                )
            })
        })
        .unwrap();
    on(cx, window, &dialog, |d, w, cx| d.set_name("fn", w, cx));
    assert_eq!(
        cx.read(|cx| dialog.read(cx).invalid()),
        Some("dialogs-rename-error-keyword")
    );

    on(cx, window, &dialog, |d, w, cx| d.set_name("welcome", w, cx));
    cx.run_until_parked();
    let files: Vec<String> = cx.read(|cx| {
        dialog
            .read(cx)
            .edits()
            .expect("preview loaded")
            .files
            .iter()
            .map(|f| f.path.file_name().unwrap().to_string_lossy().into_owned())
            .collect()
    });
    assert!(
        files.contains(&"lib.rs".to_string()) && files.contains(&"main.rs".to_string()),
        "{files:?}"
    );

    let applied = on(cx, window, &dialog, |d, w, cx| d.apply(w, cx));
    assert!(applied >= 2);
    let main = root.join("src").join("main.rs");
    let main_text = cx
        .read(|cx| {
            ws.read(cx)
                .editors
                .iter()
                .find(|e| e.read(cx).path() == main)
                .and_then(|e| e.read(cx).text(cx))
        })
        .unwrap();
    assert!(
        main_text.contains("welcome(") && !main_text.contains("greet("),
        "{main_text}"
    );
}

#[gpui_kit::test]
fn clone_validates_its_inputs(cx: &mut TestAppContext) {
    let (window, ws) = open(cx);
    let dialog = with(cx, window, &ws, clone_repo::show);
    on(cx, window, &dialog, |d, w, cx| d.start(w, cx));
    assert_eq!(
        cx.read(|cx| dialog.read(cx).error().cloned()),
        Some(ferro_ui::tr("dialogs-clone-error-url-empty"))
    );
    on(cx, window, &dialog, |d, w, cx| {
        d.set_url("not a url", w, cx);
        d.start(w, cx);
    });
    assert_eq!(
        cx.read(|cx| dialog.read(cx).error().cloned()),
        Some(ferro_ui::tr("dialogs-clone-error-url-invalid"))
    );
    assert!(!cx.read(|cx| dialog.read(cx).is_cloning()));
    on(cx, window, &dialog, |d, w, cx| {
        d.set_url("https://github.com/ferroide/ferroide.git", w, cx);
        d.start(w, cx);
        assert!(d.is_cloning());
        d.cancel(cx);
    });
    assert!(!cx.read(|cx| dialog.read(cx).is_cloning()));
}

#[gpui_kit::test]
fn the_welcome_screen_offers_theme_and_keymap(cx: &mut TestAppContext) {
    setup(cx);
    let (window, _) = cx.update(crate::open_welcome).unwrap();
    cx.run_until_parked();
    // Rendering works and the onboarding choices act immediately.
    cx.update(|cx| app_state::set_keymap(KeymapPreset::Emacs, cx));
    cx.update(|cx| app_state::set_vim_mode(true, cx));
    cx.update_window(window, |_, w, _| w.refresh()).unwrap();
    cx.run_until_parked();
    let keymap = cx.read(|cx| AppState::settings(cx).keymap);
    assert_eq!(keymap.base, KeymapPreset::Emacs);
    assert!(keymap.vim_mode);
    assert_eq!(
        shortcut(cx, "ferro::QuickOpen").as_deref(),
        Some("ctrl-x ctrl-f")
    );
}

#[gpui_kit::test]
fn keymap_table_filters_10x_actions_within_budget(cx: &mut TestAppContext) {
    let (window, _ws) = open(cx);
    // ~10× the catalog: synthetic actions plus the real ones.
    let mut rows = crate::keymap_page::catalog_rows();
    for i in 0..800 {
        rows.push(ActionRow::new(
            format!("ext::Action{i}"),
            format!("Extension action number {i}").into(),
            "Extensions".into(),
        ));
    }
    let page = cx
        .update_window(window, |_, w, cx| {
            cx.new(|cx| KeymapPage::with_rows(rows, w, cx))
        })
        .unwrap();
    let started = Instant::now();
    for query in ["e", "ex", "ext", "extension action 7", "terminal", ""] {
        on(cx, window, &page, |p, w, cx| p.set_query(query, w, cx));
    }
    let took = started.elapsed();
    let visible = cx.read(|cx| page.read(cx).visible_ids().len());
    println!("keymap table: 6 filter passes over {visible} rows took {took:?}");
    assert!(visible > 800);
    assert!(took.as_millis() < 2_000, "filtering took {took:?}");
}
