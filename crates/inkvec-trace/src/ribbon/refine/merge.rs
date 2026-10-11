//! Merges: the stroke solve's splits run backwards ([`merges`]).
//!
//! The curve fitter segments each centreline from its centre samples, and its arcs stop at
//! [`inkvec_fit::primitives::MAX_ARC_DEGREES`]; the solve then moves the segments but
//! never removes one. So a stroke the artist drew as one arc of 270° comes back as three
//! arcs on one circle, a cap as an arc and a short line that continues it, a straight run
//! as two lines meeting at 1°, a straight stroke as a cubic with its handles on the chord:
//! the right curve, in more numbers than it takes. Each of those is a joint the
//! description length would rather not pay for, and the solve can show which.

use std::f64::consts::{PI, TAU};

use inkvec_core::{Point, Vec2};
use inkvec_fit::curves::{arc_center, Segment};
use inkvec_fit::primitives::{PrimitiveFit, PrimitiveKind, PARAMS_CIRCLE};
use inkvec_fit::{FittedPath, PARAMS_LINE};

use super::super::boundary::Boundary;
use super::super::join::{tangents, Style};
use super::super::Centreline;
use super::{circle_path, drawable, local_eval, rows, solve, Model, Row, Shape, TRIAL_ITERS};

/// Most merges [`merges`] solves per face. A merge whose unsolved description length
/// already falls is kept without a solve and is not counted.
const MAX_MERGE_TRIALS: usize = 12;

/// How far the local estimate of a merge's description length may sit above the price it
/// saves for the merge to be tried at all, as a multiple of that price: a merge is tried
/// when `½·Δχ²_local < (1 + MERGE_SLACK)·λ·Δk`.
///
/// The local estimate measures the merged segment where the two it replaces stood, before
/// any solve; the solve then moves every variable, and buys back part of the damage.
const MERGE_SLACK: f64 = 1.0;

/// Half-width of the band round a half turn, degrees, in which a merged arc is not
/// written with its own radius ([`arc_about`], [`super::regular`]).
///
/// SVG rebuilds an arc's centre from its end points and radius, at a distance
/// `k = sqrt(R² - L²)` from the chord's midpoint (`L` the half-chord). Near a half turn
/// `k` is small and `∂k/∂R = R/k` large: at 165° a radius rounded by 0.005 px (two
/// decimals) moves the drawn arc by 0.04 px, at 175° by 0.11 px. Inside the band a merged
/// arc is the exact half circle on its chord instead (a radius under the half-chord,
/// which SVG scales up to it: F.6.6), which rounding cannot move.
pub(super) const HALF_TURN_BAND_DEGREES: f64 = 15.0;

/// A merged arc may sweep at most this much, degrees; a centreline closer to a full turn
/// is a circle (the closed-path candidate of [`candidates`]).
const MAX_SWEEP_DEGREES: f64 = 350.0;

/// One proposed merge.
#[derive(Clone, Debug)]
struct Merge {
    /// The centreline.
    shape: usize,
    /// What replaces it.
    with: Centreline,
    /// Parameters saved.
    saved: f64,
    /// Which joint (or segment) of the centreline as it stands, to refuse it by.
    key: (usize, usize),
    /// Local estimate of the merge's change in `Σ r²` before any solve.
    dchi2: f64,
}

