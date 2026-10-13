//! Editability mode (`--editability`): post-fit passes that spend parameters on
//! structure an artist can edit — circular arcs written as cubics with their nodes at the
//! extrema, smooth runs re-noded at their extrema ([`curves`]), G1-smooth joins within a
//! ring and across the planar map's junctions ([`junctions`]), axis-aligned and
//! equal-length handles, and self-symmetric rings locked into exact mirrors.
//!
//! Every change is accepted the same way, priced by MDL ([`price`]): it is kept when half
//! its change in `χ²` against the evidence (both ways, by exact distances) is at most `λ`
//! times the numbers it saves plus a learned bonus for its kind, what the artists' own
//! files show they pay for that structure. Acceptance is greedy and per-item: one handle
//! whose snap does not pay does not veto its neighbours. Node alignment across rings does
//! not run ([`on`]). `INKVEC_EDIT_PRICE=off` restores the earlier fixed budget, described
//! below.
//!
//! The targets are measured, not assumed: the axis spike (29% of artist handles sit
//! within 0.05° of an axis), the 45° bump (5x lift) and the dead 10–30° trough come
//! from the committed artist GT corpus (313k handle angles, scratchpad/angle_prior.py);
//! the G1 and handle-length lifts from scratchpad/artist_priors.py.
//!
//! Where it sits: after fitting, ring repair and shape harmonization, just before the
//! emitter, so nothing downstream re-breaks what was locked. `pipeline.rs` calls
//! [`edit_all`] once per colour trace when `--editability` is on and prints
//! [`EditStats::summary`]. In: one measured polyline per planar-map edge (its sub-pixel
//! points and their sigma), the fitted path for each edge, which of them were fitted as
//! primitives, the faces' rings over the edges (for the joins at junctions) and the fit's
//! configuration (for the price of a parameter). Out: the same fitted paths, edited in
//! place, and counts of what changed.
//! A "ring" below is one such fitted path: a run between two junctions, or a boundary with
//! no junction on it, which comes as an open path ending where it starts and which the
//! per-ring passes treat as closed ([`isolated_loops`]).
//!
//! Units: positions in px of the traced raster, with pixel centres at integer coordinates
//! (the canvas spans `-0.5 .. w - 0.5`); sigma in px; angles in degrees. Image y points
//! down, which none of the passes care about: the axis, diagonal and mirror tests are all
//! symmetric under that flip.
//!
//! The fixed budget (`INKVEC_EDIT_PRICE=off`): sample the edited ring at 8 points per segment
//! and require every sample `s` to have some source point `q_j` with
//! `|s − q_j| ≤ 3·σ_j + slack`, where `σ_j` is that point's positional sigma and `slack` is
//! [`EDIT_SLACK`] (or [`MIRROR_SLACK`] for mirror lock). It is one-sided on purpose: it asks
//! that the new curve stay on the evidence, not that it visit every point, because the
//! passes only rotate, rescale, slide or reflect pieces the fit already placed.

use inkvec_core::Point;
use inkvec_fit::curves::Segment;
use inkvec_fit::primitives::PrimitiveFit;
use inkvec_fit::structural::eval_segment;
use inkvec_fit::FittedPath;

use crate::diag;
use crate::faces::FaceRings;

mod curves;
mod junctions;
pub(crate) mod price;

pub(crate) use price::{accept_ring, priced, Touched};

/// `path` with every circular arc written as cubics cut at its x and y extrema
/// ([`curves::arc_as_cubics`]), for a stroke's centreline under `--editability`: exact to
/// `1.1e-3` of the radius (`2.7e-4` for a quarter), so it needs no guard of its own.
pub(crate) fn arcs_as_cubics(path: &FittedPath) -> FittedPath {
    let mut segments = Vec::with_capacity(path.segments.len());
    let mut cur = path.start;
    for s in &path.segments {
        match curves::arc_as_cubics(s, cur) {
            Some(pieces) => segments.extend(pieces),
            None => segments.push(s.clone()),
        }
        cur = s.end();
    }
    FittedPath {
        start: path.start,
        segments,
        closed: path.closed,
    }
}

/// A handle direction is "on" an axis (0/90/180/270) within this many degrees.
const AXIS_SNAP: f64 = 2.5;
/// A handle direction is "on" the 45 family within this many degrees (5x measured
/// lift, broader than the axis spike).
const DIAG_SNAP: f64 = 1.5;
/// A join whose in/out tangents disagree by at most this many degrees is G1-smooth.
const G1_SNAP_DEG: f64 = 3.0;
/// A join turning by at most this many degrees is a kink, not a corner, and [`pass_g1`]
/// tries to make it smooth: the artists' joins between curves are smooth or turn by more than
/// this (R3.1 of `docs/theory/chain-representation.md`; 8.6 % of them between 3 and 30
/// degrees on the screen set, against 25 % of the trace's).
const KINK_DEG: f64 = 30.0;
/// Two lines meeting at a turn of at most this many degrees are one straight run, and
/// [`pass_collinear`] writes them as one line (the battery's `collinear`, its `AXIS_DEG`).
const COLLINEAR_DEG: f64 = 0.5;
/// Handle length ratio inside this window symmetrises to the mean.
const LEN_MATCH: f64 = 0.10;
/// Node coordinates within this distance of a shared target align to it.
const NODE_ALIGN: f64 = 0.30;
/// Self-mirror detection: share of reflected source points that must find a partner.
const MIRROR_KEEP: f64 = 0.95;
/// Absolute deviation (px at trace size) the mode carries on top of the input's own
/// noise floor: the GT lift is measured on files whose handles sit this far from the
/// pixel-optimal fit.
const EDIT_SLACK: f64 = 0.5;
/// Mirror lock moves geometry by the asymmetry between the two traced halves --
/// usually 1-3 px on icon-size art. Its own budget: asymmetry beyond this is real
/// content, not tracing noise, and the ring stays as drawn.
const MIRROR_SLACK: f64 = 1.5;
/// The emitter's coordinate step (it writes two decimals). The mirror axis snaps to
/// it so that reflected coordinates round to each other's reflections -- and so that
/// a node *on* the axis writes the axis exactly. Snapping to half a step is not
/// enough: `2·cx` lands on the grid either way, but the pole itself then rounds off
/// the axis and the whole shape emits 0.01 px out of true. The axis moves by at most
/// half a step, which no renderer resolves.
const EMIT_GRID: f64 = 0.01;
/// How close to the axis a node counts as sitting on it. Wider than the emitter's
/// step so a node the fit put a fraction off the axis is used as the crossing
/// rather than split beside it.
const AXIS_NODE: f64 = 1.0;

