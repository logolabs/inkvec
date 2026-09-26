//! Replace short runs of segments with one cubic whose end tangents are free.
//!
//! # Why this exists
//!
//! Every rounded corner of a traced icon came out as a short chord, a cubic, then another
//! chord — three segments and ten parameters where one cubic costs six, with a visible
//! kink at each join where the straight edge meets the curve.
//!
//! The dynamic program was not making a mistake. A cubic in that program is **G1**: its
//! end tangent directions are inherited from the per-vertex tangent estimator and only the
//! two arm lengths are fitted. That constraint buys nothing in parameters — a cubic is six
//! numbers either way — it exists so the state can be a vertex index alone, which is what
//! makes the residual of any candidate span O(1) from prefix sums. At a corner the
//! inherited tangent is wrong, and the program pays four extra parameters to avoid using
//! it.
//!
//! Measured on the four corners of a traced rounded square, same span and same objective:
//!
//! ```text
//!                              chi2     cost
//!   G1, tangents inherited    284-303   174-183
//!   free tangents              95-101    80-83
//!   the split it chose        111-115   109-111
//! ```
//!
//! So a free cubic beats the split by about 25% and beats the constrained cubic by more
//! than half. The estimator's own bias at a span end is only about 14 degrees, while the
//! fit wants 40 to 55, so correcting the estimate would not have closed this.
//!
//! # Why a pass rather than a candidate in the program
//!
//! Fitting free tangents costs O(span) — the residual is no longer a difference of prefix
//! sums — which would make the program O(n^3). Run afterwards over short runs, the same
//! fit is cheap and touches exactly the case it was built for. It is a peephole
//! optimisation on the result, not a change to the search.
//!
//! # What keeps it honest
//!
//! A free cubic breaks G1 with its neighbours, so it is charged `BREAK_PARAMS`
//! parameters for the two joins it disturbs, and only replaces a run when the same
//! `0.5·chi² + λ·params` that chose the run prefers it. It is also refused if the cubic
//! crosses itself.

use inkvec_core::{Point, Polyline, Vec2};

use crate::curves::{cubic_self_intersects, eval_cubic, Segment};
use crate::{FitConfig, FittedPath, PARAMS_LINE};

/// Charged for the two joins a free-tangent cubic no longer meets smoothly.
///
/// One parameter per join, the same price the program puts on a tangent break, so a merge
/// has to be worth more than the smoothness it gives up rather than merely fitting better.
const BREAK_PARAMS: f64 = 2.0;

/// Longest run, in measured points, worth attempting. Bounds the pass at O(n · span) and
/// keeps it to the short runs that corners produce.
const MAX_SPAN: usize = 96;

/// Most segments a single cubic may absorb in one round.
///
/// This and [`MAX_ROUNDS`] were once overridable (`INKVEC_MERGE_RUN`,
/// `INKVEC_MERGE_ROUNDS`), but no sweep ever moved either, so they are plain constants
/// again; `SMOOTH_SLACK` below is the merge knob that was actually measured.
const MAX_RUN: usize = 4;

/// Rounds to run the pass for.
///
/// A peephole optimisation that is not run to a fixpoint leaves work on the table, and
/// here it left a lot: the pass advances past whatever it has just merged, so a smooth
/// boundary paved with a dozen two-pixel chords could become three cubics but never one.
/// Traced output was 21% curved where the ground truth is 78%, and the short fragments are
/// most of the difference: lines outnumber cubics 3.6 to 1 while covering only 1.26 times
/// the distance, with a median length of 2.25px.
const MAX_ROUNDS: usize = 6;

/// How much worse, in parameters, a merged curve may score and still be taken.
///
/// In units of lambda; zero leaves the objective in charge. It was the `INKVEC_SMOOTH`
/// experiment, and the default never moved.
const SMOOTH_SLACK: f64 = 0.0;

/// Points sampled along a candidate when measuring its residual.
const SAMPLES: usize = 96;

/// How far the end tangents may swing from the contour's own direction, in degrees.
///
/// Wide on purpose. The search is centred on a two-point chord at each end, which lags the
/// true tangent on a curve, and the fits that matter want 40 to 55 degrees away from the
/// *estimator's* tangent — further still from the chord. A 60-degree clamp put the optimum
/// outside the search and left the fit at chi-squared 172 where it should reach 101.
const SEARCH_DEGREES: f64 = 100.0;