/// The joints of the strokes `cur` (half-width `h`, rows `rws` measured on them, total
/// cost `best`) that the description length would rather not pay for, merged.
///
/// Every interior joint of a path (and a closed path's closing one) is offered the
/// segments that could replace the two it joins ([`joint`]): a line through their outer
/// ends; an arc on the circle of either one that is an arc, or of both when they share a
/// sense of turn, or through the three points; a cubic with the outer tangents
/// ([`cubic_through`]). Every cubic and arc is offered the line on its chord, and a
/// closed path of arcs the circle they lie on. Each is measured, unsolved, against the
/// boundary points the replaced segments explained (`Δχ²_local`), and tried, best first,
/// when `½·Δχ²_local - λ·Δk` is under [`MERGE_SLACK`] times the price it saves: kept
/// outright when the unsolved strokes' description length `χ²/2 + λ·k` already falls,
/// else solved for [`TRIAL_ITERS`] iterations and kept when it falls then. A kept merge
/// must also be drawable ([`drawable`]) and leave the strokes' `χ²` at most `chi2_cap`
/// (the caller's: what the face may cost and still be written as strokes). At most
/// [`MAX_MERGE_TRIALS`] are solved. Returns the strokes, half-width, cost and rows, and
/// whether anything was merged.
///
/// Inspired by: Pavlidis, Horowitz (1974), Segmentation of plane curves, IEEE Transactions
/// on Computers C-23(8), 860-870, doi:10.1109/T-C.1974.224041 -- split-and-merge, where
/// adjacent pieces are merged while the merged piece still fits. Ours prices the merge by
/// description length after a solve against the stroke's painted outline, and offers the
/// kinds of curve a designer draws with: Schneider (1990), An algorithm for automatically
/// fitting digitized curves, Graphics Gems, pp. 612-626, for the cubic with fixed end
/// tangents.
#[allow(clippy::too_many_arguments)]
pub(super) fn merges(
    mut cur: Vec<Centreline>,
    mut h: f64,
    mut rws: Vec<Option<Row>>,
    mut best: f64,
    b: &Boundary,
    lambda: f64,
    style: Style,
    chi2_cap: f64,
) -> (Vec<Centreline>, f64, f64, Vec<Option<Row>>, bool) {
    let reach = 8.0 * h + 4.0;
    let cost = |ls: &[Centreline], h: f64| -> (f64, f64, Vec<Option<Row>>) {
        let (e, r) = rows(&Model::new(ls, h, style), b, reach);
        (
            0.5 * e + lambda * ls.iter().map(Centreline::params).sum::<f64>(),
            e,
            r,
        )
    };
    let fits = |ls: &[Centreline], c: f64, e: f64, best: f64| {
        c < best && e <= chi2_cap && drawable(ls, style)
    };
    let mut refused: Vec<(usize, usize)> = Vec::new();
    let (mut trials, mut merged) = (0usize, false);
    while trials < MAX_MERGE_TRIALS {
        let model = Model::new(&cur, h, style);
        let mut cands: Vec<Merge> = candidates(&cur, &model, &rws, b)
            .into_iter()
            .filter(|m| {
                !refused.contains(&m.key) && 0.5 * m.dchi2 < (1.0 + MERGE_SLACK) * lambda * m.saved
            })
            .collect();
        let gain = |m: &Merge| 0.5 * m.dchi2 - lambda * m.saved;
        cands.sort_by(|x, y| gain(x).total_cmp(&gain(y)));
        let Some(m) = cands.into_iter().next() else {
            break;
        };
        let mut trial = cur.clone();
        trial[m.shape] = m.with.clone();
        let (c0, e0, r0) = cost(&trial, h);
        if fits(&trial, c0, e0, best) {
            (cur, best, rws, merged) = (trial, c0, r0, true);
            refused.clear();
            continue;
        }
        trials += 1;
        let (tl, th) = solve(&trial, h, b, style, TRIAL_ITERS);
        let (c1, e1, r1) = cost(&tl, th);
        if fits(&tl, c1, e1, best) {
            (cur, h, best, rws, merged) = (tl, th, c1, r1, true);
            refused.clear();
        } else {
            refused.push(m.key);
        }
    }
    (cur, h, best, rws, merged)
}

/// Every merge of [`merges`] on the strokes `cur` (as `model`, rows `rws`), with its
/// local estimate.
fn candidates(cur: &[Centreline], model: &Model, rws: &[Option<Row>], b: &Boundary) -> Vec<Merge> {
    // The boundary points each path segment explains: (point, foot parameter).
    let mut owned: Vec<Vec<Vec<(usize, f64)>>> = model
        .shapes
        .iter()
        .map(|s| match s {
            Shape::Path { segs, .. } => vec![Vec::new(); segs.len()],
            _ => Vec::new(),
        })
        .collect();
    for (i, r) in rws.iter().enumerate() {
        if let Some(r) = r {
            if let Some(v) = owned.get_mut(r.shape).and_then(|s| s.get_mut(r.seg)) {
                v.push((i, r.t));
            }
        }
    }
    let ctx = Ctx {
        cur,
        style: model.style,
        h: model.h(),
        rws,
        b,
        owned,
    };
    let mut out = Vec::new();
    for (s, line) in cur.iter().enumerate() {
        if line.prim.is_none() {
            ctx.singles(s, &mut out);
            ctx.joints(s, &mut out);
            ctx.circle(s, &mut out);
        }
    }
    out
}

/// What [`candidates`] measures merges against.
struct Ctx<'a> {
    /// The strokes as they stand.
    cur: &'a [Centreline],
    /// Their joins and caps.
    style: Style,
    /// Their half-width.
    h: f64,
    /// Each boundary point's row on them.
    rws: &'a [Option<Row>],
    /// The boundary.
    b: &'a Boundary,
    /// Per shape and segment, the boundary points it explains and their foot parameters.
    owned: Vec<Vec<Vec<(usize, f64)>>>,
}

