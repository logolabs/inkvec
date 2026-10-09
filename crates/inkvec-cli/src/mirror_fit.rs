//! Fit a boundary that is its own mirror image so that the fit is exactly symmetric.
//!
//! # The problem
//!
//! The trace keeps the symmetry the artist drew (`inkvec_trace::symmetry`): the label map
//! says which mirrors hold, `symmetry::enforce` makes every boundary point the exact
//! reflection of its partner, and `pipeline::apply_mirrors` replaces the fit of each
//! mirror-paired boundary by the reflection of its partner's fit. A boundary that is paired
//! with *itself* -- a closed outline lying across the axis, or an open boundary running
//! from a junction on one side to its mirror junction on the other -- has no partner to copy,
//! and was fitted as it came: its points are exactly symmetric, but the dynamic program
//! places its vertices wherever the objective's ties fall, which is not symmetrically. On
//! `lucide/beaker` (the case `real_symmetry` of `bench/cases.py`) both outlines straddle the
//! axis, and the left top corner came back at x = 21.66 against 22.51 for the reflection of
//! the right one: the render's mirror residual rose from 7.5e-5 to 2.1e-4 when the converged
//! boundary solve of 0.2.6 moved the points slightly and the program broke its ties
//! differently (the points were symmetric both times; only the fit was not).
//!
//! # The method
//!
//! Fit the *fundamental domain* and reflect it. A boundary symmetric under a mirror `M`
//! crosses the mirror's axis at fixed points of the index involution `i ↦ π(i)` the
//! pairing defines: two for a closed outline (whose reflection reverses its direction of
//! travel, `π(i) = s − i mod n`), one for an open boundary (`π(i) = n − 1 − i`). A crossing
//! is either a point of the boundary (`π(i) = i`) or the middle of one of its pieces
//! (`π(i) = i + 1`), which then lies exactly on the axis. Then:
//!
//! 1. [`half`]: the points from one crossing to the next (closed) or from the start to the
//!    crossing (open), the crossings snapped exactly onto the axis, the per-point sigmas
//!    carried along (a mid-piece crossing takes the mean of its two neighbours').
//! 2. The half is fitted by the ordinary fitter (the multimodel dynamic program, whose open
//!    form keeps both end points where they were measured), recursively when the half is
//!    itself symmetric under a second mirror (a rounded square across both axes is fitted as
//!    one quarter).
//! 3. [`assemble`]: the fit followed by its reflection, run backwards, so the result is the
//!    same curve on both sides of the axis to the last bit (`symmetry::reflect_path`).
//! 4. [`polish_joins`]: at each axis crossing the two halves meet in a vertex the fitter
//!    did not choose. Two mirrored lines are tried as one line square to the axis, and two
//!    mirrored circular arcs as one arc of twice the angle whose centre is on the axis; a
//!    cubic arriving almost at a right angle is given exactly that end tangent, so the two
//!    halves join smoothly. Each change is kept only when the description length of
//!    the whole boundary, `½χ² + λ·k` (`multimodel::path_cost`, the fitter's own objective),
//!    does not rise -- a cubic's tangent snap is allowed one λ, the price of the angle it
//!    stops describing.
//!
//! 5. [`choose`]: the ordinary fit is kept when it is already symmetric, and otherwise
//!    whichever of the two is the shorter description of the whole boundary, the
//!    symmetric one on a tie.
//!
//! A boundary whose points do not reflect onto themselves to within `SAME` (1e-6 px,
//! where `enforce` leaves them equal to about 1e-12) is not touched: [`fit`] returns
//! `None` and the caller fits it as before.
//!
//! # Where this sits
//!
//! Quality mode's fit stage: `pipeline::fit_boundaries` calls [`choose`] for every boundary
//! `Symmetry::self_mirrors` names, in place of the ordinary fit, before the whole-boundary
//! primitive is offered (a primitive that wins is centred on the axis by
//! `pipeline::apply_mirrors`, as before). Fast mode fits nothing here. The repair stage may
//! refit a boundary whose ring crosses another; it does not keep this symmetry.
//!
//! # Sources
//!
//! Not from the literature: fitting the fundamental domain of a reflection and reflecting
//! the result is the rule `inkvec_trace::symmetry` already applies to mirror-paired
//! boundaries ("fitting one of each pair and reflecting the result, which is both exact
//! and half the work"), extended to a boundary that is its own pair, because the published
//! symmetrisation methods move geometry towards symmetry rather than fit curves under it.
//! See also: N. J. Mitra, L. J. Guibas, M. Pauly (2007), *Symmetrization*, ACM TOG 26(3),
//! <https://doi.org/10.1145/1276377.1276456>, which makes approximate symmetries of a shape
//! exact by deforming it -- what `symmetry::enforce` does to the boundary points; and
//! J. Rissanen (1978), *Modeling by shortest data description*, Automatica 14(5),
//! <https://doi.org/10.1016/0005-1098(78)90005-5>, whose two-part code is the objective
//! the joins are judged by.

