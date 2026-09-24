//! Cubics as circular arcs, for CAM.
//!
//! A cutter, router or laser controller moves in straight lines and circular arcs (G1, G2,
//! G3). Handed a curve as hundreds of short chords it stutters: every chord is a move with
//! its own acceleration ramp, the motion planner's look-ahead buffer runs dry, and the edge
//! comes out faceted. Handed arcs it moves smoothly, and the file is a fraction of the size.
//! DXF says an arc inside a polyline with a vertex's *bulge*, the tangent of a quarter of
//! the arc's sweep, which CAM programs turn straight into G2/G3.
//!
//! Each cubic becomes a **biarc** (Bolton 1975; Meek and Walton 1992): two arcs that meet
//! with a common tangent and leave and arrive along the cubic's own end tangents, so the
//! whole path stays G1 wherever the curve was. The junction is the equal-tangent-length
//! point. The fit is checked both ways, cubic against arcs and arcs against cubic, and a
//! cubic that does not fit within the tolerance is halved and each half fitted again.

use crate::geom::Pt;

/// Deepest a cubic is halved before its biarc is taken as it is.
const MAX_DEPTH: usize = 12;
/// Samples along a cubic for the fit check.
const SAMPLES: usize = 24;

fn sub(a: Pt, b: Pt) -> Pt {
    [a[0] - b[0], a[1] - b[1]]
}
fn dot(a: Pt, b: Pt) -> f64 {
    a[0] * b[0] + a[1] * b[1]
}
fn cross(a: Pt, b: Pt) -> f64 {
    a[0] * b[1] - a[1] * b[0]
}
fn norm(a: Pt) -> f64 {
    a[0].hypot(a[1])
}
fn unit(a: Pt) -> Option<Pt> {
    let n = norm(a);
    (n > 1e-12).then(|| [a[0] / n, a[1] / n])
}

fn cubic_at(c: &[Pt; 4], t: f64) -> Pt {
    let u = 1.0 - t;
    let (b0, b1, b2, b3) = (u * u * u, 3.0 * u * u * t, 3.0 * u * t * t, t * t * t);
    [
        b0 * c[0][0] + b1 * c[1][0] + b2 * c[2][0] + b3 * c[3][0],
        b0 * c[0][1] + b1 * c[1][1] + b2 * c[2][1] + b3 * c[3][1],
    ]
}

fn split(c: &[Pt; 4]) -> ([Pt; 4], [Pt; 4]) {
    let mid = |a: Pt, b: Pt| [(a[0] + b[0]) / 2.0, (a[1] + b[1]) / 2.0];
    let (p01, p12, p23) = (mid(c[0], c[1]), mid(c[1], c[2]), mid(c[2], c[3]));
    let (p012, p123) = (mid(p01, p12), mid(p12, p23));
    let m = mid(p012, p123);
    ([c[0], p01, p012, m], [m, p123, p23, c[3]])
}

/// The bulge of the arc from `a` to `b` that leaves `a` along `t` (unit): the tangent of a
/// quarter of its signed sweep, positive counter-clockwise in a y-up frame.
fn bulge_leaving(a: Pt, b: Pt, t: Pt) -> f64 {
    let c = sub(b, a);
    let half = cross(t, c).atan2(dot(t, c));
    (half / 2.0).tan()
}

/// The bulge of the arc from `a` to `b` that arrives at `b` along `t` (unit).
fn bulge_arriving(a: Pt, b: Pt, t: Pt) -> f64 {
    let c = sub(b, a);
    let half = cross(c, t).atan2(dot(c, t));
    (half / 2.0).tan()
}

