//! The Git tool window (Log tab): a virtualized commit list with the commit
//! graph, ref badges, text and branch filters, and a details pane.
//!
//! Keyboard: Up/Down/PageUp/PageDown/Home/End move the selection; the
//! details pane follows it; Enter on the details' file list opens a diff.

use std::{collections::HashSet, ops::Range, rc::Rc};

use ferro_services::{Commit, Services};
use ferro_ui::{FerroIcon, components, tr, tr_args};
use gpui_kit::{
    AnyElement, AnyView, App, AppContext as _, Bounds, Context, Entity, FocusHandle, Focusable,
    Hsla, InteractiveElement as _, IntoElement, KeyDownEvent, ParentElement as _, PathBuilder,
    Pixels, Render, ScrollStrategy, SharedString, StatefulInteractiveElement as _, Styled as _,
    Subscription, UniformListScrollHandle, Window, canvas,
    component::{
        ActiveTheme as _, Icon, Sizable as _,
        button::Button,
        h_flex,
        input::{Input, InputEvent, InputState},
        menu::{DropdownMenu as _, PopupMenuItem},
        spinner::Spinner,
        v_flex,
    },
    div, fill, point,
    prelude::FluentBuilder as _,
    px, size, uniform_list,
};

use crate::{
    requests::OpenDiff,
    vcs::{
        VcsGeneration,
        graph::{GraphRow, layout_graph},
        now, relative_time,
    },
};

/// How many commits the log loads.
pub const LOG_LIMIT: usize = 5_000;
const ROW_HEIGHT: f32 = 24.;
const LANE_WIDTH: f32 = 14.;

enum Load {
    Loading,
    Loaded,
    Failed(SharedString),
}

/// A ref badge on a commit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RefBadge {
    Head,
    Local(String),
    Remote(String),
    Tag(String),
}

/// Split git's decoration list (`HEAD -> main`, `origin/main`,
/// `tag: v0.1.0`) into badges.
pub fn ref_badges(refs: &[String]) -> Vec<RefBadge> {
    let mut out = Vec::new();
    for r in refs {
        if let Some(branch) = r.strip_prefix("HEAD -> ") {
            out.push(RefBadge::Head);
            out.push(RefBadge::Local(branch.to_string()));
        } else if r == "HEAD" {
            out.push(RefBadge::Head);
        } else if let Some(tag) = r.strip_prefix("tag: ") {
            out.push(RefBadge::Tag(tag.to_string()));
        } else if r.starts_with("origin/") || r.starts_with("upstream/") {
            out.push(RefBadge::Remote(r.clone()));
        } else {
            out.push(RefBadge::Local(r.clone()));
        }
    }
    out
}

/// The branch names the branch filter offers.
fn branch_names(commits: &[Commit]) -> Vec<String> {
    let mut names: Vec<String> = commits
        .iter()
        .flat_map(|c| ref_badges(&c.refs))
        .filter_map(|b| match b {
            RefBadge::Local(n) | RefBadge::Remote(n) => Some(n),
            _ => None,
        })
        .collect();
    names.sort();
    names.dedup();
    names
}

pub struct GitLogView {
    services: Services,
    load: Load,
    commits: Vec<Commit>,
    /// Indices into `commits` that pass the filters.
    visible: Vec<usize>,
    graph: Rc<Vec<GraphRow>>,
    filter: Entity<InputState>,
    branch: Option<String>,
    /// Index into `visible`.
    selected: Option<usize>,
    scroll: UniformListScrollHandle,
    focus_handle: FocusHandle,
    _subscriptions: Vec<Subscription>,
}

