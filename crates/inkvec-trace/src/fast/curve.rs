//! Curve-run optimisation (Selinger 2003, section 2.4): replace a run of smooth pieces by
//! fewer cubics where one cubic says the same thing.
//!
//! A run of pieces between two corners may be merged when it turns one way only and by less
//! than a half turn in total; the merged cubic keeps the run's two end points and end
//! tangents, and its two arm lengths are the least-squares fit to points sampled along the
//! pieces. It is accepted when every sample lies within `tol` of it. A dynamic program then
//! takes the fewest cubics over the whole run, as Potrace's `opticurve` does.
//!
//! Stage 5c, the last of the Fast fit, run by [`super::fit_points`] after
//! [`super::smooth::pieces`]. In: a boundary's pieces, in px. Out: its cubics
//! ([`optimise`]) and then its path segments, with near-straight cubics written as lines
//! ([`to_segments`]). The tangent-constrained cubic fit [`fit`] is also what
//! `smooth::pieces` uses to place each vertex's curve.

use super::smooth::{lerp, Piece};
use inkvec_core::{Point, Vec2};
use inkvec_fit::curves::Segment;

/// The most a merged cubic may turn, in radians: just under a half turn.
const MAX_TURN: f64 = 3.10;
/// The longest run of pieces one merge may cover. Merges this long are already rare; the
/// cap bounds the program on a long wavy boundary.
const MAX_RUN: usize = 24;

/// The cubic Bézier `B(t) = (1−t)³ P0 + 3(1−t)² t P1 + 3(1−t) t² P2 + t³ P3` at `t`.
fn eval(p: &[Point; 4], t: f64) -> Point {
    let s = 1.0 - t;
    let w = [s * s * s, 3.0 * s * s * t, 3.0 * s * t * t, t * t * t];
    Point::new(
        w[0] * p[0].x + w[1] * p[1].x + w[2] * p[2].x + w[3] * p[3].x,
        w[0] * p[0].y + w[1] * p[1].y + w[2] * p[2].y + w[3] * p[3].y,
    )
}

/// Its derivative `B'(t) = 3(1−t)² (P1−P0) + 6(1−t) t (P2−P1) + 3 t² (P3−P2)`.
fn deriv(p: &[Point; 4], t: f64) -> Vec2 {
    let s = 1.0 - t;
    let (a, b, c) = (p[1] - p[0], p[2] - p[1], p[3] - p[2]);
    let (w0, w1, w2) = (3.0 * s * s, 6.0 * s * t, 3.0 * t * t);
    Vec2 {
        x: w0 * a.x + w1 * b.x + w2 * c.x,
        y: w0 * a.y + w1 * b.y + w2 * c.y,
    }
}

/// `v / |v|`, or `None` for a vector shorter than 1e-9.
fn unit(v: Vec2) -> Option<Vec2> {
    let n = v.norm();
    (n > 1e-9).then(|| Vec2 {
        x: v.x / n,
        y: v.y / n,
    })
}

/// Tangent direction leaving the start of a piece, and arriving at its end, as unit
/// vectors. A control point that coincides with its end point falls back to the next
/// control point, then the far end; `None` only for a piece of zero length.
fn end_tangents(p: &[Point; 4]) -> Option<(Vec2, Vec2)> {
    let t0 = unit(p[1] - p[0])
        .or_else(|| unit(p[2] - p[0]))
        .or_else(|| unit(p[3] - p[0]))?;
    let t1 = unit(p[3] - p[2])
        .or_else(|| unit(p[3] - p[1]))
        .or_else(|| unit(p[3] - p[0]))?;
    Some((t0, t1))
}

/// Signed turn of a piece from its start tangent `a` to its end tangent `b`, in radians:
/// `atan2(a × b, a · b)`, in (−π, π], positive towards increasing angle in image
/// coordinates. 0 for a zero-length piece.
fn turn(p: &[Point; 4]) -> f64 {
    match end_tangents(p) {
        Some((a, b)) => a.cross(b).atan2(a.dot(b)),
        None => 0.0,
    }
}

