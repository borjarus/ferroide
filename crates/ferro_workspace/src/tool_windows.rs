//! Bodies of the tool windows, on mock services.
//!
//! Project and Run are live against their services (async loading, events);
//! the others show representative static mock data. Each gets its full
//! implementation in Stage 2.

use std::{
    collections::{HashMap, HashSet},
    ops::Range,
    path::PathBuf,
};

use ferro_services::{Entry, RunEvent, Services};
use ferro_ui::{FerroIcon, components, tr};
use gpui_kit::{
    AnyElement, App, AppContext as _, Context, Entity, EventEmitter, InteractiveElement as _,
    IntoElement, ParentElement as _, Render, SharedString, StatefulInteractiveElement as _,
    Styled as _, UniformListScrollHandle, Window,
    component::{
        ActiveTheme as _, Disableable as _, Icon, Sizable as _,
        button::{Button, ButtonVariants as _},
        checkbox::Checkbox,
        h_flex,
        input::{Input, InputState},
        v_flex,
    },
    div,
    prelude::FluentBuilder as _,
    px, uniform_list,
};

use crate::tool_window::ToolWindowKind;

/// What a tool window shows.
pub enum ToolWindowBody {
    Project(Entity<ProjectTree>),
    Run(Entity<RunConsole>),
    Commit(Entity<CommitView>),
    Static(ToolWindowKind),
}

impl ToolWindowBody {
    pub fn element(&self) -> AnyElement {
        match self {
            ToolWindowBody::Project(view) => view.clone().into_any_element(),
            ToolWindowBody::Run(view) => view.clone().into_any_element(),
            ToolWindowBody::Commit(view) => view.clone().into_any_element(),
            ToolWindowBody::Static(kind) => StaticBody(*kind).into_any_element(),
        }
    }
}

pub fn build(
    kind: ToolWindowKind,
    services: Services,
    window: &mut Window,
    cx: &mut App,
) -> ToolWindowBody {
    match kind {
        ToolWindowKind::Project => {
            ToolWindowBody::Project(cx.new(|cx| ProjectTree::new(services, cx)))
        }
        ToolWindowKind::Run => ToolWindowBody::Run(cx.new(|cx| RunConsole::new(services, cx))),
        ToolWindowKind::Commit => ToolWindowBody::Commit(cx.new(|cx| CommitView::new(window, cx))),
        other => ToolWindowBody::Static(other),
    }
}

// ---------------------------------------------------------------------------
// Project tree
// ---------------------------------------------------------------------------

enum Children {
    Loading,
    Loaded(Vec<Entry>),
    Failed(SharedString),
}

#[derive(Clone)]
enum RowKind {
    Entry(Entry),
    Loading,
    Failed(PathBuf, SharedString),
}

#[derive(Clone)]
struct Row {
    depth: usize,
    kind: RowKind,
}

/// Emitted when the user opens a file from the tree.
pub struct OpenFile(pub PathBuf);

/// A lazily loaded, virtualized file tree: only expanded directories are
/// fetched, and only visible rows are rendered.
pub struct ProjectTree {
    services: Services,
    roots: Vec<PathBuf>,
    children: HashMap<PathBuf, Children>,
    expanded: HashSet<PathBuf>,
    rows: Vec<Row>,
    selected: Option<PathBuf>,
    scroll: UniformListScrollHandle,
}

impl EventEmitter<OpenFile> for ProjectTree {}

impl ProjectTree {
    fn new(services: Services, cx: &mut Context<Self>) -> Self {
        let roots = services.project.roots();
        let mut this = Self {
            services,
            roots: roots.clone(),
            children: HashMap::new(),
            expanded: HashSet::new(),
            rows: Vec::new(),
            selected: None,
            scroll: UniformListScrollHandle::new(),
        };
        for root in roots {
            this.expanded.insert(root.clone());
            this.load(root, cx);
        }
        this.rebuild_rows();
        this
    }

