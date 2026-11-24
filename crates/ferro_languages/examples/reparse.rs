//! Time incremental reparsing of 10k-line Rust files (plan Stage 5 DoD):
//! `cargo run -p ferro_languages --release --example reparse`.
//!
//! Two shapes: code grouped in modules and `impl` blocks, as real code is,
//! and the worst case of ~1,700 functions at the top level, where
//! tree-sitter itself spends milliseconds per edit.

use std::time::{Duration, Instant};

use ferro_syntax::SyntaxTree;
use ferro_text::{LINES, Rope};

const METHOD: &str = "    /// Adds two numbers.\n    pub fn add_N(&self, b: u64) -> u64 {\n        let sum = self.value.wrapping_add(b);\n        if sum > 10 { sum - 1 } else { sum + 1 }\n    }\n\n";

fn nested() -> String {
    let mut text = String::new();
    let mut n = 0;
    while text.lines().count() < 10_000 {
        text.push_str(&format!(
            "pub mod m{n} {{\npub struct Counter{n} {{ value: u64 }}\n\nimpl Counter{n} {{\n"
        ));
        for _ in 0..20 {
            text.push_str(&METHOD.replace('N', &n.to_string()));
            n += 1;
        }
        text.push_str("}\n}\n\n");
    }
    text
}

fn flat() -> String {
    let mut text = String::new();
    let mut n = 0;
    while text.lines().count() < 10_000 {
        text.push_str(
            &METHOD
                .replace('N', &n.to_string())
                .replace("&self", "value: u64")
                .replace("self.value", "value"),
        );
        n += 1;
    }
    text
}

fn main() {
    let registry = ferro_languages::LanguageRegistry::shared_builtin();
    let syntax = registry.get("rust").unwrap().syntax().unwrap();
    for (shape, text) in [
        ("modules + impl blocks", nested()),
        ("flat, top-level functions", flat()),
    ] {
        let mut rope = Rope::from_str(&text);
        let started = Instant::now();
        let mut tree = SyntaxTree::parse(syntax.clone(), &rope).unwrap();
        let full = started.elapsed();
        for (edit, column) in [
            ("typing in a name", 13usize),
            ("typing at a line start (syntax error)", 8),
        ] {
            let mut times: Vec<Duration> = Vec::new();
            for i in 0..60 {
                let line = (5_000..)
                    .map(|l| l + i * 7)
                    .find(|&l| rope.line(l, LINES).to_string().contains("let sum"))
                    .unwrap();
                let at = rope.line_to_byte_idx(line, LINES) + column;
                if i % 2 == 0 {
                    rope.insert(at, "x");
                } else {
                    rope.remove(at..at + 1);
                }
                let started = Instant::now();
                tree.reparse(&rope).unwrap();
                times.push(started.elapsed());
            }
            times.sort();
            println!(
                "{shape}: full parse {full:?}; {edit}: median {:?}, p90 {:?}",
                times[times.len() / 2],
                times[times.len() * 9 / 10]
            );
        }
    }
}
