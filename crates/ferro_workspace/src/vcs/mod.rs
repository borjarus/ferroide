//! Version control views: the Commit and Git tool windows, the diff viewer
//! and the 3-way merge view, on the `VcsService`.

pub mod commit;
pub mod diff_view;
pub mod git_log;
pub mod graph;
pub mod merge_model;
pub mod merge_view;

use ferro_services::{ChangeKind, Services};
use ferro_ui::tr_args;
use gpui_kit::{App, EntityId, Global, Hsla, SharedString, component::ActiveTheme as _};

/// Bumped whenever the repository changes from inside the IDE (commit,
/// conflict resolved), so every VCS view of every window reloads.
#[derive(Default)]
pub struct VcsGeneration(pub u64);
impl Global for VcsGeneration {}

/// Tell the VCS views the repository changed.
pub fn repository_changed(cx: &mut App) {
    cx.default_global::<VcsGeneration>().0 += 1;
}

/// Register the dock builders that restore diff and merge tabs from a saved
/// layout. `services_for` finds the services of the workspace owning a dock.
pub fn register_panels(cx: &mut App, services_for: fn(EntityId, &App) -> Services) {
    diff_view::register(cx, services_for);
    merge_view::register(cx, services_for);
}

/// The color of a change kind, as IntelliJ shows file status.
pub fn change_color(kind: ChangeKind, cx: &App) -> Hsla {
    let theme = cx.theme();
    match kind {
        ChangeKind::Modified | ChangeKind::Renamed => theme.info,
        ChangeKind::Added => theme.success,
        ChangeKind::Deleted => theme.muted_foreground,
        ChangeKind::Untracked => theme.danger.opacity(0.8),
        ChangeKind::Conflicted => theme.warning,
    }
}

/// `5 min ago`, `3 h ago`, `2 days ago`, or a date for older times.
pub fn relative_time(timestamp: i64, now: i64) -> SharedString {
    let secs = (now - timestamp).max(0);
    let (key, n) = match secs {
        0..60 => return ferro_ui::tr("vcs-time-now"),
        60..3_600 => ("vcs-time-minutes", secs / 60),
        3_600..86_400 => ("vcs-time-hours", secs / 3_600),
        86_400..2_592_000 => ("vcs-time-days", secs / 86_400),
        _ => return date(timestamp).into(),
    };
    tr_args(key, &[("n", n.into())])
}

/// `YYYY-MM-DD` (UTC) of a Unix timestamp.
pub fn date(timestamp: i64) -> String {
    // Howard Hinnant's days-to-civil algorithm.
    let z = timestamp.div_euclid(86_400) + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!("{year:04}-{month:02}-{day:02}")
}

/// Seconds since the Unix epoch, now.
pub fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dates_are_civil() {
        assert_eq!(date(0), "1970-01-01");
        assert_eq!(date(951_782_400), "2000-02-29");
        assert_eq!(date(1_794_182_400), "2026-11-09");
    }

    #[test]
    fn relative_times_pick_a_unit() {
        ferro_i18n::set_locale(ferro_i18n::Locale::En);
        let now = 1_794_182_400;
        assert_eq!(relative_time(now - 10, now), "just now");
        assert_eq!(relative_time(now - 300, now), "5 min ago");
        assert_eq!(relative_time(now - 7_200, now), "2 h ago");
        assert_eq!(relative_time(now - 86_400 * 3, now), "3 days ago");
        assert_eq!(
            relative_time(now - 86_400 * 400, now),
            date(now - 86_400 * 400)
        );
    }
}
