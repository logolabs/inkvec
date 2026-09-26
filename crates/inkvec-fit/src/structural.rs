//! Segment evaluation shared by the emitter and the minifier, and, in a `research` build,
//! the structural MDL simplifier (`INKVEC_STRUCTURAL`; see `simplify_with_poly`).
//!
//! The simplifier is an experiment that stays off by default ("keep opt-in until acceptance
//! uses calibrated image evidence"), so a release build does not compile it.

use crate::curves::{arc_ellipse_center, eval_cubic, Segment};
use inkvec_core::Point;

/// Sample a single segment at parameter `t` in [0, 1].
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
