//! Spike: a window with a dock of three panels (left, center, bottom).
//!
//! `spike_dock --bench` measures startup to first frame and frame times
//! while the left dock is resized back and forth every frame — the
//! "dragging panels" scenario from the Stage 1 budget.

use std::time::Instant;

use ferro_perf::{FrameClock, Report, SpikeArgs};
use gpui_kit::{
    App, AppContext as _, Context, Entity, EventEmitter, FocusHandle, Focusable, IntoElement,
    ParentElement as _, Render, Styled as _, Window, WindowBounds, WindowOptions,
    component::{
        ActiveTheme as _,
        dock::{
            BasePanel, DockArea, DockLayout, DockPlacement, DockSkin, Panel, PanelEvent,
            panel_handle,
        },
    },
    div, px, size,
};
use mimalloc::MiMalloc;

#[global_allocator]
static GLOBAL: MiMalloc = MiMalloc;

struct Pane {
    name: &'static str,
    focus: FocusHandle,
}

impl BasePanel for Pane {
    fn panel_name(&self) -> &'static str {
        self.name
    }
}
impl Panel for Pane {
    fn title(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        self.name
    }
}
impl EventEmitter<PanelEvent> for Pane {}
impl Focusable for Pane {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}
impl Render for Pane {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .p_2()
            .text_color(cx.theme().muted_foreground)
            .children((0..40).map(|i| div().child(format!("{} line {i}", self.name))))
    }
}

struct DockSpike {
    area: Entity<DockArea>,
    bench: bool,
    seconds: f64,
    clock: FrameClock,
    report: Report,
    done: bool,
}

impl Render for DockSpike {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.bench && !self.done {
            self.clock.tick();
            let t = self.clock.elapsed().as_secs_f64();
            if t < self.seconds {
                let width = 200. + 150. * (t * 3.0).sin().abs() as f32;
                self.area.update(cx, |area, cx| {
                    area.set_dock_size(DockPlacement::Left, px(width), window, cx)
                });
                window.request_animation_frame();
            } else {
                self.done = true;
                self.report.frames = Some(self.clock.stats());
                self.report
                    .notes
                    .push("left dock resized every frame".into());
                self.report.print();
                cx.quit();
            }
        }
        div()
            .size_full()
            .bg(cx.theme().background)
            .child(self.area.clone())
    }
}

fn main() {
    let started = Instant::now();
    let args = SpikeArgs::parse();
    gpui_kit::application()
        .with_assets(ferro_ui::FerroAssets)
        .run(move |cx| {
            gpui_kit::init(cx);
            ferro_ui::init(cx);
            ferro_ui::theme::apply_theme(ferro_ui::theme::FERRO_DARK, cx);
            let options = WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(gpui_kit::Bounds::centered(
                    None,
                    size(px(1200.), px(800.)),
                    cx,
                ))),
                ..Default::default()
            };
            let (window, view) = gpui_kit::open_window(options, cx, move |window, cx| {
                let (area, _) = DockSkin::dock_area("spike", None, window, cx);
                let pane = |name, cx: &mut App| {
                    cx.new(|cx| Pane {
                        name,
                        focus: cx.focus_handle(),
                    })
                };
                let (left, center, bottom) = (
                    pane("Project", cx),
                    pane("Editor", cx),
                    pane("Terminal", cx),
                );
                area.update(cx, |area, cx| {
                    area.set_center(
                        DockLayout::tabs().panel_view(panel_handle(center), cx),
                        window,
                        cx,
                    );
                    area.set_dock(
                        DockPlacement::Left,
                        DockLayout::tabs().panel_view(panel_handle(left), cx),
                        window,
                        cx,
                    );
                    area.set_dock(
                        DockPlacement::Bottom,
                        DockLayout::tabs().panel_view(panel_handle(bottom), cx),
                        window,
                        cx,
                    );
                    area.set_dock_size(DockPlacement::Left, px(260.), window, cx);
                    area.set_dock_size(DockPlacement::Bottom, px(200.), window, cx);
                });
                cx.new(|_| DockSpike {
                    area,
                    bench: args.bench,
                    seconds: args.seconds,
                    clock: FrameClock::default(),
                    report: Report::new("dock-3-panels"),
                    done: false,
                })
            })
            .expect("open window");
            let _ = window.update(cx, |_, window, _| {
                window.on_next_frame(move |_, cx| {
                    let ms = started.elapsed().as_secs_f64() * 1000.0;
                    view.update(cx, |spike, cx| {
                        spike.report.startup_ms = ms;
                        cx.notify();
                    });
                    eprintln!("first frame after {ms:.1} ms");
                });
            });
        });
}
