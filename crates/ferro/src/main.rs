//! FerroIDE entry point.
//!
//! ```text
//! ferroide              welcome screen
//! ferroide --demo       open the demo project directly
//! ferroide --lang pl    override the UI language for this run
//! ```
//!
//! The time from process start to the first rendered frame (the Stage 0
//! startup budget) is logged at `info` level under `ferro::startup`;
//! `--exit-after-first-frame` prints it as `startup_ms=…` and quits, which is
//! what `cargo xtask spike` measures.

#![cfg_attr(
    all(not(debug_assertions), target_os = "windows"),
    windows_subsystem = "windows"
)]

use std::time::Instant;

use ferro_settings::Paths;
use ferro_workspace::AppState;
use mimalloc::MiMalloc;

#[global_allocator]
static GLOBAL: MiMalloc = MiMalloc;

#[derive(Default)]
struct Args {
    demo: bool,
    locale: Option<ferro_i18n::Locale>,
    exit_after_first_frame: bool,
}

fn parse_args() -> Args {
    let mut args = Args::default();
    let mut iter = std::env::args().skip(1);
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--demo" => args.demo = true,
            "--lang" => {
                args.locale = iter
                    .next()
                    .and_then(|code| ferro_i18n::Locale::from_code(&code))
            }
            "--exit-after-first-frame" => args.exit_after_first_frame = true,
            "--help" | "-h" => {
                println!("Usage: ferroide [--demo] [--lang en|pl] [--exit-after-first-frame]");
                std::process::exit(0);
            }
            other => eprintln!("ignoring unknown argument {other}"),
        }
    }
    args
}

fn main() {
    let started = Instant::now();
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info,gpui=warn".into()),
        )
        .init();

    let args = parse_args();
    let paths = Paths::standard();
    let mut settings = paths.load_settings();
    if let Some(locale) = args.locale {
        settings.locale = locale;
    }

    gpui_kit::application()
        .with_assets(ferro_ui::FerroAssets)
        .run(move |cx| {
            gpui_kit::init(cx);
            ferro_ui::init(cx);
            AppState::init(settings, paths, true, cx);
            ferro_workspace::init(cx);

            let opened = if args.demo {
                ferro_workspace::open_workspace(ferro_workspace::mock_services(), cx)
                    .map(|(window, _)| window)
            } else {
                ferro_workspace::open_welcome(cx).map(|(window, _)| window)
            };
            let window = match opened {
                Ok(window) => window,
                Err(err) => {
                    tracing::error!("could not open a window: {err:#}");
                    cx.quit();
                    return;
                }
            };

            let exit = args.exit_after_first_frame;
            let _ = window.update(cx, move |_, window, _| {
                window.on_next_frame(move |_, cx| {
                    let elapsed = started.elapsed();
                    tracing::info!(
                        target: "ferro::startup",
                        "first frame after {:.1} ms",
                        elapsed.as_secs_f64() * 1000.0
                    );
                    if exit {
                        println!("startup_ms={:.1}", elapsed.as_secs_f64() * 1000.0);
                        cx.quit();
                    }
                });
            });
        });
}
