//! Per-stage diagnostics, and a saturation register.
//!
//! Set `INKVEC_DIAG=1` for a structured line per stage on stderr; `INKVEC_DIAG=json` for
//! one JSON object per line, which is what a benchmark should parse.
//!
//! # Why this exists, and why it is not just more printing
//!
//! Every defect found in this pipeline on 2026-09-08 had the same shape: **a quantity
//! silently resting against a limit.** Not an error, not an exception -- a number that had
//! been clamped, floored, capped or gated, and then used as though it were a measurement.
//!
//! * `estimate_noise` returned its `0.5/255` floor on every image in the corpus, so a
//!   noise-gain divisor that was wrong by 1.83x could not be seen by any benchmark.
//! * The oversample gate never fired on natively rendered input, so a speckle floor stayed
//!   64x too small at 1024 px and anti-aliasing was promoted to geometry.
//! * `content_scale` returned `extent / 128` -- 12.5 where the measured answer was 4.
//! * The background matcher compared against two-decimal strings that the emitter had
//!   stopped writing, so every comparison failed and `--no-background` quietly did nothing.
//!
//! None of those produce a wrong-looking log line. They produce a *plausible* one. What
//! makes them findable is knowing that the value is against its stop, which is why
//! [`saturated`] exists alongside the plain `diag!` macro: it records the fact and prints
//! it differently.
//!
//! The rule this module is built on: **log the decision and the quantity that gated it.**
//! A stage that says "kept 6 of 41 candidates" is useful; a stage that says "kept 6 of 41,
//! 35 dropped as blends, noise 0.50/255 AT FLOOR" tells you where to look next without a
//! second run.

use std::sync::atomic::{AtomicU8, Ordering};

const OFF: u8 = 0;
const TEXT: u8 = 1;
const JSON: u8 = 2;
const UNSET: u8 = 255;

static MODE: AtomicU8 = AtomicU8::new(UNSET);

fn mode() -> u8 {
    let m = MODE.load(Ordering::Relaxed);
    if m != UNSET {
        return m;
    }
    let v = match inkvec_core::env::text("INKVEC_DIAG") {
        Some("json") => JSON,
        Some("0") | Some("") | None => OFF,
        Some(_) => TEXT,
    };
    MODE.store(v, Ordering::Relaxed);
    v
}

/// Is any diagnostic output wanted? Callers use this to skip work that only feeds the log.
#[inline]
pub fn on() -> bool {
    mode() != OFF
}

/// Emit one stage record: a stage name and `key=value` fields already formatted.
///
/// Prefer the `diag!` macro, which skips formatting entirely when diagnostics are off.
pub fn record(stage: &str, fields: &str) {
    match mode() {
        TEXT => eprintln!("  diag {stage:<14} {fields}"),
        JSON => eprintln!("{{\"stage\":\"{stage}\",{}}}", to_json(fields)),
        _ => {}
    }
}

/// `a=1 b=2` to `"a":"1","b":"2"`, so the text form stays the source of truth and the JSON
/// form cannot drift from it.
fn to_json(fields: &str) -> String {
    fields
        .split_whitespace()
        .filter_map(|kv| kv.split_once('='))
        .map(|(k, v)| format!("\"{k}\":\"{}\"", v.replace('"', "'")))
        .collect::<Vec<_>>()
        .join(",")
}

/// How a value came to rest where it is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stop {
    /// Held up by a lower bound.
    Floor,
    /// Held down by an upper bound.
    Cap,
    /// Stopped by an iteration or time budget rather than by converging.
    Budget,
    /// A conditional that never became true, so the code behind it never ran.
    GateNeverFired,
}

impl Stop {
    fn label(self) -> &'static str {
        match self {
            Stop::Floor => "AT FLOOR",
            Stop::Cap => "AT CAP",
            Stop::Budget => "BUDGET SPENT",
            Stop::GateNeverFired => "GATE NEVER FIRED",
        }
    }
}

/// Report a value that is resting against a limit rather than being determined by the data.
///
/// This is the whole point of the module. A value at its stop is not a measurement of the
/// image; it is a measurement of the constant next to it, and every stage downstream is
/// then reading a constant while believing it is reading the picture. Print it differently
/// so it can be found without knowing in advance to look.
pub fn saturated(stage: &str, name: &str, value: f64, limit: f64, stop: Stop) {
    match mode() {
        TEXT => eprintln!(
            "  diag {stage:<14} {name}={value:.5} {} ({limit:.5})",
            stop.label()
        ),
        JSON => eprintln!(
            "{{\"stage\":\"{stage}\",\"{name}\":\"{value}\",\"saturated\":\"{}\",\"limit\":\"{limit}\"}}",
            stop.label()
        ),
        _ => {}
    }
}

/// Record a stage, formatting the fields only when something is listening.
///
/// ```ignore
/// diag!("palette", "candidates={} accepted={} sigma={:.5}", n, kept, sigma);
/// ```
#[macro_export]
macro_rules! diag {
    ($stage:expr, $($arg:tt)*) => {
        if $crate::diag::on() {
            $crate::diag::record($stage, &format!($($arg)*));
        }
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fields_become_json_without_a_second_source_of_truth() {
        assert_eq!(to_json("a=1 b=2.5"), r#""a":"1","b":"2.5""#);
        assert_eq!(to_json(""), "");
        // A field with no `=` is skipped rather than producing invalid JSON.
        assert_eq!(to_json("lonely a=1"), r#""a":"1""#);
    }

    #[test]
    fn every_stop_has_a_distinct_label() {
        let all = [Stop::Floor, Stop::Cap, Stop::Budget, Stop::GateNeverFired];
        for (i, a) in all.iter().enumerate() {
            for b in &all[i + 1..] {
                assert_ne!(a.label(), b.label());
            }
        }
    }
}
