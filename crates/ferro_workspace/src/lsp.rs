//! Language servers in the window (Stage 6).
//!
//! A project opened from disk gets an [`LspLanguageService`] with the
//! tree-sitter service as its fallback. Its options come from `[lsp]` in
//! `settings.toml` and follow every change of the settings; servers are
//! installed into `<data>/servers`. The status bar's language-server
//! widget shows the project's real servers (the [`LspStatus`] wrapper).

use std::{
    path::PathBuf,
    sync::{Arc, Weak},
    time::Duration,
};

use ferro_lsp::{LspLanguageService, LspOptions, ServerSettings};
use ferro_services::{
    LanguageServerStatus, LanguageService, NotificationEntry, StatusService, TaskProgress,
};
use ferro_settings::LspSettings;
use gpui_kit::{App, Global};

use crate::app_state::AppState;

/// Where the installer puts language servers.
pub fn install_dir(cx: &App) -> PathBuf {
    AppState::global(cx).paths.data_dir.join("servers")
}

/// `[lsp]` settings as service options.
pub fn options_from(settings: &LspSettings, install_dir: PathBuf) -> LspOptions {
    LspOptions {
        max_servers: settings.max_servers.max(1),
        idle_timeout: Duration::from_secs(settings.idle_minutes.max(1) * 60),
        power_save: settings.power_save,
        install_dir: Some(install_dir),
        servers: settings
            .servers
            .iter()
            .map(|(id, s)| {
                (
                    id.clone(),
                    ServerSettings {
                        enabled: s.enabled,
                        command: s.command.clone(),
                        args: s.args.clone(),
                        settings: s.settings.clone(),
                        initialization_options: s.initialization_options.clone(),
                    },
                )
            })
            .collect(),
    }
}

pub fn options(cx: &App) -> LspOptions {
    options_from(&AppState::settings(cx).lsp, install_dir(cx))
}

/// Every live language-server service, to update when settings change.
#[derive(Default)]
struct LspServices(Vec<Weak<LspLanguageService>>);
impl Global for LspServices {}

pub fn register(service: &Arc<LspLanguageService>, cx: &mut App) {
    let services = cx.default_global::<LspServices>();
    services.0.retain(|s| s.strong_count() > 0);
    services.0.push(Arc::downgrade(service));
}

/// Apply the current `[lsp]` settings to every project's servers.
pub fn sync_options(cx: &mut App) {
    if !cx.has_global::<LspServices>() {
        return;
    }
    let options = options(cx);
    let services = &mut cx.global_mut::<LspServices>().0;
    services.retain(|s| s.strong_count() > 0);
    for service in services.iter().filter_map(Weak::upgrade) {
        if service.options() != options {
            service.set_options(options.clone());
        }
    }
}

/// The concrete service of a window, when its project runs on servers.
pub fn service_of(
    language: &Arc<dyn LanguageService>,
    cx: &App,
) -> Option<Arc<LspLanguageService>> {
    let services = cx.try_global::<LspServices>()?;
    let target = Arc::as_ptr(language) as *const ();
    services
        .0
        .iter()
        .filter_map(Weak::upgrade)
        .find(|s| Arc::as_ptr(s) as *const () == target)
}

/// The status service of a project on disk: language servers from the
/// language service, everything else from `inner`.
pub struct LspStatus {
    pub inner: Arc<dyn StatusService>,
    pub language: Arc<dyn LanguageService>,
}

impl StatusService for LspStatus {
    fn language_servers(&self) -> Vec<LanguageServerStatus> {
        self.language.servers()
    }

    fn subscribe_tasks(&self) -> async_channel::Receiver<TaskProgress> {
        self.inner.subscribe_tasks()
    }

    fn notifications(&self) -> Vec<NotificationEntry> {
        self.inner.notifications()
    }

    fn mark_notifications_read(&self) {
        self.inner.mark_notifications_read()
    }

    fn clear_notifications(&self) {
        self.inner.clear_notifications()
    }
}
