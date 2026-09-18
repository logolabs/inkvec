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
pub(crate) const FIT_PIXELS_CAP: usize = 65536;
pub(crate) const CENTRE_SEARCH_SAMPLES: usize = 1024;

/// Most samples one fit evaluates; a larger region is strided down to it.
pub(crate) fn fit_cap() -> usize {
    MAX_FIT_SAMPLES
}
