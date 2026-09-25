//! Underlap: where two faces sit side by side, the one painted first reaches a little way
//! under the one painted over it.
//!
//! A renderer anti-aliases every shape on its own. Where two fills share an edge and
//! neither is painted beneath the other -- two slices of a pie, two patches of shading on
//! an emoji's cheek -- the pixel the edge crosses is covered half by each, and half-covered
//! twice is not covered: `0.5*B + 0.5*(0.5*A + 0.5*G)` leaves a quarter of the ground `G`
//! showing, a hairline of background along every such edge. Sharing the curve exactly
//! does not help, because the fault is in the compositing and not in the geometry. The
//! stacked emitter already avoids it wherever one face contains the other (the outer one
//! is painted whole underneath); this handles the rest.
//!
//! The lower face's outline is moved off each such edge by a fraction of a pixel into the
//! upper face, which is painted over it later and hides the extra exactly. The junctions at
//! the ends of the run stay where they are -- a third face meets the two there -- and the
//! reach tapers in from them. Measured with `bench/seam_eval.py` against the same document
//! rendered 8x supersampled.
//!
//! What it may not do is show. So it reaches only under a face that is opaque and painted
//! later, only as far as that face is thick enough to hide it with room to spare for its
//! own anti-aliasing, and never into the lower face's own side of the edge.
//!
//! Nor may it cost much. The CI gate prices every coordinate and every turn of the
//! outline, so no span is split: a line or cubic moves its own points, a span pinned at
//! both ends bows out instead (see [`bulge`]), and a straight span only gains a vertex
//! where it is long enough to taper gently.

use std::collections::HashMap;

use inkvec_core::Point;
use inkvec_fit::curves::{arc_ellipse_center, Segment};
use inkvec_fit::FittedPath;

use crate::rings::point_in_ring;
use crate::FaceRings;

/// How far a lower face reaches under an upper one, in pixels of the traced image.
///
/// The seam weight left at an edge pixel is at most `(1 - r)^2 / 4` for a reach of `r`
/// output pixels, so half a pixel takes the worst seam at the traced size from 25% of the
/// ground to 6%, and anything drawn at 2x or more to nothing.
pub(crate) const UNDERLAP: f64 = 0.5;

/// Room left between the reach and the upper face's far boundary, whose own anti-aliased
/// pixels would otherwise show the lower colour through them.
const CLEARANCE: f64 = 0.5;

/// Length of a tapered end, in reaches: the lower face comes off the junction point at
/// under 5 degrees. Shorter tapers mend more of the seam next to a junction but add a
/// vertex to more lines: at 4 reaches the parameter count on the gate's screen set rose
/// 3.3%, at 12 it rises 1.8% for 0.9x the seam pixels removed.
const TAPER: f64 = 12.0;

/// RGB distance (0..1 per channel) between the ground under two side-by-side faces and
/// their mean colour below which the seam between them is not worth mending: a quarter of
/// it, the most a seam can show, is then under 3% of full scale.
const FAINT_SEAM: f32 = 0.12;

/// The reach, overridable (`INKVEC_UNDERLAP`, 0 to switch it off) for A/B measurement.
pub(crate) fn underlap_width() -> f64 {
    std::env::var("INKVEC_UNDERLAP")
        .ok()
        .and_then(|v| v.parse::<f64>().ok())
        .filter(|v| v.is_finite() && *v >= 0.0)
        .unwrap_or(UNDERLAP)
}

/// Replacement geometry for `(face, edge)`: the edge as that face traverses it, moved.
pub(crate) type Overrides = HashMap<(usize, usize), FittedPath>;

/// What the underlap needs to know about the document.
pub(crate) struct Faces<'a> {
    pub order: &'a [FaceRings],
    pub fitted: &'a [FittedPath],
    /// Each ring flattened, indexed like `order`.
    pub pts: &'a [Vec<Vec<Point>>],
    /// The rings each face paints.
    pub drawn: &'a [Vec<usize>],
    /// The rings that bound each face's painted area.
    pub outer: &'a [Vec<usize>],
    /// Paint order of each face; `None` for a face that is not painted.
    pub z: &'a [Option<usize>],
    /// The face is written from its rings, so its outline can move.
    pub lower_ok: &'a [bool],
    /// The face paints opaque everywhere, so it hides what is under it.
    pub upper_ok: &'a [bool],
    /// How far the ground under these two faces is from their mean colour (RGB, 0..1 per
    /// channel): four times the most a seam between them can show.
    pub contrast: &'a dyn Fn(usize, usize) -> f32,
}