/// What the editability passes did, counted over one drawing.
#[derive(Default, Debug, Clone)]
pub(crate) struct EditStats {
    /// Rings (fitted paths) handed in, primitives included.
    pub(crate) rings: usize,
    /// Joins made G1-smooth by [`pass_g1`].
    pub(crate) g1_joins: usize,
    /// Handles snapped onto 0/90/180/270 degrees by [`pass_handles`].
    pub(crate) axis_snaps: usize,
    /// Handles snapped onto a 45-degree diagonal by [`pass_handles`].
    pub(crate) diag_snaps: usize,
    /// Cubics whose two handles [`pass_equalize`] set to one length.
    pub(crate) len_sym: usize,
    /// Node coordinate moves kept by [`pass_nodes`]; a node that moved in both x and y
    /// counts twice.
    pub(crate) nodes_aligned: usize,
    /// Rings rewritten as exact mirrors by [`pass_mirror`].
    pub(crate) mirror_rings: usize,
    /// Circular arcs written as cubics cut at their extrema by [`curves::pass_arcs`].
    pub(crate) arcs_split: usize,
    /// Smooth runs re-segmented at their extrema by [`curves::pass_extrema`].
    pub(crate) extrema_runs: usize,
    /// Joins at a junction between two edges made smooth by [`junctions::pass_junctions`].
    pub(crate) junction_g1: usize,
    /// Nodes in the middle of a straight run removed by [`pass_collinear`].
    pub(crate) collinear_merged: usize,
    /// Edits tried and undone because the result left the acceptance budget.
    pub(crate) reverted: usize,
}

impl EditStats {
    /// The one-line report the pipeline prints under `--editability`.
    pub(crate) fn summary(&self) -> String {
        format!(
            "  editability   {} rings: {} arcs as cubics, {} runs re-noded at extrema, {} G1 joins, \
{} at junctions, {} axis + {} diagonal handle snaps, {} length-symmetric curves, {} nodes \
aligned, {} collinear nodes merged, {} mirror-locked, {} reverted",
            self.rings,
            self.arcs_split,
            self.extrema_runs,
            self.g1_joins,
            self.junction_g1,
            self.axis_snaps,
            self.diag_snaps,
            self.len_sym,
            self.nodes_aligned,
            self.collinear_merged,
            self.mirror_rings,
            self.reverted
        )
    }
}

/// The ring's on-curve nodes in order: `starts[i]` is where segment `i` begins and
/// `starts[i + 1]` where it ends, whatever kind of segment it is. There are
/// `segments.len() + 1` of them; on a closed ring the last repeats the first.
fn chain_starts(fp: &FittedPath) -> Vec<Point> {
    let mut starts = Vec::with_capacity(fp.segments.len() + 1);
    starts.push(fp.start);
    for seg in &fp.segments {
        starts.push(seg.end());
    }
    starts
}

/// Points along a chain of segments, `per` to a segment at `t = 1/per, 2/per, …, 1`.
/// `t = 0` is left out because it is the previous segment's `t = 1`, so only the chain's
/// very first node is not sampled. `starts[i]` must be segment `i`'s start (see
/// [`chain_starts`]).
fn samples(segs: &[Segment], starts: &[Point], per: usize) -> Vec<Point> {
    let mut v = Vec::new();
    for (i, seg) in segs.iter().enumerate() {
        for k in 1..=per {
            v.push(eval_segment(seg, starts[i], k as f64 / per as f64));
        }
    }
    v
}

/// The changed geometry stays within the editability budget of the source points.
/// [`guarded_slack`] with the handle and node budget, [`EDIT_SLACK`].
fn guarded(segs: &[Segment], starts: &[Point], pts: &[Point], sigma: &[f64]) -> bool {
    guarded_slack(segs, starts, pts, sigma, EDIT_SLACK)
}

/// The acceptance test from the module docs: every one of 8 samples per segment must lie
/// within `3·σ_j + slack` px of at least one source point `q_j`.
///
/// `pts` are the traced boundary points (px) and `sigma` their positional sigmas (px),
/// parallel to `pts`; a shorter `sigma` repeats its last value, and it must not be empty
/// while `pts` is not. The search is brute force, O(samples × points), which is fine at
/// the size of one ring and is only paid under `--editability`. With no source points
/// nothing can be matched and every ring with a segment fails.
fn guarded_slack(
    segs: &[Segment],
    starts: &[Point],
    pts: &[Point],
    sigma: &[f64],
    slack: f64,
) -> bool {
    samples(segs, starts, 8).iter().all(|p| {
        pts.iter()
            .enumerate()
            .any(|(j, q)| p.dist(*q) <= 3.0 * sigma[j.min(sigma.len() - 1)] + slack)
    })
}

/// How far the chain strays from the evidence: the largest, over 8 samples per segment, of
/// `min_j (|s − q_j| − 3·σ_j)`, the slack the acceptance test would need for it (px).
/// `guarded_slack(.., slack)` is `stray(..) <= slack`.
fn stray(segs: &[Segment], starts: &[Point], pts: &[Point], sigma: &[f64]) -> f64 {
    samples(segs, starts, 8)
        .iter()
        .map(|p| {
            pts.iter()
                .enumerate()
                .map(|(j, q)| p.dist(*q) - 3.0 * sigma[j.min(sigma.len() - 1)])
                .fold(f64::INFINITY, f64::min)
        })
        .fold(f64::NEG_INFINITY, f64::max)
}

/// Greedy per-item acceptance: apply one change, keep it if the whole ring still
/// guards, revert just that change otherwise.
///
/// The caller has already applied the change to `fp` and passes the segments from before
/// it as `before`. Returns whether it was kept; `count` goes up by one if so. Only segments
/// are restored, so a change must not have moved `fp.start`.
fn keep_if_guarded(
    fp: &mut FittedPath,
    before: &[Segment],
    pts: &[Point],
    sigma: &[f64],
    count: &mut usize,
) -> bool {
    if accept_ring(price::SMOOTH, (fp.start, before), fp, pts, sigma, 1.0) {
        *count += 1;
        true
    } else {
        fp.segments = before.to_vec();
        false
    }
}

/// A cubic's two control points and end, or `None` for a line or an arc.
fn cubic(seg: &Segment) -> Option<(Point, Point, Point)> {
    match *seg {
        Segment::Cubic(c1, c2, e) => Some((c1, c2, e)),
        _ => None,
    }
}

/// The unit direction of travel arriving at a segment's end (`incoming`) or leaving its
/// start, from its handle at that end, or a line's own direction; `None` for an arc, a
/// degenerate segment, or a handle of zero length.
fn end_direction(seg: &Segment, start: Point, incoming: bool) -> Option<(f64, f64)> {
    let (from, to) = match (seg, incoming) {
        (&Segment::Line(e), _) => (start, e),
        (&Segment::Cubic(_, c2, e), true) => (c2, e),
        (&Segment::Cubic(c1, _, _), false) => (start, c1),
        (Segment::Arc { .. }, _) => return None,
    };
    let (dx, dy) = (to.x - from.x, to.y - from.y);
    let l = dx.hypot(dy);
    (l > 1e-9).then(|| (dx / l, dy / l))
}