impl Ctx<'_> {
    /// `Σ r²` of the points `pts` as they stand.
    fn old(&self, pts: &[(usize, f64)]) -> f64 {
        pts.iter()
            .map(|&(i, _)| {
                let d = self.rws[i].as_ref().map_or(0.0, |r| r.d);
                self.b.residual(i, d, self.h).0.powi(2)
            })
            .sum()
    }

    /// The merge that gives centreline `s` the centreline `with`, saving `saved`
    /// parameters, with its local estimate: the points `pts` (index, foot hint, segment of
    /// `with`) measured against it alone, less their `before`.
    fn local(
        &self,
        (s, key): (usize, (usize, usize)),
        with: Centreline,
        saved: f64,
        before: f64,
        pts: &[(usize, f64, usize)],
    ) -> Merge {
        let mut trial = self.cur.to_vec();
        trial[s] = with.clone();
        let tm = Model::new(&trial, self.h, self.style);
        let after: f64 = pts
            .iter()
            .map(|&(i, t, k)| {
                let d = local_eval(&tm, s, k, self.b.pts[i], t).d;
                self.b.residual(i, d, self.h).0.powi(2)
            })
            .sum();
        Merge {
            shape: s,
            with,
            saved,
            key,
            dchi2: after - before,
        }
    }

    /// Each segment of centreline `s` alone: a cubic or an arc as the line on its chord.
    fn singles(&self, s: usize, out: &mut Vec<Merge>) {
        let path = &self.cur[s].path;
        for (k, seg) in path.segments.iter().enumerate() {
            let a = if k == 0 {
                path.start
            } else {
                path.segments[k - 1].end()
            };
            if matches!(seg, Segment::Line(_)) || a.dist(seg.end()) < 0.5 {
                continue;
            }
            let mut p = path.clone();
            p.segments[k] = Segment::Line(seg.end());
            let pts = &self.owned[s][k];
            let mine: Vec<(usize, f64, usize)> = pts.iter().map(|&(i, t)| (i, t, k)).collect();
            let with = Centreline {
                path: p,
                prim: None,
            };
            let saved = seg.params() - PARAMS_LINE;
            out.push(self.local((s, (k, usize::MAX)), with, saved, self.old(pts), &mine));
        }
    }

    /// Each joint of centreline `s`, the closing one of a closed path of three or more
    /// included (the path is turned to start at the end of its first segment, so that
    /// joint is interior): every segment of [`joint`] cheaper than the pair.
    fn joints(&self, s: usize, out: &mut Vec<Merge>) {
        let path = &self.cur[s].path;
        let n = path.segments.len();
        let joints = if path.closed && n >= 3 {
            n
        } else {
            n.saturating_sub(1)
        };
        for j in 0..joints {
            let (p, at, ka, kb) = if j + 1 < n {
                (path.clone(), j, j, j + 1)
            } else {
                (rotated(path), n - 2, n - 1, 0)
            };
            let p0 = if at == 0 {
                p.start
            } else {
                p.segments[at - 1].end()
            };
            let (sa, sb) = (&p.segments[at], &p.segments[at + 1]);
            let k_pair = sa.params() + sb.params();
            let la = p0.dist(sa.end());
            let frac = la / (la + sa.end().dist(sb.end())).max(1e-12);
            // Each point's foot on the merged segment, from where it stood on its own.
            let (own_a, own_b) = (&self.owned[s][ka], &self.owned[s][kb]);
            let pts: Vec<(usize, f64, usize)> = own_a
                .iter()
                .map(|&(i, t)| (i, frac * t, at))
                .chain(own_b.iter().map(|&(i, t)| (i, frac + (1.0 - frac) * t, at)))
                .collect();
            let before = self.old(own_a) + self.old(own_b);
            for seg in joint(p0, sa, sb) {
                if seg.params() >= k_pair {
                    continue;
                }
                let mut q = p.clone();
                let saved = k_pair - seg.params();
                q.segments.splice(at..=at + 1, [seg]);
                let with = Centreline {
                    path: q,
                    prim: None,
                };
                out.push(self.local((s, (j, n)), with, saved, before, &pts));
            }
        }
    }

    /// A closed centreline `s` of arcs on one circle: that circle.
    fn circle(&self, s: usize, out: &mut Vec<Merge>) {
        let line = &self.cur[s];
        if !line.path.closed {
            return;
        }
        let Some((c, r)) = common_circle(&line.path) else {
            return;
        };
        let with = Centreline {
            path: circle_path(c, r),
            prim: Some(PrimitiveFit {
                kind: PrimitiveKind::Circle { c, r },
                chi2: 0.0,
                params: PARAMS_CIRCLE,
            }),
        };
        let before: f64 = self.owned[s].iter().map(|v| self.old(v)).sum();
        let pts: Vec<(usize, f64, usize)> = self.owned[s]
            .iter()
            .flatten()
            .map(|&(i, t)| (i, t, 0))
            .collect();
        let saved = line.params() - PARAMS_CIRCLE;
        out.push(self.local((s, (usize::MAX, s)), with, saved, before, &pts));
    }
}

