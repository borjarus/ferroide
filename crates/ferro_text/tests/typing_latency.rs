//! Stage 3 Definition of Done: typing latency on a 100,000-line file.
//!
//! Each keystroke goes through what the editor core does per key: the edit
//! with undo grouping, selections, and the incremental display map with
//! soft wrap. The budget is the weak profile's 16 ms key-to-pixel (plan
//! §6.2); the core must use only a small part of it: this test allows
//! 2 ms at the 99th percentile in an optimized build. A debug build only
//! guards against order-of-magnitude regressions, with the whole 16 ms (slow
//! CI runners such as macOS Intel measure ~9 ms there). Release numbers:
//! `cargo run -p ferro_text --release --example typing_latency`.

use std::time::{Duration, Instant};

use ferro_text::{
    Editor, Selection, SelectionSet,
    display_map::{DisplayMap, DisplayMapConfig},
};

const LINES: usize = 100_000;
const P99_BUDGET: Duration = if cfg!(debug_assertions) {
    Duration::from_millis(16)
} else {
    Duration::from_millis(2)
};

fn source() -> String {
    let mut text = String::with_capacity(LINES * 40);
    for i in 0..LINES {
        match i % 4 {
            0 => text.push_str(&format!("fn function_{i}(value: u32) -> u32 {{\n")),
            1 => text.push_str("\tlet doubled = value * 2; // zażółć 漢字\n"),
            2 => text.push_str("    doubled + 1\n"),
            _ => text.push_str("}\n"),
        }
    }
    text
}

fn percentile(samples: &mut [Duration], p: f64) -> Duration {
    samples.sort();
    samples[((samples.len() as f64 - 1.0) * p).round() as usize]
}

#[test]
fn typing_in_a_100k_line_file_fits_the_budget() {
    let text = source();
    let mut editor = Editor::new(&text);
    let config = DisplayMapConfig {
        tab_size: 4,
        wrap_column: Some(80),
    };
    let started = Instant::now();
    let mut map = DisplayMap::new(editor.text(), config);
    let layout_time = started.elapsed();
    assert!(map.row_count() >= LINES);

    // Type in the middle of the file, with a line break every 30 keys.
    let middle = editor.buffer().line_start(LINES / 2) + 4;
    editor.set_selections(SelectionSet::single(Selection::cursor(middle)));
    let keys: Vec<&str> = "let typed_value = compute(42); "
        .split("")
        .filter(|s| !s.is_empty())
        .chain(["\n"])
        .collect();
    let mut samples = Vec::new();
    let t0 = Instant::now();
    for i in 0..300 {
        let key = keys[i % keys.len()];
        let before = editor.text().clone();
        let started = Instant::now();
        editor.insert(key, t0 + Duration::from_millis(i as u64 * 30));
        let change = editor.buffer().last_change().expect("an edit").to_vec();
        map.sync_summaries(&before, editor.text(), &change);
        let caret = editor.selections().primary().head;
        std::hint::black_box(map.to_display(editor.text(), caret));
        samples.push(started.elapsed());
    }
    let p50 = percentile(&mut samples, 0.5);
    let p99 = percentile(&mut samples, 0.99);
    eprintln!("100k lines: initial layout {layout_time:?}, keystroke p50 {p50:?}, p99 {p99:?}");
    assert!(p99 < P99_BUDGET, "p99 {p99:?} over {P99_BUDGET:?}");

    // All the typing undoes in a few grouped steps.
    let mut steps = 0;
    while editor.undo() {
        steps += 1;
    }
    assert_eq!(editor.text().to_string(), text);
    assert!(steps < 300, "typing was grouped into {steps} undo steps");
}