/// G1: joins within a ring that turn by at most [`KINK_DEG`] are made smooth, so an editor
/// drags one tangent and the curve stays smooth, and a kink the separate segment fits left
/// (neither smooth nor a corner, which artists do not draw) goes.
///
/// At the node `N` between segment `i` and segment `j = i + 1`, the incoming direction is
/// `d_in` (from cubic `i`'s second control point, or line `i`'s own direction) and the
/// outgoing one `d_out`; `θ` is the angle between them. When `θ ≤ KINK_DEG` the join takes
/// one direction `b`: between two cubics their unit bisector, each handle turning by `θ/2`
/// (`c2 ← N − |d_in|·b`, `c1 ← N + |d_out|·b`), and next to a line the line's own direction,
/// the cubic's handle turning by `θ`. Handles keep their lengths and the node does not move:
/// G1 continuity, one tangent direction at the node with the two handle lengths still free.
/// Up to [`G1_SNAP_DEG`] this only makes exact what was already smooth to the eye; beyond it,
/// the guard decides whether the pixels allow it.
///
/// Two lines, an arc, or a zero-length handle: skipped. On an open ring the join from the last
/// segment back to the first does not exist and is skipped too. Each join is accepted or
/// reverted on its own ([`keep_if_guarded`]).
fn pass_g1(
    fp: &mut FittedPath,
    starts: &[Point],
    pts: &[Point],
    sigma: &[f64],
    st: &mut EditStats,
) {
    let n = fp.segments.len();
    let mut count = 0;
    for i in 0..n {
        let j = (i + 1) % n;
        if (!fp.closed && j == 0) || i == j {
            continue;
        }
        let (si, sj) = (&fp.segments[i], &fp.segments[j]);
        let (line_i, line_j) = (
            matches!(si, Segment::Line(_)),
            matches!(sj, Segment::Line(_)),
        );
        if line_i && line_j {
            continue;
        }
        let (Some(din), Some(dout)) = (
            end_direction(si, starts[i], true),
            end_direction(sj, starts[j], false),
        ) else {
            continue;
        };
        let cos = din.0 * dout.0 + din.1 * dout.1;
        let ang = cos.clamp(-1.0, 1.0).acos().to_degrees();
        if ang <= 1e-9 || ang > KINK_DEG {
            continue;
        }
        let (bx, by) = if line_i {
            din
        } else if line_j {
            dout
        } else {
            let (x, y) = (din.0 + dout.0, din.1 + dout.1);
            let l = x.hypot(y);
            if l < 1e-9 {
                continue;
            }
            (x / l, y / l)
        };
        let before = fp.segments.clone();
        if let Segment::Cubic(_, c2m, endm) = &mut fp.segments[i] {
            let l = (endm.x - c2m.x).hypot(endm.y - c2m.y);
            *c2m = Point::new(endm.x - bx * l, endm.y - by * l);
        }
        if let Segment::Cubic(c1m, _, _) = &mut fp.segments[j] {
            let l = (c1m.x - starts[j].x).hypot(c1m.y - starts[j].y);
            *c1m = Point::new(starts[j].x + bx * l, starts[j].y + by * l);
        }
        if !keep_if_guarded(fp, &before, pts, sigma, &mut count) {
            st.reverted += 1;
        }
    }
    st.g1_joins += count;
}

/// The snapped direction (degrees) for a direction `deg`, and whether it is an axis: the
/// nearest multiple of 90° within [`AXIS_SNAP`], else the 45° diagonal within [`DIAG_SNAP`].
fn snap_target(deg: f64) -> Option<(f64, bool)> {
    let rem = deg.rem_euclid(90.0);
    if rem.min(90.0 - rem) <= AXIS_SNAP {
        Some((deg - rem + if rem > 45.0 { 90.0 } else { 0.0 }, true))
    } else if (rem - 45.0).abs() <= DIAG_SNAP {
        Some((deg - rem + 45.0, false))
    } else {
        None
    }
}

/// Axis and diagonal handle snaps, greedy per node: each snap is applied alone and kept
/// only if the whole ring still guards, so one bad handle never vetoes another and none ever
/// escapes the budget.
///
/// A handle is the vector `h` from a cubic's node to its control point (start → `c1`, end
/// → `c2`). Its direction `φ = atan2(h_y, h_x)` in degrees is reduced to `r = φ mod 90` in
/// `[0, 90)`. If `min(r, 90 − r) ≤ AXIS_SNAP` (2.5°) it snaps to the nearest multiple of
/// 90°; otherwise, if `|r − 45| ≤ DIAG_SNAP` (1.5°), to the 45° diagonal in the same
/// quadrant ([`snap_target`]). The snapped handle keeps its length `|h|`, so only its angle
/// moves and the node stays put. The targets and the narrow windows come from the artist
/// corpus: a sharp spike of handles at the axes, a smaller bump at 45°, and nothing to aim
/// for between 10° and 30° (see the module docs).
///
/// A smooth node is snapped as one: its two handles share a direction, so they turn
/// together or not at all. Snapping each on its own turned one handle of a pair whose
/// direction sat on either side of the window's edge and left a kink where there was none
/// (the battery's `kinked` rose 3.9 % under `--editability`). A handle that leaves a line
/// smoothly is not snapped: the line's direction is fixed, so turning the handle would kink
/// the join. A handle at a corner, or at an open ring's end, snaps on its own as before.
///
/// Lines and arcs have no handles and are skipped, as is a zero-length handle or one that
/// is already exactly on its target.
fn pass_handles(
    fp: &mut FittedPath,
    starts: &[Point],
    pts: &[Point],
    sigma: &[f64],
    st: &mut EditStats,
) {
    let n = fp.segments.len();
    // Node k sits between segment k − 1 (its end) and segment k (its start); on an open
    // ring node 0 and node n are ends with one segment each.
    let nodes = if fp.closed { n } else { n + 1 };
    for k in 0..nodes {
        let prev = if k > 0 {
            Some(k - 1)
        } else if fp.closed && n > 0 {
            Some(n - 1)
        } else {
            None
        };
        let next = (k < n).then_some(k);
        let node = starts[k.min(n)];
        // The handles at this node: (segment, which end), with their direction.
        let mut hs: Vec<(usize, bool)> = Vec::new();
        if let Some(p) = prev {
            if cubic(&fp.segments[p]).is_some() {
                hs.push((p, true));
            }
        }
        if let Some(q) = next {
            if cubic(&fp.segments[q]).is_some() {
                hs.push((q, false));
            }
        }
        if hs.is_empty() {
            continue;
        }
        let din = prev.and_then(|p| end_direction(&fp.segments[p], starts[p], true));
        let dout = next.and_then(|q| end_direction(&fp.segments[q], starts[q], false));
        let smooth = match (din, dout) {
            (Some(a), Some(b)) => {
                (a.0 * b.0 + a.1 * b.1).clamp(-1.0, 1.0).acos().to_degrees() <= G1_SNAP_DEG
            }
            _ => false,
        };
        let groups: Vec<Vec<(usize, bool)>> = if smooth {
            if hs.len() < 2 {
                // One handle against a line, smoothly: the line decides the direction.
                continue;
            }
            vec![hs]
        } else {
            hs.into_iter().map(|h| vec![h]).collect()
        };
        for group in groups {
            snap_group(fp, &group, node, pts, sigma, st);
        }
    }
}

