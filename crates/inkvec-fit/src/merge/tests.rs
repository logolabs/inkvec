//! The merge pass's speed-ups against the plain computations they replace: each must
//! return the same bits wherever its caller can tell.

use super::*;

/// A small deterministic generator (PCG-style LCG), so failures reproduce.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> f64 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        (self.0 >> 11) as f64 / (1u64 << 53) as f64
    }
    fn pt(&mut self, scale: f64) -> Point {
        Point::new(scale * (self.next() - 0.5), scale * (self.next() - 0.5))
    }
}

/// A noisy run of `n` points along a random cubic, with random sigmas.
fn noisy_run(rng: &mut Rng, n: usize, noise: f64) -> (Polyline, [Point; 4]) {
    let c = [rng.pt(40.0), rng.pt(40.0), rng.pt(40.0), rng.pt(40.0)];
    let pts: Vec<Point> = (0..n)
        .map(|k| {
            let p = eval_cubic(c, k as f64 / (n - 1) as f64);
            Point::new(
                p.x + noise * (rng.next() - 0.5),
                p.y + noise * (rng.next() - 0.5),
            )
        })
        .collect();
    let sigma: Vec<f64> = (0..n).map(|_| 0.05 + rng.next()).collect();
    (Polyline::new(pts, sigma, false), c)
}

/// Below its bound, `chi2_n_below` is `chi2_n` bit for bit; at or above it, it may say
/// infinity but never a smaller number. Checked on bounds straddling the true value,
/// equal to it, far off either side, infinite, NaN and subnormal, for runs short enough
/// for the middle-first order and longer than its buffer.
#[test]
fn bounded_residual_agrees_with_the_plain_one_wherever_it_can_matter() {
    let mut rng = Rng(7);
    for case in 0..600 {
        let n = [3, 4, 5, 8, 17, 40, 97, 98, 150][case % 9];
        let (poly, _) = noisy_run(&mut rng, n, [0.0, 0.3, 4.0][case % 3]);
        let c = [rng.pt(40.0), rng.pt(40.0), rng.pt(40.0), rng.pt(40.0)];
        let (a, b) = (0, n - 1);
        for samples in [COARSE_SAMPLES, SAMPLES] {
            let exact = chi2_n(&c, &poly, a, b, samples);
            let near = [
                exact,
                exact * (1.0 - 1e-15),
                exact * (1.0 + 1e-15),
                exact * 0.5,
                exact * 2.0,
                exact * (1.0 + 1e-13),
                exact * (1.0 - 1e-13),
                f64::from_bits(exact.to_bits() + 1),
                f64::from_bits(exact.to_bits().saturating_sub(1)),
                0.0,
                1e-300,
                f64::INFINITY,
                f64::NAN,
                rng.next() * 2.0 * exact,
            ];
            for bound in near {
                let got = chi2_n_below(&c, &poly, a, b, samples, bound);
                if got.is_infinite() && !exact.is_infinite() {
                    assert!(
                        exact >= bound,
                        "case {case}: cut at {bound} but exact {exact}"
                    );
                } else {
                    assert_eq!(got.to_bits(), exact.to_bits(), "case {case}, bound {bound}");
                }
                // The only question callers ask.
                assert_eq!(got < bound, exact < bound, "case {case}, bound {bound}");
            }
        }
    }
}

/// A NaN coordinate makes the plain residual NaN; the bounded one must not turn that into
/// a finite number that could win a comparison.
#[test]
fn bounded_residual_keeps_nan() {
    let mut rng = Rng(3);
    let (mut poly, c) = noisy_run(&mut rng, 20, 0.2);
    poly.points[9] = Point::new(f64::NAN, 1.0);
    let exact = chi2_n(&c, &poly, 0, 19, SAMPLES);
    assert!(exact.is_nan());
    for bound in [0.1, 10.0, 1e9] {
        let got = chi2_n_below(&c, &poly, 0, 19, SAMPLES, bound);
        assert!(!(got < bound));
    }
}

