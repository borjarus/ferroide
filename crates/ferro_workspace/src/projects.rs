//! Real projects (plan §8, Stage 4): opening a folder from disk, reopening
//! recent projects, adding root folders, and trusted projects.
//!
//! A project opened from disk runs on `FsProjectService` and
//! `FsSearchService`; the other services stay mocks until their stages
//! (LSP: 6, run: 8, VCS: 9, debugger: 10).
//!
//! **Trust.** A folder that was never trusted opens in safe mode: the
//! workspace shows a bar offering to trust it, and run configurations are
//! refused until it is. Trust is stored per folder in the state database and
//! covers its subfolders.

use std::{
    collections::HashSet,
    path::{Path, PathBuf},
    sync::Arc,
};

use anyhow::Result;
use ferro_project::{FsProjectService, FsSearchService, ProjectOptions};
use ferro_services::{Services, mock::MockConfig};
use ferro_ui::{FerroIcon, tr, tr_args};
use gpui_kit::{
    AnyElement, AnyWindowHandle, App, Context, Entity, Global, InteractiveElement as _,
    IntoElement as _, ParentElement as _, PathPromptOptions, Styled as _, Window,
    component::{
        ActiveTheme as _, Icon, Sizable as _, WindowExt as _,
        button::{Button, ButtonVariants as _},
        h_flex,
        notification::Notification,
    },
    div,
};

use crate::{app_state::AppState, workspace::Workspace};

/// Project roots opened without trust in this session.
#[derive(Default)]
struct Untrusted {
    roots: HashSet<PathBuf>,
    /// Workspaces whose safe-mode bar was dismissed.
    dismissed: HashSet<PathBuf>,
}
impl Global for Untrusted {}

/// Welcome windows, closed once a project opens (as IntelliJ does).
#[derive(Default)]
pub(crate) struct WelcomeWindows(pub Vec<AnyWindowHandle>);
impl Global for WelcomeWindows {}

/// Services for a project on disk.
pub fn project_services(roots: Vec<PathBuf>) -> Result<Services> {
    let project = FsProjectService::open_with(roots, ProjectOptions::default())?;
    let mut services = Services::mock(MockConfig::default());
    services.search = Arc::new(FsSearchService::new(project.clone()));
    services.project = Arc::new(project);
    Ok(services)
}

/// Open a project from disk in a new window and remember it.
pub fn open_project(
    roots: Vec<PathBuf>,
    cx: &mut App,
) -> Result<(AnyWindowHandle, Entity<Workspace>)> {
    let services = project_services(roots)?;
    let roots = services.project.roots();
    let db = AppState::global(cx).db.clone();
    if let Err(err) = db.record_project_opened(&roots, &services.project.name()) {
        tracing::warn!("could not remember the project: {err:#}");
    }
    let trusted = roots
        .iter()
        .all(|root| db.is_trusted(root).unwrap_or(false));
    if !trusted {
        let untrusted = cx.default_global::<Untrusted>();
        untrusted.roots.insert(roots[0].clone());
        untrusted.dismissed.remove(&roots[0]);
    }
    let opened = crate::open_workspace(services, cx)?;
    close_welcome_windows(cx);
    Ok(opened)
}

/// Reopen a project from the recent list, with all its root folders.
pub fn open_recent(path: PathBuf, cx: &mut App) -> Result<(AnyWindowHandle, Entity<Workspace>)> {
    let roots = AppState::global(cx)
        .db
        .recent_projects()
        .unwrap_or_default()
        .into_iter()
        .find(|record| record.path == path)
        .map(|record| record.roots)
        .filter(|roots| !roots.is_empty())
        .unwrap_or_else(|| vec![path.clone()]);
    // Folders deleted since are left out; the first one must exist.
    let roots: Vec<PathBuf> = roots
        .into_iter()
        .enumerate()
        .filter(|(ix, root)| *ix == 0 || root.is_dir())
        .map(|(_, root)| root)
        .collect();
    open_project(roots, cx)
}

/// Ask for a folder and open it.
pub fn prompt_open_project(cx: &mut App) {
    let picked = cx.prompt_for_paths(PathPromptOptions {
        files: false,
        directories: true,
        multiple: false,
        prompt: Some(tr("project-open-prompt")),
    });
    cx.spawn(async move |cx| {
        let paths = match picked.await {
            Ok(Ok(Some(paths))) => paths,
            Ok(Err(err)) => {
                tracing::error!("cannot show the folder picker: {err:#}");
                return;
            }
            _ => return,
        };
        let Some(path) = paths.into_iter().next() else {
            return;
        };
        cx.update(|cx| {
            if let Err(err) = open_project(vec![path.clone()], cx) {
                report_open_error(&path, &err, cx);
            }
        });
    })
    .detach();
}