fn add(a: Point, b: Point) -> Point {
    Point::new(a.x + b.x, a.y + b.y)
}
fn sub(a: Point, b: Point) -> Point {
    Point::new(a.x - b.x, a.y - b.y)
}
fn scale(a: Point, s: f64) -> Point {
    Point::new(a.x * s, a.y * s)
}
fn dot(a: Point, b: Point) -> f64 {
    a.x * b.x + a.y * b.y
}
fn unit(a: Point) -> Option<Point> {
    let l = a.x.hypot(a.y);
    (l > 1e-9).then(|| scale(a, 1.0 / l))
}

fn oriented(fitted: &[FittedPath], (k, rev): (usize, bool)) -> FittedPath {
    if rev {
        fitted[k].reversed()
    } else {
        fitted[k].clone()
    }
}

fn signed_area(r: &[Point]) -> f64 {
    let n = r.len();
    (0..n)
        .map(|k| {
            let q = r[(k + 1) % n];
            r[k].x * q.y - q.x * r[k].y
        })
        .sum::<f64>()
        * 0.5
}

/// Derivative of an arc's ellipse with respect to its angle parameter.
fn arc_deriv(f: &inkvec_fit::curves::ArcFrame, t: f64) -> Point {
    let (sp, cp) = f.phi.sin_cos();
    let (x, y) = (-f.rx * t.sin(), f.ry * t.cos());
    Point::new(cp * x - sp * y, sp * x + cp * y)
}

/// Unit tangents at the start and the end of a segment that starts at `a`.
fn tangents(a: Point, s: &Segment) -> Option<(Point, Point)> {
    match *s {
        Segment::Line(p) => {
            let t = unit(sub(p, a))?;
            Some((t, t))
        }
        Segment::Cubic(c1, c2, p) => {
            let t0 = unit(sub(c1, a))
                .or_else(|| unit(sub(c2, a)))
                .or_else(|| unit(sub(p, a)))?;
            let t1 = unit(sub(p, c2))
                .or_else(|| unit(sub(p, c1)))
                .or_else(|| unit(sub(p, a)))?;
            Some((t0, t1))
        }
        Segment::Arc {
            rx,
            ry,
            phi,
            large_arc,
            sweep,
            end,
        } => {
            let f = arc_ellipse_center(a, rx, ry, phi, large_arc, sweep, end);
            if f.delta == 0.0 {
                let t = unit(sub(end, a))?;
                return Some((t, t));
            }
            let s = f.delta.signum();
            Some((
                unit(scale(arc_deriv(&f, f.theta1), s))?,
                unit(scale(arc_deriv(&f, f.theta1 + f.delta), s))?,
            ))
        }
    }
}

/// The point halfway along a segment that starts at `a`, and the unit tangent there.
fn midpoint(a: Point, s: &Segment) -> Option<(Point, Point)> {
    match *s {
        Segment::Line(p) => Some((scale(add(a, p), 0.5), unit(sub(p, a))?)),
        Segment::Cubic(c1, c2, p) => {
            let m = Point::new(
                (a.x + 3.0 * c1.x + 3.0 * c2.x + p.x) / 8.0,
                (a.y + 3.0 * c1.y + 3.0 * c2.y + p.y) / 8.0,
            );
            // B'(1/2) = 3/4 (c2 + p - a - c1)
            let t = unit(sub(add(c2, p), add(a, c1))).or_else(|| unit(sub(p, a)))?;
            Some((m, t))
        }
        Segment::Arc {
            rx,
            ry,
            phi,
            large_arc,
            sweep,
            end,
        } => {
            let f = arc_ellipse_center(a, rx, ry, phi, large_arc, sweep, end);
            if f.delta == 0.0 {
                return Some((scale(add(a, end), 0.5), unit(sub(end, a))?));
            }
            let th = f.theta1 + 0.5 * f.delta;
            Some((f.at(th), unit(scale(arc_deriv(&f, th), f.delta.signum()))?))
        }
    }
}