    fn load(&mut self, dir: PathBuf, cx: &mut Context<Self>) {
        self.children.insert(dir.clone(), Children::Loading);
        let task = cx.background_spawn(self.services.project.children(&dir));
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |this, cx| {
                this.children.insert(
                    dir,
                    match result {
                        Ok(entries) => Children::Loaded(entries),
                        Err(err) => Children::Failed(err.to_string().into()),
                    },
                );
                this.rebuild_rows();
                cx.notify();
            });
        })
        .detach();
    }

    fn rebuild_rows(&mut self) {
        fn walk(this: &ProjectTree, dir: &PathBuf, depth: usize, out: &mut Vec<Row>) {
            match this.children.get(dir) {
                None | Some(Children::Loading) => out.push(Row {
                    depth,
                    kind: RowKind::Loading,
                }),
                Some(Children::Failed(err)) => out.push(Row {
                    depth,
                    kind: RowKind::Failed(dir.clone(), err.clone()),
                }),
                Some(Children::Loaded(entries)) => {
                    for entry in entries {
                        out.push(Row {
                            depth,
                            kind: RowKind::Entry(entry.clone()),
                        });
                        if entry.is_dir() && this.expanded.contains(&entry.path) {
                            walk(this, &entry.path, depth + 1, out);
                        }
                    }
                }
            }
        }
        let mut rows = Vec::new();
        for root in &self.roots {
            rows.push(Row {
                depth: 0,
                kind: RowKind::Entry(Entry {
                    path: root.clone(),
                    name: self.services.project.name(),
                    kind: ferro_services::EntryKind::Directory,
                    size: 0,
                }),
            });
            if self.expanded.contains(root) {
                walk(self, root, 1, &mut rows);
            }
        }
        self.rows = rows;
    }

    fn activate(&mut self, ix: usize, cx: &mut Context<Self>) {
        let Some(row) = self.rows.get(ix).cloned() else {
            return;
        };
        match row.kind {
            RowKind::Entry(entry) if entry.is_dir() => {
                self.selected = Some(entry.path.clone());
                if !self.expanded.remove(&entry.path) {
                    self.expanded.insert(entry.path.clone());
                    if !self.children.contains_key(&entry.path) {
                        self.load(entry.path, cx);
                    }
                }
                self.rebuild_rows();
            }
            RowKind::Entry(entry) => {
                self.selected = Some(entry.path.clone());
                cx.emit(OpenFile(entry.path));
            }
            RowKind::Failed(dir, _) => self.load(dir, cx),
            RowKind::Loading => {}
        }
        cx.notify();
    }

    /// Number of visible rows (for tests and the storybook).
    pub fn row_count(&self) -> usize {
        self.rows.len()
    }

    fn render_rows(&mut self, range: Range<usize>, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let theme = cx.theme();
        let (muted, selected_bg, hover_bg, danger) = (
            theme.muted_foreground,
            theme.list_active,
            theme.list_hover,
            theme.danger,
        );
        range
            .filter_map(|ix| self.rows.get(ix).cloned().map(|row| (ix, row)))
            .map(|(ix, row)| {
                let indent = px(8. + row.depth as f32 * 14.);
                let (icon, label, color, chevron): (FerroIcon, SharedString, _, Option<FerroIcon>) =
                    match &row.kind {
                        RowKind::Entry(entry) if entry.is_dir() => {
                            let open = self.expanded.contains(&entry.path);
                            (
                                if open { FerroIcon::FolderOpen } else { FerroIcon::Folder },
                                entry.name.clone().into(),
                                None,
                                Some(if open { FerroIcon::ChevronDown } else { FerroIcon::ChevronRight }),
                            )
                        }
                        RowKind::Entry(entry) => {
                            (FerroIcon::for_file(&entry.name), entry.name.clone().into(), None, None)
                        }
                        RowKind::Loading => (FerroIcon::Loading, tr("tree-loading"), Some(muted), None),
                        RowKind::Failed(_, err) => (
                            FerroIcon::Error,
                            format!("{} — {}", tr("tree-load-failed"), err).into(),
                            Some(danger),
                            None,
                        ),
                    };
                let is_selected = matches!(&row.kind, RowKind::Entry(e) if Some(&e.path) == self.selected.as_ref());
                h_flex()
                    .id(ix)
                    .h(px(24.))
                    .pl(indent)
                    .pr_2()
                    .gap_1()
                    .w_full()
                    .text_sm()
                    .whitespace_nowrap()
                    .overflow_hidden()
                    .cursor_pointer()
                    .when(is_selected, |el| el.bg(selected_bg))
                    .hover(|el| el.bg(hover_bg))
                    .child(
                        div().w(px(14.)).flex_none().when_some(chevron, |el, c| {
                            el.child(Icon::from(c).xsmall().text_color(muted))
                        }),
                    )
                    .child(Icon::from(icon).small().text_color(muted))
                    .child(
                        div()
                            .truncate()
                            .when_some(color, |el, c| el.text_color(c))
                            .child(label),
                    )
                    .on_click(cx.listener(move |this, _, _, cx| this.activate(ix, cx)))
                    .into_any_element()
            })
            .collect()
    }
}

