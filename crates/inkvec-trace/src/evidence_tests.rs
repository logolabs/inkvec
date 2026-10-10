use super::*;
use crate::planar;
use inkvec_core::likelihood::{left_area, Axis};
use inkvec_core::Point;
use std::f64::consts::PI;

/// Exact area coverage of each layer (simple polygons, pairwise disjoint: they may abut but
/// not overlap) per pixel, composited over `bg`, optionally rounded to 8 bits; the label of
/// each pixel is its largest face; faces are `bg` then the layers.
struct Scene {
    w: usize,
    h: usize,
    rgb: Vec<[f32; 3]>,
    cov: Vec<Vec<f64>>, // per layer, the visible coverage
    labels: Vec<u16>,
    faces: Vec<FillModel>,
}

fn render(
    w: usize,
    h: usize,
    bg: [f32; 3],
    layers: &[(Vec<Point>, [f32; 3])],
    quantise: bool,
) -> Scene {
    let n = layers.len();
    let mut raw: Vec<Vec<f64>> = vec![vec![0.0; w * h]; n];
    for (k, (poly, _)) in layers.iter().enumerate() {
        let (x0, x1) = poly
            .iter()
            .fold((f64::INFINITY, f64::NEG_INFINITY), |(a, b), p| {
                (a.min(p.x), b.max(p.x))
            });
        let (y0, y1) = poly
            .iter()
            .fold((f64::INFINITY, f64::NEG_INFINITY), |(a, b), p| {
                (a.min(p.y), b.max(p.y))
            });
        for y in (y0.floor().max(0.0) as usize)..=(y1.ceil() as usize).min(h - 1) {
            for x in (x0.floor().max(0.0) as usize)..=(x1.ceil() as usize).min(w - 1) {
                raw[k][y * w + x] = local::area_in_pixel(poly, x as i32, y as i32);
            }
        }
    }
    let cov = raw;
    for i in 0..w * h {
        assert!(
            cov.iter().map(|l| l[i]).sum::<f64>() <= 1.0 + 1e-9,
            "layers overlap"
        );
    }
    let mut rgb = vec![[0f32; 3]; w * h];
    let mut labels = vec![0u16; w * h];
    for i in 0..w * h {
        let mut c = [0f64; 3];
        let mut used = 0.0;
        let mut best = (1.0 - cov.iter().map(|l| l[i]).sum::<f64>(), 0u16);
        for (k, (_, col)) in layers.iter().enumerate() {
            for ch in 0..3 {
                c[ch] += cov[k][i] * col[ch] as f64;
            }
            used += cov[k][i];
            if cov[k][i] > best.0 {
                best = (cov[k][i], k as u16 + 1);
            }
        }
        for ch in 0..3 {
            c[ch] += (1.0 - used) * bg[ch] as f64;
            if quantise {
                c[ch] = (c[ch] * 255.0).round() / 255.0;
            }
            rgb[i][ch] = c[ch] as f32;
        }
        labels[i] = best.1;
    }
    let mut faces = vec![FillModel::Flat(bg)];
    faces.extend(layers.iter().map(|(_, c)| FillModel::Flat(*c)));
    Scene {
        w,
        h,
        rgb,
        cov,
        labels,
        faces,
    }
}

fn map_of(s: &Scene) -> PlanarMap {
    let mut map = planar::build(&s.labels, s.w, s.h, s.faces.len());
    planar::refine_subpixel(&mut map, &s.rgb, &s.faces, 0.5 / 255.0, false);
    map
}

fn disc(cx: f64, cy: f64, r: f64, n: usize) -> Vec<Point> {
    // Area-matched polygon, as `bench/theory/exact_raster.py` draws discs.
    let rr = r * ((2.0 * PI / n as f64) / (2.0 * PI / n as f64).sin()).sqrt();
    (0..n)
        .map(|k| {
            let t = 2.0 * PI * k as f64 / n as f64;
            Point::new(cx + rr * t.cos(), cy + rr * t.sin())
        })
        .collect()
}