impl GitLogView {
    pub fn new(services: Services, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let filter =
            cx.new(|cx| InputState::new(window, cx).placeholder(tr("vcs-log-filter-placeholder")));
        let subscriptions = vec![
            cx.subscribe(&filter, |this, _, event: &InputEvent, cx| {
                if matches!(event, InputEvent::Change) {
                    this.apply_filters(cx);
                }
            }),
            cx.observe_global::<VcsGeneration>(|this, cx| this.refresh(cx)),
        ];
        let mut this = Self {
            services,
            load: Load::Loading,
            commits: Vec::new(),
            visible: Vec::new(),
            graph: Rc::new(Vec::new()),
            filter,
            branch: None,
            selected: None,
            scroll: UniformListScrollHandle::new(),
            focus_handle: cx.focus_handle(),
            _subscriptions: subscriptions,
        };
        this.refresh(cx);
        this
    }

    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        if self.commits.is_empty() {
            self.load = Load::Loading;
        }
        let task = cx.background_spawn(self.services.vcs.log(LOG_LIMIT));
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |this, cx| {
                match result {
                    Ok(commits) => this.set_commits(commits, cx),
                    Err(err) => this.load = Load::Failed(err.to_string().into()),
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Replace the history (also used by tests and the storybook).
    pub fn set_commits(&mut self, commits: Vec<Commit>, cx: &mut Context<Self>) {
        let selected_id = self.selected_commit().map(|c| c.id.clone());
        self.commits = commits;
        self.load = Load::Loaded;
        self.apply_filters(cx);
        self.selected = selected_id
            .and_then(|id| {
                self.visible
                    .iter()
                    .position(|&ix| self.commits[ix].id == id)
            })
            .or_else(|| (!self.visible.is_empty()).then_some(0));
    }

    pub fn commits(&self) -> &[Commit] {
        &self.commits
    }

    pub fn visible_count(&self) -> usize {
        self.visible.len()
    }

    pub fn graph(&self) -> &[GraphRow] {
        &self.graph
    }

    pub fn selected_commit(&self) -> Option<&Commit> {
        self.selected
            .and_then(|ix| self.visible.get(ix))
            .map(|&ix| &self.commits[ix])
    }

    pub fn select(&mut self, ix: usize, cx: &mut Context<Self>) {
        if self.visible.is_empty() {
            return;
        }
        let ix = ix.min(self.visible.len() - 1);
        self.selected = Some(ix);
        self.scroll.scroll_to_item(ix, ScrollStrategy::Nearest);
        cx.notify();
    }

    pub fn set_branch_filter(&mut self, branch: Option<String>, cx: &mut Context<Self>) {
        self.branch = branch;
        self.apply_filters(cx);
    }

    fn apply_filters(&mut self, cx: &mut Context<Self>) {
        let text = self.filter.read(cx).value().to_lowercase();
        let reachable: Option<HashSet<&str>> = self.branch.as_ref().map(|branch| {
            let tip = self.commits.iter().find(|c| {
                ref_badges(&c.refs)
                    .iter()
                    .any(|b| matches!(b, RefBadge::Local(n) | RefBadge::Remote(n) if n == branch))
            });
            let by_id: std::collections::HashMap<&str, &Commit> =
                self.commits.iter().map(|c| (c.id.as_str(), c)).collect();
            let mut seen = HashSet::new();
            let mut stack: Vec<&str> = tip.map(|c| c.id.as_str()).into_iter().collect();
            while let Some(id) = stack.pop() {
                if seen.insert(id)
                    && let Some(c) = by_id.get(id)
                {
                    stack.extend(c.parents.iter().map(String::as_str));
                }
            }
            seen
        });
        self.visible = self
            .commits
            .iter()
            .enumerate()
            .filter(|(_, c)| reachable.as_ref().is_none_or(|r| r.contains(c.id.as_str())))
            .filter(|(_, c)| {
                text.is_empty()
                    || c.subject.to_lowercase().contains(&text)
                    || c.author.to_lowercase().contains(&text)
                    || c.id.starts_with(&text)
            })
            .map(|(ix, _)| ix)
            .collect();
        let shown: Vec<Commit> = self
            .visible
            .iter()
            .map(|&ix| self.commits[ix].clone())
            .collect();
        self.graph = Rc::new(layout_graph(&shown));
        if self.selected.is_some_and(|s| s >= self.visible.len()) {
            self.selected = (!self.visible.is_empty()).then_some(0);
        }
        cx.notify();
    }

    fn on_key_down(&mut self, event: &KeyDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        let m = &event.keystroke.modifiers;
        if m.control || m.alt || m.platform || self.visible.is_empty() {
            return;
        }
        let current = self.selected.unwrap_or(0);
        let page = 20;
        let target = match event.keystroke.key.as_str() {
            "down" => current + 1,
            "up" => current.saturating_sub(1),
            "pagedown" => current + page,
            "pageup" => current.saturating_sub(page),
            "home" => 0,
            "end" => self.visible.len() - 1,
            _ => return,
        };
        self.select(target, cx);
        cx.stop_propagation();
    }

    fn render_rows(&mut self, range: Range<usize>, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let theme = cx.theme();
        let (muted, selected_bg, hover_bg) =
            (theme.muted_foreground, theme.list_active, theme.list_hover);
        let palette = graph_palette(cx);
        let lanes = self
            .graph
            .iter()
            .map(|r| r.width)
            .max()
            .unwrap_or(1)
            .clamp(1, 12);
        let now = now();
        range
            .filter_map(|ix| {
                let commit = self.commits.get(*self.visible.get(ix)?)?;
                let row = self.graph.get(ix)?.clone();
                let palette = palette.clone();
                let badges = ref_badges(&commit.refs);
                Some(
                    h_flex()
                        .id(("vcs-log-row", ix))
                        .h(px(ROW_HEIGHT))
                        .w_full()
                        .pr_2()
                        .gap_2()
                        .text_sm()
                        .cursor_pointer()
                        .when(self.selected == Some(ix), |el| el.bg(selected_bg))
                        .hover(|el| el.bg(hover_bg))
                        .aria_label(SharedString::from(format!(
                            "{}, {}",
                            commit.subject, commit.author
                        )))
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.focus_handle.focus(window, cx);
                            this.select(ix, cx);
                        }))
                        .child(
                            canvas(
                                |_, _, _| {},
                                move |bounds, _, window, _| {
                                    paint_graph_row(&row, &palette, bounds, window)
                                },
                            )
                            .w(px(LANE_WIDTH * lanes as f32 + 4.))
                            .h_full()
                            .flex_none(),
                        )
                        .child(
                            h_flex()
                                .flex_1()
                                .min_w_0()
                                .gap_1()
                                .children(badges.into_iter().map(|b| badge(b, cx)))
                                .child(div().truncate().child(commit.subject.clone())),
                        )
                        .child(
                            div()
                                .w(px(120.))
                                .flex_none()
                                .truncate()
                                .text_color(muted)
                                .child(commit.author.clone()),
                        )
                        .child(
                            div()
                                .w(px(90.))
                                .flex_none()
                                .truncate()
                                .text_color(muted)
                                .child(relative_time(commit.timestamp, now)),
                        )
                        .into_any_element(),
                )
            })
            .collect()
    }

    fn render_details(&self, cx: &mut Context<Self>) -> Option<impl IntoElement> {
        let commit = self.selected_commit()?.clone();
        let theme = cx.theme();
        let (muted, border, hover) = (theme.muted_foreground, theme.border, theme.list_hover);
        let root = self.services.project.roots().first().cloned();
        let files = commit.files.iter().enumerate().map(|(ix, file)| {
            let path = root
                .as_ref()
                .map(|r| file.split('/').fold(r.clone(), |p, part| p.join(part)));
            h_flex()
                .id(("vcs-log-file", ix))
                .gap_1()
                .px_1()
                .text_sm()
                .cursor_pointer()
                .hover(|el| el.bg(hover))
                .aria_label(SharedString::from(file.clone()))
                .child(
                    Icon::from(FerroIcon::for_file(file))
                        .xsmall()
                        .text_color(muted),
                )
                .child(div().truncate().child(file.clone()))
                .when_some(path, |el, path| {
                    el.on_click(move |_, window, cx| {
                        window.dispatch_action(Box::new(OpenDiff { path: path.clone() }), cx)
                    })
                })
        });
        Some(
            v_flex()
                .id("vcs-log-details")
                .w(px(300.))
                .flex_none()
                .h_full()
                .p_2()
                .gap_2()
                .border_l_1()
                .border_color(border)
                .overflow_y_scroll()
                .aria_label(tr("vcs-log-details"))
                .child(
                    div()
                        .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                        .child(commit.subject.clone()),
                )
                .when(!commit.body.is_empty(), |el| {
                    el.child(div().text_sm().child(commit.body.clone()))
                })
                .child(
                    v_flex()
                        .text_xs()
                        .text_color(muted)
                        .child(commit.id.clone())
                        .child(tr_args(
                            "vcs-log-authored",
                            &[
                                ("author", commit.author.clone().into()),
                                ("date", crate::vcs::date(commit.timestamp).into()),
                            ],
                        ))
                        .when(commit.parents.len() > 1, |el| {
                            el.child(tr_args(
                                "vcs-log-merge-of",
                                &[("count", commit.parents.len().into())],
                            ))
                        }),
                )
                .child(div().text_xs().text_color(muted).child(tr_args(
                    "vcs-file-count",
                    &[("count", commit.files.len().into())],
                )))
                .children(files),
        )
    }
}