/// A closed path turned to start at the end of its first segment, which becomes its last.
fn rotated(path: &FittedPath) -> FittedPath {
    let mut segments = path.segments[1..].to_vec();
    segments.push(path.segments[0].clone());
    FittedPath {
        start: path.segments[0].end(),
        segments,
        closed: true,
    }
}

/// The segments, from `p0` to `b`'s end, that could replace segment `a` (from `p0`) and
/// segment `b` (from `a`'s end): the line; arcs on `a`'s circle, `b`'s, both's (when they
/// turn the same way) and the circle through the three points; the cubic with the outer
/// tangents. The caller keeps those cheaper than the pair.
fn joint(p0: Point, a: &Segment, b: &Segment) -> Vec<Segment> {
    let (p1, p2) = (a.end(), b.end());
    let mut out = Vec::new();
    if p0.dist(p2) < 0.5 {
        return out;
    }
    out.push(Segment::Line(p2));
    let ca = circle_of(p0, a);
    let cb = circle_of(p1, b);
    match (ca, cb) {
        (Some((c1, w1, s1)), Some((c2, w2, s2))) if s1 == s2 => {
            let w = w1 + w2;
            let c = Point::new((w1 * c1.x + w2 * c2.x) / w, (w1 * c1.y + w2 * c2.y) / w);
            out.extend(arc_about(p0, p2, c, s1));
        }
        _ => {
            for (c, _, s) in [ca, cb].into_iter().flatten() {
                out.extend(arc_about(p0, p2, c, s));
            }
        }
    }
    let turn = (p1 - p0).cross(p2 - p1);
    if let Some(c) = circumcentre(p0, p1, p2) {
        out.extend(arc_about(p0, p2, c, turn > 0.0));
    }
    out.extend(cubic_through(p0, a, b));
    out
}

/// A circular arc's centre, sweep (radians, unsigned) and SVG sweep flag; `None` for
/// anything else.
fn circle_of(a: Point, s: &Segment) -> Option<(Point, f64, bool)> {
    let Segment::Arc {
        rx,
        large_arc,
        sweep,
        end,
        ..
    } = *s
    else {
        return None;
    };
    if !s.is_circular() {
        return None;
    }
    let (c, _, _, delta) = arc_center(a, rx, large_arc, sweep, end);
    (delta != 0.0).then_some((c, delta.abs(), sweep))
}

/// The circle through three points, `None` when they are (nearly) collinear.
fn circumcentre(a: Point, b: Point, c: Point) -> Option<Point> {
    let (u, v) = (b - a, c - a);
    let d = 2.0 * u.cross(v);
    let scale = u.norm() * v.norm();
    if d.abs() <= 1e-9 * scale {
        return None;
    }
    let (uu, vv) = (u.dot(u), v.dot(v));
    let x = (v.y * uu - u.y * vv) / d;
    let y = (u.x * vv - v.x * uu) / d;
    let o = Point::new(a.x + x, a.y + y);
    // A circle much larger than the points' spread is a straight run in disguise.
    (o.dist(a) <= 1e3 * a.dist(c).max(1.0)).then_some(o)
}

/// The arc from `p0` to `p2` round centre `c` the way SVG's `sweep` flag says, with the
/// mean of the two end points' distances as its radius; within
/// [`HALF_TURN_BAND_DEGREES`] of a half turn the exact half circle on the chord. `None`
/// for a sweep under 1e-3 rad or over [`MAX_SWEEP_DEGREES`].
fn arc_about(p0: Point, p2: Point, c: Point, sweep: bool) -> Option<Segment> {
    let (u, v) = (p0 - c, p2 - c);
    let mut d = v.y.atan2(v.x) - u.y.atan2(u.x);
    if sweep {
        while d <= 0.0 {
            d += TAU;
        }
    } else {
        while d >= 0.0 {
            d -= TAU;
        }
    }
    let s = d.abs();
    if !(1e-3..=MAX_SWEEP_DEGREES.to_radians()).contains(&s) {
        return None;
    }
    if (s - PI).abs() < HALF_TURN_BAND_DEGREES.to_radians() {
        let l = 0.5 * p0.dist(p2);
        return Some(Segment::circular_arc(
            half_circle_radius(l),
            false,
            sweep,
            p2,
        ));
    }
    let r = 0.5 * (u.norm() + v.norm());
    Some(Segment::circular_arc(r, s > PI, sweep, p2))
}