fn square(cx: f64, cy: f64, half: f64, rot: f64) -> Vec<Point> {
    (0..4)
        .map(|k| {
            let t = rot + PI / 4.0 + k as f64 * PI / 2.0;
            Point::new(
                cx + half * 2f64.sqrt() * t.cos(),
                cy + half * 2f64.sqrt() * t.sin(),
            )
        })
        .collect()
}

/// The polygon as line pieces, closed, in its own order or reversed.
fn pieces(poly: &[Point], reverse: bool) -> Vec<Piece> {
    let mut p: Vec<Point> = poly.to_vec();
    if reverse {
        p.reverse();
    }
    let n = p.len();
    (0..n)
        .map(|k| Piece::Line([p[k], p[(k + 1) % n]]))
        .collect()
}

fn exact_floor() -> EvidenceOptions {
    EvidenceOptions {
        floor: Some(Floor {
            lattice: 1_000_000,
            window_var: 0.0,
            edge_var: 0.0,
        }),
        ..EvidenceOptions::default()
    }
}

const B: [f32; 3] = [0.0, 0.0, 0.0];
const WHITE: [f32; 3] = [1.0, 1.0, 1.0];

/// Every partial pixel is in exactly one run window, local term, thin strip or third-ink
/// list; no pixel is in two windows. Discs and rotated squares cover every direction,
/// including the switches between columns and rows at 45°.
#[test]
fn windows_partition_every_partial_pixel() {
    let shapes: Vec<Vec<Point>> = vec![
        disc(31.4, 30.2, 17.3, 720),
        disc(32.0, 32.0, 9.6, 720),
        square(31.7, 32.2, 13.0, 0.3),
        square(30.9, 31.4, 15.0, PI / 4.0 + 0.01),
    ];
    for poly in shapes {
        let s = render(64, 64, WHITE, &[(poly, B)], false);
        let map = map_of(&s);
        let ev = build(&map, &s.rgb, &s.faces, 0.5 / 255.0, &exact_floor());
        let mut count = vec![0u32; s.w * s.h];
        let mut in_windows = vec![0u32; s.w * s.h];
        for e in 0..ev.edge_count() {
            for i in 0..ev.runs(e).len() {
                for &(x, y) in ev.window_pixels(e, i) {
                    count[y as usize * s.w + x as usize] += 1;
                    in_windows[y as usize * s.w + x as usize] += 1;
                }
            }
        }
        for j in ev.junctions() {
            for (x, y) in ev.local_pixels(j.node) {
                count[y as usize * s.w + x as usize] += 1;
            }
        }
        for i in 0..ev.corners().len() {
            for (x, y) in ev.corner_pixels(i) {
                count[y as usize * s.w + x as usize] += 1;
            }
        }
        for &(x, y) in ev.strip_pixels().iter().chain(ev.third_pixels()) {
            count[y as usize * s.w + x as usize] += 1;
        }
        let tol = 0.5 / 255.0 / 3f64.sqrt();
        let mut partial = 0;
        for i in 0..s.w * s.h {
            assert!(
                in_windows[i] <= 1,
                "pixel {} in {} windows",
                i,
                in_windows[i]
            );
            let c = s.cov[0][i];
            if c > tol && c < 1.0 - tol {
                partial += 1;
                assert_eq!(
                    count[i],
                    1,
                    "partial pixel ({}, {}) cov {c:.4} counted {}",
                    i % s.w,
                    i / s.w,
                    count[i]
                );
            }
        }
        assert!(partial > 50);
    }
}