fn badge(badge: RefBadge, cx: &App) -> impl IntoElement {
    let theme = cx.theme();
    let (text, color) = match badge {
        RefBadge::Head => ("HEAD".to_string(), theme.warning),
        RefBadge::Local(n) => (n, theme.success),
        RefBadge::Remote(n) => (n, theme.info),
        RefBadge::Tag(n) => (n, theme.magenta),
    };
    div()
        .flex_none()
        .px_1()
        .rounded_sm()
        .text_xs()
        .border_1()
        .border_color(color)
        .text_color(color)
        .child(text)
}

fn graph_palette(cx: &App) -> Rc<Vec<Hsla>> {
    let t = cx.theme();
    Rc::new(vec![
        t.blue, t.green, t.magenta, t.yellow, t.cyan, t.red, t.chart_1, t.chart_2, t.chart_3,
        t.chart_4,
    ])
}

fn paint_graph_row(row: &GraphRow, palette: &[Hsla], bounds: Bounds<Pixels>, window: &mut Window) {
    let color = |ix: usize| palette[ix % palette.len()];
    let x = |lane: usize| bounds.left() + px(2. + LANE_WIDTH * (lane as f32 + 0.5));
    let (top, bottom) = (bounds.top(), bounds.bottom());
    let mid = top + (bottom - top) / 2.;
    let mut line = |from: (Pixels, Pixels), to: (Pixels, Pixels), c: Hsla| {
        let mut path = PathBuilder::stroke(px(1.5));
        path.move_to(point(from.0, from.1));
        path.line_to(point(to.0, to.1));
        if let Ok(path) = path.build() {
            window.paint_path(path, c);
        }
    };
    for e in &row.upper {
        line((x(e.from), top), (x(e.to), mid), color(e.color));
    }
    for e in &row.lower {
        line((x(e.from), mid), (x(e.to), bottom), color(e.color));
    }
    let r = px(3.5);
    let center = point(x(row.node), mid);
    window.paint_quad(
        fill(
            Bounds::new(point(center.x - r, center.y - r), size(r * 2., r * 2.)),
            color(row.node_color),
        )
        .corner_radii(r),
    );
}

