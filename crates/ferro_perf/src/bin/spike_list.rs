//! Spike: a virtualized list of 1,000,000 rows.
//!
//! `spike_list --bench [--seconds 5]` scrolls continuously and prints a JSON
//! report with startup time, frame statistics and memory.

use std::{ops::Range, time::Instant};

use ferro_perf::{FrameClock, Report, SpikeArgs};
use gpui_kit::{
    AnyElement, AppContext as _, Context, IntoElement, ParentElement as _, Render, ScrollStrategy,
    Styled as _, UniformListScrollHandle, Window, WindowBounds, WindowOptions,
    component::{ActiveTheme as _, h_flex},
    div, px, size, uniform_list,
};
use mimalloc::MiMalloc;

#[global_allocator]
static GLOBAL: MiMalloc = MiMalloc;

const ROWS: usize = 1_000_000;

struct ListSpike {
    scroll: UniformListScrollHandle,
    bench: bool,
    seconds: f64,
    position: usize,
    clock: FrameClock,
    report: Report,
    done: bool,
}

impl ListSpike {
    fn rows(&mut self, range: Range<usize>, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let muted = cx.theme().muted_foreground;
        range
            .map(|ix| {
                h_flex()
                    .h(px(22.))
                    .px_2()
                    .gap_3()
                    .text_sm()
                    .child(div().w(px(80.)).text_color(muted).child(format!("{ix:>7}")))
                    .child(format!("src/module_{}/file_{ix}.rs", ix % 977))
                    .into_any_element()
            })
            .collect()
    }
}

impl Render for ListSpike {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.bench && !self.done {
            self.clock.tick();
            if self.clock.elapsed().as_secs_f64() < self.seconds {
                // ~40 rows per frame: fast flick scrolling.
                self.position = (self.position + 40) % (ROWS - 100);
                self.scroll
                    .scroll_to_item(self.position, ScrollStrategy::Top);
                window.request_animation_frame();
            } else {
                self.done = true;
                self.report.frames = Some(self.clock.stats());
                self.report.print();
                cx.quit();
            }
        }
        div()
            .size_full()
            .bg(cx.theme().background)
            .text_color(cx.theme().foreground)
            .child(
                uniform_list(
                    "million",
                    ROWS,
                    cx.processor(|this, range: Range<usize>, _, cx| this.rows(range, cx)),
                )
                .track_scroll(&self.scroll)
                .size_full(),
            )
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
                    size(px(1000.), px(800.)),
                    cx,
                ))),
                ..Default::default()
            };
            let (window, view) = gpui_kit::open_window(options, cx, |_, cx| {
                cx.new(|_| ListSpike {
                    scroll: UniformListScrollHandle::new(),
                    bench: args.bench,
                    seconds: args.seconds,
                    position: 0,
                    clock: FrameClock::default(),
                    report: Report::new("list-1m"),
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