/// Distance from `p` to the arc (or segment, at bulge 0) from `a` to `b`.
fn arc_distance(p: Pt, a: Pt, b: Pt, bulge: f64) -> f64 {
    let c = sub(b, a);
    let len = norm(c);
    if bulge.abs() < 1e-9 || len < 1e-12 {
        let t = if len < 1e-12 {
            0.0
        } else {
            (dot(sub(p, a), c) / (len * len)).clamp(0.0, 1.0)
        };
        return norm(sub(p, [a[0] + t * c[0], a[1] + t * c[1]]));
    }
    let theta = 4.0 * bulge.atan();
    // Signed radius; the centre sits to the chord's left for a counter-clockwise arc.
    let r = len / (2.0 * (theta / 2.0).sin());
    let n = [-c[1] / len, c[0] / len];
    let m = [(a[0] + b[0]) / 2.0, (a[1] + b[1]) / 2.0];
    let h = r * (theta / 2.0).cos();
    let centre = [m[0] + n[0] * h, m[1] + n[1] * h];
    let ang = |q: Pt| (q[1] - centre[1]).atan2(q[0] - centre[0]);
    let tau = std::f64::consts::TAU;
    let swept = if theta > 0.0 {
        (ang(p) - ang(a)).rem_euclid(tau)
    } else {
        (ang(a) - ang(p)).rem_euclid(tau)
    };
    if swept <= theta.abs() {
        (norm(sub(p, centre)) - r.abs()).abs()
    } else {
        norm(sub(p, a)).min(norm(sub(p, b)))
    }
}

/// A point on the arc from `a` to `b` with `bulge`, at fraction `s` of its sweep.
pub(crate) fn arc_at(a: Pt, b: Pt, bulge: f64, s: f64) -> Pt {
    if bulge.abs() < 1e-9 {
        return [a[0] + s * (b[0] - a[0]), a[1] + s * (b[1] - a[1])];
    }
    let c = sub(b, a);
    let len = norm(c);
    let theta = 4.0 * bulge.atan();
    let r = len / (2.0 * (theta / 2.0).sin());
    let n = [-c[1] / len, c[0] / len];
    let m = [(a[0] + b[0]) / 2.0, (a[1] + b[1]) / 2.0];
    let h = r * (theta / 2.0).cos();
    let centre = [m[0] + n[0] * h, m[1] + n[1] * h];
    let start = (a[1] - centre[1]).atan2(a[0] - centre[0]);
    let phi = start + s * theta;
    let rr = r.abs();
    [centre[0] + rr * phi.cos(), centre[1] + rr * phi.sin()]
}

/// The biarc for one cubic: its junction and the two bulges, or `None` when the end
/// tangents admit no equal-length junction (a cusp, or a curve that doubles back).
fn biarc(c: &[Pt; 4]) -> Option<(Pt, f64, f64)> {
    let t0 = unit(sub(c[1], c[0])).or_else(|| unit(sub(c[2], c[0])))?;
    let t1 = unit(sub(c[3], c[2])).or_else(|| unit(sub(c[3], c[1])))?;
    let v = sub(c[3], c[0]);
    let t = [t0[0] + t1[0], t0[1] + t1[1]];
    // |v - d t| = 2d, solved for the tangent length d.
    let qa = 2.0 * (dot(t0, t1) - 1.0);
    let qb = -2.0 * dot(v, t);
    let qc = dot(v, v);
    let d = if qa.abs() < 1e-12 {
        (-qb > 1e-12).then(|| qc / -qb)?
    } else {
        let disc = qb * qb - 4.0 * qa * qc;
        if disc < 0.0 {
            return None;
        }
        let s = disc.sqrt();
        [(-qb + s) / (2.0 * qa), (-qb - s) / (2.0 * qa)]
            .into_iter()
            .filter(|d| *d > 1e-12)
            .fold(None, |m: Option<f64>, d| Some(m.map_or(d, |m| m.min(d))))?
    };
    let q0 = [c[0][0] + d * t0[0], c[0][1] + d * t0[1]];
    let q1 = [c[3][0] - d * t1[0], c[3][1] - d * t1[1]];
    let j = [(q0[0] + q1[0]) / 2.0, (q0[1] + q1[1]) / 2.0];
    Some((j, bulge_leaving(c[0], j, t0), bulge_arriving(j, c[3], t1)))
}