use inkvec_core::{Point, Polyline};
use inkvec_fit::curves::Segment;
use inkvec_fit::{multimodel, FitConfig, FittedPath};
use inkvec_trace::symmetry::{reflect_path, Mirror};

/// How close a point and the reflection of its partner must be for a boundary to count as
/// its own mirror image, in px. `symmetry::enforce` leaves them equal to rounding (about
/// 1e-12); anything further apart was not symmetrised and is left to the ordinary fit.
const SAME: f64 = 1e-6;

/// Fewest points a boundary must have for its symmetric fit; shorter ones are fitted as
/// they are (a half of one or two points has nothing to fit).
const MIN_POINTS: usize = 4;

/// Largest angle, in degrees, between a cubic's end tangent at an axis crossing and the
/// axis normal for [`polish_joins`] to try making the join smooth. The fitter's own
/// threshold for a smooth join (`G1_BREAK_DEGREES` in `inkvec_fit::multimodel`) is ten
/// degrees; a turn larger than that at the axis is a corner the artist drew.
const SNAP_DEGREES: f64 = 10.0;

/// The exactly symmetric fit of `poly` under `mirrors` (the mirrors that carry this
/// boundary onto itself, from `Symmetry::self_mirrors`), or `None` when `poly` is not its
/// own reflection under the first of them to within `SAME`, is too short, or the fitter
/// moved an end of the half (which it does not do). `fit_plain` is the ordinary fitter;
/// `cfg` supplies the λ the joins are priced at.
///
/// Both sides of the axis are tried, and the shorter description kept: the two halves are
/// mirror images as data, but the dynamic program walks them in opposite directions
/// relative to the shape, so their fits differ, and either can be the better copy (on
/// `lucide/bell` the side fitted first came back with a squarer corner than the other).
/// The other side of a closed boundary is its half from the second crossing to the first
/// (the ring rotated to start there); of an open boundary, its half from the end, fitted on
/// the reversed boundary and reversed back.
pub(crate) fn fit(
    poly: &Polyline,
    mirrors: &[Mirror],
    cfg: &FitConfig,
    fit_plain: &dyn Fn(&Polyline) -> FittedPath,
) -> Option<FittedPath> {
    let &m = mirrors.first()?;
    if poly.points.len() < MIN_POINTS {
        return None;
    }
    let one = fit_side(poly, mirrors, cfg, fit_plain)?;
    let other = if poly.closed {
        let rotated = rotate_to_second_crossing(poly, m)?;
        fit_side(&rotated, mirrors, cfg, fit_plain)
    } else {
        let reversed = Polyline::new(
            poly.points.iter().rev().copied().collect(),
            poly.sigma.iter().rev().copied().collect(),
            false,
        );
        fit_side(&reversed, mirrors, cfg, fit_plain).map(|p| p.reversed())
    };
    let Some(other) = other else {
        return Some(one);
    };
    let cost = |p: &FittedPath| multimodel::path_cost(poly, p, cfg);
    Some(if cost(&other) < cost(&one) {
        other
    } else {
        one
    })
}

/// [`fit`] from the half that starts at the first axis crossing in index order (closed)
/// or at the start (open).
fn fit_side(
    poly: &Polyline,
    mirrors: &[Mirror],
    cfg: &FitConfig,
    fit_plain: &dyn Fn(&Polyline) -> FittedPath,
) -> Option<FittedPath> {
    let (&m, rest) = mirrors.split_first()?;
    let half = half(poly, m)?;
    // A second mirror that also carries the half onto itself (reversed) is used on the
    // half; one that does not is dropped, and the half is fitted without it.
    let further: Vec<Mirror> = rest
        .iter()
        .copied()
        .filter(|&m2| open_self_mirror(&half, m2))
        .collect();
    let mut part = fit(&half, &further, cfg, fit_plain).unwrap_or_else(|| fit_plain(&half));
    let (first, last) = (half.points[0], half.points[half.points.len() - 1]);
    if part.segments.is_empty() || part.start.dist(first) > SAME || end_of(&part).dist(last) > SAME
    {
        return None;
    }
    // The fitter works on a centred copy and translates back, which can move an end by a
    // rounding error; put the ends back exactly, so a crossing is exactly on the axis and
    // its reflection is itself.
    part.start = first;
    if let Some(s) = part.segments.last_mut() {
        set_end(s, last);
    }
    let path = assemble(&part, m, poly);
    Some(polish_joins(path, part, m, poly, cfg))
}

