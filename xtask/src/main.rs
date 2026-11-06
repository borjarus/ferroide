//! Build automation. Run as `cargo xtask <command>`.
//!
//! ```text
//! cargo xtask ci                 format check, clippy, tests, cargo-deny
//! cargo xtask spike [--seconds N] [--profile spike|release]
//!                                build and run the Stage 0 spikes, write
//!                                docs/reports/spike-<os>-<arch>.json and
//!                                print the budget table
//! ```

use std::{
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

use anyhow::{Context as _, Result, bail};

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("ci") => ci(),
        Some("spike") => spike(&args[1..]),
        _ => {
            eprintln!("usage: cargo xtask <ci | spike [--seconds N] [--profile NAME]>");
            std::process::exit(2);
        }
    }
}

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("xtask lives in the workspace root")
        .to_path_buf()
}

fn run(cmd: &mut Command) -> Result<()> {
    eprintln!("$ {cmd:?}");
    let status = cmd
        .current_dir(root())
        .status()
        .with_context(|| format!("running {cmd:?}"))?;
    if !status.success() {
        bail!("{cmd:?} failed with {status}");
    }
    Ok(())
}

fn has(tool: &str) -> bool {
    Command::new("cargo")
        .args([tool, "--version"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

fn ci() -> Result<()> {
    run(Command::new("cargo").args(["fmt", "--all", "--check"]))?;
    run(Command::new("cargo").args([
        "clippy",
        "--workspace",
        "--all-targets",
        "--",
        "-D",
        "warnings",
    ]))?;
    if has("nextest") {
        run(Command::new("cargo").args(["nextest", "run", "--workspace"]))?;
    } else {
        run(Command::new("cargo").args(["test", "--workspace"]))?;
    }
    if has("deny") {
        run(Command::new("cargo").args(["deny", "check"]))?;
    } else {
        eprintln!("cargo-deny not installed; skipping license/advisory check");
    }
    Ok(())
}

/// A budget row: metric, value, medium-profile limit, unit, lower-is-better.
struct Budget {
    name: &'static str,
    value: f64,
    limit: f64,
    unit: &'static str,
    lower_is_better: bool,
}

fn spike(args: &[String]) -> Result<()> {
    let opt = |name: &str| {
        args.iter()
            .position(|a| a == name)
            .and_then(|i| args.get(i + 1))
            .cloned()
    };
    let profile = opt("--profile").unwrap_or_else(|| "spike".into());
    let seconds = opt("--seconds").unwrap_or_else(|| "5".into());

    run(Command::new("cargo").args([
        "build",
        "--profile",
        &profile,
        "-p",
        "ferro_perf",
        "-p",
        "ferro",
    ]))?;
    let bin_dir = root().join("target").join(&profile);
    let exe = |name: &str| bin_dir.join(format!("{name}{}", std::env::consts::EXE_SUFFIX));

    let mut reports = Vec::new();
    for spike in ["spike_dock", "spike_list", "spike_editor"] {
        let output = Command::new(exe(spike))
            .args(["--bench", "--seconds", &seconds])
            .current_dir(root())
            .output()
            .with_context(|| format!("running {spike}"))?;
        let stdout = String::from_utf8_lossy(&output.stdout);
        let line = stdout
            .lines()
            .rev()
            .find(|l| l.starts_with('{'))
            .with_context(|| {
                format!(
                    "{spike} printed no report; stderr:\n{}",
                    String::from_utf8_lossy(&output.stderr)
                )
            })?;
        reports.push(serde_json_lite::parse(line)?);
    }

    // Application cold start to first frame, median of 5.
    let mut startups = Vec::new();
    for _ in 0..5 {
        let output = Command::new(exe("ferroide"))
            .args(["--demo", "--exit-after-first-frame"])
            .current_dir(root())
            .output()
            .context("running ferroide")?;
        let stdout = String::from_utf8_lossy(&output.stdout);
        if let Some(ms) = stdout
            .lines()
            .find_map(|l| l.strip_prefix("startup_ms="))
            .and_then(|v| v.trim().parse::<f64>().ok())
        {
            startups.push(ms);
        }
    }
    startups.sort_by(f64::total_cmp);
    let app_startup = startups
        .get(startups.len() / 2)
        .copied()
        .unwrap_or(f64::NAN);

    let get = |spike: &str, path: &[&str]| -> f64 {
        reports
            .iter()
            .find(
                |r| matches!(r.get(&["spike"]), Some(serde_json_lite::Value::Str(s)) if s == spike),
            )
            .and_then(|r| r.get(path))
            .and_then(|v| v.as_f64())
            .unwrap_or(f64::NAN)
    };

    let budgets = [
        Budget {
            name: "App cold start → first frame (median of 5)",
            value: app_startup,
            limit: 400.0,
            unit: "ms",
            lower_is_better: true,
        },
        Budget {
            name: "Key → frame p99 (editor, 100k lines)",
            value: get("editor-100k", &["input_latency", "p99_ms"]),
            limit: 8.0,
            unit: "ms",
            lower_is_better: true,
        },
        Budget {
            name: "Scrolling 100k-line file",
            value: get("editor-100k", &["frames", "fps"]),
            limit: 120.0,
            unit: "FPS",
            lower_is_better: false,
        },
        Budget {
            name: "Scrolling 1M-row list",
            value: get("list-1m", &["frames", "fps"]),
            limit: 120.0,
            unit: "FPS",
            lower_is_better: false,
        },
        Budget {
            name: "Resizing dock every frame",
            value: get("dock-3-panels", &["frames", "fps"]),
            limit: 120.0,
            unit: "FPS",
            lower_is_better: false,
        },
        Budget {
            name: "Memory: dock window",
            value: get("dock-3-panels", &["rss_mb"]),
            limit: 120.0,
            unit: "MB",
            lower_is_better: true,
        },
    ];

    let os = std::env::consts::OS;
    let arch = std::env::consts::ARCH;
    let mut table = String::from(
        "| Metric | Measured | Budget (medium profile) | Status |\n|---|---|---|---|\n",
    );
    for b in &budgets {
        let ok = if b.lower_is_better {
            b.value <= b.limit
        } else {
            b.value >= b.limit
        };
        let status = if b.value.is_nan() {
            "n/a"
        } else if ok {
            "✔ within"
        } else {
            "✘ over"
        };
        let op = if b.lower_is_better { "<" } else { "≥" };
        table.push_str(&format!(
            "| {} | {:.1} {} | {op} {} {} | {status} |\n",
            b.name, b.value, b.unit, b.limit, b.unit
        ));
    }
    println!("{table}");

    let reports_dir = root().join("docs").join("reports");
    std::fs::create_dir_all(&reports_dir)?;
    let json_path = reports_dir.join(format!("spike-{os}-{arch}.json"));
    let raw: Vec<&str> = reports.iter().map(|r| r.raw.as_str()).collect();
    std::fs::write(
        &json_path,
        format!(
            "{{\"profile\":\"{profile}\",\"app_startup_ms\":{app_startup:.1},\"spikes\":[{}]}}\n",
            raw.join(",")
        ),
    )?;
    eprintln!("wrote {}", json_path.display());
    Ok(())
}

/// A minimal JSON reader, so xtask stays dependency-light.
mod serde_json_lite {
    use anyhow::{Result, bail};

    pub struct Doc {
        pub raw: String,
    }

    pub enum Value<'a> {
        Num(f64),
        Str(&'a str),
    }

    impl Value<'_> {
        pub fn as_f64(&self) -> Option<f64> {
            match self {
                Value::Num(n) => Some(*n),
                Value::Str(_) => None,
            }
        }
    }

    pub fn parse(line: &str) -> Result<Doc> {
        if !line.trim_start().starts_with('{') {
            bail!("not a JSON object: {line}");
        }
        Ok(Doc {
            raw: line.trim().to_string(),
        })
    }

    impl Doc {
        /// Look up a nested key path like `["frames", "fps"]`. Good enough for
        /// the flat, known shape of `ferro_perf::Report`.
        pub fn get(&self, path: &[&str]) -> Option<Value<'_>> {
            let mut haystack: &str = &self.raw;
            for (i, key) in path.iter().enumerate() {
                let needle = format!("\"{key}\":");
                let at = haystack.find(&needle)? + needle.len();
                haystack = &haystack[at..];
                if i + 1 < path.len() && !haystack.starts_with('{') {
                    return None;
                }
            }
            if let Some(rest) = haystack.strip_prefix('"') {
                let end = rest.find('"')?;
                return Some(Value::Str(&rest[..end]));
            }
            let end = haystack.find([',', '}']).unwrap_or(haystack.len());
            haystack[..end].trim().parse().ok().map(Value::Num)
        }
    }
}
