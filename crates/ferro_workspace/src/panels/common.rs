//! Small pieces every tool window body shares: list keyboard navigation,
//! toolbars, row styling, relative paths and times, severity colors.

use std::{ops::Range, path::Path};

use ferro_services::{Severity, SymbolKind};
use ferro_ui::{FerroIcon, components, tr, tr_args};
use gpui_kit::{
    App, ElementId, HighlightStyle, Hsla, IntoElement, ParentElement as _, SharedString,
    Styled as _, StyledText,
    component::{ActiveTheme as _, Sizable as _, button::Button, h_flex},
    div, px,
};

/// Height of one list row, in pixels.
pub const ROW_HEIGHT: f32 = 24.;

/// Rows a Page Up / Page Down moves.
const PAGE: usize = 10;

/// The row a navigation key moves the selection to, or `None` when the key
/// is not a navigation key or the list is empty.
pub fn nav(key: &str, current: Option<usize>, len: usize) -> Option<usize> {
    if len == 0 {
        return None;
    }
    let last = len - 1;
    Some(match (key, current) {
        ("down", None) | ("home", _) | ("pagedown", None) => 0,
        ("up", None) | ("end", _) | ("pageup", None) => last,
        ("down", Some(ix)) => (ix + 1).min(last),
        ("up", Some(ix)) => ix.saturating_sub(1),
        ("pagedown", Some(ix)) => (ix + PAGE).min(last),
        ("pageup", Some(ix)) => ix.saturating_sub(PAGE),
        _ => return None,
    })
}

/// Whether a key event carries a modifier that makes it a shortcut rather
/// than typing (ctrl, alt, cmd, fn).
pub fn has_command_modifier(keystroke: &gpui_kit::Keystroke) -> bool {
    let m = &keystroke.modifiers;
    m.control || m.alt || m.platform || m.function
}

/// The path of `path` relative to `root`, `/`-separated; the full path when
/// it is outside.
pub fn relative(path: &Path, root: Option<&Path>) -> String {
    let rel = root
        .and_then(|root| path.strip_prefix(root).ok())
        .unwrap_or(path);
    rel.to_string_lossy().replace('\\', "/")
}

/// "just now", "5 min ago", "2 h ago", "3 days ago".
pub fn relative_time(timestamp: i64, now: i64) -> SharedString {
    let secs = (now - timestamp).max(0);
    if secs < 60 {
        tr("panels-time-now")
    } else if secs < 3600 {
        tr_args("panels-time-minutes", &[("n", (secs / 60).into())])
    } else if secs < 86_400 {
        tr_args("panels-time-hours", &[("n", (secs / 3600).into())])
    } else {
        tr_args("panels-time-days", &[("n", (secs / 86_400).into())])
    }
}

pub fn severity_icon(severity: Severity) -> FerroIcon {
    match severity {
        Severity::Error => FerroIcon::Error,
        Severity::Warning => FerroIcon::Warning,
        Severity::Info | Severity::Hint => FerroIcon::Info,
    }
}

pub fn severity_color(severity: Severity, cx: &App) -> Hsla {
    let theme = cx.theme();
    match severity {
        Severity::Error => theme.danger,
        Severity::Warning => theme.warning,
        Severity::Info => theme.info,
        Severity::Hint => theme.muted_foreground,
    }
}

pub fn symbol_icon(kind: SymbolKind) -> FerroIcon {
    match kind {
        SymbolKind::Module => FerroIcon::SymbolModule,
        SymbolKind::Struct => FerroIcon::SymbolStruct,
        SymbolKind::Enum => FerroIcon::SymbolEnum,
        SymbolKind::Trait => FerroIcon::SymbolTrait,
        SymbolKind::Function | SymbolKind::Method => FerroIcon::SymbolFunction,
        SymbolKind::Field => FerroIcon::SymbolField,
        SymbolKind::Variable => FerroIcon::Variable,
        SymbolKind::Constant => FerroIcon::SymbolConstant,
        SymbolKind::Keyword => FerroIcon::SymbolKeyword,
        SymbolKind::Snippet => FerroIcon::FileCode,
        SymbolKind::Macro => FerroIcon::SymbolMacro,
        SymbolKind::Test => FerroIcon::Test,
    }
}

/// A slim toolbar at the top of a tool window.
pub fn toolbar(id: impl Into<ElementId>, cx: &App) -> gpui_kit::Stateful<gpui_kit::Div> {
    use gpui_kit::InteractiveElement as _;
    let theme = cx.theme();
    h_flex()
        .id(id)
        .h(px(30.))
        .flex_none()
        .px_1()
        .gap_1()
        .border_b_1()
        .border_color(theme.border)
        .text_xs()
        .text_color(theme.muted_foreground)
}

/// A small icon button for tool-window toolbars.
pub fn tool_button(id: impl Into<ElementId>, icon: FerroIcon, tooltip: SharedString) -> Button {
    components::icon_button(id, icon, tooltip.clone()).xsmall()
}

/// A text with matched byte ranges highlighted (speed search, filters).
pub fn highlighted(text: SharedString, ranges: Vec<Range<usize>>, color: Hsla) -> StyledText {
    let style = HighlightStyle {
        color: Some(color),
        font_weight: Some(gpui_kit::FontWeight::BOLD),
        ..Default::default()
    };
    StyledText::new(text).with_highlights(ranges.into_iter().map(move |r| (r, style)))
}