/// The closed `poly` rotated to start at its second axis crossing under `m` (at the
/// crossing's point, or at the second point of the piece it cuts), so that [`half`] takes
/// the other side of the axis. `None` when `poly` has no two crossings.
fn rotate_to_second_crossing(poly: &Polyline, m: Mirror) -> Option<Polyline> {
    let c = crossings(poly, m)?;
    let [_, second] = c[..] else {
        return None;
    };
    let r = match second {
        Crossing::Vertex(j) | Crossing::Piece(j) => j,
    };
    let mut pts = poly.points.clone();
    let mut sg = poly.sigma.clone();
    pts.rotate_left(r);
    sg.rotate_left(r);
    Some(Polyline::new(pts, sg, true))
}

/// The fit of a boundary that is its own mirror image under `mirrors`.
///
/// The ordinary fit when that is already symmetric (every anchor's reflection is an
/// anchor: the dynamic program found the symmetric optimum by itself, which it does on
/// most such boundaries); otherwise the symmetric fit ([`fit`]) when its description length
/// is no greater than the ordinary fit's, both measured by the fitter's own objective
/// `½χ² + λ·k` over the whole boundary (`multimodel::path_cost`); otherwise the ordinary
/// fit. `fit_plain` is the ordinary fitter.
///
/// Why the plain comparison. The symmetric fit cannot use a segment that spans the axis
/// unless [`polish_joins`] can merge its two halves back into one, so on a smooth crossing
/// it can cost a segment more than the ordinary fit. Three rules were measured on the gate
/// (2026-10-04, the 246-icon screen set judged at 1024 px, family-macro dE00 against v0.2.5,
/// with main at −15.95 % / −11.68 % / −7.15 % at 128 / 512 / 512 px opaque): taking the
/// symmetric fit always read −15.41 % / −11.77 % / −7.23 %; discounting its reflected half's
/// parameters (an MDL code that charges only the half it has to state) −15.72 % / −11.84 % /
/// −7.23 %; this rule −16.11 % / −11.85 % / −7.24 %, with the parameter ratio −3.18 % /
/// −3.85 % / −1.66 % against v0.2.5. On `lucide/beaker` (`real_symmetry`) the symmetric
/// fit is the cheaper one, and the render's mirror residual is 3.3e-5.
///
/// Method from: J. Rissanen (1978), *Modeling by shortest data description*, Automatica
/// 14(5), <https://doi.org/10.1016/0005-1098(78)90005-5>: of two descriptions of the same
/// data, keep the shorter; a tie goes to the symmetric one, which the artist drew.
pub(crate) fn choose(
    poly: &Polyline,
    mirrors: &[Mirror],
    cfg: &FitConfig,
    fit_plain: &dyn Fn(&Polyline) -> FittedPath,
) -> FittedPath {
    let plain = fit_plain(poly);
    if mirrors.is_empty() || mirrors.iter().all(|&m| anchors_mirror(&plain, m)) {
        return plain;
    }
    match fit(poly, mirrors, cfg, fit_plain) {
        Some(sym)
            if multimodel::path_cost(poly, &sym, cfg)
                <= multimodel::path_cost(poly, &plain, cfg) =>
        {
            sym
        }
        _ => plain,
    }
}

/// Whether every anchor of `path` (its start and each segment's end) has its reflection
/// under `m` among the anchors, to `SAME`.
fn anchors_mirror(path: &FittedPath, m: Mirror) -> bool {
    let a: Vec<Point> = std::iter::once(path.start)
        .chain(path.segments.iter().map(Segment::end))
        .collect();
    a.iter()
        .all(|&q| a.iter().any(|&r| r.dist(m.point(q)) <= SAME))
}

/// The coordinate of the mirror's axis: `x = k/2` for `V(k)`, `y = k/2` for `H(k)`. Exact,
/// since `k` is an integer.
fn axis(m: Mirror) -> f64 {
    match m {
        Mirror::V(k) | Mirror::H(k) => k as f64 * 0.5,
    }
}

