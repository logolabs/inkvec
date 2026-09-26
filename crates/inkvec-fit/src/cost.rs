//! The prices the fit charges, chosen per trace rather than per process.
//!
//! The MDL objective `0.5·chi² + λ·params` is only as good as what it charges a segment. Two
//! of those charges decide how willing the fit is to draw a curve:
//!
//! * **`cubic_params`** — what one Bézier segment costs, in parameters. A line costs two
//!   ([`crate::PARAMS_LINE`]); a Bézier's two control points and endpoint cost six. Charging
//!   a curve three times a line is why the output is 21 % curved where hand-drawn artwork is
//!   78 % curved: a chain of short lines is cheaper than the one curve that describes it.
//!   Lowering it buys more curves and fewer lines.
//! * **`g1_break_degrees`** — the turn, at a join, that is charged as a full corner. Below it
//!   the charge ramps quadratically, so gentle bends between lines are nearly free; raising
//!   it changes which bends read as smooth and which as corners.
//!
//! Both used to be process-wide values, read once from `INKVEC_PARAMS_CUBIC` and
//! `INKVEC_G1_BREAK`, which is fine for an experiment and no use to an application that
//! traces different images with different wishes in one process. They are now a
//! [`CostModel`] a caller can set for the duration of one trace with [`with_cost_model`]
//! (`--bezier-cost` and `--corner-angle` on the command line). The defaults are exactly
//! what they were, so a trace that asks for nothing is byte-identical to what it was; the
//! two environment variables are gone.
//!
//! # How the scope works
//!
//! The values live in two atomics that the fitter reads, so they are visible on every rayon
//! worker the trace spawns; a thread-local would not be. Because they are shared, a trace
//! that changes them must be the only trace running: it takes the write side of a gate and
//! restores the standard values when it leaves. A trace that asks for the standard values
//! takes the read side, so ordinary traces still run in parallel with one another. Nested
//! calls on the same thread inherit the scope they are inside rather than taking the gate
//! again, which would deadlock.

use std::cell::Cell;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::RwLock;

/// The prices a trace charges for a curve.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CostModel {
    /// Parameters charged for one Bézier segment.
    pub cubic_params: f64,
    /// Degrees of turn, at a join, charged as a full corner.
    pub g1_break_degrees: f64,
}

impl CostModel {
    /// The range `cubic_params` is held to. Below a line's own price a curve would be
    /// cheaper than the segment it is meant to compete with; above twelve none is drawn.
    pub const CUBIC_RANGE: (f64, f64) = (2.0, 12.0);
    /// The range `g1_break_degrees` is held to.
    pub const G1_RANGE: (f64, f64) = (1.0, 60.0);

    /// The shipped prices: a Bézier costs its six numbers, and a turn of
    /// [`crate::tangents::G1_BREAK_DEGREES`] is a full corner.
    pub const STANDARD: CostModel = CostModel {
        cubic_params: 6.0,
        g1_break_degrees: crate::tangents::G1_BREAK_DEGREES,
    };

    /// The process default, [`CostModel::STANDARD`].
    pub fn standard() -> Self {
        Self::STANDARD
    }

    /// The standard prices with any that were asked for replaced, and held to their range.
    /// `None` leaves a price where it was, so a caller that asks for nothing gets exactly
    /// [`CostModel::standard`].
    pub fn with_overrides(cubic_params: Option<f64>, g1_break_degrees: Option<f64>) -> Self {
        let std = Self::standard();
        let pick = |asked: Option<f64>, base: f64, (lo, hi): (f64, f64)| match asked {
            Some(v) if v.is_finite() => v.clamp(lo, hi),
            _ => base,
        };
        CostModel {
            cubic_params: pick(cubic_params, std.cubic_params, Self::CUBIC_RANGE),
            g1_break_degrees: pick(g1_break_degrees, std.g1_break_degrees, Self::G1_RANGE),
        }
    }
}

/// "No override": the value in this slot is the standard one.
const UNSET: u64 = f64::NAN.to_bits();

static CUBIC: AtomicU64 = AtomicU64::new(UNSET);
static G1_DEGREES: AtomicU64 = AtomicU64::new(UNSET);

/// Serialises traces that change the prices against every other trace.
static GATE: RwLock<()> = RwLock::new(());

thread_local! {
    /// Whether this thread is already inside a [`with_cost_model`] scope.
    static INSIDE: Cell<bool> = const { Cell::new(false) };
}

/// Parameters charged for a cubic segment in the trace that is running.
pub fn cubic_params() -> f64 {
    let bits = CUBIC.load(Ordering::Relaxed);
    if bits == UNSET {
        CostModel::standard().cubic_params
    } else {
        f64::from_bits(bits)
    }
}

/// The break angle, in radians, in the trace that is running.
pub fn g1_break_radians() -> f64 {
    let bits = G1_DEGREES.load(Ordering::Relaxed);
    let degrees = if bits == UNSET {
        CostModel::standard().g1_break_degrees
    } else {
        f64::from_bits(bits)
    };
    degrees.to_radians()
}

/// Run `f` with `model`'s prices in force, then put the standard ones back.
///
/// Wrap one whole trace. A trace that asks for the standard prices runs alongside others; one
/// that asks for anything else runs alone, because the prices are shared with every thread
/// the trace uses. A call made from inside another scope on the same thread inherits that
/// scope's prices.
pub fn with_cost_model<R>(model: CostModel, f: impl FnOnce() -> R) -> R {
    if INSIDE.with(Cell::get) {
        return f();
    }

    struct Inside;
    impl Drop for Inside {
        fn drop(&mut self) {
            INSIDE.with(|c| c.set(false));
        }
    }
    INSIDE.with(|c| c.set(true));
    let _inside = Inside;

    if model == CostModel::standard() {
        let _shared = GATE.read().unwrap_or_else(|e| e.into_inner());
        return f();
    }

    struct Restore;
    impl Drop for Restore {
        fn drop(&mut self) {
            CUBIC.store(UNSET, Ordering::Relaxed);
            G1_DEGREES.store(UNSET, Ordering::Relaxed);
        }
    }
    let _exclusive = GATE.write().unwrap_or_else(|e| e.into_inner());
    CUBIC.store(model.cubic_params.to_bits(), Ordering::Relaxed);
    G1_DEGREES.store(model.g1_break_degrees.to_bits(), Ordering::Relaxed);
    let _restore = Restore;
    f()
}

#[cfg(test)]
mod tests {
    //! Only what cannot change the shared prices. The tests that enter a scope are in
    //! `tests/cost_scope.rs`, a process of their own: a scope moves the prices for every thread,
    //! and unit tests share one.
    use super::*;

    #[test]
    fn asking_for_nothing_is_the_standard_model() {
        assert_eq!(CostModel::with_overrides(None, None), CostModel::standard());
    }

    #[test]
    fn requested_prices_are_held_to_their_range() {
        let m = CostModel::with_overrides(Some(0.1), Some(500.0));
        assert_eq!(m.cubic_params, CostModel::CUBIC_RANGE.0);
        assert_eq!(m.g1_break_degrees, CostModel::G1_RANGE.1);
        let n = CostModel::with_overrides(Some(f64::NAN), Some(f64::INFINITY));
        assert_eq!(
            n,
            CostModel::standard(),
            "a non-finite request means no request"
        );
    }
}
