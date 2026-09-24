//! Polygon to lines and cubics within a tolerance: Schneider's algorithm.
//!
//! P. J. Schneider, "An Algorithm for Automatically Fitting Digitized Curves", Graphics
//! Gems (1990) — the method most vector editors use to simplify a path. The contour is cut
//! at corners; a run that stays within the tolerance of its chord is one line; any other
//! run gets one cubic by least squares with its end tangents fixed, reparameterised by
//! Newton steps, and is split at its worst point and fitted again as two when it still
//! misses. It is linear-logarithmic and has no input on which it fails to finish.
//!
//! The tracer's own fitter (MDL over lines, cubics and arcs) was tried first and is the
//! better fitter on what it was built for, dense pixel boundaries. On boolean-operation
//! output (long straight edges, sparse vertices) its cubic stage subdivides without end.

use crate::geom::Pt;

/// One output segment, from the previous segment's end.
#[derive(Clone, Copy, Debug)]
pub enum Seg {
    /// A line to this point.
    Line(Pt),
    /// A cubic: two controls, then the end.
    Cubic(Pt, Pt, Pt),
}

/// Turn at a vertex, in degrees, above which it is a corner.
pub const CORNER_DEGREES: f64 = 40.0;

fn sub(a: Pt, b: Pt) -> Pt {
    [a[0] - b[0], a[1] - b[1]]
}
fn add(a: Pt, b: Pt) -> Pt {
    [a[0] + b[0], a[1] + b[1]]
}
fn mul(a: Pt, s: f64) -> Pt {
    [a[0] * s, a[1] * s]
}
fn dot(a: Pt, b: Pt) -> f64 {
    a[0] * b[0] + a[1] * b[1]
}
fn len(a: Pt) -> f64 {
    a[0].hypot(a[1])
}
fn unit(a: Pt) -> Pt {
    let l = len(a);
    if l > 0.0 {
        mul(a, 1.0 / l)
    } else {
        [0.0, 0.0]
    }
}

fn bez(p: &[Pt; 4], t: f64) -> Pt {
    let u = 1.0 - t;
    let (b0, b1, b2, b3) = (u * u * u, 3.0 * u * u * t, 3.0 * u * t * t, t * t * t);
    [
        b0 * p[0][0] + b1 * p[1][0] + b2 * p[2][0] + b3 * p[3][0],
        b0 * p[0][1] + b1 * p[1][1] + b2 * p[2][1] + b3 * p[3][1],
    ]
}

fn bez_d1(p: &[Pt; 4], t: f64) -> Pt {
    let u = 1.0 - t;
    add(
        add(
            mul(sub(p[1], p[0]), 3.0 * u * u),
            mul(sub(p[2], p[1]), 6.0 * u * t),
        ),
        mul(sub(p[3], p[2]), 3.0 * t * t),
    )
}

fn bez_d2(p: &[Pt; 4], t: f64) -> Pt {
    let u = 1.0 - t;
    add(
        mul(add(sub(p[2], mul(p[1], 2.0)), p[0]), 6.0 * u),
        mul(add(sub(p[3], mul(p[2], 2.0)), p[1]), 6.0 * t),
    )
}

/// Fit a closed contour. Returns the start point and the segments back to it.
pub fn fit_closed(c: &[Pt], tol: f64) -> (Pt, Vec<Seg>) {
    let n = c.len();
    if n < 3 {
        return (c.first().copied().unwrap_or([0.0, 0.0]), Vec::new());
    }
    let turn = |i: usize| -> f64 {
        let (a, b, d) = (c[(i + n - 1) % n], c[i], c[(i + 1) % n]);
        let (u, v) = (unit(sub(b, a)), unit(sub(d, b)));
        dot(u, v).clamp(-1.0, 1.0).acos().to_degrees()
    };
    let corner_cos = CORNER_DEGREES;
    let mut corners: Vec<usize> = (0..n).filter(|&i| turn(i) > corner_cos).collect();
    if corners.is_empty() {
        // A smooth loop: break it at its sharpest vertex and let the fit find the rest.
        let sharpest = (0..n)
            .max_by(|&a, &b| turn(a).total_cmp(&turn(b)))
            .unwrap_or(0);
        corners.push(sharpest);
    }
    let start = c[corners[0]];
    let mut out = Vec::new();
    for w in 0..corners.len() {
        let (i, j) = (corners[w], corners[(w + 1) % corners.len()]);
        let mut run: Vec<Pt> = Vec::new();
        let mut k = i;
        loop {
            run.push(c[k]);
            if k == j && run.len() > 1 {
                break;
            }
            k = (k + 1) % n;
            if k == i && run.len() > 1 {
                run.push(c[k]);
                break;
            }
        }
        fit_run(&run, tol, &mut out);
    }
    (start, out)
}

/// Fit an open run of points from `run[0]` to its last point.
fn fit_run(run: &[Pt], tol: f64, out: &mut Vec<Seg>) {
    let last = run[run.len() - 1];
    if run.len() <= 2 || straight(run, tol) {
        out.push(Seg::Line(last));
        return;
    }
    let t1 = unit(sub(run[1], run[0]));
    let t2 = unit(sub(run[run.len() - 2], last));
    fit_cubic(run, t1, t2, tol, out, 0);
}

