//! How long a superseded trace takes to stop, measured.
//!
//!     cargo run --release -p inkvec-studio-core --example cancel_latency -- IMAGE [quality|fast] [TRACE_PX]
//!
//! Traces IMAGE once to time it, then again under a progress that is cancelled at a series
//! of points through the run, and reports how long each took to stop after the cancel. The
//! "before" column is what the same moment cost before traces could be stopped: the rest
//! of the run, which the next trace had to wait out for the slot.

use std::sync::Arc;
use std::time::{Duration, Instant};

use inkvec_studio_core::options::{Settings, TraceMode};
use inkvec_studio_core::progress::{self, Progress};
use inkvec_studio_core::trace::{self, MeasureLevel, Outcome, Source, Tier};

fn main() {
    let mut args = std::env::args().skip(1);
    let path = args.next().expect("an image path");
    let mode = match args.next().as_deref() {
        Some("fast") => TraceMode::Fast,
        _ => TraceMode::Quality,
    };
    let trace_size = args.next().and_then(|s| s.parse().ok()).unwrap_or(2048);
    let bytes = std::fs::read(&path).expect("readable image");
    let settings = Settings {
        mode,
        trace_size,
        ..Settings::default()
    };

    // A fresh source per run, so no run is helped by a raster another decoded.
    let source = || Arc::new(Source::open(bytes.clone(), None).expect("an image"));
    let draw = |src: &Arc<Source>| {
        trace::draw(
            src,
            &settings,
            Tier::Final,
            None,
            MeasureLevel::Full,
            |_, _| {},
        )
    };

    let t = Instant::now();
    let live = Arc::new(Progress::new());
    let full = progress::with(Arc::clone(&live), || draw(&source()));
    let total = t.elapsed();
    assert!(full.is_ok(), "the uncancelled trace failed");
    let begins: Vec<(f64, &'static str)> = live
        .take()
        .entries
        .into_iter()
        .filter_map(|e| match e {
            progress::Entry::Begin { stage, at_ms } => Some((at_ms, stage)),
            _ => None,
        })
        .collect();
    println!(
        "{path} ({mode:?}, {trace_size} px): {:.2} s uncancelled",
        total.as_secs_f64()
    );

    println!(
        "{:>9} {:>18} {:>12} {:>12}",
        "cancel at", "stage", "stop after", "before"
    );
    let mut after_ms = Vec::new();
    for tenth in 1..=19 {
        let at = total.mul_f64(tenth as f64 / 20.0);
        let src = source();
        let live = Arc::new(Progress::new());
        let canceller = {
            let live = Arc::clone(&live);
            std::thread::spawn(move || {
                std::thread::sleep(at);
                live.cancel();
                Instant::now()
            })
        };
        let outcome = progress::with(Arc::clone(&live), || draw(&src));
        let done = Instant::now();
        let asked = canceller.join().unwrap();
        let stopped = done.saturating_duration_since(asked);
        let stage = begins
            .iter()
            .rev()
            .find(|(ms, _)| *ms <= at.as_secs_f64() * 1e3)
            .map_or("start", |(_, s)| *s);
        let ok = matches!(outcome, Err(Outcome::Cancelled));
        let before = total.saturating_sub(at);
        println!(
            "{:>7.2} s {:>18} {:>9.0} ms {:>9.0} ms{}",
            at.as_secs_f64(),
            stage,
            stopped.as_secs_f64() * 1e3,
            before.as_secs_f64() * 1e3,
            if ok { "" } else { "   (finished first)" }
        );
        if ok {
            after_ms.push(stopped.as_secs_f64() * 1e3);
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    after_ms.sort_by(f64::total_cmp);
    if !after_ms.is_empty() {
        let median = after_ms[after_ms.len() / 2];
        let max = after_ms[after_ms.len() - 1];
        println!(
            "stop latency: median {median:.0} ms, max {max:.0} ms over {} cancels; before: median {:.0} ms, max {:.0} ms",
            after_ms.len(),
            total.as_secs_f64() * 1e3 * 0.5,
            total.as_secs_f64() * 1e3 * 0.95
        );
    }
}