/// Samples used while ranking grid candidates. The winner is then re-scored in full.
const COARSE_SAMPLES: usize = 24;

/// An arm longer than the chord describes more than a half turn.
use crate::multimodel::MAX_ARM;

/// Weighted sum of squared distances from the measured points `a..=b` to a curve.
pub fn chi2(c: &[Point; 4], poly: &Polyline, a: usize, b: usize) -> f64 {
    chi2_n(c, poly, a, b, SAMPLES)
}

/// `chi2_n` is the hottest function in this pass — its pattern search calls it millions
/// of times per icon — and both call sites pass one of exactly two compile-time
/// constants, [`SAMPLES`] or [`COARSE_SAMPLES`], neither exceeding `SAMPLES`. A stack
/// buffer sized to `SAMPLES` therefore always has room, and replacing the `Vec<Point>`
/// that used to be heap-allocated fresh on every call removes a malloc/free pair from
/// each of those millions of calls without changing which points are sampled or in what
/// order the distances are folded.
fn chi2_n(c: &[Point; 4], poly: &Polyline, a: usize, b: usize, n: usize) -> f64 {
    // On the stack: both callers pass one of two compile-time constants, neither above
    // `SAMPLES`, and this function is called thousands of times per merge candidate.
    debug_assert!(n <= SAMPLES);
    let mut samples = [Point::new(0.0, 0.0); SAMPLES + 1];
    for (k, s) in samples.iter_mut().enumerate().take(n + 1) {
        *s = eval_cubic(*c, k as f64 / n as f64);
    }
    let samples = &samples[..=n];
    let mut total = 0.0;
    for i in a..=b {
        let p = poly.points[i];
        // The nearest of up to 97 sample points, by brute force, is what this pass spends
        // almost all of its time on. `Point::dist` goes through `hypot`, priced for
        // overflow safety we do not need at pixel-scale coordinates; calling it on every
        // candidate just to throw away all but the smallest is the expense. Squared
        // distance is monotone in true (unrounded) distance, so it ranks the same
        // candidates in the same order without ever calling `hypot` — comparing
        // `dx*dx + dy*dy` can only disagree with comparing `hypot` outputs if two
        // candidates' true distances are so close that both round to the same winner
        // regardless, which changes nothing downstream. `d` itself is then computed by
        // calling `.dist()` on that one winning pair — the exact same call the old fold
        // would have produced for it — so the value summed into `total` is bit-identical
        // to before; only the `n` candidates that lose are spared a `hypot` call.
        let mut best_dist2 = f64::INFINITY;
        let mut best_q = samples[0];
        for &q in samples {
            let dx = p.x - q.x;
            let dy = p.y - q.y;
            let dist2 = dx * dx + dy * dy;
            if dist2 < best_dist2 {
                best_dist2 = dist2;
                best_q = q;
            }
        }
        let d = p.dist(best_q);
        let s = poly.sigma[i].max(1e-6);
        total += (d / s) * (d / s);
    }
    total
}