/// A point and the unit tangent of a cubic at `t`.
fn cubic_at(q: [Point; 4], t: f64) -> (Point, Option<Point>) {
    let u = 1.0 - t;
    let b = [u * u * u, 3.0 * u * u * t, 3.0 * u * t * t, t * t * t];
    let pt = Point::new(
        b[0] * q[0].x + b[1] * q[1].x + b[2] * q[2].x + b[3] * q[3].x,
        b[0] * q[0].y + b[1] * q[1].y + b[2] * q[2].y + b[3] * q[3].y,
    );
    let d = [u * u, 2.0 * u * t, t * t];
    let dv = Point::new(
        d[0] * (q[1].x - q[0].x) + d[1] * (q[2].x - q[1].x) + d[2] * (q[3].x - q[2].x),
        d[0] * (q[1].y - q[0].y) + d[1] * (q[2].y - q[1].y) + d[2] * (q[3].y - q[2].y),
    );
    (pt, unit(dv))
}

/// Parameters at which a moved cubic is checked to have moved outward.
const PROBES: [f64; 7] = [0.125, 0.25, 0.375, 0.5, 0.625, 0.75, 0.875];

/// A cubic moved by translating each end's leg with that end. Exact for a straight
/// cubic and close for a gently turning one; a turning one offsets its control polygon
/// instead (Tiller and Hanson), each leg moved out along its own normal and the inner
/// control points to where neighbouring legs meet. `None` when neither moves it outward
/// everywhere: two opposite end displacements can cancel in the middle of a cubic that
/// turns far and pull it *into* the lower face -- the brim of `twemoji/1faa3`, one cubic
/// turning 150 degrees, came out with a hairline of ground along it.
fn move_cubic(
    q: [Point; 4],
    ds: Point,
    de: Point,
    delta: f64,
    normal: &dyn Fn(Point) -> Point,
) -> Option<Segment> {
    // The move at `t`, given the moves of the four control points, points outward.
    let outward = |m0: Point, m1: Point, m2: Point, m3: Point, t: f64| -> bool {
        let u = 1.0 - t;
        let b = [u * u * u, 3.0 * u * u * t, 3.0 * u * t * t, t * t * t];
        let m = add(
            add(scale(m0, b[0]), scale(m1, b[1])),
            add(scale(m2, b[2]), scale(m3, b[3])),
        );
        let (_, tan) = cubic_at(q, t);
        tan.is_none_or(|tn| dot(m, normal(tn)) >= 0.0)
    };
    let (t0, t1) = (cubic_at(q, 0.0).1, cubic_at(q, 1.0).1);
    let gentle = matches!((t0, t1), (Some(a), Some(b)) if dot(a, b) >= 0.5);
    if gentle && PROBES.iter().all(|&t| outward(ds, ds, de, de, t)) {
        return Some(Segment::Cubic(add(q[1], ds), add(q[2], de), add(q[3], de)));
    }
    let legs = [
        unit(sub(q[1], q[0])),
        unit(sub(q[2], q[1])),
        unit(sub(q[3], q[2])),
    ];
    let [Some(l1), Some(l2), Some(l3)] = legs else {
        return None;
    };
    let mitre = |a: Point, b: Point| {
        let (na, nb) = (normal(a), normal(b));
        scale(add(na, nb), delta / (1.0 + dot(na, nb)).max(0.5))
    };
    let (d1, d2) = (mitre(l1, l2), mitre(l2, l3));
    PROBES
        .iter()
        .all(|&t| outward(ds, d1, d2, de, t))
        .then(|| Segment::Cubic(add(q[1], d1), add(q[2], d2), add(q[3], de)))
}