/// Snap the handles of `group` (each `(segment, at its end?)`, all at `node`) onto one target
/// together, taken from the first handle's direction, kept only if the ring still guards.
fn snap_group(
    fp: &mut FittedPath,
    group: &[(usize, bool)],
    node: Point,
    pts: &[Point],
    sigma: &[f64],
    st: &mut EditStats,
) {
    let handle = |fp: &FittedPath, (q, at_end): (usize, bool)| -> Option<Point> {
        cubic(&fp.segments[q]).map(|(c1, c2, _)| if at_end { c2 } else { c1 })
    };
    // The direction of travel through the node, read from the first handle.
    let Some(h0) = handle(fp, group[0]) else {
        return;
    };
    let (hx, hy) = (h0.x - node.x, h0.y - node.y);
    if hx.hypot(hy) < 1e-9 {
        return;
    }
    let travel = if group[0].1 { (-hx, -hy) } else { (hx, hy) };
    let deg = travel.1.atan2(travel.0).to_degrees();
    let Some((target, axis)) = snap_target(deg) else {
        return;
    };
    if (target - deg).abs() <= 1e-9 {
        return;
    }
    let before = fp.segments.clone();
    let r = target.to_radians();
    let (tx, ty) = (r.cos(), r.sin());
    for &(q, at_end) in group {
        let Some(h) = handle(fp, (q, at_end)) else {
            continue;
        };
        let l = node.dist(h);
        if l < 1e-9 {
            continue;
        }
        // Into the segment: against the travel at its end, with it at its start.
        let s = if at_end { -1.0 } else { 1.0 };
        let p = Point::new(node.x + s * tx * l, node.y + s * ty * l);
        if let Segment::Cubic(a, b, _) = &mut fp.segments[q] {
            if at_end {
                *b = p;
            } else {
                *a = p;
            }
        }
    }
    let kind = if axis { price::AXIS } else { price::DIAG };
    if accept_ring(kind, (fp.start, &before), fp, pts, sigma, 0.0) {
        let k = group.len();
        if axis {
            st.axis_snaps += k;
        } else {
            st.diag_snaps += k;
        }
    } else {
        fp.segments = before;
        st.reverted += 1;
    }
}

/// Two lines meeting at a turn of at most [`COLLINEAR_DEG`] are one straight run: written
/// as one line from the first's start to the second's end. Node alignment ([`pass_nodes`])
/// lines nodes up on shared coordinates, and mirror lock ([`pass_mirror`]) splits a line
/// where the axis crosses it; both leave a node in the middle of a straight run, which
/// artists do not draw (the battery's `collinear` rose 15 % under `--editability`). On a
/// closed ring the join where it starts counts too: the ring then starts at the next node.
///
/// Only the merged line is judged, against the two it replaces: it is kept when it strays
/// from the evidence ([`stray`]) no further than [`EDIT_SLACK`] or than they did. Judged
/// on the whole ring, a mirror-locked ring, held to the wider [`MIRROR_SLACK`], could never
/// lose the node its lock put on the axis.
fn pass_collinear(fp: &mut FittedPath, pts: &[Point], sigma: &[f64], st: &mut EditStats) {
    let mut i = 0;
    while fp.segments.len() >= 2 && i < fp.segments.len() {
        let n = fp.segments.len();
        let j = (i + 1) % n;
        if !fp.closed && j == 0 {
            break;
        }
        let starts = chain_starts(fp);
        let (Segment::Line(a), Segment::Line(b)) = (&fp.segments[i], &fp.segments[j]) else {
            i += 1;
            continue;
        };
        let (a, b) = (*a, *b);
        let (d1, d2) = (
            (a.x - starts[i].x, a.y - starts[i].y),
            (b.x - a.x, b.y - a.y),
        );
        let (l1, l2) = (d1.0.hypot(d1.1), d2.0.hypot(d2.1));
        let collinear = l1 > 1e-9
            && l2 > 1e-9
            && ((d1.0 * d2.0 + d1.1 * d2.1) / (l1 * l2))
                .clamp(-1.0, 1.0)
                .acos()
                .to_degrees()
                <= COLLINEAR_DEG;
        if !collinear {
            i += 1;
            continue;
        }
        let old = stray(
            &[Segment::Line(a), Segment::Line(b)],
            &[starts[i], a],
            pts,
            sigma,
        );
        let new = stray(&[Segment::Line(b)], &[starts[i]], pts, sigma);
        let mut merged = fp.clone();
        if j == 0 {
            // The ring's own start is the middle node: start at the next one instead.
            merged.start = merged.segments[0].end();
            merged.segments.remove(0);
            let last = merged.segments.len() - 1;
            merged.segments[last] = Segment::Line(merged.start);
        } else {
            merged.segments[i] = Segment::Line(b);
            merged.segments.remove(j);
        }
        let ring = Touched {
            pts,
            sigma,
            before: (fp.start, &fp.segments),
            after: (merged.start, &merged.segments),
        };
        let ok = priced(price::COLLINEAR, &[ring], 0.0).unwrap_or(new <= old.max(EDIT_SLACK));
        if !ok {
            st.reverted += 1;
            i += 1;
            continue;
        }
        *fp = merged;
        st.collinear_merged += 1;
    }
}

/// Equal handle lengths within [`LEN_MATCH`]. (This used to be called "the smooth-point
/// tool's signature", but a smooth node in an editor equalises the two handles *at a
/// node*, which belong to different segments; this pass does something else.)
///
/// Per cubic, not per node: the two handles compared are the same segment's, `l1 =
/// |c1 − start|` and `l2 = |end − c2|` (px). When the shorter is at least `1 − LEN_MATCH`
/// (90%) of the longer, both are rescaled to their mean `m = (l1 + l2) / 2` along their own
/// directions, so the curve's two ends pull equally. Directions and nodes do not move.
/// Lines, arcs, zero-length handles and handles already equal are skipped; each change is
/// kept or reverted on its own.
fn pass_equalize(
    fp: &mut FittedPath,
    starts: &[Point],
    pts: &[Point],
    sigma: &[f64],
    st: &mut EditStats,
) {
    let n = fp.segments.len();
    for i in 0..n {
        let end = starts[i + 1];
        let Some((c1, c2, _)) = cubic(&fp.segments[i]) else {
            continue;
        };
        let l1 = (c1.x - starts[i].x).hypot(c1.y - starts[i].y);
        let l2 = (end.x - c2.x).hypot(end.y - c2.y);
        if l1 < 1e-9 || l2 < 1e-9 {
            continue;
        }
        let r = l1.min(l2) / l1.max(l2);
        if !(1.0 - LEN_MATCH..=1.0).contains(&r) || (l1 - l2).abs() <= 1e-9 {
            continue;
        }
        let before = fp.segments.clone();
        let m = (l1 + l2) / 2.0;
        let nc1 = Point::new(
            starts[i].x + (c1.x - starts[i].x) / l1 * m,
            starts[i].y + (c1.y - starts[i].y) / l1 * m,
        );
        let nc2 = Point::new(
            end.x + (c2.x - end.x) / l2 * m,
            end.y + (c2.y - end.y) / l2 * m,
        );
        if let Segment::Cubic(a, b, _) = &mut fp.segments[i] {
            *a = nc1;
            *b = nc2;
        }
        if accept_ring(price::EQUAL, (fp.start, &before), fp, pts, sigma, 0.0) {
            st.len_sym += 1;
        } else {
            fp.segments = before;
            st.reverted += 1;
        }
    }
}