impl Focusable for GitLogView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for GitLogView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let muted = cx.theme().muted_foreground;
        let branches = branch_names(&self.commits);
        let current = self.branch.clone();
        let this = cx.entity().downgrade();
        let branch_label: SharedString = self
            .branch
            .clone()
            .map(Into::into)
            .unwrap_or_else(|| tr("vcs-log-all-branches"));
        let toolbar = h_flex()
            .gap_1()
            .p_1()
            .child(
                div()
                    .flex_1()
                    .child(Input::new(&self.filter).small().cleanable(true)),
            )
            .child(
                components::toolbar_widget("vcs-log-branch", Some(FerroIcon::Branch), branch_label)
                    .tooltip(tr("vcs-log-branch-filter"))
                    .dropdown_menu(move |mut menu, _, _| {
                        let this2 = this.clone();
                        menu = menu.item(
                            PopupMenuItem::new(tr("vcs-log-all-branches"))
                                .checked(current.is_none())
                                .on_click(move |_, _, cx| {
                                    let _ = this2.update(cx, |v, cx| v.set_branch_filter(None, cx));
                                }),
                        );
                        for name in &branches {
                            let this = this.clone();
                            let branch = name.clone();
                            menu = menu.item(
                                PopupMenuItem::new(SharedString::from(name.clone()))
                                    .checked(current.as_deref() == Some(name.as_str()))
                                    .on_click(move |_, _, cx| {
                                        let branch = branch.clone();
                                        let _ = this.update(cx, |v, cx| {
                                            v.set_branch_filter(Some(branch), cx)
                                        });
                                    }),
                            );
                        }
                        menu
                    }),
            )
            .child(
                components::icon_button("vcs-log-refresh", FerroIcon::Refresh, tr("vcs-refresh"))
                    .on_click(cx.listener(|this, _, _, cx| this.refresh(cx))),
            );

        let body: AnyElement = match &self.load {
            Load::Loading => h_flex()
                .flex_1()
                .justify_center()
                .gap_2()
                .text_sm()
                .text_color(muted)
                .child(Spinner::new().small())
                .child(tr("vcs-log-loading"))
                .into_any_element(),
            Load::Failed(err) => v_flex()
                .flex_1()
                .child(components::empty_state(
                    FerroIcon::Error,
                    tr("vcs-log-failed"),
                    Some(err.clone()),
                    cx,
                ))
                .child(
                    h_flex().justify_center().pb_2().child(
                        Button::new("vcs-log-retry")
                            .small()
                            .label(tr("vcs-retry"))
                            .on_click(cx.listener(|this, _, _, cx| this.refresh(cx))),
                    ),
                )
                .into_any_element(),
            Load::Loaded if self.visible.is_empty() => div()
                .flex_1()
                .child(components::empty_state(
                    FerroIcon::Git,
                    if self.commits.is_empty() {
                        tr("vcs-log-empty")
                    } else {
                        tr("vcs-log-no-match")
                    },
                    None,
                    cx,
                ))
                .into_any_element(),
            Load::Loaded => h_flex()
                .flex_1()
                .min_h_0()
                .child(
                    div()
                        .id("vcs-log-list")
                        .flex_1()
                        .min_w_0()
                        .h_full()
                        .track_focus(&self.focus_handle)
                        .aria_label(tr("vcs-log"))
                        .on_key_down(cx.listener(Self::on_key_down))
                        .child(
                            uniform_list(
                                "vcs-log-rows",
                                self.visible.len(),
                                cx.processor(|this, range: Range<usize>, _, cx| {
                                    this.render_rows(range, cx)
                                }),
                            )
                            .track_scroll(&self.scroll)
                            .size_full(),
                        ),
                )
                .children(self.render_details(cx))
                .into_any_element(),
        };

        v_flex()
            .id("vcs-git-log")
            .key_context("GitLog")
            .size_full()
            .child(toolbar)
            .child(body)
    }
}

pub fn build(services: Services, window: &mut Window, cx: &mut App) -> AnyView {
    cx.new(|cx| GitLogView::new(services, window, cx)).into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decorations_become_badges() {
        let refs = vec![
            "HEAD -> main".to_string(),
            "origin/main".to_string(),
            "tag: v0.1.0".to_string(),
            "develop".to_string(),
        ];
        assert_eq!(
            ref_badges(&refs),
            vec![
                RefBadge::Head,
                RefBadge::Local("main".into()),
                RefBadge::Remote("origin/main".into()),
                RefBadge::Tag("v0.1.0".into()),
                RefBadge::Local("develop".into()),
            ]
        );
    }
}