/// A circular arc starting at `a` with its ends moved by `ds` and `de`: the concentric arc,
/// its radius grown or shrunk by the mean of the two moves, through the moved ends. Exact
/// when both ends move the full reach along the arc's own normal, which is what a smooth
/// join hands it; otherwise a close neighbour of that offset, so it is checked the way a
/// moved cubic is. `None` unless every probe along it moved outward, by no more than twice
/// the reach, and the arc still sweeps the same way -- an arc whose ends went off the
/// offset circle can swing its centre and cut into the lower face.
fn move_arc(
    a: Point,
    seg: &Segment,
    ds: Point,
    de: Point,
    delta: f64,
    normal: &dyn Fn(Point) -> Point,
) -> Option<Segment> {
    let Segment::Arc {
        rx,
        ry,
        phi,
        large_arc,
        sweep,
        end,
    } = *seg
    else {
        return None;
    };
    if !seg.is_circular() {
        return None;
    }
    let f = arc_ellipse_center(a, rx, ry, phi, large_arc, sweep, end);
    if f.delta == 0.0 {
        return None;
    }
    let (mid, t) = midpoint(a, seg)?;
    let away = dot(sub(mid, f.c), normal(t)) > 0.0;
    let grow = 0.5 * (ds.x.hypot(ds.y) + de.x.hypot(de.y));
    let rad = if away { f.rx + grow } else { f.rx - grow };
    if rad <= 2.0 * grow {
        return None;
    }
    let (a2, e2) = (add(a, ds), add(end, de));
    let moved = Segment::Arc {
        rx: rad,
        ry: rad,
        phi,
        large_arc,
        sweep,
        end: e2,
    };
    let g = arc_ellipse_center(a2, rad, rad, phi, large_arc, sweep, e2);
    if g.delta == 0.0 || g.delta.signum() != f.delta.signum() {
        return None;
    }
    let ok = PROBES.iter().all(|&s| {
        let th = f.theta1 + s * f.delta;
        let (p, q) = (f.at(th), g.at(g.theta1 + s * g.delta));
        let Some(tn) = unit(scale(arc_deriv(&f, th), f.delta.signum())) else {
            return false;
        };
        let m = sub(q, p);
        dot(m, normal(tn)) >= 0.0 && m.x.hypot(m.y) <= 2.0 * delta + 1e-9
    });
    ok.then_some(moved)
}

/// One segment starting at `a`, its start moved by `ds` and its end by `de`, each roughly
/// `delta` along `normal`. Lines and cubics move their points; an arc moves to the arc
/// concentric with it (see [`move_arc`]).
fn move_segment(
    a: Point,
    seg: &Segment,
    ds: Point,
    de: Point,
    delta: f64,
    normal: &dyn Fn(Point) -> Point,
) -> Option<Segment> {
    match *seg {
        Segment::Line(p) => Some(Segment::Line(add(p, de))),
        Segment::Cubic(c1, c2, p) => move_cubic([a, c1, c2, p], ds, de, delta, normal),
        Segment::Arc { .. } => move_arc(a, seg, ds, de, delta, normal),
    }
}

/// A segment whose two ends stay put, bowed out by `r` at its middle: a cubic's inner
/// control points moved together, a circular arc through the same ends with a sagitta `r`
/// larger or smaller. Neither adds a number to the path. `None` for a line, which cannot bow
/// without one, or where the bow would not point outward all along.
fn bulge(a: Point, seg: &Segment, r: f64, normal: &dyn Fn(Point) -> Point) -> Option<Segment> {
    match *seg {
        Segment::Line(_) => None,
        Segment::Cubic(c1, c2, p) => {
            let (_, t) = midpoint(a, seg)?;
            let n = normal(t);
            // The curve moves by 4t(1-t) of the shift along `n`: all of it at the middle.
            let k = scale(n, 4.0 / 3.0 * r);
            let q = [a, c1, c2, p];
            if PROBES
                .iter()
                .any(|&t| cubic_at(q, t).1.is_some_and(|tn| dot(n, normal(tn)) < 0.5))
            {
                return None;
            }
            Some(Segment::Cubic(add(c1, k), add(c2, k), p))
        }
        Segment::Arc {
            rx,
            ry,
            phi,
            large_arc,
            sweep,
            end,
        } => {
            if !seg.is_circular() || large_arc {
                return None;
            }
            let f = arc_ellipse_center(a, rx, ry, phi, large_arc, sweep, end);
            if f.delta == 0.0 {
                return None;
            }
            let (mid, t) = midpoint(a, seg)?;
            let chord = a.dist(end);
            let sag = mid.dist(scale(add(a, end), 0.5));
            // Outward away from the centre deepens the bow; towards it flattens it. The
            // short arc with the same sweep flag stays on the same side of the chord.
            let s2 = if dot(sub(mid, f.c), normal(t)) > 0.0 {
                sag + r
            } else {
                sag - r
            };
            if s2 < 0.05 || s2 >= 0.5 * chord {
                return None;
            }
            let rad = (0.25 * chord * chord + s2 * s2) / (2.0 * s2);
            Some(Segment::Arc {
                rx: rad,
                ry: rad,
                phi,
                large_arc: false,
                sweep,
                end,
            })
        }
    }
}