/// On an exact render, every run window's sum is the area the true curve gives the edge's
/// left face there (the window identity), to rounding: sums of exact coverages against the
/// evaluator's closed-form integrals.
#[test]
fn window_sums_are_exact_areas() {
    for (poly, ink) in [
        (disc(31.4, 30.2, 17.3, 720), B),
        (square(31.7, 32.2, 13.0, 0.3), B),
        (square(31.7, 32.2, 13.0, 0.3), [0.9, 0.3, 0.1]),
    ] {
        let s = render(64, 64, WHITE, &[(poly.clone(), ink)], false);
        let map = map_of(&s);
        let ev = build(&map, &s.rgb, &s.faces, 0.5 / 255.0, &exact_floor());
        let mut checked = 0;
        for e in 0..ev.edge_count() {
            let obs = ev.runs(e);
            if obs.is_empty() {
                continue;
            }
            // The orientation that gives the edge's left face: the one matching window 0.
            let fwd = pieces(&poly, false);
            let rev = pieces(&poly, true);
            let curve = if (left_area(&fwd, &obs[0].window) - obs[0].sum).abs()
                < (left_area(&rev, &obs[0].window) - obs[0].sum).abs()
            {
                fwd
            } else {
                rev
            };
            for o in obs {
                let a = left_area(&curve, &o.window);
                // The image holds f32: each pixel is rounded to a part in 1e7.
                assert!(
                    (a - o.sum).abs() < 1e-6,
                    "window {:?}: area {a} sum {}",
                    o.window,
                    o.sum
                );
                checked += 1;
            }
        }
        assert!(checked > 40, "{checked}");
    }
}

/// At the truth, with 8-bit quantisation and the variances of B1.2, `χ²/M` is near 1; the
/// certified χ² (the straight-line checker kernel) agrees with the evaluator's.
#[test]
fn the_truth_is_calibrated_and_certified() {
    let mut chi = Chi2::default();
    for (k, poly) in [
        disc(31.4, 30.2, 17.3, 720),
        disc(30.7, 33.1, 21.9, 720),
        square(31.7, 32.2, 13.0, 0.3),
        square(32.3, 31.1, 17.0, 1.1),
    ]
    .into_iter()
    .enumerate()
    {
        // Inks on the 8-bit lattice, as a palette read off pure pixels is: an ink half a level
        // off scales every weight by (1 - 0.0025) and shows as a -3σ bias per window.
        let l = |v: f32| v / 255.0;
        let ink = [
            [l(26.0), l(51.0), l(179.0)],
            B,
            [l(230.0), l(77.0), l(26.0)],
            [0.0, l(128.0), l(128.0)],
        ][k];
        let s = render(64, 64, WHITE, &[(poly.clone(), ink)], true);
        let map = map_of(&s);
        let ev = build(&map, &s.rgb, &s.faces, 0.5 / 255.0, &exact_floor());
        for e in 0..ev.edge_count() {
            let obs = ev.runs(e);
            if obs.is_empty() {
                continue;
            }
            let fwd = pieces(&poly, false);
            let rev = pieces(&poly, true);
            let c1 = ev.chi2_run(e, 0..obs.len(), &fwd);
            let c2 = ev.chi2_run(e, 0..obs.len(), &rev);
            let (best, curve) = if c1.chi2 < c2.chi2 {
                (c1, fwd)
            } else {
                (c2, rev)
            };
            chi = chi + best;
            let mut closed: Vec<Point> = curve
                .iter()
                .map(|p| match p {
                    Piece::Line([a, _]) => *a,
                    _ => unreachable!(),
                })
                .collect();
            closed.push(closed[0]);
            let cert = checks::certify_run_chi2_polyline(obs, &closed, best.chi2, 1e-9);
            assert!(cert.is_ok(), "{cert:?}");
        }
    }
    let ratio = chi.chi2 / chi.m as f64;
    // Measured 0.97 over 442 windows (1.14, 1.03, 0.84, 0.86 by shape; σ of χ²/M ≈ 0.07).
    assert!(
        chi.m > 300 && (0.8..1.2).contains(&ratio),
        "chi2/M = {ratio:.3} over {}",
        chi.m
    );
}