/// The radius written for a half circle on a chord of half-length `l`: enough under `l`
/// that two-decimal rounding of it and of the end points cannot lift it over the chord
/// (SVG then draws the half circle whatever the radius, F.6.6).
pub(super) fn half_circle_radius(l: f64) -> f64 {
    (l - 0.02).max(0.5 * l)
}

/// The cubic from `p0` to `b`'s end leaving along `a`'s first tangent and arriving along
/// `b`'s last, its two arm lengths fitted by least squares to 16 samples of each segment,
/// from chord-length parameters reparametrised [`REPARAM_ROUNDS`] times (Schneider's), or a third of the
/// chord each when that fit is degenerate or turns an arm backwards.
fn cubic_through(p0: Point, a: &Segment, b: &Segment) -> Option<Segment> {
    const PER: usize = 16;
    const REPARAM_ROUNDS: usize = 8;
    let p1 = a.end();
    let p2 = b.end();
    let (t0, _) = tangents(p0, a)?;
    let (_, t1) = tangents(p1, b)?;
    let mut pts = vec![p0];
    sample(p0, a, PER, &mut pts);
    sample(p1, b, PER, &mut pts);
    let mut u = vec![0.0];
    for w in pts.windows(2) {
        u.push(u[u.len() - 1] + w[0].dist(w[1]));
    }
    let len = u[u.len() - 1];
    if len < 1e-9 {
        return None;
    }
    for t in &mut u {
        *t /= len;
    }
    let chord = p0.dist(p2);
    let third = chord / 3.0;
    let mut arms = arms_for(p0, p2, t0, t1, &pts, &u).unwrap_or((third, third));
    // Rounds of Schneider's reparametrisation: each sample's parameter moved to its
    // foot on the cubic by one Newton step, and the arms fitted again.
    for _ in 0..REPARAM_ROUNDS {
        let q = cubic_of(p0, p2, t0, t1, arms);
        for (p, t) in pts.iter().zip(u.iter_mut()) {
            // Derivatives come back as points: (x, y) of the vectors.
            let (c, d1, d2) = super::cubic_eval(&q, *t);
            let (rx, ry) = (c.x - p.x, c.y - p.y);
            let den = d1.x * d1.x + d1.y * d1.y + rx * d2.x + ry * d2.y;
            if den.abs() > 1e-12 {
                *t = (*t - (rx * d1.x + ry * d1.y) / den).clamp(0.0, 1.0);
            }
        }
        arms = arms_for(p0, p2, t0, t1, &pts, &u).unwrap_or(arms);
    }
    if arms.0 <= 1e-6 * chord || arms.1 <= 1e-6 * chord {
        arms = (third, third);
    }
    let q = cubic_of(p0, p2, t0, t1, arms);
    Some(Segment::Cubic(q[1], q[2], p2))
}

/// The cubic from `p0` along `t0` to `p2` along `t1` with arm lengths `arms`.
fn cubic_of(p0: Point, p2: Point, t0: Vec2, t1: Vec2, arms: (f64, f64)) -> [Point; 4] {
    [
        p0,
        Point::new(p0.x + arms.0 * t0.x, p0.y + arms.0 * t0.y),
        Point::new(p2.x - arms.1 * t1.x, p2.y - arms.1 * t1.y),
        p2,
    ]
}