fn straight(run: &[Pt], tol: f64) -> bool {
    let (a, b) = (run[0], run[run.len() - 1]);
    let d = sub(b, a);
    let l = len(d);
    if l <= 0.0 {
        return run.iter().all(|p| len(sub(*p, a)) <= tol);
    }
    run.iter()
        .all(|p| ((p[0] - a[0]) * d[1] - (p[1] - a[1]) * d[0]).abs() / l <= tol)
}

fn chord_params(run: &[Pt]) -> Vec<f64> {
    let mut u = vec![0.0; run.len()];
    for i in 1..run.len() {
        u[i] = u[i - 1] + len(sub(run[i], run[i - 1]));
    }
    let total = u[run.len() - 1].max(1e-12);
    u.iter_mut().for_each(|x| *x /= total);
    u
}

fn least_squares(run: &[Pt], u: &[f64], t1: Pt, t2: Pt) -> [Pt; 4] {
    let (p0, p3) = (run[0], run[run.len() - 1]);
    let (mut c00, mut c01, mut c11, mut x0, mut x1) = (0.0, 0.0, 0.0, 0.0, 0.0);
    for (i, &t) in u.iter().enumerate() {
        let s = 1.0 - t;
        let (b0, b1, b2, b3) = (s * s * s, 3.0 * s * s * t, 3.0 * s * t * t, t * t * t);
        let (a1, a2) = (mul(t1, b1), mul(t2, b2));
        c00 += dot(a1, a1);
        c01 += dot(a1, a2);
        c11 += dot(a2, a2);
        let tmp = sub(run[i], add(mul(p0, b0 + b1), mul(p3, b2 + b3)));
        x0 += dot(a1, tmp);
        x1 += dot(a2, tmp);
    }
    let det = c00 * c11 - c01 * c01;
    let seg = len(sub(p3, p0));
    let (mut al, mut ar) = if det.abs() > 1e-12 {
        ((x0 * c11 - x1 * c01) / det, (c00 * x1 - c01 * x0) / det)
    } else {
        (seg / 3.0, seg / 3.0)
    };
    if al < 1e-6 * seg || ar < 1e-6 * seg {
        al = seg / 3.0;
        ar = seg / 3.0;
    }
    [p0, add(p0, mul(t1, al)), add(p3, mul(t2, ar)), p3]
}

fn max_error(run: &[Pt], u: &[f64], b: &[Pt; 4]) -> (f64, usize) {
    let mut worst = (0.0, run.len() / 2);
    for i in 1..run.len() - 1 {
        let e = len(sub(bez(b, u[i]), run[i]));
        if e > worst.0 {
            worst = (e, i);
        }
    }
    worst
}

fn reparameterise(run: &[Pt], u: &mut [f64], b: &[Pt; 4]) {
    for (i, t) in u.iter_mut().enumerate() {
        let d = sub(bez(b, *t), run[i]);
        let (d1, d2) = (bez_d1(b, *t), bez_d2(b, *t));
        let den = dot(d1, d1) + dot(d, d2);
        if den.abs() > 1e-12 {
            *t = (*t - dot(d, d1) / den).clamp(0.0, 1.0);
        }
    }
}

fn fit_cubic(run: &[Pt], t1: Pt, t2: Pt, tol: f64, out: &mut Vec<Seg>, depth: usize) {
    let last = run[run.len() - 1];
    if run.len() <= 2 {
        out.push(Seg::Line(last));
        return;
    }
    if straight(run, tol) {
        out.push(Seg::Line(last));
        return;
    }
    let mut u = chord_params(run);
    let mut b = least_squares(run, &u, t1, t2);
    let (mut err, mut split) = max_error(run, &u, &b);
    if err <= tol {
        out.push(Seg::Cubic(b[1], b[2], b[3]));
        return;
    }
    if err <= 4.0 * tol {
        for _ in 0..4 {
            reparameterise(run, &mut u, &b);
            b = least_squares(run, &u, t1, t2);
            (err, split) = max_error(run, &u, &b);
            if err <= tol {
                out.push(Seg::Cubic(b[1], b[2], b[3]));
                return;
            }
        }
    }
    if depth > 24 || run.len() < 4 {
        // Nothing left to split sensibly: the points themselves, as lines.
        for p in &run[1..] {
            out.push(Seg::Line(*p));
        }
        return;
    }
    let split = split.clamp(1, run.len() - 2);
    let tc = unit(sub(run[split - 1], run[split + 1]));
    fit_cubic(&run[..=split], t1, tc, tol, out, depth + 1);
    fit_cubic(&run[split..], mul(tc, -1.0), t2, tol, out, depth + 1);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_circle_is_a_few_cubics_and_a_square_is_four_lines() {
        let circle: Vec<Pt> = (0..720)
            .map(|i| {
                let t = i as f64 / 720.0 * std::f64::consts::TAU;
                [50.0 + 20.0 * t.cos(), 50.0 + 20.0 * t.sin()]
            })
            .collect();
        let (_, segs) = fit_closed(&circle, 0.05);
        assert!(
            !segs.is_empty() && segs.len() <= 8,
            "{} segments",
            segs.len()
        );
        let square = vec![[0.0, 0.0], [10.0, 0.0], [10.0, 10.0], [0.0, 10.0]];
        let (_, segs) = fit_closed(&square, 0.05);
        assert_eq!(segs.len(), 4);
        assert!(segs.iter().all(|s| matches!(s, Seg::Line(_))));
    }
}