/// The cubic from `p0` to `p3` that best fits the measured points `a..=b`, with both end
/// tangents free.
///
/// The endpoints are passed in rather than read from the polyline, and that matters. A
/// `Segment::Cubic` stores only its endpoint — a segment starts wherever the previous one
/// ended, at a *refined* vertex position. Fitting through the raw contour point at `a`
/// instead scores a curve that is not the curve that gets emitted, and the difference
/// shows up as a quality regression rather than as an error: DISTS worse on 125 of 180
/// real emoji, with the parameter count going *up*, which a merge cannot do honestly.
///
/// The search is over the residual the merge is decided by — point to curve — because the
/// tidier formulations optimise something else and fail here. Solving for the two control
/// points in closed form is linear and exact for the parameters it is given, but chord
/// length across a quarter turn is not those parameters: it returned chi-squared 210 where
/// the true optimum is 100, and reprojecting between solves did not rescue it. Solving the
/// arms in closed form for fixed directions has the same flaw for the same reason.
pub fn free_cubic(poly: &Polyline, a: usize, b: usize, p0: Point, p3: Point) -> Option<[Point; 4]> {
    let chord = p0.dist(p3);
    if chord <= 1e-9 || b <= a + 1 {
        return None;
    }
    // Where the contour leaves and arrives, as the centre of the search.
    let d0 = {
        let q = poly.points[(a + 2).min(b)];
        Vec2 {
            x: q.x - p0.x,
            y: q.y - p0.y,
        }
    };
    let d1 = {
        let q = poly.points[b.saturating_sub(2).max(a)];
        Vec2 {
            x: p3.x - q.x,
            y: p3.y - q.y,
        }
    };
    let (n0, n1) = (d0.norm(), d1.norm());
    if n0 <= 1e-9 || n1 <= 1e-9 {
        return None;
    }
    let base0 = Vec2 {
        x: d0.x / n0,
        y: d0.y / n0,
    };
    let base1 = Vec2 {
        x: d1.x / n1,
        y: d1.y / n1,
    };

    // Only the total arc length is used below (as a degeneracy guard); the per-index
    // cumulative lengths this loop used to build into a heap-allocated `Vec` were never
    // read by anything — the chord-length parametrisation they name in the comment above
    // is superseded by the grid-and-pattern search below, which works in the rotation/arm
    // parameters instead. Keeping only the sum removes a dead allocation from every call.
    let mut acc = 0.0;
    for i in a..b {
        acc += poly.points[i].dist(poly.points[i + 1]);
    }
    if acc <= 1e-9 {
        return None;
    }

    // Pattern search on the residual that actually decides the merge.
    //
    // Solving the arms in closed form is tempting and wrong: that least-squares minimises
    // distance to `B(t_k)` at chord-length parameters, which is not the point-to-curve
    // residual the objective measures. Fitted that way the corner came out at chi-squared
    // 210-250 where searching the true residual finds 100, and the merge never fired.
    let build = |r0: f64, r1: f64, d0: f64, d1: f64| -> [Point; 4] {
        let (e0, e1) = (rotate(base0, r0), rotate(base1, r1));
        [
            p0,
            Point::new(p0.x + e0.x * d0 * chord, p0.y + e0.y * d0 * chord),
            Point::new(p3.x - e1.x * d1 * chord, p3.y - e1.y * d1 * chord),
            p3,
        ]
    };
    let score = |r0: f64, r1: f64, d0: f64, d1: f64| -> f64 {
        if !(0.02..=MAX_ARM).contains(&d0) || !(0.02..=MAX_ARM).contains(&d1) {
            return f64::INFINITY;
        }
        if r0.abs() > SEARCH_DEGREES || r1.abs() > SEARCH_DEGREES {
            return f64::INFINITY;
        }
        let c = build(r0, r1, d0, d1);
        if cubic_self_intersects(c[0], c[1], c[2], c[3]) {
            return f64::INFINITY;
        }
        chi2(&c, poly, a, b)
    };

    // Coarse grid first, on a cheap residual, then refine on the full one.
    //
    // A pattern search alone gets stuck here: the fits that matter are *asymmetric* —
    // around -10 and +55 degrees at the two ends — and a search started from equal arms
    // and equal angles settles into a symmetric basin at chi-squared 174 where the true
    // optimum is 101. The grid is what escapes it; the refinement is what makes the grid
    // affordable, since it can then be coarse.
    let coarse = |r0: f64, r1: f64, d0: f64, d1: f64| -> f64 {
        if !(0.02..=MAX_ARM).contains(&d0) || !(0.02..=MAX_ARM).contains(&d1) {
            return f64::INFINITY;
        }
        let c = build(r0, r1, d0, d1);
        if cubic_self_intersects(c[0], c[1], c[2], c[3]) {
            return f64::INFINITY;
        }
        chi2_n(&c, poly, a, b, COARSE_SAMPLES)
    };

    const ANGLES: [f64; 9] = [-90.0, -65.0, -45.0, -22.0, 0.0, 22.0, 45.0, 65.0, 90.0];
    const ARMS: [f64; 5] = [0.15, 0.3, 0.45, 0.6, 0.8];
    let mut cur = [0.0f64, 0.0, 0.35, 0.35];
    let mut rough = f64::INFINITY;
    for &r0 in &ANGLES {
        for &r1 in &ANGLES {
            for &d0 in &ARMS {
                for &d1 in &ARMS {
                    let x = coarse(r0, r1, d0, d1);
                    if x < rough {
                        rough = x;
                        cur = [r0, r1, d0, d1];
                    }
                }
            }
        }
    }
    if !rough.is_finite() {
        return None;
    }

    let mut best = score(cur[0], cur[1], cur[2], cur[3]);
    let mut step = [10.0f64, 10.0, 0.1, 0.1];
    for _ in 0..6 {
        let mut improved = true;
        while improved {
            improved = false;
            for k in 0..4 {
                for sign in [-1.0f64, 1.0] {
                    let mut trial = cur;
                    trial[k] += sign * step[k];
                    let x = score(trial[0], trial[1], trial[2], trial[3]);
                    if x < best {
                        best = x;
                        cur = trial;
                        improved = true;
                    }
                }
            }
        }
        for v in step.iter_mut() {
            *v *= 0.5;
        }
    }
    if !best.is_finite() {
        return None;
    }
    Some(build(cur[0], cur[1], cur[2], cur[3]))
}