/// The run's spans in order, each with the index of the edge it belongs to. A long
/// straight span at either end of the run gains a vertex a few reaches in from the
/// junction, so it reaches full depth there rather than only at its far end: a spoke of a
/// pie tapers over six pixels, not fifty. Only where the upper face is thick enough there
/// to hide it.
fn spans(
    paths: &[FittedPath],
    delta: f64,
    normal: &dyn Fn(Point) -> Point,
    inside_u: &dyn Fn(Point) -> bool,
) -> Vec<(Segment, usize)> {
    let j0 = paths[0].start;
    let mut segs: Vec<(Segment, usize)> = Vec::new();
    for (i, p) in paths.iter().enumerate() {
        segs.extend(p.segments.iter().map(|s| (s.clone(), i)));
    }
    let taper = TAPER * delta;
    let hidden_at = |at: Point, along: Point| {
        unit(along).is_some_and(|t| inside_u(add(at, scale(normal(t), delta + CLEARANCE))))
    };
    if let Some(&(Segment::Line(p), e)) = segs.last() {
        let a = if segs.len() >= 2 {
            segs[segs.len() - 2].0.end()
        } else {
            j0
        };
        let l = a.dist(p);
        let at = add(p, scale(sub(a, p), taper / l.max(1e-9)));
        if l > 3.0 * taper && hidden_at(at, sub(p, a)) {
            let n = segs.len();
            segs.insert(n - 1, (Segment::Line(at), e));
        }
    }
    if let Some(&(Segment::Line(p), e)) = segs.first() {
        let l = j0.dist(p);
        let at = add(j0, scale(sub(p, j0), taper / l.max(1e-9)));
        if l > 3.0 * taper && hidden_at(at, sub(p, j0)) {
            segs.insert(0, (Segment::Line(at), e));
        }
    }
    segs
}

