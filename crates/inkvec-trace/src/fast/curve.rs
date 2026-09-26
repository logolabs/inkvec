//! Curve-run optimisation (Selinger 2003, section 2.4): replace a run of smooth pieces by
//! fewer cubics where one cubic says the same thing.
//!
//! A run of pieces between two corners may be merged when it turns one way only and by less
//! than a half turn in total; the merged cubic keeps the run's two end points and end
//! tangents, and its two arm lengths are the least-squares fit to points sampled along the
//! pieces. It is accepted when every sample lies within `tol` of it. A dynamic program then
//! takes the fewest cubics over the whole run, as Potrace's `opticurve` does.

use super::smooth::{lerp, Piece};
use inkvec_core::{Point, Vec2};
use inkvec_fit::curves::Segment;

/// The most a merged cubic may turn, in radians: just under a half turn.
const MAX_TURN: f64 = 3.10;
/// The longest run of pieces one merge may cover. Merges this long are already rare; the
/// cap bounds the program on a long wavy boundary.
const MAX_RUN: usize = 24;

fn eval(p: &[Point; 4], t: f64) -> Point {
    let s = 1.0 - t;
    let w = [s * s * s, 3.0 * s * s * t, 3.0 * s * t * t, t * t * t];
    Point::new(
        w[0] * p[0].x + w[1] * p[1].x + w[2] * p[2].x + w[3] * p[3].x,
        w[0] * p[0].y + w[1] * p[1].y + w[2] * p[2].y + w[3] * p[3].y,
    )
}

fn deriv(p: &[Point; 4], t: f64) -> Vec2 {
    let s = 1.0 - t;
    let (a, b, c) = (p[1] - p[0], p[2] - p[1], p[3] - p[2]);
    let (w0, w1, w2) = (3.0 * s * s, 6.0 * s * t, 3.0 * t * t);
    Vec2 {
        x: w0 * a.x + w1 * b.x + w2 * c.x,
        y: w0 * a.y + w1 * b.y + w2 * c.y,
    }
}

fn unit(v: Vec2) -> Option<Vec2> {
    let n = v.norm();
    (n > 1e-9).then(|| Vec2 {
        x: v.x / n,
        y: v.y / n,
    })
}

/// Tangent direction leaving the start of a piece, and arriving at its end.
fn end_tangents(p: &[Point; 4]) -> Option<(Vec2, Vec2)> {
    let t0 = unit(p[1] - p[0])
        .or_else(|| unit(p[2] - p[0]))
        .or_else(|| unit(p[3] - p[0]))?;
    let t1 = unit(p[3] - p[2])
        .or_else(|| unit(p[3] - p[1]))
        .or_else(|| unit(p[3] - p[0]))?;
    Some((t0, t1))
}

/// Signed turn of a piece from its start tangent to its end tangent.
fn turn(p: &[Point; 4]) -> f64 {
    match end_tangents(p) {
        Some((a, b)) => a.cross(b).atan2(a.dot(b)),
        None => 0.0,
    }
}

/// The cubic from `p0` leaving along `t0` to `p3` arriving along `t3` that best fits
/// `samples` (with their chord-length parameters), and its worst sample distance.
pub(super) fn fit(
    p0: Point,
    t0: Vec2,
    p3: Point,
    t3: Vec2,
    samples: &[Point],
) -> Option<([Point; 4], f64)> {
    // Chord-length parameters.
    let mut u = Vec::with_capacity(samples.len());
    let mut acc = 0.0;
    let mut last = p0;
    for &s in samples {
        acc += s.dist(last);
        u.push(acc);
        last = s;
    }
    let total = acc + p3.dist(last);
    if total < 1e-9 {
        return None;
    }
    for x in u.iter_mut() {
        *x /= total;
    }
    let mut curve = [p0, p0, p3, p3];
    for round in 0..3 {
        // Normal equations for the two arm lengths.
        let (mut a11, mut a12, mut a22, mut b1, mut b2) = (0.0, 0.0, 0.0, 0.0, 0.0);
        for (k, &s) in samples.iter().enumerate() {
            let t = u[k];
            let r = 1.0 - t;
            let w = [r * r * r, 3.0 * r * r * t, 3.0 * r * t * t, t * t * t];
            let (ax, ay) = (w[1] * t0.x, w[1] * t0.y);
            let (cx, cy) = (w[2] * t3.x, w[2] * t3.y);
            let base_x = (w[0] + w[1]) * p0.x + (w[2] + w[3]) * p3.x;
            let base_y = (w[0] + w[1]) * p0.y + (w[2] + w[3]) * p3.y;
            let (rx, ry) = (s.x - base_x, s.y - base_y);
            a11 += ax * ax + ay * ay;
            a12 += ax * cx + ay * cy;
            a22 += cx * cx + cy * cy;
            b1 += ax * rx + ay * ry;
            b2 += cx * rx + cy * ry;
        }
        let det = a11 * a22 - a12 * a12;
        if det.abs() < 1e-12 {
            return None;
        }
        let l0 = (b1 * a22 - b2 * a12) / det;
        let l3 = (a11 * b2 - a12 * b1) / det;
        if l0 <= 0.0 || l3 <= 0.0 {
            return None;
        }
        curve = [
            p0,
            Point::new(p0.x + l0 * t0.x, p0.y + l0 * t0.y),
            Point::new(p3.x + l3 * t3.x, p3.y + l3 * t3.y),
            p3,
        ];
        // Reparameterise each sample onto the curve (one Newton step), then refit.
        for (k, &s) in samples.iter().enumerate() {
            let t = u[k];
            let q = eval(&curve, t);
            let d = deriv(&curve, t);
            let dd = d.dot(d);
            if dd > 1e-12 {
                u[k] = (t + ((s - q).dot(d)) / dd).clamp(0.0, 1.0);
            }
        }
        if round == 2 {
            break;
        }
    }
    let err = samples
        .iter()
        .zip(&u)
        .map(|(&s, &t)| s.dist(eval(&curve, t)))
        .fold(0.0f64, f64::max);
    Some((curve, err))
}