impl Render for ProjectTree {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        uniform_list(
            "project-tree",
            self.rows.len(),
            cx.processor(|this, range: Range<usize>, _, cx| this.render_rows(range, cx)),
        )
        .track_scroll(&self.scroll)
        .size_full()
        .py_1()
    }
}

// ---------------------------------------------------------------------------
// Run console
// ---------------------------------------------------------------------------

pub struct RunConsole {
    lines: Vec<SharedString>,
}

impl RunConsole {
    fn new(services: Services, cx: &mut Context<Self>) -> Self {
        let events = services.run.subscribe();
        cx.spawn(async move |this, cx| {
            while let Ok(event) = events.recv().await {
                let alive = this.update(cx, |this, cx| {
                    match event {
                        RunEvent::Output { line, .. } => this.lines.push(line.into()),
                        RunEvent::StateChanged { state, .. } => {
                            if let ferro_services::RunState::Finished { exit_code } = state {
                                this.lines.push(ferro_ui::tr_args(
                                    "run-finished",
                                    &[("code", exit_code.into())],
                                ));
                            }
                        }
                    }
                    cx.notify();
                });
                if alive.is_err() {
                    break;
                }
            }
        })
        .detach();
        Self { lines: Vec::new() }
    }
}

impl Render for RunConsole {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.lines.is_empty() {
            return components::empty_state(FerroIcon::Run, tr("run-empty"), None, cx)
                .into_any_element();
        }
        v_flex()
            .id("run-console")
            .size_full()
            .p_2()
            .overflow_y_scroll()
            .font_family(cx.theme().mono_font_family.clone())
            .text_size(cx.theme().mono_font_size)
            .children(self.lines.iter().cloned().map(|line| div().child(line)))
            .into_any_element()
    }
}

// ---------------------------------------------------------------------------
// Commit
// ---------------------------------------------------------------------------

pub struct CommitView {
    message: Entity<InputState>,
    checked: Vec<bool>,
}

const CHANGES: &[(&str, &str)] = &[
    ("src/config.rs", "M"),
    ("src/main.rs", "M"),
    ("web/src/app.ts", "A"),
    ("README.md", "M"),
];

impl CommitView {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        Self {
            message: cx.new(|cx| InputState::new(window, cx).placeholder(tr("commit-message"))),
            checked: vec![true; CHANGES.len()],
        }
    }
}

impl Render for CommitView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let (muted, success, warning) = (theme.muted_foreground, theme.success, theme.warning);
        v_flex()
            .size_full()
            .p_2()
            .gap_2()
            .child(
                div()
                    .text_xs()
                    .text_color(muted)
                    .child(tr("commit-changes")),
            )
            .child(v_flex().gap_1().children(CHANGES.iter().enumerate().map(
                |(ix, (path, status))| {
                    h_flex()
                        .gap_2()
                        .text_sm()
                        .child(
                            Checkbox::new(("commit-file", ix))
                                .checked(self.checked[ix])
                                .on_click(cx.listener(move |this, checked: &bool, _, cx| {
                                    this.checked[ix] = *checked;
                                    cx.notify();
                                })),
                        )
                        .child(
                            Icon::from(FerroIcon::for_file(path))
                                .small()
                                .text_color(muted),
                        )
                        .child(div().flex_1().truncate().child(*path))
                        .child(
                            div()
                                .text_xs()
                                .text_color(if *status == "A" { success } else { warning })
                                .child(*status),
                        )
                },
            )))
            .child(div().flex_1())
            .child(Input::new(&self.message))
            .child(
                h_flex().gap_2().child(
                    Button::new("commit")
                        .primary()
                        .small()
                        .label(tr("commit-button"))
                        .disabled(!self.checked.iter().any(|c| *c)),
                ),
            )
    }
}

