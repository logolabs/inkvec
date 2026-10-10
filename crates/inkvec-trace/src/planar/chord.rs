//! The lattice normal of a boundary vertex: the chord between its neighbours, which
//! `refine_vertex` reads the image across. Split from `planar.rs`, unchanged but for the
//! wrap round a closed edge's seam.

use inkvec_core::{Point, Vec2};

/// Points each side of a vertex its tangent is taken over; see the comment where it is
/// used, in `probe_chord`, for the measurement that left it at one (it was
/// `INKVEC_SUBPX_WIN`, 1..=8).
const SUBPX_WIN: usize = 1;

/// Cosine of the turning angle between a point's two chords above which the point is
/// treated as a corner by `refine_subpixel` (60 degrees). A staircase at any slope turns
/// by at most 45 degrees between chords two points long, so slanted edges stay smooth.
const CORNER_COS: f64 = 0.5;

/// The chord whose perpendicular is taken as vertex `k`'s normal, and whether `k` is a
/// corner.
///
/// The chord runs from the point `SUBPX_WIN` before `k` to the one `SUBPX_WIN` after,
/// clamped to the ends of an open edge and, with `wrap`, wrapped round a closed one (whose
/// first point is not repeated at its end; clamped, its seam took a one-sided chord, and on a
/// horizontal run a horizontal normal: 0.30 px off on an exact disc, in Fast mode). With a
/// wider window, a vertex whose two chords turn by more than 60 degrees (`CORNER_COS`) is a
/// corner and falls back to its immediate neighbours. At the shipped window of 1 the corner
/// test never fires.
pub(super) fn probe_chord(points: &[Point], k: usize, wrap: bool) -> (Vec2, bool) {
    let n = points.len();
    let p = points[k];
    let before = |d: usize| {
        if wrap {
            (k + n - d % n) % n
        } else {
            k.saturating_sub(d)
        }
    };
    let after = |d: usize| {
        if wrap {
            (k + d) % n
        } else {
            (k + d).min(n - 1)
        }
    };
    // Local tangent from neighbours, normal perpendicular to it. The window is
    // `SUBPX_WIN` points each side.
    //
    // A wider `SUBPX_WIN` averages the tangent over more points. The
    // marching-squares polyline is a staircase, so a tangent from the immediate
    // neighbours is quantised to a few directions and on a slanted edge the probe
    // line is off-axis. Two points each side averages that out and improves the
    // colour error (620-icon subset: dE00 0.2663 -> 0.2534 against the root-find,
    // where one point each side gives 0.2591) - but it costs DISTS (0.0418 ->
    // 0.0435), because a wide tangent at a corner moves the vertex sideways, and
    // it has a failure mode: on one openmoji juggler the body's boundary moved
    // enough to change which face the emitter paints on top (dE00 0.23 -> 3.20).
    // Corners keep the narrow tangent already, so the remaining harm is elsewhere.
    // Default is one point each side until that is understood (LOG-43).
    let win = SUBPX_WIN;
    let (pa, pb) = (points[before(win)], points[after(win)]);
    let corner = win > 1 && {
        let (c0, c1) = (p - pa, pb - p);
        let (l0, l1) = (c0.norm(), c1.norm());
        l0 > 1e-9 && l1 > 1e-9 && (c0.x * c1.x + c0.y * c1.y) / (l0 * l1) < CORNER_COS
    };
    let (pa, pb) = if corner {
        (points[before(1)], points[after(1)])
    } else {
        (pa, pb)
    };
    (pb - pa, corner)
}