/// A T: a black band over white with a red stem below it. Where the stem meets the band, two
/// junctions; at each, the band's two arms continue one another (A4), and the local term of
/// the true lines is calibrated.
#[test]
fn a_t_junction_reports_its_continuation() {
    let (yb, xl, xr) = (30.3, 28.6, 35.2);
    let band = vec![
        Point::new(-1.0, -1.0),
        Point::new(65.0, -1.0),
        Point::new(65.0, yb),
        Point::new(-1.0, yb),
    ];
    let stem = vec![
        Point::new(xl, yb),
        Point::new(xr, yb),
        Point::new(xr, 65.0),
        Point::new(xl, 65.0),
    ];
    let s = render(64, 64, WHITE, &[(band, B), (stem, [0.9, 0.1, 0.1])], true);
    let map = map_of(&s);
    let ev = build(&map, &s.rgb, &s.faces, 0.5 / 255.0, &exact_floor());
    let js: Vec<&JunctionReport> = ev
        .junctions()
        .iter()
        .filter(|j| {
            (j.at.y - yb).abs() < 2.0 && ((j.at.x - xl).abs() < 2.0 || (j.at.x - xr).abs() < 2.0)
        })
        .collect();
    assert_eq!(
        js.len(),
        2,
        "{:?}",
        ev.junctions().iter().map(|j| j.at).collect::<Vec<_>>()
    );
    for j in js {
        assert_eq!(j.arms.len(), 3);
        // The horizontal pair continues; the stem does not continue either.
        let horiz: Vec<usize> = (0..3)
            .filter(|&k| j.arms[k].direction.sin().abs() < 0.2)
            .collect();
        assert_eq!(horiz.len(), 2, "{:?}", j.arms);
        assert!(
            j.continuations
                .iter()
                .any(|&(a, b, _)| horiz.contains(&a) && horiz.contains(&b)),
            "{:?}",
            j.continuations
        );
        assert_eq!(j.continuations.len(), 1, "{:?}", j.continuations);
        // The local term against the true lines, each oriented as its edge.
        let x_j = if (j.at.x - xl).abs() < 2.0 { xl } else { xr };
        let mut curves: Vec<(usize, Vec<Piece>)> = Vec::new();
        for a in &j.arms {
            let e = &map.edges[a.edge as usize];
            let (p0, p1) = (e.points[0], *e.points.last().unwrap());
            let proj = |p: Point| {
                if a.direction.sin().abs() < 0.2 {
                    Point::new(p.x, yb)
                } else {
                    Point::new(x_j, p.y)
                }
            };
            let (mut q0, mut q1) = (proj(p0), proj(p1));
            if a.at_start {
                q0 = Point::new(x_j, yb);
            } else {
                q1 = Point::new(x_j, yb);
            }
            curves.push((a.edge as usize, vec![Piece::Line([q0, q1])]));
        }
        let refs: Vec<(usize, &[Piece])> = curves.iter().map(|(e, c)| (*e, c.as_slice())).collect();
        let c = ev.chi2_local(Owner::Junction(j.node), &refs);
        assert!(c.m > 30, "{c:?}");
        let ratio = c.chi2 / c.m as f64;
        assert!(ratio < 2.0, "local chi2/M = {ratio:.3} ({c:?})");
        // A vertex one pixel off is many standard deviations worse.
        let moved: Vec<(usize, Vec<Piece>)> = curves
            .iter()
            .map(|(e, c)| {
                let Piece::Line([a, b]) = c[0] else {
                    unreachable!()
                };
                let sh = |p: Point| {
                    if (p.x - x_j).abs() < 1e-9 && (p.y - yb).abs() < 1e-9 {
                        Point::new(p.x + 1.0, p.y)
                    } else {
                        p
                    }
                };
                (*e, vec![Piece::Line([sh(a), sh(b)])])
            })
            .collect();
        let refs2: Vec<(usize, &[Piece])> = moved.iter().map(|(e, c)| (*e, c.as_slice())).collect();
        let c2 = ev.chi2_local(Owner::Junction(j.node), &refs2);
        assert!(c2.chi2 > c.chi2 + 100.0, "{c2:?} vs {c:?}");
    }
}