/// The same curve reflected about `cx` and traversed backwards, so that it ends
/// where the original began. Reversing an arc flips its sweep and reflecting it
/// flips the sweep again, so the two cancel and only the rotation negates.
///
/// The reflection is `mir(x, y) = (2·cx − x, y)`, a mirror in the vertical line `x = cx`.
/// A cubic from `s` through `c1`, `c2` to `e` becomes one from `mir(e)` through `mir(c2)`,
/// `mir(c1)` to `mir(s)`: reversing a Bézier curve reverses its control points. The
/// caller supplies `seg_start` because a segment stores only its end; the new segment's
/// start is implied by whatever precedes it, and must be `mir(e)`.
fn mirror_reverse(seg: &Segment, seg_start: Point, cx: f64) -> Segment {
    let mir = |p: Point| Point::new(2.0 * cx - p.x, p.y);
    match *seg {
        Segment::Cubic(c1, c2, _) => Segment::Cubic(mir(c2), mir(c1), mir(seg_start)),
        Segment::Line(_) => Segment::Line(mir(seg_start)),
        Segment::Arc {
            rx,
            ry,
            phi,
            large_arc,
            sweep,
            ..
        } => Segment::Arc {
            rx,
            ry,
            phi: -phi,
            large_arc,
            sweep,
            end: mir(seg_start),
        },
    }
}

/// One segment cut in two at `t`, drawing exactly what it drew before. De
/// Casteljau for a cubic, a lerp for a line; an arc has no exact split here and
/// declines, so a ring whose axis crosses one is left alone.
///
/// De Casteljau: interpolate each consecutive pair of control points at `t`, then the
/// results again, until one point is left. That point `m` is the curve at `t`, and the
/// intermediate points are the control points of the two halves, `(s, p01, p012, m)` and
/// `(m, p123, p23, e)`. It is exact, so the two halves draw precisely the original curve.
/// `t` is expected in `(0, 1)`; the caller only splits strictly inside a segment.
fn split_at(seg: &Segment, start: Point, t: f64) -> Option<(Segment, Segment)> {
    let lerp = |p: Point, q: Point| Point::new(p.x + (q.x - p.x) * t, p.y + (q.y - p.y) * t);
    match *seg {
        Segment::Cubic(c1, c2, e) => {
            let (p01, p12, p23) = (lerp(start, c1), lerp(c1, c2), lerp(c2, e));
            let (p012, p123) = (lerp(p01, p12), lerp(p12, p23));
            let m = lerp(p012, p123);
            Some((Segment::Cubic(p01, p012, m), Segment::Cubic(p123, p23, e)))
        }
        Segment::Line(e) => Some((Segment::Line(lerp(start, e)), Segment::Line(e))),
        Segment::Arc { .. } => None,
    }
}

/// Where inside a segment the vertical line `x = cx` cuts it, if it does. Found by
/// bracketing a sign change and bisecting, which needs nothing of the segment but
/// the ability to evaluate it.
///
/// `f(t) = x(t) − cx` is sampled at 32 even steps of `t`; the first interval where `f`
/// changes sign or touches zero is narrowed by 50 rounds of bisection, far below any
/// visible distance. Only the first crossing in the segment is found. Returns
/// `t` strictly inside `(1e-6, 1 − 1e-6)`, and `None` when the segment does not cross,
/// when the crossing is at an end (already a node), or when two crossings fall inside one
/// sampling step and cancel out.
fn interior_crossing(seg: &Segment, start: Point, cx: f64) -> Option<f64> {
    const STEPS: usize = 32;
    let x = |t: f64| eval_segment(seg, start, t).x - cx;
    let f0 = x(0.0);
    if f0 == 0.0 {
        // the start node is already the crossing; the caller counts that case
        return None;
    }
    let (mut lo, mut flo) = (0.0_f64, f0);
    for k in 1..=STEPS {
        let hi = k as f64 / STEPS as f64;
        let fhi = x(hi);
        // `<=`, not `<`: a sample can land exactly on the axis, and a strict test
        // then walks straight past the crossing it is looking for. Axis-aligned
        // artwork makes that the common case, not the corner one -- a line crossing
        // at t = 1/2 was missed entirely.
        if flo * fhi <= 0.0 {
            let (mut a, mut b) = (lo, hi);
            for _ in 0..50 {
                let m = 0.5 * (a + b);
                if x(a) * x(m) <= 0.0 {
                    b = m;
                } else {
                    a = m;
                }
            }
            let t = 0.5 * (a + b);
            return (t > 1e-6 && t < 1.0 - 1e-6).then_some(t);
        }
        lo = hi;
        flo = fhi;
    }
    None
}

/// The ring with the axis crossings turned into nodes, splitting whichever segment
/// each one falls inside. Returns the segments, the chain of node positions and how
/// many splits it took; `None` when a crossing lands inside an arc, which has no
/// exact split here.
///
/// A segment whose start or end is already within [`AXIS_NODE`] (1 px) of the axis is
/// not split: that node will serve as the crossing. The chain is rebuilt only when
/// something was split; otherwise it is `starts` unchanged.
fn with_axis_nodes(
    fp: &FittedPath,
    starts: &[Point],
    cx: f64,
) -> Option<(Vec<Segment>, Vec<Point>, usize)> {
    let mut segs = fp.segments.clone();
    let mut chain = starts.to_vec();
    let on_axis = |p: Point| (p.x - cx).abs() <= AXIS_NODE;
    let mut splits: Vec<(usize, f64)> = Vec::new();
    for i in 0..segs.len() {
        if on_axis(chain[i]) || on_axis(chain[i + 1]) {
            continue; // an endpoint is already the crossing
        }
        if let Some(t) = interior_crossing(&segs[i], chain[i], cx) {
            splits.push((i, t));
        }
    }
    // Highest index first, so the earlier ones keep their positions.
    for &(i, t) in splits.iter().rev() {
        let (a, b) = split_at(&segs[i], chain[i], t)?;
        segs.splice(i..=i, [a, b]);
    }
    if !splits.is_empty() {
        chain = std::iter::once(fp.start)
            .chain(segs.iter().map(|s| s.end()))
            .collect();
    }
    Some((segs, chain, splits.len()))
}