/// The cubic from `p0` leaving along `t0` to `p3` that best fits `samples` (with their
/// chord-length parameters), and its worst sample distance. `t3` is the unit direction
/// from `p3` *back* towards its control point -- the reverse of the arrival tangent --
/// so both arms are `P1 = p0 + l0 t0`, `P2 = p3 + l3 t3` with positive lengths.
///
/// Schneider's Bézier fit with fixed end tangents. With `u_k` the parameter of sample
/// `s_k` and `b_i(u)` the Bernstein weights, the residual is linear in the two arm lengths:
///
/// `s_k − [(b0+b1) p0 + (b2+b3) p3] = l0 b1 t0 + l3 b2 t3 + e_k`
///
/// and `Σ |e_k|²` is minimised through the 2 × 2 normal equations, solved by Cramer's rule.
/// Parameters start as normalised chord length (the polyline `p0, s_1, …, s_m, p3`) and
/// are improved by one Newton step on `(B(u) − s) · B'(u) = 0` after each of three
/// solves. Returns `None` when the samples span no length, the system is singular, or an
/// arm comes out non-positive (the tangents cannot be honoured), so the caller falls back
/// to a corner or keeps the pieces. The error is the largest distance `|s_k − B(u_k)|` in
/// px.
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
/// parameter and two Newton steps. An approximation of the true point-to-curve distance
/// from above (a sample's nearest point could lie at another parameter), in px; 0 when the
/// samples and the curve's ends span no length.
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

