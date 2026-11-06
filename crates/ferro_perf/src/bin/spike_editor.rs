//! Spike: GPUI Kit's code editor with a 100,000-line Rust file.
//!
//! `spike_editor --bench` measures startup (file opened and highlighted),
//! key → frame latency for typed characters, and frame times while paging
//! through the file, then prints a JSON report.

use std::{
    cell::RefCell,
    rc::Rc,
    time::{Duration, Instant},
};

use ferro_perf::{FrameClock, Report, SpikeArgs, generate, latency_stats};
use gpui_kit::{
    AppContext as _, Context, Entity, Focusable as _, IntoElement, Keystroke, ParentElement as _,
    Render, Styled as _, Window, WindowBounds, WindowOptions,
    component::{
        ActiveTheme as _,
        input::{Editor, EditorState},
    },
    div, px, size,
};
use mimalloc::MiMalloc;

#[global_allocator]
static GLOBAL: MiMalloc = MiMalloc;

const LINES: usize = 100_000;
const TYPING_SAMPLES: usize = 200;

enum Phase {
    Warmup(usize),
    Typing,
    Scrolling,
    Done,
}

struct EditorSpike {
    editor: Entity<EditorState>,
    bench: bool,
    seconds: f64,
    phase: Phase,
    latencies: Rc<RefCell<Vec<Duration>>>,
    pending_key: Rc<RefCell<Option<Instant>>>,
    clock: FrameClock,
    report: Report,
}

impl EditorSpike {
    fn type_one(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let t0 = Instant::now();
        *self.pending_key.borrow_mut() = Some(t0);
        window.dispatch_keystroke(Keystroke::parse("x").expect("valid keystroke"), cx);
        let latencies = self.latencies.clone();
        let pending = self.pending_key.clone();
        window.on_next_frame(move |_, _| {
            if let Some(t0) = pending.borrow_mut().take() {
                latencies.borrow_mut().push(t0.elapsed());
            }
        });
    }
}

impl Render for EditorSpike {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.bench {
            match self.phase {
                Phase::Warmup(n) => {
                    // Let highlighting and layout settle before measuring.
                    self.phase = if n > 30 {
                        Phase::Typing
                    } else {
                        Phase::Warmup(n + 1)
                    };
                    window.request_animation_frame();
                }
                Phase::Typing => {
                    if self.latencies.borrow().len() < TYPING_SAMPLES {
                        self.type_one(window, cx);
                        window.request_animation_frame();
                    } else {
                        self.report.input_latency = Some(latency_stats(&self.latencies.borrow()));
                        self.phase = Phase::Scrolling;
                        window.request_animation_frame();
                    }
                }
                Phase::Scrolling => {
                    self.clock.tick();
                    if self.clock.elapsed().as_secs_f64() < self.seconds {
                        window.dispatch_keystroke(Keystroke::parse("pagedown").expect("valid"), cx);
                        window.request_animation_frame();
                    } else {
                        self.report.frames = Some(self.clock.stats());
                        self.report
                            .notes
                            .push(format!("{LINES} lines, tree-sitter Rust highlighting"));
                        self.report.print();
                        self.phase = Phase::Done;
                        cx.quit();
                    }
                }
                Phase::Done => {}
            }
        }
        div()
            .size_full()
            .bg(cx.theme().background)
            .child(Editor::new(&self.editor).bordered(false).h_full())
    }
}

fn main() {
    let started = Instant::now();
    let args = SpikeArgs::parse();
    let source = generate::rust_source(LINES);
    gpui_kit::application()
        .with_assets(ferro_ui::FerroAssets)
        .run(move |cx| {
            gpui_kit::init(cx);
            ferro_ui::init(cx);
            ferro_ui::theme::apply_theme(ferro_ui::theme::FERRO_DARK, cx);
            let options = WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(gpui_kit::Bounds::centered(
                    None,
                    size(px(1200.), px(900.)),
                    cx,
                ))),
                ..Default::default()
            };
            let (window, view) = gpui_kit::open_window(options, cx, move |window, cx| {
                let editor = cx.new(|cx| {
                    EditorState::new(window, cx)
                        .language("rust")
                        .line_number(true)
                        .default_value(source)
                });
                editor.read(cx).focus_handle(cx).focus(window, cx);
                cx.new(|_| EditorSpike {
                    editor,
                    bench: args.bench,
                    seconds: args.seconds,
                    phase: Phase::Warmup(0),
                    latencies: Rc::default(),
                    pending_key: Rc::default(),
                    clock: FrameClock::default(),
                    report: Report::new("editor-100k"),
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