/// The least-squares arm lengths of the cubic from `p0` along `t0` to `p2` along `t1`
/// through `pts` at parameters `u`; `None` when the system is singular.
fn arms_for(
    p0: Point,
    p2: Point,
    t0: Vec2,
    t1: Vec2,
    pts: &[Point],
    u: &[f64],
) -> Option<(f64, f64)> {
    let (mut c00, mut c01, mut c11, mut x0, mut x1) = (0.0, 0.0, 0.0, 0.0, 0.0);
    for (p, &t) in pts.iter().zip(u) {
        let s = 1.0 - t;
        let (b0, b1, b2, b3) = (s * s * s, 3.0 * s * s * t, 3.0 * s * t * t, t * t * t);
        let a1 = Vec2 {
            x: b1 * t0.x,
            y: b1 * t0.y,
        };
        let a2 = Vec2 {
            x: -b2 * t1.x,
            y: -b2 * t1.y,
        };
        let rest = Vec2 {
            x: p.x - (b0 + b1) * p0.x - (b2 + b3) * p2.x,
            y: p.y - (b0 + b1) * p0.y - (b2 + b3) * p2.y,
        };
        c00 += a1.dot(a1);
        c01 += a1.dot(a2);
        c11 += a2.dot(a2);
        x0 += a1.dot(rest);
        x1 += a2.dot(rest);
    }
    let det = c00 * c11 - c01 * c01;
    (det.abs() > 1e-12 * (c00 * c11).max(1e-300))
        .then(|| ((x0 * c11 - x1 * c01) / det, (c00 * x1 - c01 * x0) / det))
}

/// `n` points along segment `s` from `a`, at equal steps of its own parameter, its start
/// excluded and its end included.
fn sample(a: Point, s: &Segment, n: usize, out: &mut Vec<Point>) {
    for k in 1..=n {
        let t = k as f64 / n as f64;
        out.push(match *s {
            Segment::Line(e) => Point::new(a.x + t * (e.x - a.x), a.y + t * (e.y - a.y)),
            Segment::Cubic(c1, c2, e) => super::cubic_eval(&[a, c1, c2, e], t).0,
            Segment::Arc {
                rx,
                ry,
                phi,
                large_arc,
                sweep,
                end,
            } => {
                let f =
                    inkvec_fit::curves::arc_ellipse_center(a, rx, ry, phi, large_arc, sweep, end);
                if k == n {
                    end
                } else {
                    f.at(f.theta1 + f.delta * t)
                }
            }
        });
    }
}