// ---------------------------------------------------------------------------
// Static mock bodies
// ---------------------------------------------------------------------------

#[derive(IntoElement)]
pub struct StaticBody(ToolWindowKind);

struct MockRow {
    icon: FerroIcon,
    text: SharedString,
    detail: Option<SharedString>,
}

fn row(icon: FerroIcon, text: impl Into<SharedString>, detail: Option<&str>) -> MockRow {
    MockRow {
        icon,
        text: text.into(),
        detail: detail.map(|d| SharedString::from(d.to_string())),
    }
}

fn mock_rows(kind: ToolWindowKind) -> Result<Vec<MockRow>, (FerroIcon, &'static str)> {
    use FerroIcon as I;
    Ok(match kind {
        ToolWindowKind::Structure => vec![
            row(I::FileCode, "use anyhow::Result", None),
            row(I::FileCode, "use demo_app::{Config, greet}", None),
            row(I::Run, "fn main() -> Result<()>", Some("main.rs:4")),
        ],
        ToolWindowKind::Problems => vec![
            row(
                I::Error,
                "mismatched types: expected `u32`, found `&str`",
                Some("src/config.rs:12"),
            ),
            row(
                I::Warning,
                "unused variable: `retries`",
                Some("src/main.rs:6"),
            ),
            row(I::Info, "Typo: In word 'ferro'", Some("README.md:1")),
        ],
        ToolWindowKind::Git => vec![
            row(
                I::Commit,
                "Add config loading",
                Some("a1b2c3d · Ada L. · 2 h"),
            ),
            row(
                I::Commit,
                "Greet strangers politely",
                Some("9f8e7d6 · Linus T. · yesterday"),
            ),
            row(
                I::Commit,
                "Merge branch 'feature/web'",
                Some("5c4b3a2 · Grace H. · 3 d"),
            ),
            row(I::Commit, "Initial commit", Some("0000001 · Ada L. · 2 w")),
        ],
        ToolWindowKind::Terminal => vec![
            row(I::Terminal, "PS C:\\Projects\\demo-app> cargo run", None),
            row(I::Terminal, "    Finished `dev` profile in 0.08s", None),
            row(I::Terminal, "Hello, FerroIDE", None),
        ],
        ToolWindowKind::Todo => vec![
            row(
                I::Todo,
                "TODO: retry with exponential backoff",
                Some("src/config.rs:18"),
            ),
            row(I::Todo, "FIXME: handle empty names", Some("src/lib.rs:7")),
        ],
        ToolWindowKind::Notifications => vec![
            row(I::Ok, "Indexing finished", Some("demo-app")),
            row(I::Info, "Running on mock services (Stage 1)", None),
        ],
        ToolWindowKind::Build => vec![
            row(I::Build, "cargo build", None),
            row(I::Build, "cargo test", None),
            row(I::Build, "cargo clippy", None),
            row(I::Run, "cargo run", None),
        ],
        ToolWindowKind::Debug => return Err((I::Debug, "debug-empty")),
        ToolWindowKind::Database => return Err((I::Database, "database-empty")),
        ToolWindowKind::Project | ToolWindowKind::Run | ToolWindowKind::Commit => {
            return Err((I::Info, "tool-empty"));
        }
    })
}

impl gpui_kit::RenderOnce for StaticBody {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.theme();
        let (muted, hover) = (theme.muted_foreground, theme.list_hover);
        match mock_rows(self.0) {
            Err((icon, key)) => components::empty_state(icon, tr(key), None, cx).into_any_element(),
            Ok(rows) => v_flex()
                .id(self.0.id())
                .size_full()
                .py_1()
                .overflow_y_scroll()
                .children(rows.into_iter().map(|r| {
                    h_flex()
                        .h(px(24.))
                        .px_2()
                        .gap_2()
                        .text_sm()
                        .hover(|el| el.bg(hover))
                        .child(Icon::from(r.icon).small().text_color(muted))
                        .child(div().flex_1().truncate().child(r.text))
                        .when_some(r.detail, |el, d| {
                            el.child(div().text_xs().text_color(muted).child(d))
                        })
                }))
                .into_any_element(),
        }
    }
}
