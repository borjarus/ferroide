//! Measure the editor core's per-keystroke cost on a 100,000-line file:
//! the edit (with undo grouping) plus the incremental display map with
//! soft wrap.
//!
//! ```sh
//! cargo run -p ferro_text --release --example typing_latency
//! ```

use std::time::{Duration, Instant};

use ferro_text::{
    Editor, Selection, SelectionSet,
    display_map::{DisplayMap, DisplayMapConfig},
};

fn main() {
    let lines: usize = std::env::args()
        .nth(1)
        .and_then(|a| a.parse().ok())
        .unwrap_or(100_000);
    let mut text = String::new();
    for i in 0..lines {
        text.push_str(&format!(
            "    let value_{i} = compute({i}) + 1; // zażółć\n"
        ));
    }

    let started = Instant::now();
    let mut editor = Editor::new(&text);
    let load = started.elapsed();
    let started = Instant::now();
    let mut map = DisplayMap::new(
        editor.text(),
        DisplayMapConfig {
            tab_size: 4,
            wrap_column: Some(80),
        },
    );
    let layout = started.elapsed();

    let at = editor.buffer().line_start(lines / 2);
    editor.set_selections(SelectionSet::single(Selection::cursor(at)));
    let mut samples = Vec::with_capacity(2000);
    let t0 = Instant::now();
    for i in 0..2000u64 {
        let key = if i % 40 == 39 { "\n" } else { "x" };
        let before = editor.text().clone();
        let started = Instant::now();
        editor.insert(key, t0 + Duration::from_millis(i * 30));
        let change = editor.buffer().last_change().unwrap_or_default().to_vec();
        map.sync_summaries(&before, editor.text(), &change);
        std::hint::black_box(map.to_display(editor.text(), editor.selections().primary().head));
        samples.push(started.elapsed());
    }
    samples.sort();
    let at = |p: f64| samples[((samples.len() - 1) as f64 * p) as usize];
    println!("lines:            {lines}");
    println!("buffer load:      {load:?}");
    println!("display layout:   {layout:?}");
    println!("keystroke p50:    {:?}", at(0.5));
    println!("keystroke p99:    {:?}", at(0.99));
    println!("keystroke max:    {:?}", samples[samples.len() - 1]);
}