/// `p` moved onto the mirror's axis along the axis normal.
fn onto_axis(p: Point, m: Mirror) -> Point {
    match m {
        Mirror::V(_) => Point::new(axis(m), p.y),
        Mirror::H(_) => Point::new(p.x, axis(m)),
    }
}

/// Whether the open polyline `p` reflects onto itself reversed under `m`:
/// `M(p_i) = p_{n−1−i}` to within `SAME` for every `i`.
fn open_self_mirror(p: &Polyline, m: Mirror) -> bool {
    let n = p.points.len();
    n >= 2 && (0..n).all(|i| m.point(p.points[i]).dist(p.points[n - 1 - i]) <= SAME)
}

/// The shift `s` with `M(p_i) = p_{(s − i) mod n}` for every `i` of a closed polyline, the
/// pairing of a closed boundary with itself (a reflection reverses its direction of
/// travel). Found by the index of `M(p_0)` and checked at every point; `None` when no shift
/// works.
fn closed_shift(p: &Polyline, m: Mirror) -> Option<usize> {
    let n = p.points.len();
    let target = m.point(p.points[0]);
    (0..n)
        .filter(|&j| p.points[j].dist(target) <= SAME)
        .find(|&s| {
            (0..n).all(|i| {
                // (s − i) mod n without `%` (the wazero arm64 miscompile, see `band.rs`).
                let j = if s >= i { s - i } else { s + n - i };
                m.point(p.points[i]).dist(p.points[j]) <= SAME
            })
        })
}

/// One axis crossing of a self-symmetric polyline: a point of it (`Vertex(i)`), or the
/// middle of the piece from point `i` to point `i + 1` (`Piece(i)`).
#[derive(Clone, Copy, Debug, PartialEq)]
enum Crossing {
    Vertex(usize),
    Piece(usize),
}

/// The axis crossings of `poly` under `m`, in order along it: two for a closed boundary,
/// one for an open one. `None` when `poly` is not its own reflection under `m`.
fn crossings(poly: &Polyline, m: Mirror) -> Option<Vec<Crossing>> {
    let n = poly.points.len();
    Some(if poly.closed {
        let s = closed_shift(poly, m)?;
        // π(i) = (s − i) mod n is i at a vertex on the axis (2i ≡ s mod n) and i + 1 at a
        // piece the axis cuts in half (2i + 1 ≡ s mod n). With 0 ≤ i, s < n the left side
        // minus s lies in (−n, 2n), so "≡ 0 mod n" is "equals s or s + n", tested without
        // `%`.
        let mut out = Vec::new();
        for i in 0..n {
            if 2 * i == s || 2 * i == s + n {
                out.push(Crossing::Vertex(i));
            } else if 2 * i + 1 == s || 2 * i + 1 == s + n {
                out.push(Crossing::Piece(i));
            }
        }
        out
    } else {
        if !open_self_mirror(poly, m) {
            return None;
        }
        // π(i) = n − 1 − i: the middle point when n is odd, else the middle piece.
        vec![if n & 1 == 1 {
            Crossing::Vertex(n / 2)
        } else {
            Crossing::Piece(n / 2 - 1)
        }]
    })
}

/// The fundamental domain of `poly` under `m`: the points from one axis crossing to the
/// next for a closed boundary, from the start to the crossing for an open one, with the
/// crossings placed exactly on the axis and their sigmas carried along. `None` when `poly`
/// is not its own reflection, or a closed one does not cross the axis exactly twice (which
/// the index involution guarantees when it holds).
fn half(poly: &Polyline, m: Mirror) -> Option<Polyline> {
    let n = poly.points.len();
    let (pts, sg) = (&poly.points, &poly.sigma);
    let crossings = crossings(poly, m)?;
    let at = |c: Crossing| -> (Point, f64) {
        match c {
            Crossing::Vertex(i) => (onto_axis(pts[i], m), sg[i]),
            Crossing::Piece(i) => {
                let j = if i + 1 == n { 0 } else { i + 1 };
                let mid = Point::new(0.5 * (pts[i].x + pts[j].x), 0.5 * (pts[i].y + pts[j].y));
                (onto_axis(mid, m), 0.5 * (sg[i] + sg[j]))
            }
        }
    };
    let (mut p, mut s) = (Vec::new(), Vec::new());
    if poly.closed {
        let [a, b] = crossings[..] else {
            return None;
        };
        let (pa, sa) = at(a);
        p.push(pa);
        s.push(sa);
        // The points strictly after crossing `a` and up to (not including) crossing `b`'s
        // own point, walking forward round the ring.
        let first = match a {
            Crossing::Vertex(i) | Crossing::Piece(i) => i + 1,
        };
        let stop = match b {
            Crossing::Vertex(i) => i,
            Crossing::Piece(i) => i + 1,
        };
        let stop = if stop == n { 0 } else { stop };
        let mut i = if first == n { 0 } else { first };
        while i != stop {
            p.push(pts[i]);
            s.push(sg[i]);
            i = if i + 1 == n { 0 } else { i + 1 };
        }
        let (pb, sb) = at(b);
        p.push(pb);
        s.push(sb);
    } else {
        let c = crossings[0];
        let upto = match c {
            Crossing::Vertex(i) => i,
            Crossing::Piece(i) => i + 1,
        };
        p.extend_from_slice(&pts[..upto]);
        s.extend_from_slice(&sg[..upto]);
        let (pc, sc) = at(c);
        p.push(pc);
        s.push(sc);
    }
    (p.len() >= 2).then(|| Polyline::new(p, s, false))
}