/// A square rotated 10°: corner proposals near its four vertices, and few elsewhere.
#[test]
fn corner_proposals_find_the_corners() {
    let poly = square(31.7, 32.2, 15.0, 10f64.to_radians());
    let s = render(64, 64, WHITE, &[(poly.clone(), B)], true);
    let map = map_of(&s);
    let ev = build(&map, &s.rgb, &s.faces, 0.5 / 255.0, &exact_floor());
    let props = ev.corners();
    for v in &poly {
        assert!(
            props.iter().any(|c| c.at.dist(*v) < 2.5),
            "no proposal near {v:?}: {:?}",
            props.iter().map(|c| (c.at, c.z)).collect::<Vec<_>>()
        );
    }
    let windows: usize = (0..ev.edge_count()).map(|e| ev.runs(e).len()).sum();
    let false_props = props
        .iter()
        .filter(|c| poly.iter().all(|v| c.at.dist(*v) >= 2.5))
        .count();
    assert!(
        false_props * 10 <= windows,
        "{false_props} false proposals over {windows} windows"
    );
    // Edges at 10° are not on the lattice; the evidence says so.
    assert!((0..ev.edge_count()).all(|e| ev.runs(e).is_empty() || !ev.edge_on_lattice(e)));
    // The corner terms, scored against the true square in the edge's direction, are
    // calibrated, and a corner cut by a 1 px chamfer is many standard deviations worse
    // (pooled over several rotations: most corners lie inside windows the curve crosses,
    // and only a few pixels per square go to corner terms).
    let mut chi = Chi2::default();
    let mut cut = Chi2::default();
    for deg in [10.0f64, 23.0, 37.0, 52.0, 66.0, 79.0] {
        let poly = square(31.7, 32.2, 15.0, deg.to_radians());
        let s = render(64, 64, WHITE, &[(poly.clone(), B)], true);
        let map = map_of(&s);
        let ev = build(&map, &s.rgb, &s.faces, 0.5 / 255.0, &exact_floor());
        corner_terms_against(&ev, &poly, &mut chi, &mut cut);
    }
    assert!(chi.m >= 4, "{chi:?}");
    let ratio = chi.chi2 / chi.m as f64;
    assert!(ratio < 2.5, "corner chi2/M = {ratio:.3} ({chi:?})");
    assert!(cut.chi2 > chi.chi2 + 100.0, "{cut:?} vs {chi:?}");
    let _ = Axis::Row;
}

/// Score `ev`'s corner terms against the polygon `poly` (into `chi`) and against it with the
/// nearest vertex chamfered by 1 px (into `cut`).
fn corner_terms_against(ev: &Evidence, poly: &[Point], chi: &mut Chi2, cut: &mut Chi2) {
    let props = ev.corners();
    for (i, c) in props.iter().enumerate() {
        if ev.corner_pixels(i).is_empty() {
            continue;
        }
        let e = c.edge as usize;
        let (f, r) = (pieces(poly, false), pieces(poly, true));
        let cf = ev.chi2_local(Owner::Corner(i as u32), &[(e, &f)]);
        let cr = ev.chi2_local(Owner::Corner(i as u32), &[(e, &r)]);
        let (best, rev) = if cf.chi2 <= cr.chi2 {
            (cf, false)
        } else {
            (cr, true)
        };
        *chi = *chi + best;
        // Chamfer the nearest vertex.
        let k = (0..4)
            .min_by(|&a, &b| poly[a].dist(c.at).total_cmp(&poly[b].dist(c.at)))
            .unwrap();
        let (vp, v, vn) = (poly[(k + 3) % 4], poly[k], poly[(k + 1) % 4]);
        let towards = |a: Point, b: Point| {
            let l = a.dist(b);
            Point::new(a.x + (b.x - a.x) / l, a.y + (b.y - a.y) / l)
        };
        let mut chamfered: Vec<Point> = Vec::new();
        for (j, q) in poly.iter().enumerate() {
            if j == k {
                chamfered.push(towards(v, vp));
                chamfered.push(towards(v, vn));
            } else {
                chamfered.push(*q);
            }
        }
        *cut = *cut + ev.chi2_local(Owner::Corner(i as u32), &[(e, &pieces(&chamfered, rev))]);
    }
}

