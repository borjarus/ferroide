//! Performance spikes and measurement helpers (Stage 0).
//!
//! Each spike binary opens a real window, drives it automatically and prints
//! one JSON [`Report`] line to stdout, so `cargo xtask spike` can collect
//! results on every platform and hardware profile.

use std::time::{Duration, Instant};

use serde::Serialize;

pub mod generate;

/// One spike's measurements.
#[derive(Debug, Clone, Serialize, Default)]
pub struct Report {
    pub spike: String,
    pub os: String,
    pub arch: String,
    pub profile: String,
    /// Process start → first frame.
    pub startup_ms: f64,
    /// Resident memory after the scenario.
    pub rss_mb: f64,
    pub frames: Option<FrameStats>,
    pub input_latency: Option<LatencyStats>,
    pub notes: Vec<String>,
}

impl Report {
    pub fn new(spike: &str) -> Self {
        Self {
            spike: spike.into(),
            os: std::env::consts::OS.into(),
            arch: std::env::consts::ARCH.into(),
            profile: if cfg!(debug_assertions) {
                "debug"
            } else {
                "release"
            }
            .into(),
            ..Self::default()
        }
    }

    pub fn print(&mut self) {
        self.rss_mb = rss_mb();
        println!("{}", serde_json::to_string(self).unwrap_or_default());
    }
}

/// Current resident set size in MB.
pub fn rss_mb() -> f64 {
    memory_stats::memory_stats()
        .map(|m| m.physical_mem as f64 / (1024.0 * 1024.0))
        .unwrap_or(0.0)
}

#[derive(Debug, Clone, Serialize)]
pub struct FrameStats {
    pub frames: usize,
    pub fps: f64,
    pub p50_ms: f64,
    pub p99_ms: f64,
    pub max_ms: f64,
}

#[derive(Debug, Clone, Serialize)]
pub struct LatencyStats {
    pub samples: usize,
    pub p50_ms: f64,
    pub p99_ms: f64,
    pub max_ms: f64,
}

fn percentile(sorted: &[f64], p: f64) -> f64 {
    if sorted.is_empty() {
        return 0.0;
    }
    let ix = ((sorted.len() - 1) as f64 * p).round() as usize;
    sorted[ix.min(sorted.len() - 1)]
}

/// Records frame timestamps during an animated scenario.
#[derive(Default)]
pub struct FrameClock {
    started: Option<Instant>,
    last: Option<Instant>,
    intervals: Vec<f64>,
}

impl FrameClock {
    pub fn tick(&mut self) {
        let now = Instant::now();
        self.started.get_or_insert(now);
        if let Some(last) = self.last.replace(now) {
            self.intervals.push((now - last).as_secs_f64() * 1000.0);
        }
    }

    pub fn elapsed(&self) -> Duration {
        self.started.map(|s| s.elapsed()).unwrap_or_default()
    }

    pub fn stats(&self) -> FrameStats {
        let mut sorted = self.intervals.clone();
        sorted.sort_by(f64::total_cmp);
        let total: f64 = sorted.iter().sum();
        FrameStats {
            frames: sorted.len(),
            fps: if total > 0.0 {
                sorted.len() as f64 * 1000.0 / total
            } else {
                0.0
            },
            p50_ms: percentile(&sorted, 0.50),
            p99_ms: percentile(&sorted, 0.99),
            max_ms: sorted.last().copied().unwrap_or(0.0),
        }
    }
}

pub fn latency_stats(samples: &[Duration]) -> LatencyStats {
    let mut sorted: Vec<f64> = samples.iter().map(|d| d.as_secs_f64() * 1000.0).collect();
    sorted.sort_by(f64::total_cmp);
    LatencyStats {
        samples: sorted.len(),
        p50_ms: percentile(&sorted, 0.50),
        p99_ms: percentile(&sorted, 0.99),
        max_ms: sorted.last().copied().unwrap_or(0.0),
    }
}

/// Command-line options shared by the spikes.
pub struct SpikeArgs {
    /// Run the automated scenario and exit (otherwise stay open for manual use).
    pub bench: bool,
    pub seconds: f64,
}

impl SpikeArgs {
    pub fn parse() -> Self {
        let args: Vec<String> = std::env::args().collect();
        let seconds = args
            .iter()
            .position(|a| a == "--seconds")
            .and_then(|i| args.get(i + 1))
            .and_then(|s| s.parse().ok())
            .unwrap_or(5.0);
        Self {
            bench: args.iter().any(|a| a == "--bench"),
            seconds,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frame_stats_compute_fps_and_percentiles() {
        let clock = FrameClock {
            started: None,
            last: None,
            intervals: vec![10.0; 99].into_iter().chain([50.0]).collect(),
        };
        let stats = clock.stats();
        assert_eq!(stats.frames, 100);
        assert!((stats.fps - 100.0 / 1.04).abs() < 0.01);
        assert_eq!(stats.p50_ms, 10.0);
        assert_eq!(stats.max_ms, 50.0);
    }
}
