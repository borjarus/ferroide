//! The FerroIDE workspace: windows, dock, tool windows, editor tabs,
//! toolbar, status bar, menus, Quick Open, the Command Palette, Settings,
//! dialogs, the VCS views and the welcome screen.
//!
//! Views depend only on the service traits in `ferro_services`. A project
//! opened from disk runs on the real file system (Stage 4, [`projects`]);
//! `--demo` and the storybook run every view on the mocks (see
//! `docs/FerroIDE_plan.md`, §5.3).

pub mod actions;
pub mod app_state;
mod chrome;
pub mod dialogs;
pub mod editor;
pub mod find_in_files;
pub mod keymap_page;
pub mod menus;
pub mod panels;
pub mod projects;
pub mod quick_open;
pub mod requests;
pub mod settings_window;
#[cfg(test)]
mod tests;
#[cfg(test)]
mod tests_editor;
#[cfg(test)]
mod tests_files;
#[cfg(test)]
mod tests_panels;
#[cfg(test)]
mod tests_projects;
#[cfg(test)]
mod tests_search;
#[cfg(test)]
mod tests_settings;
#[cfg(test)]
mod tests_vcs;
pub mod tool_window;
pub mod vcs;
pub mod welcome;
pub mod workspace;

use ferro_services::{Services, mock::MockConfig};
use gpui_kit::{
    AnyWindowHandle, App, AppContext as _, Bounds, Entity, Result, WindowBounds, WindowOptions,
    component::TitleBar, px, size,
};

pub use app_state::AppState;
pub use chrome::display_keystroke;
pub use welcome::WelcomeScreen;
pub use workspace::Workspace;

use actions::*;

/// Install application-level behavior. Call once after `gpui_kit::init`,
/// `ferro_ui::init` and [`AppState::init`].
pub fn init(cx: &mut App) {
    cx.on_action(|_: &NewWindow, cx| log_err(open_welcome(cx)));
    cx.on_action(|_: &ShowWelcome, cx| log_err(open_welcome(cx)));
    cx.on_action(|_: &OpenProject, cx| projects::prompt_open_project(cx));
    cx.on_action(|_: &NewProject, cx| log_err(open_workspace(mock_services(), cx)));
    cx.on_action(|_: &GetFromVcs, cx| log_err(open_workspace(mock_services(), cx)));
    cx.on_action(|_: &CloseProject, cx| {
        if let Some(window) = cx.active_window() {
            let _ = window.update(cx, |_, window, _| window.remove_window());
        }
        log_err(open_welcome(cx));
    });
}

fn log_err<T>(result: Result<T>) {
    if let Err(err) = result {
        tracing::error!("{err:#}");
    }
}

/// Mock services: the demo project (`--demo`, New Project, the storybook).
pub fn mock_services() -> Services {
    Services::mock(MockConfig::default())
}

fn window_options(width: f32, height: f32, cx: &App) -> WindowOptions {
    WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(Bounds::centered(
            None,
            size(px(width), px(height)),
            cx,
        ))),
        window_min_size: Some(size(px(640.), px(420.))),
        ..TitleBar::window_options()
    }
}

/// Open the welcome screen in its own window.
pub fn open_welcome(cx: &mut App) -> Result<(AnyWindowHandle, Entity<WelcomeScreen>)> {
    let recent = ferro_db::DbRecentProjects::new(AppState::global(cx).db.clone());
    let opened = gpui_kit::open_window(window_options(860., 560., cx), cx, move |window, cx| {
        window.set_window_title("FerroIDE");
        cx.new(|cx| WelcomeScreen::new(&recent, window, cx))
    })?;
    cx.default_global::<projects::WelcomeWindows>()
        .0
        .push(opened.0);
    Ok(opened)
}

/// Open a workspace window for a project.
pub fn open_workspace(
    services: Services,
    cx: &mut App,
) -> Result<(AnyWindowHandle, Entity<Workspace>)> {
    gpui_kit::open_window(window_options(1400., 900., cx), cx, move |window, cx| {
        window.set_window_title(&format!("{} — FerroIDE", services.project.name()));
        Workspace::build(services, window, cx)
    })
}
