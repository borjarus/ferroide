//! Stage 7 Definition of Done (plan §6.2): Quick Open over 100k files, one
//! result per keystroke, and Find in Files over a 1 GB repository.
//!
//! ```text
//! cargo run -p ferro_index --release --example search_bench -- [--dir <folder>] [--mb 1024] [--files 100000]
//! ```
//!
//! The text corpus is generated once in `--dir` (default: a temporary
//! folder) and reused when it already has the requested size. Reported
//! times are wall-clock; the corpus is in the OS cache after generation,
//! which is the common case for a project being worked on.

use std::{
    fs,
    io::Write as _,
    path::{Path, PathBuf},
    time::Instant,
};

use ferro_index::{Candidate, fuzzy::PathMatcher, search_files};
use ferro_services::TextQuery;

fn arg(name: &str) -> Option<String> {
    let args: Vec<String> = std::env::args().collect();
    args.iter()
        .position(|a| a == name)
        .and_then(|i| args.get(i + 1).cloned())
}

fn percentile(sorted: &[f64], p: f64) -> f64 {
    sorted[((sorted.len() - 1) as f64 * p).round() as usize]
}

/// Paths shaped like a large monorepo.
fn synthetic_paths(count: usize) -> Vec<String> {
    let areas = [
        "src", "lib", "tests", "packages", "services", "tools", "docs",
    ];
    let names = [
        "editor",
        "workspace",
        "project",
        "search",
        "index",
        "parser",
        "render",
        "network",
        "storage",
        "config",
        "theme",
        "keymap",
        "terminal",
        "debug",
        "language",
    ];
    let exts = [
        "rs", "ts", "tsx", "go", "py", "java", "md", "json", "toml", "c",
    ];
    (0..count)
        .map(|i| {
            format!(
                "{}/{}_{}/{}/{}_{}_{i}.{}",
                areas[i % areas.len()],
                names[i % names.len()],
                i % 97,
                names[(i / 7) % names.len()],
                names[(i / 3) % names.len()],
                ["model", "view", "service", "utils", "tests"][i % 5],
                exts[i % exts.len()]
            )
        })
        .collect()
}

fn quick_open(files: usize) {
    let paths = synthetic_paths(files);
    let started = Instant::now();
    let mut matcher = PathMatcher::new(paths);
    println!(
        "quick open: {} paths prepared in {:.1} ms",
        matcher.len(),
        started.elapsed().as_secs_f64() * 1000.0
    );
    let mut worst = Vec::new();
    for query in [
        "workspacemodel",
        "src/edit/view",
        "searchidx",
        "termutilsrs",
        "cfg json",
    ] {
        let mut times = Vec::new();
        for end in 1..=query.len() {
            let typed = &query[..end];
            let started = Instant::now();
            let found = matcher.search(typed, 200);
            times.push(started.elapsed().as_secs_f64() * 1000.0);
            std::hint::black_box(found);
        }
        let max = times.iter().cloned().fold(0.0, f64::max);
        println!(
            "  {query:<16} first keystroke {:>6.2} ms, worst {:>6.2} ms, all {}",
            times[0],
            max,
            times
                .iter()
                .map(|t| format!("{t:.1}"))
                .collect::<Vec<_>>()
                .join(" ")
        );
        worst.extend(times);
    }
    worst.sort_by(f64::total_cmp);
    println!(
        "quick open per keystroke: p50 {:.2} ms, p99 {:.2} ms, max {:.2} ms (budget 30 ms medium / 60 ms weak)",
        percentile(&worst, 0.5),
        percentile(&worst, 0.99),
        worst.last().copied().unwrap_or_default()
    );
}