/// Largest distance from `samples` to `curve`, each sample placed by its chord-length
/// parameter and two Newton steps.
pub(super) fn max_error(curve: &[Point; 4], samples: &[Point]) -> f64 {
    let mut acc = 0.0;
    let mut last = curve[0];
    let mut u: Vec<f64> = samples
        .iter()
        .map(|&s| {
            acc += s.dist(last);
            last = s;
            acc
        })
        .collect();
    let total = acc + curve[3].dist(last);
    if total < 1e-9 {
        return 0.0;
    }
    let mut worst = 0.0f64;
    for (t, &s) in u.iter_mut().zip(samples) {
        *t /= total;
        for _ in 0..2 {
            let q = eval(curve, *t);
            let d = deriv(curve, *t);
            let dd = d.dot(d);
            if dd > 1e-12 {
                *t = (*t + (s - q).dot(d) / dd).clamp(0.0, 1.0);
            }
        }
        worst = worst.max(s.dist(eval(curve, *t)));
    }
    worst
}

/// Points sampled along pieces `run`, excluding the run's own two ends.
fn samples(run: &[Piece]) -> Vec<Point> {
    let mut out = Vec::with_capacity(run.len() * 4);
    for (k, pc) in run.iter().enumerate() {
        for t in [0.25, 0.5, 0.75] {
            out.push(eval(&pc.p, t));
        }
        if k + 1 < run.len() {
            out.push(pc.p[3]);
        }
    }
    out
}

/// One cubic for the whole of `run`, when it turns one way, less than `MAX_TURN`, and fits.
fn merge(run: &[Piece], tol: f64) -> Option<[Point; 4]> {
    let (t0, _) = end_tangents(&run[0].p)?;
    let (_, t3) = end_tangents(&run[run.len() - 1].p)?;
    let p0 = run[0].p[0];
    let p3 = run[run.len() - 1].p[3];
    let back = Vec2 { x: -t3.x, y: -t3.y };
    let (c, err) = fit(p0, t0, p3, back, &samples(run))?;
    (err <= tol).then_some(c)
}

/// Fewest cubics for one run of smooth pieces.
fn optimise_run(run: &[Piece], tol: f64) -> Vec<[Point; 4]> {
    let n = run.len();
    let turns: Vec<f64> = run.iter().map(|p| turn(&p.p)).collect();
    let mut best: Vec<(usize, usize)> = vec![(usize::MAX, 0); n + 1];
    best[0] = (0, 0);
    let mut cache: std::collections::HashMap<(usize, usize), [Point; 4]> =
        std::collections::HashMap::new();
    for i in 0..n {
        if best[i].0 == usize::MAX {
            continue;
        }
        let (mut total, mut sign) = (0.0f64, 0.0f64);
        for j in i..n.min(i + MAX_RUN) {
            let tj = turns[j];
            if tj.abs() > 1e-6 {
                if sign == 0.0 {
                    sign = tj.signum();
                } else if tj.signum() != sign {
                    break;
                }
            }
            total += tj.abs();
            if total > MAX_TURN {
                break;
            }
            let cand = best[i].0 + 1;
            if cand >= best[j + 1].0 {
                continue;
            }
            let curve = if j == i {
                Some(run[i].p)
            } else {
                merge(&run[i..=j], tol)
            };
            if let Some(c) = curve {
                best[j + 1] = (cand, i);
                cache.insert((i, j), c);
            }
        }
    }
    let mut out = Vec::new();
    let mut k = n;
    while k > 0 {
        let i = best[k].1;
        out.push(cache.get(&(i, k - 1)).copied().unwrap_or(run[k - 1].p));
        k = i;
    }
    out.reverse();
    out
}