fn rotate(v: Vec2, deg: f64) -> Vec2 {
    let (s, c) = deg.to_radians().sin_cos();
    Vec2 {
        x: v.x * c - v.y * s,
        y: v.x * s + v.y * c,
    }
}

fn params_of(s: &Segment) -> f64 {
    match s {
        Segment::Line(_) => PARAMS_LINE,
        Segment::Cubic(..) => crate::multimodel::params_cubic(),
        Segment::Arc { .. } => s.params(),
    }
}

/// Merge runs of segments into single free-tangent cubics wherever the objective prefers
/// it. `vertices` are the measured-point indices the segmentation chose.
pub fn merge_free_cubics(
    path: &mut FittedPath,
    poly: &Polyline,
    vertices: &[usize],
    cfg: &FitConfig,
) -> usize {
    if path.segments.len() < 2 || vertices.len() != path.segments.len() + 1 {
        return 0;
    }
    // `vertices` has to be kept in step with the segments it indexes. Splicing the path
    // alone leaves every later span misaligned with the segments it is compared against,
    // and the pass then merges on nonsense: parameters rose 34% on a pass whose whole
    // purpose is to remove segments, which is impossible if the bookkeeping is right.
    let mut verts: Vec<usize> = vertices.to_vec();
    let mut merged = 0usize;
    for _ in 0..MAX_ROUNDS {
        let before = merged;
        merged += merge_round(path, poly, &mut verts, cfg);
        if merged == before {
            break;
        }
    }
    merged
}

