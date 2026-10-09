//! Segment evaluation shared by the emitter and the minifier, and, in a `research` build,
//! the structural MDL simplifier (`INKVEC_STRUCTURAL`; see `simplify_with_poly`).
//!
//! The simplifier is an experiment that stays off by default ("keep opt-in until acceptance
//! uses calibrated image evidence"), so a release build does not compile it.

use crate::curves::{arc_ellipse_center, eval_cubic, Segment};
use inkvec_core::{Point, Vec2};

/// Unsigned angle between two 2D vectors in radians [0, pi].
///
/// Returns 0.0 for zero-length, subnormal, or non-finite (NaN, inf) vectors.
pub fn vector_angle(a: Vec2, b: Vec2) -> f64 {
    let na = a.norm();
    let nb = b.norm();
    if !na.is_finite() || !nb.is_finite() || na <= 1e-12 || nb <= 1e-12 {
        return 0.0;
    }
    let dot = (a.dot(b) / (na * nb)).clamp(-1.0, 1.0);
    if !dot.is_finite() {
        return 0.0;
    }
    dot.acos()
}

/// Sample a single segment at parameter `t` in [0, 1].
///
/// `start` is where the segment begins (the previous segment's end). A line is
/// interpolated linearly, a cubic evaluated by [`eval_cubic`], and an arc at the angle
/// `theta1 + t·delta` of its SVG centre parametrisation ([`arc_ellipse_center`]), so
/// uniform `t` is uniform in angle rather than in arc length on an ellipse. `t` is
/// clamped to `[0, 1]`, and the two ends return `start` and the stored end exactly, not
/// as recomputed, so consecutive segments sampled this way share their join bit for bit.
pub fn eval_segment(seg: &Segment, start: Point, t: f64) -> Point {
    let t = t.clamp(0.0, 1.0);
    // Preserve the stored incidence exactly, including SVG arc roundoff.
    if t == 0.0 {
        return start;
    }
    if t == 1.0 {
        return seg.end();
    }
    match *seg {
        Segment::Line(end) => Point::new(
            start.x + (end.x - start.x) * t,
            start.y + (end.y - start.y) * t,
        ),
        Segment::Cubic(c1, c2, end) => eval_cubic([start, c1, c2, end], t),
        Segment::Arc {
            rx,
            ry,
            phi,
            large_arc,
            sweep,
            end,
        } => {
            let f = arc_ellipse_center(start, rx, ry, phi, large_arc, sweep, end);
            f.at(f.theta1 + f.delta * t)
        }
    }
}

#[cfg(feature = "research")]
mod simplify;
#[cfg(feature = "research")]
pub use simplify::*;