/// Move one run -- consecutive edges of the lower face with one upper neighbour, oriented
/// as the lower face walks them -- `delta` into the upper face, its two ends pinned. One
/// path per edge, or `None` when nothing could be moved safely.
fn displace_run(
    paths: &[FittedPath],
    sgn: f64,
    delta: f64,
    inside_u: &dyn Fn(Point) -> bool,
    inside_v: &dyn Fn(Point) -> bool,
    arcs_move: bool,
) -> Option<Vec<FittedPath>> {
    // Outward normal: the lower face lies to the left of travel when its ring's signed
    // area is positive.
    let normal = |t: Point| scale(Point::new(t.y, -t.x), sgn);
    let j0 = paths[0].start;

    let segs = spans(paths, delta, &normal, inside_u);

    let m = segs.len();
    let mut verts = Vec::with_capacity(m + 1);
    verts.push(j0);
    for (s, _) in &segs {
        verts.push(s.end());
    }
    let tans: Vec<(Point, Point)> = (0..m)
        .map(|i| tangents(verts[i], &segs[i].0))
        .collect::<Option<_>>()?;

    // It has to be hidden, so each span reaches only as far as the upper face beside it is
    // thick enough to cover with room to spare: the whole reach, half, or not at all. A
    // span that cannot move pins its ends and its neighbours taper to it, rather than one
    // thin stretch -- the tail of an outline where it meets another -- costing the run.
    let reach: Vec<f64> = (0..m)
        .map(|i| {
            let Some((mid, t)) = midpoint(verts[i], &segs[i].0) else {
                return 0.0;
            };
            [delta, 0.5 * delta]
                .into_iter()
                .find(|&a| inside_u(add(mid, scale(normal(t), a + CLEARANCE))))
                .unwrap_or(0.0)
        })
        .collect();
    let zero = Point::new(0.0, 0.0);
    let is_arc = |i: usize| matches!(segs[i].0, Segment::Arc { .. });
    // The run's ends are junctions and stay. An arc's ends move with their neighbours to
    // the concentric arc when `arcs_move`; otherwise they stay and the arc bows instead.
    let mut disp = vec![zero; m + 1];
    for i in 1..m {
        if !arcs_move && (is_arc(i - 1) || is_arc(i)) {
            continue;
        }
        // A mitred offset at a corner between two spans, never more than twice the reach.
        let (n1, n2) = (normal(tans[i - 1].1), normal(tans[i].0));
        let s = 1.0 + dot(n1, n2);
        let mitre = scale(add(n1, n2), 1.0 / s.max(0.5));
        let a = reach[i - 1].min(reach[i]);
        disp[i] = [a, 0.5 * a]
            .into_iter()
            .filter(|&a| a > 0.0)
            .map(|a| scale(mitre, a))
            .find(|d| {
                let l = d.x.hypot(d.y);
                unit(*d).is_some_and(|dir| inside_u(add(verts[i], scale(dir, l + CLEARANCE))))
            })
            .unwrap_or(zero);
    }

    let mut out: Vec<FittedPath> = paths
        .iter()
        .map(|p| FittedPath {
            start: p.start,
            segments: Vec::new(),
            closed: p.closed,
        })
        .collect();
    let still = |d: Point| d.x == 0.0 && d.y == 0.0;
    let mut changed = false;
    for i in 0..m {
        let (ds, de) = (disp[i], disp[i + 1]);
        let (seg, e) = &segs[i];
        // Either way it must not cut into the lower face's own side of the edge. A bow that
        // would is left out; a moved span that would sinks the run, its ends having moved.
        let cuts = |s: &Segment| midpoint(add(verts[i], ds), s).is_none_or(|(m, _)| inside_v(m));
        let moved = if still(ds) && still(de) {
            (reach[i] > 0.0)
                .then(|| bulge(verts[i], seg, reach[i], &normal))
                .flatten()
                .filter(|b| !cuts(b))
        } else {
            let moved = move_segment(verts[i], seg, ds, de, reach[i], &normal)?;
            if cuts(&moved) {
                return None;
            }
            Some(moved)
        };
        let Some(moved) = moved else {
            out[*e].segments.push(seg.clone());
            continue;
        };
        changed = true;
        out[*e].segments.push(moved);
    }
    if !changed {
        return None;
    }
    // Every edge after the first starts where the previous one now ends.
    for i in 1..out.len() {
        let prev_end = out[i - 1].end();
        out[i].start = prev_end;
    }
    Some(out)
}