/// One sweep of the pass. Repeated by the caller until it stops finding anything.
fn merge_round(
    path: &mut FittedPath,
    poly: &Polyline,
    verts: &mut Vec<usize>,
    cfg: &FitConfig,
) -> usize {
    let mut merged = 0usize;
    let mut m = 0usize;
    while m + 1 < path.segments.len() {
        let mut best: Option<(usize, [Point; 4], f64)> = None;
        // Longest run first: a corner is usually three segments, and absorbing all of it
        // is what removes the kink rather than moving it.
        for run in (2..=MAX_RUN.min(path.segments.len() - m)).rev() {
            if m + run >= verts.len() {
                continue;
            }
            let (a, b) = (verts[m], verts[m + run]);
            if b <= a + 3 || b - a > MAX_SPAN {
                continue;
            }
            // An arc carries its own parametrisation; leave those runs alone.
            if path.segments[m..m + run]
                .iter()
                .any(|s| matches!(s, Segment::Arc { .. }))
            {
                continue;
            }
            // Where this run actually starts and ends on the path, not on the contour.
            let run_start = if m == 0 {
                path.start
            } else {
                path.segments[m - 1].end()
            };
            let run_end = path.segments[m + run - 1].end();
            let Some(c) = free_cubic(poly, a, b, run_start, run_end) else {
                continue;
            };
            if cubic_self_intersects(c[0], c[1], c[2], c[3]) {
                continue;
            }

            // Both sides scored by the objective that chose the run.
            let mut old_chi2 = 0.0;
            let mut old_params = 0.0;
            let mut cur = if m == 0 {
                path.start
            } else {
                path.segments[m - 1].end()
            };
            for q in m..m + run {
                let (sa, sb) = (verts[q], verts[q + 1]);
                let seg = &path.segments[q];
                old_params += params_of(seg);
                let quad = match *seg {
                    Segment::Cubic(c1, c2, e) => [cur, c1, c2, e],
                    Segment::Line(e) => [cur, cur, e, e],
                    Segment::Arc { end, .. } => [cur, cur, end, end],
                };
                old_chi2 += chi2(&quad, poly, sa, sb);
                cur = seg.end();
            }
            let new_chi2 = chi2(&c, poly, a, b);
            let old_cost = 0.5 * old_chi2 + cfg.lambda * old_params;
            let new_cost =
                0.5 * new_chi2 + cfg.lambda * (crate::multimodel::params_cubic() + BREAK_PARAMS);
            // A smoothness prior, expressed where it can be paid for.
            //
            // The objective has no preference between a curve and a polyline that fit
            // equally well, and a line is a third the price, so runs of short chords
            // survive wherever they are honest. Preferring the curve is a claim about
            // icons rather than about the pixels, and this is the one place it can be made
            // without disturbing anything else: the run's own vertices are kept, only the
            // model through them changes, and the cost of being wrong is bounded by the
            // slack allowed here. Zero slack is the objective's own answer.
            if new_cost < old_cost + SMOOTH_SLACK * cfg.lambda {
                best = Some((run, c, new_cost));
                break;
            }
        }

        if let Some((run, c, _)) = best {
            path.segments
                .splice(m..m + run, [Segment::Cubic(c[1], c[2], c[3])]);
            // Drop the interior vertices the merge absorbed, so the two stay aligned.
            verts.drain(m + 1..m + run);
            debug_assert_eq!(verts.len(), path.segments.len() + 1);
            merged += 1;
            m += 1;
        } else {
            m += 1;
        }
    }
    merged
}

// --- sharp corners the program rounded off --------------------------------------------

/// Longest chord, in pixels, of a cubic that may be a rounded-off corner rather than a
/// curve the artist drew. The anti-aliasing chamfer at a corner spans about one pixel
/// per side, so a cubic bridging two lines over less than this is far more likely to be
/// the chamfer than a fillet: at 128 px intake a genuine fillet that small is invisible.
pub const SHARPEN_MAX_CHORD: f64 = 2.5;
/// Longest chord of a cubic that may be a whole short *edge* with a chamfered corner at
/// each end — the end of a 3-6 px bar, a glyph terminal — which the program fits as one
/// cubic through both chamfers. Replaced by the edge's own line, taken from the cubic's
/// middle, meeting the neighbours at two sharp corners.
pub const SHARPEN_MAX_EDGE: f64 = 8.0;
/// Turn between the two lines from which their meeting is treated as a corner.
///
/// The same 30 degrees as [`crate::CORNER_TURN_MIN`], and deliberately so: a corner is a
/// corner, and the two passes asking the question should not be able to answer it
/// differently. They were independent copies of `PI / 6.0` until 2026-09-08.
pub const SHARPEN_MIN_TURN: f64 = crate::CORNER_TURN_MIN;