/// The ring rebuilt from whichever of its two halves describes the traced points
/// better: that half kept as drawn, and the other replaced by its reflection.
/// Returns how far the result strays from those points, where it starts, and its
/// segments.
///
/// `poles` are the indices in `chain` of the two nodes on the axis. Both are first moved
/// horizontally onto `x = cx`, since a point on the mirror line is its own reflection;
/// that is what lets the rebuilt ring close. For each direction, pole to pole, the
/// segments of that half are kept (the last one ending on the far pole) and followed by the
/// same segments reflected and reversed ([`mirror_reverse`]), which walk back to the start.
///
/// The "stray" is the one-sided (directed) Hausdorff distance from the new ring to the
/// traced points: `max over samples s of min over points q of |s − q|`, in px, with 16
/// samples per segment. The direction with the smaller stray wins (the first on a tie).
/// `None` only when neither half has a segment to keep, which cannot happen with two
/// distinct poles.
fn better_half(
    segs: &[Segment],
    chain: &[Point],
    poles: (usize, usize),
    cx: f64,
    pts: &[Point],
) -> Option<(f64, Point, Vec<Segment>)> {
    let (p, q) = poles;
    let n = segs.len();
    // A node the axis passes through is its own mirror, so put it exactly on the
    // axis. That is what lets the ring close: the reflected run ends on the mirror
    // of the first pole, which is the first pole itself only once it sits at `cx`.
    let mut nodes = chain.to_vec();
    nodes[p] = Point::new(cx, chain[p].y);
    nodes[q] = Point::new(cx, chain[q].y);

    let mut best: Option<(f64, Point, Vec<Segment>)> = None;
    for (from, to) in [(p, q), (q, p)] {
        // keep the segments from one pole to the other, then append that same run
        // reflected and reversed
        let mut keep = Vec::new();
        let mut k = from;
        while k != to {
            keep.push(k);
            k = (k + 1) % n;
        }
        let mut out: Vec<Segment> = keep.iter().map(|&k| segs[k].clone()).collect();
        if let Some(last) = out.last_mut() {
            set_end(last, nodes[to]); // the kept run finishes on the far pole
        }
        for &k in keep.iter().rev() {
            out.push(mirror_reverse(&segs[k], nodes[k], cx));
        }
        if out.len() < 2 {
            continue;
        }
        let start = nodes[from];
        let built: Vec<Point> = std::iter::once(start)
            .chain(out.iter().map(|s| s.end()))
            .collect();
        let stray = samples(&out, &built, 16)
            .iter()
            .map(|p| pts.iter().map(|q| p.dist(*q)).fold(f64::INFINITY, f64::min))
            .fold(0.0, f64::max);
        if best.as_ref().is_none_or(|(b, _, _)| stray < *b) {
            best = Some((stray, start, out));
        }
    }
    best
}

/// Mirror lock: a closed ring whose reflection about its own vertical centre axis
/// lands back on itself is rewritten so that it does so *exactly* -- one half is
/// kept and the other replaced by its reflection.
///
/// Replacing a half, rather than averaging the two, is what makes this work on
/// traced geometry. The fitter segments the two halves independently and rarely
/// gives them the same number of segments (5 against 6 on the test heart), so no
/// pairing between them exists to average: pairing by nearest mirrored midpoint
/// lets segment i pair with j while i+1 pairs with j-2, the shared endpoints the
/// two pairs compute then disagree, and the chain tears -- measured as a 21 px
/// stray on a ring that is symmetric to the pixel. Reflecting a whole half cannot
/// tear, because the seam sits on the two points where the axis meets the ring,
/// and each of those is its own mirror.
///
/// Those two points are rarely nodes. Measured over the corpus, every ring that
/// passed the symmetry test was then thrown away for want of them: 9 rings had one
/// node on the axis and 9 had none, and not one had two. A traced shape has no
/// reason to put a node where its axis happens to fall, so the crossing is found
/// inside whichever segment contains it and that segment is split there -- which
/// draws the same curve, and costs one node.
///
/// Both halves are tried and the one that strays less from the traced points wins,
/// so the sloppier half is the one that gets replaced.
///
/// The steps, all in px:
///
/// 1. Only a ring that is closed (by flag, or with its end within 0.05 px of its start)
///    and has at least four segments is considered.
/// 2. The candidate axis is the vertical line through the middle of the traced points'
///    x range, `cx = (min x + max x) / 2`, rounded to the emitter's grid [`EMIT_GRID`].
///    Only left-right symmetry is looked for.
/// 3. Symmetry test on the evidence, not the fit: at least [`MIRROR_KEEP`] (95%) of the
///    traced points must have a traced point within `3·σ_0 + EDIT_SLACK` of their
///    reflection. `σ_0` is the first point's sigma, used for every point here (unlike the
///    acceptance test, which uses each point's own).
/// 4. The axis crossings are made into nodes ([`with_axis_nodes`]); exactly two nodes must
///    then lie within [`AXIS_NODE`] of the axis. An arc at a crossing, or any other count,
///    declines.
/// 5. The better half is mirrored ([`better_half`]) and the result is kept only if it
///    passes the acceptance test with the larger [`MIRROR_SLACK`] budget; otherwise the
///    ring is left exactly as it was and counted as reverted.
///
/// `INKVEC_EDIT_DEBUG` prints why each ring was declined or how far the locked one strays.
fn pass_mirror(
    fp: &mut FittedPath,
    starts: &[Point],
    pts: &[Point],
    sigma: &[f64],
    st: &mut EditStats,
) {
    let geometrically_closed = fp
        .segments
        .last()
        .map(|s| s.end().dist(fp.start) <= 0.05)
        .unwrap_or(false);
    if (!fp.closed && !geometrically_closed) || fp.segments.len() < 4 {
        return;
    }
    let debug = inkvec_core::env::flag("INKVEC_EDIT_DEBUG");
    let xs: Vec<f64> = pts.iter().map(|p| p.x).collect();
    let cx_raw = (xs.iter().cloned().fold(f64::MAX, f64::min)
        + xs.iter().cloned().fold(f64::MIN, f64::max))
        / 2.0;
    // Put the axis on the grid the emitter writes, so that a mirrored pair rounds to
    // a mirrored pair and a node *on* the axis writes the axis exactly. Without it
    // an exactly-mirrored ring still emitted pairs 0.01 px apart.
    let cx = (cx_raw / EMIT_GRID).round() * EMIT_GRID;
    let mir = |p: Point| Point::new(2.0 * cx - p.x, p.y);

    // Is the drawing itself symmetric? Asked of the measured points, not of the
    // fit, so a ring is only locked when the evidence says it was drawn that way.
    let mut hit = 0;
    for p in pts {
        if pts
            .iter()
            .any(|q| mir(*p).dist(*q) <= 3.0 * sigma[0] + EDIT_SLACK)
        {
            hit += 1;
        }
    }
    if (hit as f64) < MIRROR_KEEP * pts.len() as f64 {
        return;
    }

    let Some((segs, chain, splits)) = with_axis_nodes(fp, starts, cx) else {
        diag::debug(debug, || {
            "    mirror: the axis crosses an arc -- declined".into()
        });
        return;
    };
    let n = segs.len();
    let poles: Vec<usize> = (0..n)
        .filter(|&k| (chain[k].x - cx).abs() <= AXIS_NODE)
        .collect();
    if poles.len() != 2 {
        diag::debug(debug, || {
            format!(
                "    mirror: {} points on the axis after {splits} split(s), want 2 -- declined",
                poles.len(),
            )
        });
        return;
    }

    let (p, q) = (poles[0], poles[1]);
    let Some((stray, start, out)) = better_half(&segs, &chain, (p, q), cx, pts) else {
        return;
    };
    diag::debug(debug, || {
        format!(
            "    mirror: {}/{} points reflect, {} split(s), poles {p},{q}, \
locked ring strays {stray:.3} px (budget {MIRROR_SLACK})",
            hit,
            pts.len(),
            splits
        )
    });

    // Judged against the mirror budget, not the handle one: locking moves geometry
    // by the asymmetry between the two traced halves, which is the whole point of
    // the pass and is larger than a handle snap's.
    let chain: Vec<Point> = std::iter::once(start)
        .chain(out.iter().map(|s| s.end()))
        .collect();
    let ring = Touched {
        pts,
        sigma,
        before: (fp.start, &fp.segments),
        after: (start, &out),
    };
    let ok = priced(price::MIRROR, &[ring], 0.0)
        .unwrap_or_else(|| guarded_slack(&out, &chain, pts, sigma, MIRROR_SLACK));
    if ok {
        st.mirror_rings += 1;
        fp.segments = out;
        fp.start = start;
    } else {
        st.reverted += 1;
    }
}