/// What the evidence sets aside, and how it answers requests it cannot score: a thin line's
/// pixels (both of its edges reach them) are a strip, pixels holding a third ink are listed,
/// an edge between inks too close to unmix has no windows, and a local term asked for an
/// unknown owner, or without the curves it needs, scores nothing.
#[test]
fn thin_features_third_inks_and_refusals() {
    let pt = Point::new;
    // A 1.2 px line between two blue blocks: its two long edges end at junctions there.
    let line = vec![pt(4.0, 20.3), pt(30.3, 20.3), pt(30.3, 21.5), pt(4.0, 21.5)];
    let block = vec![
        pt(30.3, 10.0),
        pt(50.0, 10.0),
        pt(50.0, 30.0),
        pt(30.3, 30.0),
    ];
    let post = vec![pt(1.0, 12.0), pt(4.0, 12.0), pt(4.0, 30.0), pt(1.0, 30.0)];
    // A black square with a 0.4 px red sliver along part of its top edge.
    let sq = vec![
        pt(10.3, 40.6),
        pt(50.2, 40.6),
        pt(50.2, 58.4),
        pt(10.3, 58.4),
    ];
    let sliver = vec![
        pt(20.0, 40.2),
        pt(40.0, 40.2),
        pt(40.0, 40.6),
        pt(20.0, 40.6),
    ];
    // A square too faint against the page to unmix.
    let faint = vec![pt(2.0, 2.0), pt(8.0, 2.0), pt(8.0, 8.0), pt(2.0, 8.0)];
    let s = render(
        64,
        64,
        WHITE,
        &[
            (line, B),
            (block, [0.1, 0.2, 0.8]),
            (post, [0.1, 0.2, 0.8]),
            (sq, B),
            (sliver, [0.9, 0.1, 0.1]),
            (faint, [0.99, 0.99, 0.99]),
        ],
        true,
    );
    let map = map_of(&s);
    let ev = build(
        &map,
        &s.rgb,
        &s.faces,
        0.5 / 255.0,
        &EvidenceOptions::default(),
    );
    assert_eq!(ev.size(), (64, 64));
    assert_eq!(ev.floor(), Floor::lattice(32));
    assert!(
        ev.strip_pixels()
            .iter()
            .any(|&(x, y)| (10..25).contains(&x) && (19..=22).contains(&y)),
        "{:?}",
        ev.strip_pixels()
    );
    assert!(
        ev.third_pixels()
            .iter()
            .any(|&(x, y)| (21..40).contains(&x) && (39..=41).contains(&y)),
        "{:?}",
        ev.third_pixels()
    );
    // No window lies on the faint square's edge.
    for e in 0..ev.edge_count() {
        for i in 0..ev.runs(e).len() {
            assert!(ev.window_pixels(e, i).iter().all(|&(x, y)| x > 9 || y > 9));
        }
    }
    // The information density is positive and finite wherever an edge has windows.
    let e = (0..ev.edge_count())
        .find(|&e| !ev.runs(e).is_empty())
        .expect("runs");
    let d = ev.density(e);
    assert_eq!(d.len(), ev.runs(e).len());
    assert!(d.iter().all(|&(_, r)| r > 0.0 && r.is_finite()));
    // Refusals.
    let none = Chi2::default();
    assert!(ev.local_pixels(u32::MAX).is_empty());
    assert_eq!(ev.chi2_local(Owner::Junction(u32::MAX), &[]), none);
    assert_eq!(ev.chi2_local(Owner::Corner(u32::MAX), &[]), none);
    let j = ev.junctions().first().expect("the line meets the block");
    assert_eq!(ev.chi2_local(Owner::Junction(j.node), &[]), none);
    assert!(!ev.corners().is_empty());
    assert_eq!(ev.chi2_local(Owner::Corner(0), &[]), none);
}