/// Where a path ends.
fn end_of(path: &FittedPath) -> Point {
    path.segments.last().map_or(path.start, Segment::end)
}

/// The boundary from the fit `part` of its fundamental domain: `part`, then its reflection
/// run backwards. Closed when `poly` is; an open boundary ends exactly on its own last
/// point (its junction), which the reflection of the first reaches to rounding.
fn assemble(part: &FittedPath, m: Mirror, poly: &Polyline) -> FittedPath {
    let back = reflect_path(part, m).reversed();
    let mut segments = part.segments.clone();
    segments.extend(back.segments);
    if !poly.closed {
        if let (Some(last), Some(&end)) = (segments.last_mut(), poly.points.last()) {
            set_end(last, end);
        }
    }
    FittedPath {
        start: part.start,
        segments,
        closed: poly.closed,
    }
}

/// Move a segment's end point, keeping everything else.
fn set_end(s: &mut Segment, p: Point) {
    match s {
        Segment::Line(e) | Segment::Cubic(_, _, e) | Segment::Arc { end: e, .. } => *e = p,
    }
}

/// The two mirrored segments meeting at an axis crossing as one segment: two lines become
/// the line from `from` to its reflection (square to the axis), two circular arcs one arc
/// of the same radius and twice the angle, whose centre is then on the axis. `from` is
/// where `a` starts; `b` is the reflection of `a` run backwards, so it ends at `M(from)`.
/// The merged segment no longer passes exactly through the crossing unless the two were
/// already one curve; [`polish_joins`] keeps it only when the description length does not
/// rise.
fn merged(from: Point, a: &Segment, b: &Segment) -> Option<Segment> {
    let end = b.end();
    match (a, b) {
        // The line from `from` to its own reflection is square to the axis by construction;
        // it passes the crossing at `from`'s height instead of the measured one, which the
        // caller prices.
        (Segment::Line(_), Segment::Line(_)) => Some(Segment::Line(end)),
        (
            Segment::Arc {
                rx,
                large_arc,
                sweep,
                end: j,
                ..
            },
            Segment::Arc { .. },
        ) if a.is_circular() && b.is_circular() => {
            let (rx, large_arc, sweep) = (*rx, *large_arc, *sweep);
            // The angle the first arc turns through, from its chord and radius.
            let chord = from.dist(*j);
            if rx.is_nan() || rx <= 0.0 || chord > 2.0 * rx {
                return None;
            }
            let small = 2.0 * (chord / (2.0 * rx)).min(1.0).asin();
            let theta = if large_arc {
                2.0 * std::f64::consts::PI - small
            } else {
                small
            };
            let total = 2.0 * theta;
            // One arc can turn through less than a full circle, and its chord, the distance
            // from `from` to its own reflection, must fit inside the circle.
            if total >= 2.0 * std::f64::consts::PI - 1e-9 || from.dist(end) > 2.0 * rx {
                return None;
            }
            Some(Segment::circular_arc(
                rx,
                total > std::f64::consts::PI,
                sweep,
                end,
            ))
        }
        _ => None,
    }
}