/// Move node `node` of `fp` (0 its start, `k` the end of segment `k − 1`) to `p`, with its
/// handles: the control points either side of it move by the same amount. On a closed ring
/// node 0 is also the last segment's end.
fn move_node(fp: &mut FittedPath, node: usize, p: Point) {
    let n = fp.segments.len();
    let old = match node {
        0 => fp.start,
        k if k <= n => fp.segments[k - 1].end(),
        _ => return,
    };
    let d = (p.x - old.x, p.y - old.y);
    // The segment arriving at the node and the one leaving it.
    let arriving = if node > 0 {
        Some(node - 1)
    } else if fp.closed && n > 0 {
        Some(n - 1)
    } else {
        None
    };
    if node == 0 {
        fp.start = p;
    }
    if let Some(k) = arriving {
        set_end(&mut fp.segments[k], p);
        if let Segment::Cubic(_, c2, _) = &mut fp.segments[k] {
            *c2 = Point::new(c2.x + d.0, c2.y + d.1);
        }
    }
    if node < n {
        if let Segment::Cubic(c1, _, _) = &mut fp.segments[node] {
            *c1 = Point::new(c1.x + d.0, c1.y + d.1);
        }
    }
}

/// Move a segment's on-curve endpoint, whatever kind of segment it is. Control points
/// and arc parameters stay as they were: an arc, which SVG defines by its endpoints, is
/// redrawn with the same radii and flags between the new endpoints (SVG scales the radii
/// up if they have become too small to span them).
fn set_end(seg: &mut Segment, p: Point) {
    match seg {
        Segment::Cubic(_, _, e) | Segment::Line(e) => *e = p,
        Segment::Arc { end, .. } => *end = p,
    }
}

/// Node alignment across the whole drawing: node coordinates within [`NODE_ALIGN`]
/// of a shared target move onto it, so that shapes an artist would expect to line
/// up actually do. 98% of the nodes in the artist corpus share an exact x or y with
/// another node, which is what this is reaching for.
///
/// A ring's on-curve nodes are its segment *ends*: `start`, then `segments[k].end()`.
/// There is no start field to write, and `Cubic`'s first field is a control point --
/// treating it as the node moved handles onto node positions and collapsed every
/// `Line` to zero length, which is how the heart lost its bottom point.
///
/// The clustering is one-dimensional and done for x and for y separately. All nodes of
/// the rings listed in `free` (the non-primitive ones) are sorted by that coordinate; a
/// cluster is a run of consecutive values each within [`NODE_ALIGN`] (0.3 px) of the run's
/// first value, and every member moves to the value of the run's middle element (its
/// median). A node can therefore join one cluster in x and another in y. A moved node
/// carries its handles, as it does in an editor: the control points either side of it move
/// by the same amount, so its handles keep their directions and a smooth node stays smooth.
/// (Left behind, a handle a pixel long turned by up to 15 degrees for a 0.3 px move, and
/// the smooth node became a kink.)
///
/// The moves are then applied ring by ring, and each ring is held to the acceptance test
/// against its own polyline (`polys[i]` belongs to `fps[i]`). Unlike the other passes the
/// unit of acceptance is the whole ring: if any of its moves breaks the budget, all of that
/// ring's moves are undone and it counts as one revert. Because a shared cluster target is
/// computed before any ring is judged, a node in one ring may stay put while its partner in
/// another ring moves; they then no longer line up exactly, but neither has left the
/// evidence.
fn pass_nodes(
    fps: &mut [FittedPath],
    free: &[usize],
    polys: &[inkvec_core::Polyline],
    st: &mut EditStats,
) {
    #[derive(Clone, Copy)]
    struct NodeRef {
        fp: usize,
        node: usize, // 0 is the ring's start; k is the end of segment k-1
        x: f64,
        y: f64,
    }
    let mut nodes: Vec<NodeRef> = Vec::new();
    for &fi in free {
        let fp = &fps[fi];
        nodes.push(NodeRef {
            fp: fi,
            node: 0,
            x: fp.start.x,
            y: fp.start.y,
        });
        for (k, seg) in fp.segments.iter().enumerate() {
            // For a closed ring the last segment ends on node 0, which is already in.
            if fp.closed && k + 1 == fp.segments.len() {
                continue;
            }
            let e = seg.end();
            nodes.push(NodeRef {
                fp: fi,
                node: k + 1,
                x: e.x,
                y: e.y,
            });
        }
    }

    // Cluster each axis on its own: a node may share an x with one neighbour and a
    // y with another, and the two moves are independent.
    let mut moved: Vec<(usize, usize, f64, f64)> = Vec::new();
    for axis in 0..2 {
        let mut idx: Vec<usize> = (0..nodes.len()).collect();
        idx.sort_by(|&a, &b| {
            if axis == 0 {
                nodes[a].x.total_cmp(&nodes[b].x)
            } else {
                nodes[a].y.total_cmp(&nodes[b].y)
            }
        });
        let val = |nd: &NodeRef| if axis == 0 { nd.x } else { nd.y };
        let mut i = 0;
        while i < idx.len() {
            let mut j = i;
            while j + 1 < idx.len() && val(&nodes[idx[j + 1]]) - val(&nodes[idx[i]]) <= NODE_ALIGN {
                j += 1;
            }
            if j > i {
                let target = val(&nodes[idx[i + (j - i) / 2]]);
                for k in i..=j {
                    let nd = &mut nodes[idx[k]];
                    if (val(nd) - target).abs() > 1e-9 {
                        if axis == 0 {
                            nd.x = target;
                        } else {
                            nd.y = target;
                        }
                        moved.push((nd.fp, nd.node, nd.x, nd.y));
                    }
                }
            }
            i = j + 1;
        }
    }

    // Apply per ring, and hold each ring to the same budget as every other pass:
    // this one crosses ring borders, so it is the one most able to drag geometry
    // somewhere the evidence does not support.
    let mut by_ring: std::collections::HashMap<usize, Vec<(usize, f64, f64)>> =
        std::collections::HashMap::new();
    for (fi, node, x, y) in moved {
        by_ring.entry(fi).or_default().push((node, x, y));
    }
    for (fi, edits) in by_ring {
        let fp = &mut fps[fi];
        let before = fp.clone();
        for &(node, x, y) in &edits {
            move_node(fp, node, Point::new(x, y));
        }
        let poly = &polys[fi];
        let ok = accept_ring(
            price::NODES,
            (before.start, &before.segments),
            fp,
            &poly.points,
            &poly.sigma,
            0.0,
        );
        if ok {
            st.nodes_aligned += edits.len();
        } else {
            *fp = before;
            st.reverted += 1;
        }
    }
}