/// Replace short cubics that bridge two lines meeting at an angle with the lines' actual
/// intersection: one vertex instead of a cubic, four parameters fewer, and the corner
/// where the artist put it.
///
/// Why the program produces these: the contour samples around a corner lie on the
/// coverage level set, which rounds the corner off by about a pixel. To the residual
/// those samples *are* a small fillet, so a cubic through them beats two lines that
/// miss them — the residual cannot tell a rasterised sharp corner from a sub-pixel
/// fillet. The prior settles it: icon and logo artists draw corners; a fillet under
/// two pixels at this scale is not something they draw, it is something the renderer
/// did. This is the same argument that lets `adjust_vertices_at` cross the chamfer.
///
/// Guarded by geometry, not residual: the two lines must turn by at least
/// [`SHARPEN_MIN_TURN`], their intersection must lie ahead of the first line and behind
/// the second (a convex corner between them, not a crossing behind the cubic), and it
/// must sit within the chamfer allowance of the cubic's endpoints so a genuine long
/// fillet is never collapsed. Returns the number of corners sharpened.
pub fn sharpen_corners(path: &mut FittedPath) -> usize {
    let n = path.segments.len();
    if n < 3 {
        return 0;
    }
    let mut starts: Vec<Point> = Vec::with_capacity(n);
    let mut cur = path.start;
    for s in &path.segments {
        starts.push(cur);
        cur = s.end();
    }
    // A ring: the last segment returns to the start, so segment 0 has a predecessor.
    let looped = cur.dist(path.start) < 1e-9;
    let mut out: Vec<Segment> = Vec::with_capacity(n);
    let mut new_start = path.start;
    // When segment 0 is sharpened its predecessor is the *last* segment, which is not
    // in `out` yet; remember where it must end.
    let mut last_end: Option<Point> = None;
    let mut sharpened = 0usize;
    let mut i = 0usize;
    while i < n {
        let seg = path.segments[i].clone();
        let prev = if i >= 1 {
            Some(i - 1)
        } else if looped {
            Some(n - 1)
        } else {
            None
        };
        let next = if i + 1 < n {
            Some(i + 1)
        } else if looped {
            Some(0)
        } else {
            None
        };
        let prev_is_line = prev.is_some_and(|p| matches!(path.segments[p], Segment::Line(_)));
        let next_is_line = next.is_some_and(|p| matches!(path.segments[p], Segment::Line(_)));

        if let Segment::Cubic(c1, c2, e) = seg {
            let s0 = starts[i];
            if prev_is_line && next_is_line && s0.dist(e) <= SHARPEN_MAX_EDGE {
                let a = starts[prev.unwrap()];
                let b = path.segments[next.unwrap()].end();
                if let (Some(d0), Some(d1)) = (unit_vec(s0 - a), unit_vec(b - e)) {
                    let m = cubic_at([s0, c1, c2, e], 0.5);
                    let dm = unit_vec(cubic_tangent_at([s0, c1, c2, e], 0.5));
                    // One corner (the cubic is the chamfer itself), else two corners
                    // (a short edge with a chamfer at each end, direction from the
                    // cubic's own middle).
                    let mut found: Option<(Point, Option<Point>)> = None;
                    if s0.dist(e) <= SHARPEN_MAX_CHORD {
                        if let Some(hit) = corner_between(a, d0, s0, b, d1, e) {
                            found = Some((hit, None));
                        }
                    }
                    if found.is_none() {
                        if let Some(dm) = dm {
                            let h1 = corner_between(a, d0, s0, m, dm, s0);
                            let h2 = corner_between(m, dm, e, b, d1, e);
                            if let (Some(h1), Some(h2)) = (h1, h2) {
                                if h1.dist(h2) >= 1.0 && h1.dist(h2) <= SHARPEN_MAX_EDGE {
                                    found = Some((h1, Some(h2)));
                                }
                            }
                        }
                    }
                    if let Some((h1, h2)) = found {
                        if i == 0 {
                            last_end = Some(h1);
                            new_start = h1;
                        } else if i == n - 1 && looped {
                            new_start = h1;
                            if let Some(Segment::Line(_)) = out.last() {
                                *out.last_mut().unwrap() = Segment::Line(h1);
                            }
                        } else if let Some(Segment::Line(_)) = out.last() {
                            *out.last_mut().unwrap() = Segment::Line(h1);
                        }
                        if let Some(h2) = h2 {
                            out.push(Segment::Line(h2));
                            sharpened += 1;
                        }
                        sharpened += 1;
                        i += 1;
                        continue;
                    }
                }
            }
        } else if let Segment::Line(e) = seg {
            let s0 = starts[i];
            if prev_is_line && next_is_line && s0.dist(e) <= SHARPEN_MAX_CHORD {
                let a = starts[prev.unwrap()];
                let b = path.segments[next.unwrap()].end();
                if let (Some(d0), Some(d1)) = (unit_vec(s0 - a), unit_vec(b - e)) {
                    if let Some(hit) = corner_between(a, d0, s0, b, d1, e) {
                        if i == 0 {
                            last_end = Some(hit);
                            new_start = hit;
                        } else if i == n - 1 && looped {
                            new_start = hit;
                            if let Some(Segment::Line(_)) = out.last() {
                                *out.last_mut().unwrap() = Segment::Line(hit);
                            }
                        } else if let Some(Segment::Line(_)) = out.last() {
                            *out.last_mut().unwrap() = Segment::Line(hit);
                        }
                        sharpened += 1;
                        i += 1;
                        continue;
                    }
                }
            }
        }
        out.push(seg);
        i += 1;
    }
    if sharpened > 0 {
        if let Some(p) = last_end {
            if let Some(Segment::Line(_)) = out.last() {
                *out.last_mut().unwrap() = Segment::Line(p);
            }
        }
        path.start = new_start;
        path.segments = out;
    }
    sharpened
}