/// Largest distance, both ways, between the cubic and the two arcs.
fn misfit(c: &[Pt; 4], j: Pt, b1: f64, b2: f64) -> f64 {
    let curve: Vec<Pt> = (0..=SAMPLES)
        .map(|k| cubic_at(c, k as f64 / SAMPLES as f64))
        .collect();
    let to_arcs = curve
        .iter()
        .map(|&p| arc_distance(p, c[0], j, b1).min(arc_distance(p, j, c[3], b2)))
        .fold(0.0, f64::max);
    let to_curve = |p: Pt| {
        curve
            .windows(2)
            .map(|s| arc_distance(p, s[0], s[1], 0.0))
            .fold(f64::INFINITY, f64::min)
    };
    let back = (1..8)
        .flat_map(|k| {
            let s = k as f64 / 8.0;
            [arc_at(c[0], j, b1, s), arc_at(j, c[3], b2, s)]
        })
        .map(to_curve)
        .fold(0.0, f64::max);
    to_arcs.max(back)
}

fn push_cubic(c: &[Pt; 4], tol: f64, depth: usize, out: &mut Vec<(Pt, f64)>) {
    if let Some((j, b1, b2)) = biarc(c) {
        if depth >= MAX_DEPTH || misfit(c, j, b1, b2) <= tol {
            out.push((c[0], b1));
            out.push((j, b2));
            return;
        }
    } else if depth >= MAX_DEPTH {
        out.push((c[0], 0.0));
        return;
    }
    let (l, r) = split(c);
    push_cubic(&l, tol, depth + 1, out);
    push_cubic(&r, tol, depth + 1, out);
}

/// A cubic from `p0` to `p3` as arcs within `tol`: each entry is a start point and the
/// bulge of the arc from it to the next entry's point (or to `p3` after the last).
pub fn cubic(p0: Pt, c1: Pt, c2: Pt, p3: Pt, tol: f64) -> Vec<(Pt, f64)> {
    let mut out = Vec::new();
    push_cubic(&[p0, c1, c2, p3], tol, 0, &mut out);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_quarter_circle_cubic_is_one_or_two_arcs_on_the_circle() {
        // The standard quarter-circle cubic, radius 10 (error 0.027 % of r).
        let k = 0.552_284_75 * 10.0;
        let arcs = cubic([10.0, 0.0], [10.0, k], [k, 10.0], [0.0, 10.0], 0.01);
        assert!(arcs.len() <= 4, "{} arcs", arcs.len());
        assert!(arcs.iter().all(|(_, b)| *b > 0.0), "counter-clockwise");
        let ends: Vec<Pt> = arcs
            .iter()
            .map(|a| a.0)
            .skip(1)
            .chain([[0.0, 10.0]])
            .collect();
        for ((a, b), e) in arcs.iter().zip(&ends) {
            for s in [0.25, 0.5, 0.75] {
                let p = arc_at(*a, *e, *b, s);
                assert!((norm(p) - 10.0).abs() < 0.02, "{p:?} off the circle");
            }
        }
    }

    #[test]
    fn an_s_curve_gets_arcs_of_both_signs_within_the_tolerance() {
        let c = [[0.0, 0.0], [10.0, 10.0], [20.0, -10.0], [30.0, 0.0]];
        let arcs = cubic(c[0], c[1], c[2], c[3], 0.02);
        assert!(arcs.iter().any(|a| a.1 > 0.0) && arcs.iter().any(|a| a.1 < 0.0));
        // Every cubic sample is near some arc.
        let ends: Vec<Pt> = arcs.iter().map(|a| a.0).skip(1).chain([c[3]]).collect();
        for k in 0..=50 {
            let p = cubic_at(&c, k as f64 / 50.0);
            let d = arcs
                .iter()
                .zip(&ends)
                .map(|((a, b), e)| arc_distance(p, *a, *e, *b))
                .fold(f64::INFINITY, f64::min);
            assert!(d < 0.021, "{d} at {p:?}");
        }
    }
}
