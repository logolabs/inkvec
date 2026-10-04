//! The turn charge of the written-arcs prices (`crate::cost::CostModel::with_written_arcs`,
//! an experiment no product setting enables): a cubic that turns further than a quarter circle
//! is charged as the two cubics that would replace it.
//!
//! Read by every place a cubic is priced: the scan (`crate::multimodel::scan`, G1 and free
//! cubics), `crate::multimodel::segment_cost_direct`, and both sides of the free-cubic merge
//! (`crate::merge`). At the default prices the limit is infinite and the charge is 0, without
//! the angle being computed.

use super::params_cubic;
use crate::tangents::turn_angle;
use inkvec_core::{Point, Vec2};

/// The most a single cubic may turn, in degrees, before it is charged as two
/// ([`over_turn_params`]) under the written-arcs prices.
///
/// A cubic Bézier is a good circle only over a short sweep. The best-known G1 cubic for a
/// circular arc of sweep `θ` (arms `(4/3)·tan(θ/4)` of the radius) misses the circle by a
/// radial error that grows like `θ⁶` — Goldapp (1991), "Approximation of circular arcs by
/// cubic polynomials", *Computer Aided Geometric Design* 8(3):227-238,
/// doi:10.1016/0167-8396(91)90007-X. Evaluated here (2026-10-04, 20,001 samples per arc):
/// 2.7e-4 of the radius at 90°, 1.5e-3 at 120°, 6.0e-3 at 150°, 1.8e-2 at 180°. A 180° round
/// cap of a 2-unit lucide stroke at 128 px has a radius of 5.3 px, so one cubic misses it by
/// about 0.1 px, and by 0.4 px at 512 px; at 90° the miss is 0.0015 px. That is why
/// drawing programs and SVG arc converters split an arc into quarter-circle pieces.
///
/// The fit accepts the 0.1 px anyway once an arc costs seven numbers: the two arcs a
/// half circle needs (at most [`crate::primitives::MAX_ARC_DEGREES`] each) then cost `8λ`
/// more than one cubic, about 57 nats at 128 px, against about 15 nats of `½χ²` for a 0.1 px
/// miss over 17 points at the plain sigma of 0.05 px, and less still where the cap's points
/// carry a sigma inflated for curvature (`inkvec_trace::contour::inflate_for_curvature`,
/// the r2-compact diagnosis). While an arc cost five the cap went to arcs; at seven it went
/// to one cubic, and lucide's dE00 rose 12 % (r2-compact). `tests/cost_scope.rs` reproduces
/// both caps. Charging a cubic that turns further than this as two cubics puts the price
/// where the error is, without touching the sigma model.
///
/// Inspired by: Goldapp (1991) above, for where a single cubic stops being a circle; the
/// charge itself is not from the literature, because the published methods split by a
/// tolerance on the known circle, and here the circle is not known, only the sweep the
/// cubic's own end tangents imply. Measured on the gate set at 90° and arcs at five (the
/// default prices): turning -2.4 % / -1.7 % / -1.8 % on the gate (geometry turning -0.1 %),
/// dE00 within noise, parameter ratio +0.9-1.0 %, so it is not used on its own.
pub(crate) const CAP_TURN_DEGREES: f64 = 90.0;

/// Extra parameters, beyond [`params_cubic`], a cubic whose end directions `t0`, `t1` (any
/// length) turn by more than the limit in force ([`crate::cost::cubic_max_turn_radians`],
/// [`CAP_TURN_DEGREES`] under the written-arcs prices, infinite otherwise) is charged: one more
/// cubic's worth, so it costs what the two cubics that could replace it cost.
///
/// The turn is the angle between the end directions, `acos(t0·t1 / |t0||t1|)` in `[0, π]`,
/// which for a cubic without an inflection is its whole turn. 0 when the limit is infinite,
/// without computing the angle, so the default prices pay nothing and the costs this is
/// added to are unchanged bit for bit (`x + 0.0 == x`).
pub(crate) fn over_turn_params(t0: Vec2, t1: Vec2) -> f64 {
    let limit = crate::cost::cubic_max_turn_radians();
    if limit.is_finite() && turn_angle(t0, t1) > limit {
        params_cubic()
    } else {
        0.0
    }
}

/// [`over_turn_params`] of a cubic given by its control points (px): the end directions are
/// `p1 − p0` and `p3 − p2`, each falling back to the chord through the next control point
/// when its arm has zero length.
pub(crate) fn over_turn_params_of(p0: Point, p1: Point, p2: Point, p3: Point) -> f64 {
    let d0 = if (p1 - p0).norm() > 1e-12 {
        p1 - p0
    } else {
        p2 - p0
    };
    let d1 = if (p3 - p2).norm() > 1e-12 {
        p3 - p2
    } else {
        p3 - p1
    };
    over_turn_params(d0, d1)
}