/// The edges each lower face has to move, and where to.
///
/// `under(v, u)` says `v` is already painted whole beneath `u` (an ancestor in the stack),
/// where no seam can form and nothing moves.
pub(crate) fn underlap(f: &Faces, under: &dyn Fn(usize, usize) -> bool, delta: f64) -> Overrides {
    let mut out = Overrides::new();
    if delta <= 0.0 {
        return out;
    }
    let mut owners: HashMap<usize, Vec<usize>> = HashMap::new();
    for (i, face) in f.order.iter().enumerate() {
        for ring in face {
            for &(e, _) in ring {
                owners.entry(e).or_default().push(i);
            }
        }
    }
    for v in 0..f.order.len() {
        let Some(zv) = f.z.get(v).copied().flatten() else {
            continue;
        };
        if !f.lower_ok.get(v).copied().unwrap_or(false) {
            continue;
        }
        for &k in &f.drawn[v] {
            let ring = &f.order[v][k];
            let n = ring.len();
            if n < 2 {
                continue;
            }
            let area = signed_area(&f.pts[v][k]);
            if area.abs() < 1.0 {
                continue;
            }
            // The upper face across each edge, where that edge needs mending.
            let need: Vec<Option<usize>> = ring
                .iter()
                .map(|&(e, _)| {
                    let own = owners.get(&e)?;
                    if own.len() != 2 {
                        return None;
                    }
                    let u = *own.iter().find(|&&o| o != v)?;
                    let zu = f.z.get(u).copied().flatten()?;
                    (f.upper_ok.get(u).copied().unwrap_or(false)
                        && zu > zv
                        && !under(v, u)
                        && (f.contrast)(v, u) >= FAINT_SEAM)
                        .then_some(u)
                })
                .collect();
            let starts: Vec<usize> = (0..n)
                .filter(|&p| need[p].is_some() && need[(p + n - 1) % n] != need[p])
                .collect();
            for &s in &starts {
                let Some(u) = need[s] else { continue };
                let mut len = 1;
                while len < n && need[(s + len) % n] == need[s] {
                    len += 1;
                }
                let positions: Vec<usize> = (0..len).map(|i| (s + i) % n).collect();
                let paths: Vec<FittedPath> = positions
                    .iter()
                    .map(|&p| oriented(f.fitted, ring[p]))
                    .collect();
                if paths.iter().any(|p| p.segments.is_empty()) {
                    continue;
                }
                let inside_u =
                    |q: Point| f.outer[u].iter().any(|&ko| point_in_ring(q, &f.pts[u][ko]));
                let inside_v = |q: Point| point_in_ring(q, &f.pts[v][k]);
                // An arc's ends first move with the run; where that cannot be done
                // safely anywhere in it, the run is tried again with them pinned.
                for (d, arcs_move) in [
                    (delta, true),
                    (delta, false),
                    (0.5 * delta, true),
                    (0.5 * delta, false),
                ] {
                    let r = displace_run(&paths, area.signum(), d, &inside_u, &inside_v, arcs_move);
                    if let Some(moved) = r {
                        for (i, &p) in positions.iter().enumerate() {
                            out.insert((v, ring[p].0), moved[i].clone());
                        }
                        break;
                    }
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line_path(a: Point, b: Point) -> FittedPath {
        FittedPath {
            start: a,
            segments: vec![Segment::Line(b)],
            closed: false,
        }
    }

    /// Two 40-pixel-tall rectangles side by side, the lower one on the left: its shared
    /// edge moves right, into the upper one, tapering in from the two junctions.
    #[test]
    fn a_long_shared_edge_moves_into_the_upper_face_and_tapers_to_its_ends() {
        // The left face walks its ring counter-clockwise in y-up terms, so the shared edge
        // x = 4 runs upward from (4,0) to (4,40) with the face on its left.
        let shared = line_path(Point::new(4.0, 0.0), Point::new(4.0, 40.0));
        let inside_u = |q: Point| q.x > 4.0 && q.x < 8.0 && q.y > 0.0 && q.y < 40.0;
        let inside_v = |q: Point| q.x > 0.0 && q.x < 4.0 && q.y > 0.0 && q.y < 40.0;
        let moved = displace_run(&[shared], 1.0, 0.5, &inside_u, &inside_v, true).unwrap();
        let ends: Vec<Point> = moved[0].segments.iter().map(|s| s.end()).collect();
        assert_eq!(moved[0].start, Point::new(4.0, 0.0));
        assert_eq!(ends.len(), 3, "{ends:?}");
        assert!(ends[0].dist(Point::new(4.5, 6.0)) < 1e-9, "{ends:?}");
        assert!(ends[1].dist(Point::new(4.5, 34.0)) < 1e-9, "{ends:?}");
        assert!(ends[2].dist(Point::new(4.0, 40.0)) < 1e-9, "{ends:?}");
    }

    /// A face too thin to hide the reach is left alone.
    #[test]
    fn nothing_moves_under_a_face_too_thin_to_hide_it() {
        let shared = line_path(Point::new(4.0, 0.0), Point::new(4.0, 40.0));
        let inside_u = |q: Point| q.x > 4.0 && q.x < 4.6 && q.y > 0.0 && q.y < 40.0;
        let inside_v = |q: Point| q.x > 0.0 && q.x < 4.0 && q.y > 0.0 && q.y < 40.0;
        assert!(displace_run(&[shared], 1.0, 0.5, &inside_u, &inside_v, true).is_none());
    }

    /// An arc keeps its ends and bows out by the reach at its middle: still one arc, with
    /// the same ends and a larger sagitta.
    #[test]
    fn an_arc_between_two_junctions_bows_into_the_upper_face() {
        // Quarter circle of radius 10 about the origin from (10,0) to (0,10), sweep=true
        // (increasing angle). The lower face is the disc, whose ring runs this way with
        // positive signed area, so the upper face lies outside the circle.
        let arc = FittedPath {
            start: Point::new(10.0, 0.0),
            segments: vec![Segment::Arc {
                rx: 10.0,
                ry: 10.0,
                phi: 0.0,
                large_arc: false,
                sweep: true,
                end: Point::new(0.0, 10.0),
            }],
            closed: false,
        };
        let inside_u = |q: Point| {
            let r = q.x.hypot(q.y);
            r > 10.0 && r < 20.0
        };
        let inside_v = |q: Point| q.x.hypot(q.y) < 10.0;
        let moved = displace_run(&[arc], 1.0, 0.5, &inside_u, &inside_v, true).unwrap();
        let segs = &moved[0].segments;
        assert_eq!(segs.len(), 1, "{segs:?}");
        assert!(matches!(segs[0], Segment::Arc { end, .. } if end == Point::new(0.0, 10.0)));
        let (mid, _) = midpoint(Point::new(10.0, 0.0), &segs[0]).unwrap();
        assert!((mid.x.hypot(mid.y) - 10.5).abs() < 1e-9, "{mid:?}");
    }

    /// Where an arc meets a line smoothly, their shared point moves with the run and the
    /// arc becomes the concentric one through it. Pinning it -- which is what arcs used to
    /// do -- left the reach tapering to nothing there, and on the Noto princess's hair that
    /// stub of unmended edge was the one hairline the gradient faces still showed.
    #[test]
    fn a_smooth_join_with_an_arc_moves_with_the_run() {
        // Quarter circle of radius 10 from (10,0) to (0,10), then the line tangent to it
        // there, which runs in -x to (-30,10).
        let run = FittedPath {
            start: Point::new(10.0, 0.0),
            segments: vec![
                Segment::Arc {
                    rx: 10.0,
                    ry: 10.0,
                    phi: 0.0,
                    large_arc: false,
                    sweep: true,
                    end: Point::new(0.0, 10.0),
                },
                Segment::Line(Point::new(-30.0, 10.0)),
            ],
            closed: false,
        };
        // Lower face inside (the disc and the band under the line); upper face outside.
        let inside_v = |q: Point| {
            if q.x >= 0.0 {
                q.x.hypot(q.y) < 10.0 && q.y > 0.0
            } else {
                q.x > -30.0 && q.y > 0.0 && q.y < 10.0
            }
        };
        let inside_u = |q: Point| {
            let outer = if q.x >= 0.0 {
                q.x.hypot(q.y) < 20.0
            } else {
                q.x > -30.0 && q.y < 20.0
            };
            outer && !inside_v(q) && q.y > 0.0
        };
        let run_copy = run.clone();
        let moved = displace_run(&[run], 1.0, 0.5, &inside_u, &inside_v, true).unwrap();
        let segs = &moved[0].segments;
        let Segment::Arc { rx, end, .. } = segs[0] else {
            panic!("{segs:?}");
        };
        assert!((rx - 10.25).abs() < 1e-9, "{segs:?}");
        assert!(end.dist(Point::new(0.0, 10.5)) < 1e-9, "{segs:?}");
        // Pinned, the join stays where it was.
        let pinned = displace_run(&[run_copy], 1.0, 0.5, &inside_u, &inside_v, false).unwrap();
        assert_eq!(pinned[0].segments[0].end(), Point::new(0.0, 10.0));
    }

    /// A cubic pinned at both ends bows by moving its two inner control points: its middle
    /// moves exactly the reach, its ends not at all, and it gains no numbers.
    #[test]
    fn a_cubic_between_two_junctions_bows_without_new_points() {
        let a = Point::new(0.0, 0.0);
        let seg = Segment::Cubic(
            Point::new(10.0, 1.0),
            Point::new(20.0, 1.0),
            Point::new(30.0, 0.0),
        );
        // Lower face below (y < curve), walked left to right with it on the right: sgn -1.
        let normal = |t: Point| scale(Point::new(t.y, -t.x), -1.0);
        let b = bulge(a, &seg, 0.5, &normal).unwrap();
        let (m0, _) = midpoint(a, &seg).unwrap();
        let (m1, _) = midpoint(a, &b).unwrap();
        assert!((m1.y - m0.y - 0.5).abs() < 1e-9 && (m1.x - m0.x).abs() < 1e-9);
        assert_eq!(b.end(), Point::new(30.0, 0.0));
    }
}