fn report_open_error(path: &Path, err: &anyhow::Error, cx: &mut App) {
    tracing::error!("cannot open {}: {err:#}", path.display());
    let message = tr_args(
        "project-open-failed",
        &[
            ("path", path.display().to_string().into()),
            ("error", format!("{err:#}").into()),
        ],
    );
    if let Some(window) = cx.active_window() {
        let _ = window.update(cx, |_, window, cx| {
            window.push_notification(Notification::error(message.to_string()), cx);
        });
    }
}

/// Ask for a folder and add it to the workspace's project.
pub fn prompt_add_folder(workspace: &mut Workspace, _: &mut Window, cx: &mut Context<Workspace>) {
    let picked = cx.prompt_for_paths(PathPromptOptions {
        files: false,
        directories: true,
        multiple: true,
        prompt: Some(tr("project-add-folder-prompt")),
    });
    let project = workspace.services.project.clone();
    let key = crate::workspace::project_key(&workspace.services);
    cx.spawn(async move |_, cx| {
        let Ok(Ok(Some(paths))) = picked.await else {
            return;
        };
        cx.update(|cx| {
            for path in paths {
                if let Err(err) = project.add_root(&path) {
                    report_open_error(&path, &err, cx);
                    continue;
                }
                let db = AppState::global(cx).db.clone();
                if let Err(err) = db.record_project_opened(&project.roots(), &project.name()) {
                    tracing::warn!("could not remember the folders: {err:#}");
                }
                if !db.is_trusted(&path).unwrap_or(false) {
                    let untrusted = cx.default_global::<Untrusted>();
                    untrusted.roots.insert(PathBuf::from(&key));
                    untrusted.dismissed.remove(&PathBuf::from(&key));
                }
            }
            cx.refresh_windows();
        });
    })
    .detach();
}

/// Whether the project's code may run (run configurations, tasks, LSP).
pub fn is_trusted(services: &Services, cx: &App) -> bool {
    let Some(root) = services.project.roots().into_iter().next() else {
        return true;
    };
    cx.try_global::<Untrusted>()
        .is_none_or(|untrusted| !untrusted.roots.contains(&root))
}

/// Trust every root folder of the project.
pub fn trust(services: &Services, cx: &mut App) {
    let roots = services.project.roots();
    let db = AppState::global(cx).db.clone();
    for root in &roots {
        if let Err(err) = db.trust(root) {
            tracing::warn!("could not trust {}: {err:#}", root.display());
        }
    }
    if let Some(first) = roots.first() {
        let untrusted = cx.default_global::<Untrusted>();
        untrusted.roots.remove(first);
        untrusted.dismissed.remove(first);
    }
    cx.refresh_windows();
}

fn dismiss_trust_bar(services: &Services, cx: &mut App) {
    if let Some(root) = services.project.roots().into_iter().next() {
        cx.default_global::<Untrusted>().dismissed.insert(root);
    }
    cx.refresh_windows();
}

/// The safe-mode bar under the title bar of an untrusted project.
pub fn render_trust_bar(workspace: &Workspace, cx: &mut Context<Workspace>) -> Option<AnyElement> {
    let services = workspace.services.clone();
    let root = services.project.roots().into_iter().next()?;
    let untrusted = cx.try_global::<Untrusted>()?;
    if !untrusted.roots.contains(&root) || untrusted.dismissed.contains(&root) {
        return None;
    }
    let theme = cx.theme();
    let (warning, border, bg) = (theme.warning, theme.border, theme.secondary);
    let message = tr_args(
        "project-untrusted",
        &[("name", services.project.name().into())],
    );
    let trusting = services.clone();
    Some(
        h_flex()
            .id("project-trust-bar")
            .gap_2()
            .px_2()
            .py_1()
            .text_xs()
            .bg(bg)
            .border_b_1()
            .border_color(border)
            .child(Icon::from(FerroIcon::Warning).small().text_color(warning))
            .child(div().flex_1().min_w_0().child(message))
            .child(
                Button::new("project-trust")
                    .xsmall()
                    .label(tr("project-trust"))
                    .on_click(move |_, _, cx| trust(&trusting, cx)),
            )
            .child(
                Button::new("project-stay-safe")
                    .xsmall()
                    .ghost()
                    .label(tr("project-stay-safe"))
                    .on_click(move |_, _, cx| dismiss_trust_bar(&services, cx)),
            )
            .into_any_element(),
    )
}

/// Close every welcome window (after a project opened).
fn close_welcome_windows(cx: &mut App) {
    let windows = std::mem::take(&mut cx.default_global::<WelcomeWindows>().0);
    for handle in windows {
        let _ = handle.update(cx, |_, window, _| window.remove_window());
    }
}
