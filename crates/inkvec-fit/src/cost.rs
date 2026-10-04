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
//! # What a parameter costs, and why
//!
//! Every fitter in the crate minimises `E = ½·χ² + λ·P + (break costs)`, all in nats:
//!
//! - `½·χ²` is the Gaussian negative log-likelihood of the measured points given the
//!   emitted geometry, `χ² = Σ (d_k/σ_k)²`;
//! - `λ` ([`crate::FitConfig::lambda`]) is the price of writing one number, derived as
//!   `ln(extent / precision)`: a coordinate that can take `extent / precision`
//!   distinguishable values carries that many nats of information (about 7.85 for a
//!   256 px canvas at 0.1 px);
//! - `P` counts the numbers a segment writes: a line 2 ([`crate::PARAMS_LINE`]), a cubic
//!   [`cubic_params`] (6 by default), a circular arc [`arc_params`] (5, or 7 as written),
//!   an elliptical arc 7 ([`crate::curves::PARAMS_ELLIPTICAL_ARC`]). The start point
//!   of a segment is the previous one's end and is not charged again;
//! - a join where the tangent turns costs up to one more parameter, `λ`, because the
//!   outgoing direction is then a number of its own rather than implied by the incoming
//!   one (`crate::tangents::break_cost`, ramping quadratically up to
//!   [`g1_break_radians`]).
//!
//! So a segment is kept exactly when the misfit it removes is worth more than the numbers
//! it adds. Users set the two above, and `--arcs-as-written` the arc's and a cubic's turn.
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
    /// Parameters charged for one circular arc: [`crate::curves::PARAMS_ARC`] (5), or
    /// [`crate::curves::PARAMS_ARC_WRITTEN`] (7) under [`CostModel::with_written_arcs`].
    pub arc_params: f64,
    /// The turn, degrees, beyond which one cubic is charged as two
    /// ([`crate::candidates::over_turn_params`]); infinite, never, unless
    /// [`CostModel::with_written_arcs`].
    pub cubic_max_turn_degrees: f64,
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
        arc_params: crate::curves::PARAMS_ARC,
        cubic_max_turn_degrees: f64::INFINITY,
    };

    /// The process default, [`CostModel::STANDARD`].
    pub fn standard() -> Self {
        Self::STANDARD
    }

    /// These prices with a circular arc charged the seven numbers SVG writes for it
    /// ([`crate::curves::PARAMS_ARC_WRITTEN`]) and one cubic charged as two beyond
    /// [`crate::candidates::CAP_TURN_DEGREES`] of turn (`--arcs-as-written`).
    ///
    /// The two go together. At five numbers an arc undercuts a six-number cubic on every
    /// span where both fit, where the document and the benchmark both count it at seven, so
    /// the fit writes arcs it believes cheaper and are not (35 % of the segments written on
    /// the 246-icon screen set against 5.6 % of the artists'). Priced at seven, a round cap
    /// that was two arcs becomes one cubic over 180 degrees, which the curvature-inflated
    /// sigma at the cap lets pass with a 0.1 px error at 128 px: lucide caps lost +12 % dE00
    /// in the r2-compact measurement. The second price is what stops that; see
    /// [`crate::candidates::over_turn_params`].
    pub fn with_written_arcs(self) -> Self {
        CostModel {
            arc_params: crate::curves::PARAMS_ARC_WRITTEN,
            cubic_max_turn_degrees: crate::candidates::CAP_TURN_DEGREES,
            ..self
        }
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
            ..std
        }
    }
}

/// "No override": the value in this slot is the standard one. A NaN bit pattern, which no
/// clamped price can have (see [`CostModel::with_overrides`]).
const UNSET: u64 = f64::NAN.to_bits();

/// The cubic price in force, as `f64` bits, or [`UNSET`].
static CUBIC: AtomicU64 = AtomicU64::new(UNSET);
/// The full-corner turn in force, degrees as `f64` bits, or [`UNSET`].
static G1_DEGREES: AtomicU64 = AtomicU64::new(UNSET);
/// The circular-arc price in force, as `f64` bits, or [`UNSET`].
static ARC: AtomicU64 = AtomicU64::new(UNSET);
/// The cubic's turn limit in force, degrees as `f64` bits, or [`UNSET`].
static CUBIC_TURN: AtomicU64 = AtomicU64::new(UNSET);

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

/// Parameters charged for a circular arc in the trace that is running.
pub fn arc_params() -> f64 {
    let bits = ARC.load(Ordering::Relaxed);
    if bits == UNSET {
        CostModel::standard().arc_params
    } else {
        f64::from_bits(bits)
    }
}

/// The turn, radians, beyond which one cubic is charged as two in the trace that is
/// running; infinite in the standard model.
pub fn cubic_max_turn_radians() -> f64 {
    let bits = CUBIC_TURN.load(Ordering::Relaxed);
    let degrees = if bits == UNSET {
        CostModel::standard().cubic_max_turn_degrees
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
            ARC.store(UNSET, Ordering::Relaxed);
            CUBIC_TURN.store(UNSET, Ordering::Relaxed);
        }
    }
    let _exclusive = GATE.write().unwrap_or_else(|e| e.into_inner());
    CUBIC.store(model.cubic_params.to_bits(), Ordering::Relaxed);
    G1_DEGREES.store(model.g1_break_degrees.to_bits(), Ordering::Relaxed);
    ARC.store(model.arc_params.to_bits(), Ordering::Relaxed);
    CUBIC_TURN.store(model.cubic_max_turn_degrees.to_bits(), Ordering::Relaxed);
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
