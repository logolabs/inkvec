//! The sampling caps and per-model timing counters every fill fit shares.
//!
//! The caps bound the cost of one fit independently of the region's size: a region is
//! gathered from at most [`FIT_PIXELS_CAP`] pixels, scored on at most
//! [`MAX_FIT_SAMPLES`] of them, and a centre search runs on at most
//! [`CENTRE_SEARCH_SAMPLES`]. Each subsampled sum is scaled back to the full count where
//! it enters a cost. The counters are process-wide and only printed under
//! `INKVEC_TIMING` (by `bands.rs`); they never affect the output.

use std::sync::atomic::{AtomicU64, Ordering};

/// Where a fit's time goes, in nanoseconds summed over threads, for the timing log:
/// gathering samples.
pub(crate) static FIT_NS_COLLECT: AtomicU64 = AtomicU64::new(0);
/// Time in the flat fit.
pub(crate) static FIT_NS_FLAT: AtomicU64 = AtomicU64::new(0);
/// Time in the linear fit.
pub(crate) static FIT_NS_LINEAR: AtomicU64 = AtomicU64::new(0);
/// Time in the circular radial fit.
pub(crate) static FIT_NS_RADIAL: AtomicU64 = AtomicU64::new(0);
/// Time in the elliptical radial fit.
pub(crate) static FIT_NS_ELLIPTIC: AtomicU64 = AtomicU64::new(0);

/// Add the time elapsed since `t` to `slot`.
pub(crate) fn tick(slot: &AtomicU64, t: inkvec_core::clock::Instant) {
    slot.fetch_add(t.elapsed().as_nanos() as u64, Ordering::Relaxed);
}

/// Most samples one fit scores: a larger sample set is strided down to about this many
/// (see [`fit_cap`]).
pub(crate) const MAX_FIT_SAMPLES: usize = 4096;
/// Pixels one fit gathers before it is sampled down to [`MAX_FIT_SAMPLES`] for the fitting
/// itself. (It was the
/// `INKVEC_FIT_PIXELS_CAP` knob; nothing set it.)
///
/// Gathering is not free — every pixel is tested for being interior (its four neighbours in
/// the same region), the survivors are sorted, and four arrays are gathered from them — and
/// on a band-heavy image that gathering is the largest single cost in the tracer. The
/// default gathers sixteen times what the fit will evaluate, which buys a sample spread
/// evenly over the *interior* rather than over the region; how much of that matters is a
/// question for the corpus, not for taste, which is why it was a knob first.
pub(crate) const FIT_PIXELS_CAP: usize = 65536;
/// Samples a radial or elliptical centre search evaluates per candidate centre; the
/// final stops are fitted on every sample.
pub(crate) const CENTRE_SEARCH_SAMPLES: usize = 1024;

/// Most samples one fit evaluates; a larger region is strided down to it.
pub(crate) fn fit_cap() -> usize {
    MAX_FIT_SAMPLES
}