/// Points sampled along pieces `run`, excluding the run's own two ends: each piece at
/// `t = ¼, ½, ¾`, and every join between two pieces, in order along the run.
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
///
/// The turn conditions are checked by the caller ([`optimise_run`]); this keeps the run's
/// end points and end tangents, fits the arm lengths to [`samples`] of the pieces
/// ([`fit`]) and accepts the cubic when no sample is more than `tol` px from it.
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
///
/// Dynamic programming over piece boundaries: `best[j+1]` is the fewest cubics that draw
/// pieces `0..=j`, `best[j+1] = min_i best[i] + 1` over every `i` for which `i == j` (a
/// piece alone), or pieces `i..=j` all turn the same way (pieces turning less than 1e-6 rad
/// count as either), turn at most `MAX_TURN` in total, number at most `MAX_RUN`, and
/// [`merge`] into one cubic within `tol`. On a tie the smallest `i` is kept. The chosen
/// cubics are read back from the last piece. `whole_ring` says the run is a whole closed
/// ring (a ring with at most one corner is one run); then the one candidate that covers
/// every piece, `i = 0, j = n − 1`, is not tried, so a ring is never one cubic.
///
/// **A single piece is always its own cubic, whatever it turns.** This invariant is what
/// makes every prefix reachable, and it once failed: the piece alone was tried only after
/// the turn test, so a piece turning more than `MAX_TURN` -- a U-turn at a vertex whose
/// sides double back, |turn| ≈ π > 3.10 -- left `best[j+1]` unreachable, every later
/// prefix with it, and the read-back then started from an unset `best[n] = (MAX, 0)`: it
/// returned the run's last piece alone, as if it were the whole run. On a ring that is
/// one cubic, which `super::fit_points` then closes on itself, and the face is not drawn
/// (`synthetic/gradient_radial` at 512 px, Fast with the boundary solve on, research
/// r2-fastq). Now the piece alone is admitted before the turn test, and only merges of two
/// or more pieces are held to it. Where every piece turns at most `MAX_TURN` and no whole
/// ring merges into one cubic -- every boundary the old code drew correctly -- the loop
/// makes the same choices in the same order, so those fits are unchanged bit for bit.
///
/// The ring guard is the same defect's other door: a run that is the whole ring starts
/// and ends at the same join, and one cubic with both ends there can only draw a loop or a
/// sliver. A ring always turns a full turn, so with honest turns `MAX_TURN` already rules
/// that merge out; the guard keeps it out when the measured turns do not add up (each is
/// read in (−π, π], so a piece that turns further reads short).
///
/// Method from: Selinger, P. (2003), "Potrace: a polygon-based tracing algorithm",
/// <https://potrace.sourceforge.net/potrace.pdf>, section 2.4 (`opticurve`), the fewest
/// curves over runs of consistent convexity and less than a half turn. Adapted: Potrace's
/// own program has no read-back failure because its polygon never doubles back; here the
/// vertices are moved off the points by `smooth::adjust_vertices` with loosened boxes, and
/// can.
///
/// Cost: O(n · MAX_RUN) candidate runs, each [`merge`] O(run length); n = pieces in the
/// run (one per polygon vertex, two at a corner).
fn optimise_run(run: &[Piece], tol: f64, whole_ring: bool) -> Vec<[Point; 4]> {
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
        let cand = best[i].0 + 1;
        // The piece alone, before any turn test: see the invariant above.
        if cand < best[i + 1].0 {
            best[i + 1] = (cand, i);
            cache.insert((i, i), run[i].p);
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
            // `j == i` was admitted above; the whole ring is never one cubic.
            if j == i || (whole_ring && i == 0 && j + 1 == n) {
                continue;
            }
            if cand >= best[j + 1].0 {
                continue;
            }
            if let Some(c) = merge(&run[i..=j], tol) {
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
///
/// Runs are maximal stretches of pieces joined smoothly (`smooth_in`); each is optimised
/// on its own by [`optimise_run`], so a corner is never smoothed over. A ring with no
/// corner is one run starting at piece 0, and so is a ring with one corner (from the
/// corner round to it); such a run is the whole ring and is never merged into one cubic,
/// so a closed boundary of two or more pieces comes back as two or more cubics.
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
        let whole_ring = closed && s == 0 && e == n;
        out.extend(optimise_run(&order[s..e], tol, whole_ring));
        s = e;
    }
    out
}

/// Distance from `p` to the segment `a`-`b`, and where along it `p` projects (0 to 1).
/// The distance uses the projection clamped to the segment; the returned parameter is
/// unclamped, so callers can tell a point beyond an end. A degenerate segment gives the
/// distance to `a` and parameter 0.
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
///
/// A cubic is straight when both control points lie within `flat` px of the chord and
/// project inside it (parameter within −0.01..1.01, so a control point beyond an end --
/// a hook -- keeps the cubic). A line is merged into the line before it when their shared
/// point lies within `flat` px of the merged chord and strictly inside it. The segments
/// continue from `curves[0][0]`, which the caller writes as the path's start.
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
        let out = optimise_run(&a, 10.0, false);
        assert!(out.len() >= 2);
    }

    /// A smooth piece that doubles back on itself: leaves `from` along +x and arrives at
    /// `from + (0, 2)` along −x, a U-turn whose measured turn is π, more than `MAX_TURN`.
    fn u_turn(from: Point) -> Piece {
        Piece {
            p: [
                from,
                Point::new(from.x + 6.0, from.y),
                Point::new(from.x + 6.0, from.y + 2.0),
                Point::new(from.x, from.y + 2.0),
            ],
            smooth_in: true,
        }
    }

    /// The repro of the ring collapse: a piece turning more than `MAX_TURN` made every
    /// later prefix unreachable, and the read-back returned the run's last piece alone. A
    /// piece is always its own cubic: a quarter arc, a U-turn and another piece come back
    /// as three cubics, in order, from the first piece's start to the last one's end.
    #[test]
    fn a_piece_turning_more_than_the_limit_is_still_its_own_cubic() {
        let mut run = arc_pieces(2, std::f64::consts::FRAC_PI_2);
        let end = run[1].p[3];
        assert!(turn(&u_turn(end).p).abs() > MAX_TURN);
        run.push(u_turn(end));
        let back = run[2].p[3];
        run.push(Piece {
            p: [
                back,
                Point::new(back.x - 1.0, back.y),
                Point::new(back.x - 2.0, back.y),
                Point::new(back.x - 3.0, back.y),
            ],
            smooth_in: true,
        });
        let out = optimise_run(&run, 0.2, false);
        assert!(out.len() >= 3, "{} cubics: {out:?}", out.len());
        assert_eq!(out[0][0], run[0].p[0]);
        assert_eq!(out[out.len() - 1][3], run[3].p[3]);
        for k in 1..out.len() {
            assert_eq!(
                out[k][0],
                out[k - 1][3],
                "cubic {k} does not start where {} ends",
                k - 1
            );
        }
    }

    /// The ring guard: two U-turns make a closed ring (each piece the other's way back),
    /// which must come back as two cubics, never one. Before the fix it came back as one.
    #[test]
    fn a_closed_ring_is_never_one_cubic() {
        let a = u_turn(Point::new(0.0, 0.0));
        // The way back: from (0, 2) along −x round to (0, 0) arriving along +x.
        let b = Piece {
            p: [
                Point::new(0.0, 2.0),
                Point::new(-6.0, 2.0),
                Point::new(-6.0, 0.0),
                Point::new(0.0, 0.0),
            ],
            smooth_in: true,
        };
        let out = optimise(&[a, b], true, 0.2);
        assert_eq!(out.len(), 2, "{out:?}");
        assert_eq!(out[0][0], out[1][3]);
        // Within the turn limit a ring's pieces can still be many: a full circle in
        // twelve pieces is three or four cubics, as before (see the test above), and the
        // whole-ring merge, if the turns ever allowed it, is not tried.
        let mut circle = arc_pieces(12, std::f64::consts::TAU);
        circle[0].smooth_in = true;
        for (i, pc) in circle.iter().enumerate() {
            assert!(turn(&pc.p).abs() <= MAX_TURN, "piece {i}");
        }
        assert!(optimise(&circle, true, 0.2).len() >= 2);
        // The guard itself: a run that would merge into one cubic is kept to two when it
        // is the whole ring.
        let quarter = arc_pieces(2, std::f64::consts::FRAC_PI_2);
        assert_eq!(optimise_run(&quarter, 0.2, false).len(), 1);
        assert_eq!(optimise_run(&quarter, 0.2, true).len(), 2);
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