/// [`assemble`]`(part)` with the joins at the axis crossings improved where that does not
/// cost description length (see the module docs, step 4).
///
/// The crossings are where `part` ends (always) and where it starts (for a closed
/// boundary). At each, in turn: a cubic arriving within `SNAP_DEGREES` of the axis normal
/// has its last control point moved onto the normal through the crossing (kept if
/// `path_cost` rises by at most λ), and two mirrored lines or circular arcs are merged into
/// one ([`merged`], kept if `path_cost` does not rise). The cost is the fitter's own,
/// `½χ² + λ·k`, on the whole boundary `poly`.
fn polish_joins(
    base: FittedPath,
    part: FittedPath,
    m: Mirror,
    poly: &Polyline,
    cfg: &FitConfig,
) -> FittedPath {
    let cost = |p: &FittedPath| multimodel::path_cost(poly, p, cfg);
    let mut best = base;
    let mut best_cost = cost(&best);
    let mut part = part;
    // 1. Smooth cubic joins: snap the cubic's control point next to each crossing onto the
    //    axis normal, which makes the reflected control point lie on the same line.
    for at_end in [true, false] {
        if !at_end && !poly.closed {
            continue;
        }
        let Some(snapped) = snap_cubic(&part, m, at_end) else {
            continue;
        };
        let path = assemble(&snapped, m, poly);
        let c = cost(&path);
        if c <= best_cost + cfg.lambda {
            part = snapped;
            best = path;
            best_cost = c;
        }
    }
    // 2. Merges, at the far crossing (an inner join) and then, for a closed boundary, at the
    //    start (the join that closes the ring).
    // A closed boundary needs two segments in its half: merging a single one with its
    // reflection would join the start to itself.
    let k = part.segments.len();
    if k >= 2 || (k == 1 && !poly.closed) {
        // Where `part`'s last segment starts.
        let from = if k == 1 {
            part.start
        } else {
            part.segments[k - 2].end()
        };
        if let Some(seg) = merged(from, &best.segments[k - 1], &best.segments[k]) {
            let mut path = best.clone();
            path.segments.splice(k - 1..k + 1, [seg]);
            let c = cost(&path);
            if c <= best_cost {
                best = path;
                best_cost = c;
            }
        }
    }
    if poly.closed && k >= 2 {
        let n = best.segments.len();
        // The ring closes where the last segment (the reflection of `part`'s first, run
        // backwards) meets the first. Merged, the ring starts where that last segment
        // started instead.
        let last_from = best.segments[n - 2].end();
        // `merged` takes the segment that arrives at the crossing first; here that is the
        // last one, arriving at the start.
        if let Some(seg) = merged(last_from, &best.segments[n - 1], &best.segments[0]) {
            let mut segments = vec![seg];
            segments.extend_from_slice(&best.segments[1..n - 1]);
            let path = FittedPath {
                start: last_from,
                segments,
                closed: true,
            };
            let c = cost(&path);
            if c <= best_cost {
                best = path;
            }
        }
    }
    best
}

/// `part` with the cubic next to an axis crossing (its last segment when `at_end`, else its
/// first) given an end tangent along the axis normal: the control point next to the
/// crossing moved onto the normal through it, keeping its distance along the normal.
/// `None` when that segment is not a cubic or its tangent is more than `SNAP_DEGREES` off
/// the normal (a corner the artist drew).
fn snap_cubic(part: &FittedPath, m: Mirror, at_end: bool) -> Option<FittedPath> {
    let idx = if at_end {
        part.segments.len().checked_sub(1)?
    } else {
        0
    };
    let Segment::Cubic(c1, c2, e) = part.segments[idx] else {
        return None;
    };
    let (cross, ctrl) = if at_end { (e, c2) } else { (part.start, c1) };
    let (dn, da) = match m {
        // The normal to a vertical axis is horizontal: the offset across is in x, along in y.
        Mirror::V(_) => (ctrl.x - cross.x, ctrl.y - cross.y),
        Mirror::H(_) => (ctrl.y - cross.y, ctrl.x - cross.x),
    };
    if dn.abs() <= 1e-12 || da.atan2(dn.abs()).abs().to_degrees() > SNAP_DEGREES {
        return None;
    }
    let on = match m {
        Mirror::V(_) => Point::new(ctrl.x, cross.y),
        Mirror::H(_) => Point::new(cross.x, ctrl.y),
    };
    let mut out = part.clone();
    out.segments[idx] = if at_end {
        Segment::Cubic(c1, on, e)
    } else {
        Segment::Cubic(on, c2, e)
    };
    Some(out)
}

#[cfg(test)]
#[path = "mirror_fit_tests.rs"]
mod tests;
