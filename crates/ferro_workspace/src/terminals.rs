//! Terminal sessions of projects opened from disk (Stage 8): the
//! `[terminal]` settings as `ferro_terminal` options.

use std::path::PathBuf;

use ferro_settings::TerminalSettings;
use ferro_terminal::TerminalOptions;
use gpui_kit::App;

use crate::AppState;

/// `[terminal]` settings as service options for a project rooted at `cwd`.
pub fn options_from(
    settings: &TerminalSettings,
    cwd: PathBuf,
    data_dir: PathBuf,
) -> TerminalOptions {
    TerminalOptions {
        cwd,
        default_shell: Some(settings.shell.trim().to_string()).filter(|s| !s.is_empty()),
        scrollback: settings.scrollback.clamp(100, 1_000_000),
        shell_integration: settings.shell_integration,
        integration_dir: data_dir.join("shell"),
        env: settings
            .env
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect(),
    }
}

pub fn options(cwd: PathBuf, cx: &App) -> TerminalOptions {
    let state = AppState::global(cx);
    options_from(&state.settings.terminal, cwd, state.paths.data_dir.clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_become_options() {
        let settings = TerminalSettings {
            shell: " pwsh ".into(),
            scrollback: 5,
            ..Default::default()
        };
        let o = options_from(&settings, "/p".into(), "/data".into());
        assert_eq!(o.default_shell.as_deref(), Some("pwsh"));
        assert_eq!(o.scrollback, 100);
        assert_eq!(o.integration_dir, PathBuf::from("/data").join("shell"));
        let o = options_from(&TerminalSettings::default(), "/p".into(), "/d".into());
        assert_eq!(o.default_shell, None);
    }
}