/// The corner where the line through `a` with direction `d0` meets the line through `b`
/// with direction `d1`, if the two turn by at least [`SHARPEN_MIN_TURN`], the meeting
/// lies ahead of `p0` along the first line and behind `p1` along the second (a convex
/// corner between them, not a crossing behind the cubic), and it is within the chamfer
/// allowance of both `p0` and `p1` — the measured points the corner is recovered from.
fn corner_between(a: Point, d0: Vec2, p0: Point, b: Point, d1: Vec2, p1: Point) -> Option<Point> {
    let turn = d0.cross(d1).abs().atan2(d0.dot(d1));
    let denom = d0.cross(d1);
    if turn < SHARPEN_MIN_TURN || denom.abs() <= 1e-9 {
        return None;
    }
    let t = (b - a).cross(d1) / denom;
    let hit = Point::new(a.x + d0.x * t, a.y + d0.y * t);
    let half_interior = 0.5 * (std::f64::consts::PI - turn);
    let allow = (crate::CORNER_CHAMFER / half_interior.sin().max(0.2)).min(3.0) + 0.5;
    // The corner lies ahead of the first line's last sample and behind the second's
    // first, give or take the chamfer: a sample can sit a fraction past the corner along
    // the *other* edge, so the sign test carries the same slack as the distance test.
    let ahead = (hit - p0).dot(d0) >= -allow;
    let behind = (p1 - hit).dot(d1) >= -allow;
    if ahead && behind && hit.dist(p0) <= allow && hit.dist(p1) <= allow {
        Some(hit)
    } else {
        None
    }
}

fn cubic_at(p: [Point; 4], t: f64) -> Point {
    let u = 1.0 - t;
    let (b0, b1, b2, b3) = (u * u * u, 3.0 * u * u * t, 3.0 * u * t * t, t * t * t);
    Point::new(
        b0 * p[0].x + b1 * p[1].x + b2 * p[2].x + b3 * p[3].x,
        b0 * p[0].y + b1 * p[1].y + b2 * p[2].y + b3 * p[3].y,
    )
}

fn cubic_tangent_at(p: [Point; 4], t: f64) -> Vec2 {
    let u = 1.0 - t;
    Vec2 {
        x: 3.0
            * (u * u * (p[1].x - p[0].x)
                + 2.0 * u * t * (p[2].x - p[1].x)
                + t * t * (p[3].x - p[2].x)),
        y: 3.0
            * (u * u * (p[1].y - p[0].y)
                + 2.0 * u * t * (p[2].y - p[1].y)
                + t * t * (p[3].y - p[2].y)),
    }
}

fn unit_vec(v: Vec2) -> Option<Vec2> {
    let n = v.norm();
    if n > 1e-12 {
        Some(Vec2 {
            x: v.x / n,
            y: v.y / n,
        })
    } else {
        None
    }
}

// The research-only snap passes, `snap_axis_aligned` (`INKVEC_AXIS`) and
// `snap_smooth_joins` (`INKVEC_G1`).
#[cfg(feature = "research")]
mod snap;
#[cfg(feature = "research")]
pub use snap::{snap_axis_aligned, snap_smooth_joins, PARAMS_AXIS_LINE, PARAMS_SMOOTH_CUBIC};
