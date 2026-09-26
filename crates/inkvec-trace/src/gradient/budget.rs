//! The sampling caps and per-model timing counters every fill fit shares.

use std::sync::atomic::{AtomicU64, Ordering};

/// Where a fit's time goes, in nanoseconds, for the timing log.
pub(crate) static FIT_NS_COLLECT: AtomicU64 = AtomicU64::new(0);
pub(crate) static FIT_NS_FLAT: AtomicU64 = AtomicU64::new(0);
pub(crate) static FIT_NS_LINEAR: AtomicU64 = AtomicU64::new(0);
pub(crate) static FIT_NS_RADIAL: AtomicU64 = AtomicU64::new(0);
pub(crate) static FIT_NS_ELLIPTIC: AtomicU64 = AtomicU64::new(0);

/// Add the time elapsed since `t` to `slot`.
pub(crate) fn tick(slot: &AtomicU64, t: inkvec_core::clock::Instant) {
    slot.fetch_add(t.elapsed().as_nanos() as u64, Ordering::Relaxed);
}

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
pub(crate) const CENTRE_SEARCH_SAMPLES: usize = 1024;

/// Most samples one fit evaluates; a larger region is strided down to it.
pub(crate) fn fit_cap() -> usize {
    MAX_FIT_SAMPLES
}