/// The circle a closed path of circular arcs lies on: the sweep-weighted mean of their
/// centres and the mean distance of their end points from it, when every centre is
/// within 2% of the radius of it and every end point within 2% of the radius from the
/// circle. `None` otherwise.
fn common_circle(path: &FittedPath) -> Option<(Point, f64)> {
    let mut a = path.start;
    let mut acc = (0.0, 0.0, 0.0);
    let mut cs = Vec::new();
    for s in &path.segments {
        let (c, w, _) = circle_of(a, s)?;
        acc = (acc.0 + w * c.x, acc.1 + w * c.y, acc.2 + w);
        cs.push(c);
        a = s.end();
    }
    if acc.2 <= 0.0 {
        return None;
    }
    let c = Point::new(acc.0 / acc.2, acc.1 / acc.2);
    let ends: Vec<Point> = path.segments.iter().map(Segment::end).collect();
    let r = ends.iter().map(|e| e.dist(c)).sum::<f64>() / ends.len() as f64;
    let tol = 0.02 * r;
    (cs.iter().all(|q| q.dist(c) <= tol) && ends.iter().all(|e| (e.dist(c) - r).abs() <= tol))
        .then_some((c, r))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ribbon::join::Join;
    use inkvec_core::Polyline;

    /// The outline of a round-capped stroke of half-width `h` along the arc of centre `c`,
    /// radius `r`, from angle 0 to `sweep` (radians, increasing), as one dense ring, and
    /// the stroke's inside test.
    fn arc_stroke(c: Point, r: f64, sweep: f64, h: f64) -> Boundary {
        let at = |rad: f64, th: f64| Point::new(c.x + rad * th.cos(), c.y + rad * th.sin());
        let mut pts = Vec::new();
        let n_arc = (sweep * (r + h) / 0.25) as usize;
        for i in 0..n_arc {
            pts.push(at(r + h, sweep * i as f64 / n_arc as f64));
        }
        let cap = |e: Point, t0: f64, pts: &mut Vec<Point>| {
            for j in 0..60 {
                let t = t0 + PI * j as f64 / 60.0;
                pts.push(Point::new(e.x + h * t.cos(), e.y + h * t.sin()));
            }
        };
        cap(at(r, sweep), sweep, &mut pts);
        for i in 0..n_arc {
            pts.push(at(r - h, sweep * (1.0 - i as f64 / n_arc as f64)));
        }
        cap(at(r, 0.0), PI, &mut pts);
        let ring = Polyline::with_uniform_sigma(pts, 0.05, true);
        let inside = move |p: Point| {
            let v = p - c;
            let th = v.y.atan2(v.x).rem_euclid(TAU);
            let d = if th <= sweep {
                (v.norm() - r).abs()
            } else {
                p.dist(at(r, 0.0)).min(p.dist(at(r, sweep)))
            };
            d < h
        };
        Boundary::new(&[ring], &inside, 2.0).expect("a ring")
    }

    fn run(lines: Vec<Centreline>, h: f64, b: &Boundary) -> (Vec<Centreline>, bool) {
        let style: Style = Join::Round.into();
        let lambda = 10.0;
        let (e, rws) = rows(&Model::new(&lines, h, style), b, 8.0 * h + 4.0);
        let best = 0.5 * e + lambda * lines.iter().map(Centreline::params).sum::<f64>();
        let (out, _, _, _, merged) = merges(lines, h, rws, best, b, lambda, style, f64::INFINITY);
        (out, merged)
    }

    #[test]
    fn three_arcs_on_one_circle_become_one_large_arc() {
        let (c, r) = (Point::new(50.0, 50.0), 30.0);
        let b = arc_stroke(c, r, 1.5 * PI, 4.0);
        let at = |th: f64| Point::new(c.x + r * th.cos(), c.y + r * th.sin());
        let line = Centreline {
            path: FittedPath {
                start: at(0.0),
                segments: (1..=3)
                    .map(|k| Segment::circular_arc(r, false, true, at(0.5 * PI * k as f64)))
                    .collect(),
                closed: false,
            },
            prim: None,
        };
        let (out, merged) = run(vec![line], 4.0, &b);
        assert!(merged);
        let segs = &out[0].path.segments;
        assert_eq!(segs.len(), 1, "{segs:?}");
        let Segment::Arc {
            rx,
            large_arc,
            sweep,
            ..
        } = segs[0]
        else {
            panic!("{segs:?}")
        };
        assert!(large_arc && sweep && (rx - r).abs() < 0.05, "{segs:?}");
    }

    #[test]
    fn two_lines_in_line_become_one() {
        let b = arc_stroke(Point::new(0.0, -1e4), 1e4, 30.0 / 1e4, 3.0);
        let p0 = Point::new(1e4 * 0.0f64.cos(), -1e4 + 1e4 * 0.0f64.sin());
        let p2 = Point::new(
            1e4 * (30.0f64 / 1e4).cos(),
            -1e4 + 1e4 * (30.0f64 / 1e4).sin(),
        );
        let mid = Point::new(0.5 * (p0.x + p2.x), 0.5 * (p0.y + p2.y));
        let line = Centreline {
            path: FittedPath {
                start: p0,
                segments: vec![Segment::Line(mid), Segment::Line(p2)],
                closed: false,
            },
            prim: None,
        };
        let (out, merged) = run(vec![line], 3.0, &b);
        assert!(merged);
        assert_eq!(out[0].path.segments.len(), 1, "{:?}", out[0].path);
    }

    #[test]
    fn a_bent_pair_is_kept() {
        // Two lines meeting at a right angle: no merge fits the boundary of that corner.
        let c = Point::new(50.0, 50.0);
        let b = arc_stroke(c, 30.0, 1.5 * PI, 4.0);
        let line = Centreline {
            path: FittedPath {
                start: Point::new(80.0, 50.0),
                segments: vec![
                    Segment::circular_arc(30.0, false, true, Point::new(50.0, 80.0)),
                    Segment::circular_arc(30.0, false, true, Point::new(20.0, 50.0)),
                    Segment::Line(Point::new(50.0, 20.0)),
                ],
                closed: false,
            },
            prim: None,
        };
        let (out, _) = run(vec![line], 4.0, &b);
        // The line cuts the last quarter's corner by 8.8 px: it is never merged into a
        // line with its neighbour, whatever else is.
        assert!(!out[0].path.segments.iter().any(
            |s| matches!(s, Segment::Line(e) if e.dist(Point::new(50.0, 20.0)) < 1.0)
                && out[0].path.segments.len() == 1
        ));
    }

    #[test]
    fn a_closed_ring_of_arcs_becomes_a_circle() {
        let (c, r) = (Point::new(40.0, 40.0), 20.0);
        let at = |th: f64| Point::new(c.x + r * th.cos(), c.y + r * th.sin());
        let mut pts = Vec::new();
        for i in 0..800 {
            let th = TAU * i as f64 / 800.0;
            pts.push(Point::new(c.x + 23.0 * th.cos(), c.y + 23.0 * th.sin()));
        }
        let outer = Polyline::with_uniform_sigma(pts.clone(), 0.05, true);
        let inner = Polyline::with_uniform_sigma(
            pts.iter()
                .rev()
                .map(|p| {
                    Point::new(
                        c.x + (p.x - c.x) * 17.0 / 23.0,
                        c.y + (p.y - c.y) * 17.0 / 23.0,
                    )
                })
                .collect(),
            0.05,
            true,
        );
        let inside = move |p: Point| (p.dist(c) - r).abs() < 3.0;
        let b = Boundary::new(&[outer, inner], &inside, 2.0).expect("rings");
        let third = TAU / 3.0;
        let line = Centreline {
            path: FittedPath {
                start: at(0.0),
                segments: (1..=3)
                    .map(|k| Segment::circular_arc(r, false, true, at(third * k as f64)))
                    .collect(),
                closed: true,
            },
            prim: None,
        };
        let line = Centreline {
            path: FittedPath {
                segments: {
                    let mut s = line.path.segments.clone();
                    if let Some(Segment::Arc { end, .. }) = s.last_mut() {
                        *end = at(0.0);
                    }
                    s
                },
                ..line.path.clone()
            },
            prim: None,
        };
        let (out, merged) = run(vec![line], 3.0, &b);
        assert!(merged);
        assert!(
            matches!(
                out[0].prim,
                Some(PrimitiveFit {
                    kind: PrimitiveKind::Circle { .. },
                    ..
                })
            ),
            "{:?}",
            out[0]
        );
    }

    #[test]
    fn the_cubic_through_two_halves_is_the_whole() {
        let q = [
            Point::new(0.0, 0.0),
            Point::new(10.0, 20.0),
            Point::new(30.0, 25.0),
            Point::new(40.0, 0.0),
        ];
        let lerp = |p: Point, r: Point| Point::new(0.5 * (p.x + r.x), 0.5 * (p.y + r.y));
        let (p01, p12, p23) = (lerp(q[0], q[1]), lerp(q[1], q[2]), lerp(q[2], q[3]));
        let (p012, p123) = (lerp(p01, p12), lerp(p12, p23));
        let m = lerp(p012, p123);
        let a = Segment::Cubic(p01, p012, m);
        let b = Segment::Cubic(p123, p23, q[3]);
        let Some(Segment::Cubic(c1, c2, e)) = cubic_through(q[0], &a, &b) else {
            panic!("a cubic");
        };
        assert_eq!(e, q[3]);
        // The arms are a start for the solve, not the cubic's own: the curve they draw
        // stays within a few hundredths of a pixel of it.
        let fitted = [q[0], c1, c2, e];
        let worst = (0..=64)
            .map(|k| {
                let p = super::super::cubic_eval(&q, k as f64 / 64.0).0;
                crate::ribbon::dist::cubic_dist(p, fitted, k as f64 / 64.0).0
            })
            .fold(0.0, f64::max);
        assert!(worst < 0.1, "{worst} px: {c1:?} {c2:?}");
    }

    #[test]
    fn arcs_near_a_half_turn_are_written_as_the_half_circle() {
        let (p0, p2) = (Point::new(0.0, 0.0), Point::new(20.0, 0.0));
        // Centre 0.5 px off the chord: a sweep of 177°.
        let s = arc_about(p0, p2, Point::new(10.0, 0.5), true).expect("an arc");
        let Segment::Arc { rx, .. } = s else {
            panic!("{s:?}")
        };
        assert!(rx < 10.0 && super::super::conditioned(p0, &s));
        // Centre 3 px off: 147°, its own radius.
        let s = arc_about(p0, p2, Point::new(10.0, 3.0), true).expect("an arc");
        let Segment::Arc { rx, .. } = s else {
            panic!("{s:?}")
        };
        assert!((rx - 109f64.sqrt()).abs() < 1e-9 && super::super::conditioned(p0, &s));
        // The same chord with a radius 0.05 px over the half-chord (168.6°) is refused by
        // the solve.
        let near = Segment::circular_arc(10.05, false, true, p2);
        assert!(!super::super::conditioned(p0, &near));
        // Nearly a full turn is not an arc.
        assert!(arc_about(p0, Point::new(0.5, -0.1), Point::new(0.0, 10.0), false).is_none());
    }

    #[test]
    fn the_half_circle_radius_survives_rounding() {
        for l in [0.3, 1.0, 10.0, 213.33] {
            let r = half_circle_radius(l);
            let rounded = (r * 100.0).round() / 100.0;
            assert!(rounded < l - 0.0071, "{l} -> {r}");
        }
    }
}