/// Write `mb` megabytes of source-like text in 50 kB files.
fn corpus(dir: &Path, mb: u64) -> Vec<PathBuf> {
    const FILE: usize = 50 * 1024;
    let count = (mb * 1024 * 1024) as usize / FILE;
    let marker = dir.join(format!("corpus-{mb}mb.done"));
    let path_of = |i: usize| {
        dir.join(format!("d{:03}", i % 400))
            .join(format!("f{i:06}.rs"))
    };
    if !marker.exists() {
        println!(
            "generating {mb} MB in {} files under {} …",
            count,
            dir.display()
        );
        let started = Instant::now();
        let words = [
            "let", "value", "config", "fn", "return", "match", "Some", "None", "self", "impl",
            "struct", "buffer", "index", "search", "result", "error", "context", "window",
        ];
        for i in 0..count {
            let path = path_of(i);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            let mut text = String::with_capacity(FILE + 128);
            let mut n = i.wrapping_mul(2654435761);
            let mut line = 0;
            while text.len() < FILE {
                text.push_str("    ");
                for _ in 0..8 {
                    n = n
                        .wrapping_mul(6364136223846793005)
                        .wrapping_add(1442695040888963407);
                    text.push_str(words[(n >> 33) % words.len()]);
                    text.push(' ');
                }
                // A rare needle: one line in ~10,000.
                if (i * 131 + line) % 9973 == 0 {
                    text.push_str("FERRO_NEEDLE_42");
                }
                text.push('\n');
                line += 1;
            }
            fs::File::create(&path)
                .unwrap()
                .write_all(text.as_bytes())
                .unwrap();
        }
        fs::write(&marker, b"").unwrap();
        println!("  generated in {:.1} s", started.elapsed().as_secs_f64());
    }
    (0..count).map(path_of).collect()
}

fn find_in_files(dir: &Path, mb: u64) {
    let files = corpus(dir, mb);
    let candidates: Vec<Candidate> = files
        .iter()
        .map(|p| Candidate {
            shown: p
                .strip_prefix(dir)
                .unwrap()
                .to_string_lossy()
                .replace('\\', "/"),
            path: p.clone(),
        })
        .collect();
    let queries = [
        ("rare literal", "FERRO_NEEDLE_42", false, true),
        ("rare literal, ignore case", "ferro_needle_42", false, false),
        ("regex", r"NEEDLE_\d+", true, true),
        ("no match", "zzz_not_there_zzz", false, true),
    ];
    // Warm the OS cache once (the corpus was just written or read before).
    let _ = search_files(
        &candidates,
        &TextQuery {
            text: "zzz_warm_zzz".into(),
            case_sensitive: true,
            ..TextQuery::default()
        },
        2_000,
    );
    // The floor: reading every file in parallel without searching.
    let started = Instant::now();
    let cursor = std::sync::atomic::AtomicUsize::new(0);
    let bytes = std::sync::atomic::AtomicU64::new(0);
    std::thread::scope(|scope| {
        for _ in 0..std::thread::available_parallelism().map_or(4, |n| n.get()) {
            scope.spawn(|| {
                loop {
                    let ix = cursor.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    let Some(path) = files.get(ix) else { break };
                    let n = fs::read(path).map_or(0, |b| b.len());
                    bytes.fetch_add(n as u64, std::sync::atomic::Ordering::Relaxed);
                }
            });
        }
    });
    println!(
        "reading {} MB without searching: {:.3} s",
        bytes.into_inner() / (1024 * 1024),
        started.elapsed().as_secs_f64()
    );
    for (name, text, regex, case_sensitive) in queries {
        let query = TextQuery {
            text: text.into(),
            regex,
            case_sensitive,
            ..TextQuery::default()
        };
        let mut times = Vec::new();
        let mut found = 0;
        for _ in 0..3 {
            let started = Instant::now();
            found = search_files(&candidates, &query, 2_000).unwrap().len();
            times.push(started.elapsed().as_secs_f64());
        }
        times.sort_by(f64::total_cmp);
        println!(
            "find in files, {mb} MB, {name:<26}: {:>5} lines, median {:.3} s (budget 1 s medium / 3 s weak)",
            found, times[1]
        );
    }
}

fn main() {
    let files: usize = arg("--files")
        .and_then(|v| v.parse().ok())
        .unwrap_or(100_000);
    let mb: u64 = arg("--mb").and_then(|v| v.parse().ok()).unwrap_or(1024);
    quick_open(files);
    let temp;
    let dir = match arg("--dir") {
        Some(dir) => PathBuf::from(dir),
        None => {
            temp = std::env::temp_dir().join("ferro-search-bench");
            temp
        }
    };
    if mb > 0 {
        find_in_files(&dir, mb);
    }
}