/// Merge the pieces of one boundary. A closed boundary is rotated to start at a corner when
/// it has one, so no run is cut in two by where the ring happens to start.
pub(crate) fn optimise(pieces: &[Piece], closed: bool, tol: f64) -> Vec<[Point; 4]> {
    let n = pieces.len();
    if n == 0 {
        return Vec::new();
    }
    let start = if closed {
        pieces.iter().position(|p| !p.smooth_in).unwrap_or(0)
    } else {
        0
    };
    let order: Vec<Piece> = (0..n).map(|k| pieces[(start + k) % n]).collect();
    let mut out = Vec::with_capacity(n);
    let mut s = 0;
    while s < n {
        let mut e = s + 1;
        while e < n && order[e].smooth_in {
            e += 1;
        }
        out.extend(optimise_run(&order[s..e], tol));
        s = e;
    }
    out
}

/// Distance from `p` to the segment `a`-`b`, and where along it `p` projects (0 to 1).
fn to_chord(p: Point, a: Point, b: Point) -> (f64, f64) {
    let d = b - a;
    let l2 = d.dot(d);
    if l2 < 1e-18 {
        return (p.dist(a), 0.0);
    }
    let t = (p - a).dot(d) / l2;
    let q = lerp(a, b, t.clamp(0.0, 1.0));
    (p.dist(q), t)
}

/// The cubics as path segments: a cubic whose control points lie on its chord is a line,
/// and consecutive lines along one direction are one line.
pub(crate) fn to_segments(curves: &[[Point; 4]], flat: f64) -> Vec<Segment> {
    let mut out: Vec<(Point, Segment)> = Vec::with_capacity(curves.len());
    for c in curves {
        let (d1, t1) = to_chord(c[1], c[0], c[3]);
        let (d2, t2) = to_chord(c[2], c[0], c[3]);
        let straight = d1 <= flat
            && d2 <= flat
            && (-0.01..=1.01).contains(&t1)
            && (-0.01..=1.01).contains(&t2);
        let seg = if straight {
            Segment::Line(c[3])
        } else {
            Segment::Cubic(c[1], c[2], c[3])
        };
        if let (Segment::Line(end), Some((start, Segment::Line(prev_end)))) = (&seg, out.last()) {
            let (d, t) = to_chord(*prev_end, *start, *end);
            if d <= flat && t > 0.0 && t < 1.0 {
                let (s, e) = (*start, *end);
                out.pop();
                out.push((s, Segment::Line(e)));
                continue;
            }
        }
        out.push((c[0], seg));
    }
    out.into_iter().map(|(_, s)| s).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn arc_pieces(n: usize, sweep: f64) -> Vec<Piece> {
        // n exact quarter-ish arcs of a radius-20 circle, each as a cubic.
        let r = 20.0;
        let k = 4.0 / 3.0 * (sweep / n as f64 / 4.0).tan();
        (0..n)
            .map(|i| {
                let a0 = sweep * i as f64 / n as f64;
                let a1 = sweep * (i + 1) as f64 / n as f64;
                let (c0, s0, c1, s1) = (a0.cos(), a0.sin(), a1.cos(), a1.sin());
                Piece {
                    p: [
                        Point::new(r * c0, r * s0),
                        Point::new(r * (c0 - k * s0), r * (s0 + k * c0)),
                        Point::new(r * (c1 + k * s1), r * (s1 - k * c1)),
                        Point::new(r * c1, r * s1),
                    ],
                    smooth_in: i > 0,
                }
            })
            .collect()
    }

    #[test]
    fn a_quarter_circle_in_six_pieces_becomes_one_cubic() {
        let out = optimise(&arc_pieces(6, std::f64::consts::FRAC_PI_2), false, 0.2);
        assert_eq!(out.len(), 1);
    }

    #[test]
    fn a_full_circle_needs_more_than_two_cubics() {
        let mut p = arc_pieces(12, std::f64::consts::TAU);
        p[0].smooth_in = true;
        let out = optimise(&p, true, 0.2);
        assert!((3..=4).contains(&out.len()), "{}", out.len());
    }

    #[test]
    fn an_s_curve_is_not_merged_into_one() {
        let mut a = arc_pieces(2, 1.0);
        let b: Vec<Piece> = arc_pieces(2, 1.0)
            .into_iter()
            .map(|mut pc| {
                // Mirror to turn the other way, continuing from the first arc's end.
                for q in pc.p.iter_mut() {
                    *q = Point::new(q.x, -q.y);
                }
                pc
            })
            .collect();
        a.extend(b);
        for pc in a.iter_mut().skip(1) {
            pc.smooth_in = true;
        }
        let turns: Vec<f64> = a.iter().map(|p| turn(&p.p)).collect();
        assert!(turns[0] * turns[3] < 0.0);
        let out = optimise_run(&a, 10.0);
        assert!(out.len() >= 2);
    }

    #[test]
    fn collinear_lines_become_one_line() {
        let l = |a: Point, b: Point| [a, lerp(a, b, 1.0 / 3.0), lerp(a, b, 2.0 / 3.0), b];
        let (a, b, c) = (
            Point::new(0.0, 0.0),
            Point::new(5.0, 5.0),
            Point::new(10.0, 10.0),
        );
        let segs = to_segments(&[l(a, b), l(b, c)], 0.05);
        assert_eq!(segs.len(), 1);
        assert!(matches!(segs[0], Segment::Line(e) if e == c));
    }
}