/// Byte ranges of one-character matches at `positions` in `text`.
pub fn char_ranges(text: &str, positions: &[usize]) -> Vec<Range<usize>> {
    positions
        .iter()
        .filter_map(|&p| {
            text.get(p..)
                .and_then(|rest| rest.chars().next())
                .map(|c| p..p + c.len_utf8())
        })
        .collect()
}

/// Case-insensitive substring ranges of `needle` in `text`.
pub fn substring_ranges(text: &str, needle: &str) -> Vec<Range<usize>> {
    if needle.is_empty() {
        return Vec::new();
    }
    let lower = text.to_lowercase();
    let needle = needle.to_lowercase();
    // Lowercasing can change byte lengths for some scripts; only highlight
    // when offsets still line up.
    if lower.len() != text.len() {
        return Vec::new();
    }
    lower
        .match_indices(&needle)
        .map(|(i, m)| i..i + m.len())
        .collect()
}

/// A muted one-line message under a toolbar (counts, hints).
pub fn caption(text: impl Into<SharedString>, cx: &App) -> impl IntoElement {
    div()
        .px_2()
        .py_1()
        .text_xs()
        .text_color(cx.theme().muted_foreground)
        .child(text.into())
}

/// The position of a node in a tree: child indices from the roots down.
pub type NodePath = Vec<usize>;

/// Flatten a tree into the visible rows, depth first. A node is kept when
/// `keep` accepts it or any descendant; children of collapsed nodes are
/// skipped.
pub fn flatten<T>(
    roots: &[T],
    children: &dyn Fn(&T) -> &[T],
    is_collapsed: &dyn Fn(&[usize]) -> bool,
    keep: &dyn Fn(&T) -> bool,
) -> Vec<NodePath> {
    fn walk<T>(
        nodes: &[T],
        prefix: &mut NodePath,
        children: &dyn Fn(&T) -> &[T],
        is_collapsed: &dyn Fn(&[usize]) -> bool,
        keep: &dyn Fn(&T) -> bool,
        out: &mut Vec<NodePath>,
    ) -> bool {
        let mut any = false;
        for (ix, node) in nodes.iter().enumerate() {
            prefix.push(ix);
            let at = out.len();
            out.push(prefix.clone());
            let kids = children(node);
            let kept_child = if is_collapsed(prefix) {
                // Still decide visibility from the hidden descendants.
                let mut scratch = Vec::new();
                walk(kids, prefix, children, &|_| false, keep, &mut scratch)
            } else {
                walk(kids, prefix, children, is_collapsed, keep, out)
            };
            if keep(node) || kept_child {
                any = true;
            } else {
                out.truncate(at);
            }
            prefix.pop();
        }
        any
    }
    let mut out = Vec::new();
    walk(
        roots,
        &mut Vec::new(),
        children,
        is_collapsed,
        keep,
        &mut out,
    );
    out
}

/// The node at `path`.
pub fn node_at<'a, T>(
    roots: &'a [T],
    path: &[usize],
    children: &dyn Fn(&T) -> &[T],
) -> Option<&'a T> {
    let (first, rest) = path.split_first()?;
    let mut node = roots.get(*first)?;
    for &ix in rest {
        node = children(node).get(ix)?;
    }
    Some(node)
}

#[cfg(test)]
mod tests {
    use super::*;

    struct N(&'static str, Vec<N>);

    #[test]
    fn flatten_respects_collapse_and_filters() {
        let tree = vec![
            N("a", vec![N("b", vec![]), N("c", vec![N("d", vec![])])]),
            N("e", vec![]),
        ];
        fn kids(n: &N) -> &[N] {
            &n.1
        }
        let all = flatten(&tree, &kids, &|_| false, &|_| true);
        assert_eq!(all.len(), 5);
        let collapsed = flatten(&tree, &kids, &|p| p == [0, 1], &|_| true);
        assert_eq!(collapsed.len(), 4);
        let only_d = flatten(&tree, &kids, &|_| false, &|n| n.0 == "d");
        assert_eq!(only_d, vec![vec![0], vec![0, 1], vec![0, 1, 0]]);
        assert_eq!(node_at(&tree, &[0, 1, 0], &kids).map(|n| n.0), Some("d"));
    }

    #[test]
    fn navigation_keys_clamp_to_the_list() {
        assert_eq!(nav("down", None, 3), Some(0));
        assert_eq!(nav("down", Some(2), 3), Some(2));
        assert_eq!(nav("up", Some(0), 3), Some(0));
        assert_eq!(nav("end", Some(0), 3), Some(2));
        assert_eq!(nav("pagedown", Some(0), 30), Some(10));
        assert_eq!(nav("pageup", Some(5), 30), Some(0));
        assert_eq!(nav("x", Some(0), 3), None);
        assert_eq!(nav("down", None, 0), None);
    }

    #[test]
    fn ranges_respect_char_boundaries() {
        let text = "żółć.rs";
        let ranges = char_ranges(text, &[0, 2]);
        assert!(ranges.iter().all(|r| text.is_char_boundary(r.end)));
        assert_eq!(substring_ranges("src/Main.rs", "main"), vec![4..8]);
    }

    #[test]
    fn relative_paths_use_slashes() {
        let root = Path::new("/p");
        assert_eq!(relative(Path::new("/p/src/a.rs"), Some(root)), "src/a.rs");
        assert_eq!(relative(Path::new("/q/a.rs"), Some(root)), "/q/a.rs");
    }
}