/// The free-cubic search as it was before `chi2_n_below`: every candidate scored in full.
fn free_cubic_reference(
    poly: &Polyline,
    a: usize,
    b: usize,
    p0: Point,
    p3: Point,
) -> Option<[Point; 4]> {
    let chord = p0.dist(p3);
    if chord <= 1e-9 || b <= a + 1 {
        return None;
    }
    let q = poly.points[(a + 2).min(b)];
    let d0 = Vec2 {
        x: q.x - p0.x,
        y: q.y - p0.y,
    };
    let q = poly.points[b.saturating_sub(2).max(a)];
    let d1 = Vec2 {
        x: p3.x - q.x,
        y: p3.y - q.y,
    };
    let (n0, n1) = (d0.norm(), d1.norm());
    if n0 <= 1e-9 || n1 <= 1e-9 {
        return None;
    }
    let mut acc = 0.0;
    for i in a..b {
        acc += poly.points[i].dist(poly.points[i + 1]);
    }
    if acc <= 1e-9 {
        return None;
    }
    let s = FreeCubicSearch {
        poly,
        a,
        b,
        p0,
        p3,
        chord,
        base0: Vec2 {
            x: d0.x / n0,
            y: d0.y / n0,
        },
        base1: Vec2 {
            x: d1.x / n1,
            y: d1.y / n1,
        },
    };
    let admissible = |r0: f64, r1: f64, d0: f64, d1: f64| -> Option<[Point; 4]> {
        if !(0.02..=MAX_ARM).contains(&d0) || !(0.02..=MAX_ARM).contains(&d1) {
            return None;
        }
        if r0.abs() > SEARCH_DEGREES || r1.abs() > SEARCH_DEGREES {
            return None;
        }
        let c = s.build(r0, r1, d0, d1);
        (!cubic_self_intersects(c[0], c[1], c[2], c[3])).then_some(c)
    };
    const ANGLES: [f64; 9] = [-90.0, -65.0, -45.0, -22.0, 0.0, 22.0, 45.0, 65.0, 90.0];
    const ARMS: [f64; 5] = [0.15, 0.3, 0.45, 0.6, 0.8];
    let mut cur = [0.0f64, 0.0, 0.35, 0.35];
    let mut rough = f64::INFINITY;
    for &r0 in &ANGLES {
        for &r1 in &ANGLES {
            for &e0 in &ARMS {
                for &e1 in &ARMS {
                    let x = admissible(r0, r1, e0, e1)
                        .map_or(f64::INFINITY, |c| chi2_n(&c, poly, a, b, COARSE_SAMPLES));
                    if x < rough {
                        rough = x;
                        cur = [r0, r1, e0, e1];
                    }
                }
            }
        }
    }
    if !rough.is_finite() {
        return None;
    }
    let score = |t: [f64; 4]| {
        admissible(t[0], t[1], t[2], t[3]).map_or(f64::INFINITY, |c| chi2(&c, poly, a, b))
    };
    let mut best = score(cur);
    let mut step = [10.0f64, 10.0, 0.1, 0.1];
    for _ in 0..6 {
        let mut improved = true;
        while improved {
            improved = false;
            for k in 0..4 {
                for sign in [-1.0f64, 1.0] {
                    let mut trial = cur;
                    trial[k] += sign * step[k];
                    let x = score(trial);
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
    best.is_finite()
        .then(|| s.build(cur[0], cur[1], cur[2], cur[3]))
}

/// The early-exit search finds the same cubic as the exhaustive one, bit for bit, on
/// corners, smooth runs, noise and near-straight runs.
#[test]
fn free_cubic_is_unchanged_by_the_early_exit() {
    let mut rng = Rng(19);
    let bits = |c: Option<[Point; 4]>| c.map(|c| c.map(|p| (p.x.to_bits(), p.y.to_bits())));
    for case in 0..60 {
        let n = 5 + (case * 7) % 60;
        let (poly, _) = noisy_run(&mut rng, n, [0.0, 0.2, 1.5][case % 3]);
        let (a, b) = (0, n - 1);
        // The path's own end points sit near, not on, the contour's.
        let p0 = Point::new(poly.points[a].x + 0.1 * rng.next(), poly.points[a].y);
        let p3 = Point::new(poly.points[b].x, poly.points[b].y - 0.1 * rng.next());
        assert_eq!(
            bits(free_cubic(&poly, a, b, p0, p3)),
            bits(free_cubic_reference(&poly, a, b, p0, p3)),
            "case {case}"
        );
    }
}

/// Closed test boundaries with corners, fillets and wobble: what the merge pass is for.
fn test_rings(rng: &mut Rng) -> Vec<Polyline> {
    let mut rings = Vec::new();
    for case in 0..12 {
        let n = 60 + 20 * (case % 5);
        let (w, h, r) = (
            20.0 + 10.0 * rng.next(),
            14.0 + 10.0 * rng.next(),
            1.0 + 5.0 * rng.next(),
        );
        let wobble = [0.0, 0.15, 0.4][case % 3];
        let pts: Vec<Point> = (0..n)
            .map(|k| {
                let t = std::f64::consts::TAU * k as f64 / n as f64;
                // A superellipse: square-ish with rounded corners, sharper as `e` grows.
                let e = 2.0 + r;
                let (c, s) = (t.cos(), t.sin());
                let x = w * c.signum() * c.abs().powf(2.0 / e);
                let y = h * s.signum() * s.abs().powf(2.0 / e);
                Point::new(
                    x + wobble * (rng.next() - 0.5),
                    y + wobble * (rng.next() - 0.5),
                )
            })
            .collect();
        rings.push(Polyline::new(pts, vec![0.1 + 0.2 * rng.next(); n], true));
    }
    rings
}

/// Remembering turned-down runs across sweeps changes nothing: the pass with the memory
/// makes exactly the merges, and the cubics, of the pass that forgets after every sweep
/// (within one sweep no run is tried twice, so a fresh memory per sweep is no memory).
#[test]
fn remembered_rejections_change_no_merge() {
    let mut rng = Rng(29);
    let cfg = FitConfig::default();
    let mut total = 0;
    for poly in test_rings(&mut rng) {
        for span in [3, 5, 8] {
            let fit = crate::multimodel::optimal_multimodel_capped_full(&poly, &cfg, span);
            let mut with = fit.path.clone();
            let merged = merge_free_cubics(&mut with, &poly, &fit.vertices, &cfg);
            let mut without = fit.path.clone();
            let mut verts = fit.vertices.clone();
            let mut merged_without = 0;
            if without.segments.len() >= 2 && verts.len() == without.segments.len() + 1 {
                for _ in 0..MAX_ROUNDS {
                    let got = merge_round(
                        &mut without,
                        &poly,
                        &mut verts,
                        &cfg,
                        &mut RejectedRuns::default(),
                    );
                    merged_without += got;
                    if got == 0 {
                        break;
                    }
                }
            }
            assert_eq!(merged, merged_without);
            assert_eq!(format!("{with:?}"), format!("{without:?}"));
            total += merged;
        }
    }
    assert!(total > 0, "the test boundaries never merged anything");
}

/// The merge pass as it was before its speed-ups: every run tried in every sweep, every
/// candidate searched exhaustively and priced afterwards.
fn merge_reference(
    path: &mut FittedPath,
    poly: &Polyline,
    vertices: &[usize],
    cfg: &FitConfig,
) -> usize {
    if path.segments.len() < 2 || vertices.len() != path.segments.len() + 1 {
        return 0;
    }
    let mut verts = vertices.to_vec();
    let mut merged = 0usize;
    for _ in 0..MAX_ROUNDS {
        let before = merged;
        let mut m = 0usize;
        while m + 1 < path.segments.len() {
            let mut best: Option<(usize, [Point; 4])> = None;
            for run in (2..=MAX_RUN.min(path.segments.len() - m)).rev() {
                if m + run >= verts.len() {
                    continue;
                }
                let (a, b) = (verts[m], verts[m + run]);
                if b <= a + 3 || b - a > MAX_SPAN {
                    continue;
                }
                if path.segments[m..m + run]
                    .iter()
                    .any(|s| matches!(s, Segment::Arc { .. }))
                {
                    continue;
                }
                let run_start = if m == 0 {
                    path.start
                } else {
                    path.segments[m - 1].end()
                };
                let run_end = path.segments[m + run - 1].end();
                let Some(c) = free_cubic_reference(poly, a, b, run_start, run_end) else {
                    continue;
                };
                if cubic_self_intersects(c[0], c[1], c[2], c[3]) {
                    continue;
                }
                let (mut old_chi2, mut old_params, mut cur) = (0.0, 0.0, run_start);
                for q in m..m + run {
                    let seg = &path.segments[q];
                    old_params += params_of(seg);
                    let quad = match *seg {
                        Segment::Cubic(c1, c2, e) => [cur, c1, c2, e],
                        Segment::Line(e) => [cur, cur, e, e],
                        Segment::Arc { end, .. } => [cur, cur, end, end],
                    };
                    old_chi2 += chi2(&quad, poly, verts[q], verts[q + 1]);
                    cur = seg.end();
                }
                let new_chi2 = chi2(&c, poly, a, b);
                let old_cost = 0.5 * old_chi2 + cfg.lambda * old_params;
                let new_cost = 0.5 * new_chi2
                    + cfg.lambda * (crate::multimodel::params_cubic() + BREAK_PARAMS);
                if new_cost < old_cost + SMOOTH_SLACK * cfg.lambda {
                    best = Some((run, c));
                    break;
                }
            }
            if let Some((run, c)) = best {
                path.segments
                    .splice(m..m + run, [Segment::Cubic(c[1], c[2], c[3])]);
                verts.drain(m + 1..m + run);
                merged += 1;
            }
            m += 1;
        }
        if merged == before {
            break;
        }
    }
    merged
}

/// The whole pass, with its early exits, memory and price floor, against the pass as it
/// was: the same merges and the same cubics, bit for bit.
#[test]
fn merge_pass_is_unchanged_by_its_speed_ups() {
    let mut rng = Rng(31);
    let cfg = FitConfig::default();
    let mut total = 0;
    for poly in test_rings(&mut rng).into_iter().take(8) {
        for span in [3, 6] {
            let fit = crate::multimodel::optimal_multimodel_capped_full(&poly, &cfg, span);
            let mut fast = fit.path.clone();
            let merged = merge_free_cubics(&mut fast, &poly, &fit.vertices, &cfg);
            let mut slow = fit.path.clone();
            let merged_slow = merge_reference(&mut slow, &poly, &fit.vertices, &cfg);
            assert_eq!(merged, merged_slow);
            assert_eq!(format!("{fast:?}"), format!("{slow:?}"));
            total += merged;
        }
    }
    assert!(total > 0, "the test boundaries never merged anything");
}

/// The price floor's one piece of arithmetic: a non-negative half-residual added to the
/// floor never rounds below it, for residuals from subnormal to huge.
#[test]
fn a_free_cubic_never_costs_less_than_its_parameters() {
    let mut rng = Rng(5);
    for lambda in [1e-3, 1.0, 7.85, 9.9, 1e3] {
        let floor = lambda * (crate::multimodel::params_cubic() + BREAK_PARAMS);
        for x in [
            0.0,
            f64::MIN_POSITIVE / 4.0,
            1e-300,
            1e-17,
            1e-8,
            0.3,
            1e3,
            1e300,
        ] {
            assert!(0.5 * x + floor >= floor);
            let y = x * rng.next();
            assert!(0.5 * y + floor >= floor);
        }
    }
}