/// Apply every pass to every ring: first the passes that rewrite segments, arcs as cubics
/// and runs re-noded at their extrema; then the passes that move on-curve nodes, mirror
/// lock (it rewrites halves wholesale) and node alignment across the whole drawing (it
/// crosses ring borders); then, ring by ring, collinear lines merged, G1, the handle snaps
/// and equal lengths; last the joins at junctions, across rings. The comments in the body
/// give the reasons for that order. Rings that are already a primitive element (circle/ellipse/rect) are
/// finished geometry — their nodes are load-bearing for the element and are left alone.
///
/// `polys`, `fps` and `prims` are parallel, one entry per planar-map edge: the measured
/// boundary (points and sigma, px), its fitted path (edited in place) and its primitive,
/// if the fit found one. `order` holds each face's rings over those edges, and `cfg` the
/// fit's price of a parameter. Each pass is guarded, so the output never strays from the
/// evidence further than the budgets in the module docs. In a `research` build
/// `INKVEC_EDIT_PASSES` can switch passes off ([`on`]): `arcs`, `extrema`, `mirror`,
/// `nodes`, `collinear`, `g1`, `handles`, `equalize`, `junctions`.
pub(crate) fn edit_all(
    polys: &[inkvec_core::Polyline],
    fps: &mut [FittedPath],
    prims: &[Option<PrimitiveFit>],
    order: &[FaceRings],
    cfg: &inkvec_fit::FitConfig,
    side_px: usize,
) -> EditStats {
    let mut st = EditStats {
        rings: fps.len(),
        ..Default::default()
    };
    // The prices the passes charge on this thread: a number's (λ), and the raster's longer
    // side, which scales the smooth bonus (`price::scaled`).
    price::set_lambda(cfg.lambda);
    price::set_side(side_px as f64);
    let free: Vec<usize> = (0..fps.len()).filter(|&i| prims[i].is_none()).collect();
    // First the curves themselves: arcs written as the cubics a drawing tool writes, cut at
    // their extrema, then each smooth run re-segmented at its extrema where the stronger
    // preference of this mode finds it worth it. Both rewrite segments, so they run before
    // anything that reads a node or a handle.
    for (fp, (poly, prim)) in fps.iter_mut().zip(polys.iter().zip(prims.iter())) {
        if prim.is_some() {
            continue;
        }
        if on("arcs") {
            curves::pass_arcs(fp, poly, &mut st);
        }
        if on("extrema") {
            curves::pass_extrema(fp, poly, cfg, &mut st);
        }
    }
    // The passes that move on-curve NODES run first: mirror lock rewrites a whole
    // half, and node alignment slides nodes onto shared targets. Only then are the
    // handles pointed, because a handle's direction is measured from its node, and
    // moving the node afterwards turns the handle back off the axis it was snapped
    // to -- priced on the corpus, alignment last cost three fifths of what handle
    // snapping had just bought (5.0% of handles on an axis, down to 3.2%).
    for (fp, (poly, prim)) in fps.iter_mut().zip(polys.iter().zip(prims.iter())) {
        if prim.is_some() {
            continue;
        }
        if on("mirror") {
            pass_mirror(fp, &chain_starts(fp), &poly.points, &poly.sigma, &mut st);
        }
    }
    if on("nodes") {
        pass_nodes(fps, &free, polys, &mut st);
    }
    // A boundary with no junction on it is one edge that ends where it starts, an open path
    // from a point the planar map chose. Its closing join is a join like any other, so the
    // per-ring passes below see it closed (and no other edge ends there, so its start may
    // move); the flag is put back after them.
    let loops = isolated_loops(fps, &free);
    for &i in &loops {
        fps[i].closed = true;
    }
    for (fp, (poly, prim)) in fps.iter_mut().zip(polys.iter().zip(prims.iter())) {
        if prim.is_some() {
            continue;
        }
        let sigma = &poly.sigma;
        // Each reads the chain the pass before it left: mirror lock can change how
        // many segments a ring has, and alignment can leave two lines end to end.
        if on("collinear") {
            pass_collinear(fp, &poly.points, sigma, &mut st);
        }
        if on("g1") {
            pass_g1(fp, &chain_starts(fp), &poly.points, sigma, &mut st);
        }
        if on("handles") {
            pass_handles(fp, &chain_starts(fp), &poly.points, sigma, &mut st);
        }
        if on("equalize") {
            pass_equalize(fp, &chain_starts(fp), &poly.points, sigma, &mut st);
        }
    }
    for &i in &loops {
        fps[i].closed = false;
    }
    // Joins between edges, which no per-ring pass sees, last: a junction whose one side was
    // just snapped onto an axis keeps that side's direction (`junctions`), so the snap
    // survives the smoothing.
    if on("junctions") {
        junctions::pass_junctions(order, fps, polys, prims, &mut st);
    }
    st
}

/// The rings among `free` that are open paths ending exactly where they start, at a point no
/// other ring starts or ends at: a boundary with no junction on it.
fn isolated_loops(fps: &[FittedPath], free: &[usize]) -> Vec<usize> {
    const SAME: f64 = 1e-6;
    let ends: Vec<(usize, Point)> = fps
        .iter()
        .enumerate()
        .filter(|(_, fp)| !fp.segments.is_empty())
        .flat_map(|(i, fp)| [(i, fp.start), (i, fp.segments[fp.segments.len() - 1].end())])
        .collect();
    free.iter()
        .copied()
        .filter(|&i| {
            let fp = &fps[i];
            let Some(last) = fp.segments.last() else {
                return false;
            };
            !fp.closed
                && fp.segments.len() >= 2
                && last.end().dist(fp.start) <= SAME
                && ends.iter().all(|&(j, p)| j == i || p.dist(fp.start) > SAME)
        })
        .collect()
}

/// Whether a pass runs. Every pass but `nodes`: node alignment across rings, under MDL
/// pricing ([`price`]), alone moved the gate's turning_gap +3.8 % on the screen set and
/// brought the design battery no closer to the artists (2026-10-13). In a `research` build
/// `INKVEC_EDIT_PASSES` names the ones to keep, comma separated, so each can be priced on
/// its own against the corpus.
fn on(pass: &str) -> bool {
    if cfg!(feature = "research") {
        if let Some(v) = inkvec_core::env::text("INKVEC_EDIT_PASSES") {
            return v.split(',').any(|p| p.trim() == pass);
        }
    }
    pass != "nodes"
}

#[cfg(test)]
mod tests;
