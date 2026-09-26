//! Gradient geometry against known answers.
//!
//! The fits are fed exact synthetic fields -- colour an affine function of a known axis
//! coordinate, of the distance to a known centre, or of a known elliptical radius -- so the
//! true parameters are the unique zero-residual solution and can be asserted tightly. The
//! integration tests in `tests/gradient.rs` cover the same fits end to end on quantised
//! renders, with tolerances loose enough for 8-bit input.

use super::*;

/// Samples of the pixels of a `w x h` grid inside `member`, with the given linear-light
/// colour. Nothing is quantised: the field is exact.
fn exact_samples(
    w: usize,
    h: usize,
    member: impl Fn(f64, f64) -> bool,
    lin: impl Fn(f64, f64) -> [f64; 3],
) -> Samples {
    let mut s = Samples {
        px: vec![],
        x: vec![],
        y: vec![],
        srgb: vec![],
        lin: vec![],
    };
    for p in 0..w * h {
        let (x, y) = ((p % w) as f64, (p / w) as f64);
        if !member(x, y) {
            continue;
        }
        let c = lin(x, y);
        s.px.push(p);
        s.x.push(x);
        s.y.push(y);
        s.lin.push(c);
        s.srgb.push(to_srgb(c));
    }
    s
}

fn disc(cx: f64, cy: f64, r: f64) -> impl Fn(f64, f64) -> bool {
    move |x, y| (x - cx).powi(2) + (y - cy).powi(2) <= r * r
}

/// A colour affine in `t`: `base + t * slope`, per channel.
fn affine(base: [f64; 3], slope: [f64; 3], t: f64) -> [f64; 3] {
    [
        base[0] + slope[0] * t,
        base[1] + slope[1] * t,
        base[2] + slope[2] * t,
    ]
}

fn close3(a: [f32; 3], b: [f32; 3], tol: f32) -> bool {
    (0..3).all(|k| (a[k] - b[k]).abs() <= tol)
}

/// Difference of two axis directions, in degrees, modulo 180.
fn axis_diff_deg(a: f64, b: f64) -> f64 {
    let d = (a - b).to_degrees().rem_euclid(180.0);
    d.min(180.0 - d)
}

// ------------------------------------------------------------------ coordinates

#[test]
fn radial_coordinate_of_an_ellipse() {
    let ang = 30f64.to_radians();
    let (c, r, aspect) = ((3.0, -2.0), 10.0, 2.0);
    // Along the r semi-axis, 5 out: half way.
    let (x, y) = (3.0 + 5.0 * ang.cos(), -2.0 + 5.0 * ang.sin());
    assert!((radial_t(x, y, c, r, aspect, ang) - 0.5).abs() < 1e-12);
    // Across it, the semi-axis is r / aspect = 5: 2.5 out is half way.
    let (x, y) = (3.0 - 2.5 * ang.sin(), -2.0 + 2.5 * ang.cos());
    assert!((radial_t(x, y, c, r, aspect, ang) - 0.5).abs() < 1e-12);
    // Diagonal in the ellipse's frame: u = 3, v = 2 * 2 = 4, rho = 5.
    let (u, v) = (3.0, 2.0);
    let (x, y) = (
        3.0 + u * ang.cos() - v * ang.sin(),
        -2.0 + u * ang.sin() + v * ang.cos(),
    );
    assert!((radial_t(x, y, c, r, aspect, ang) - 0.5).abs() < 1e-12);
    // Circular, padded beyond the rim, and degenerate.
    assert!((radial_t(6.0, 8.0, (0.0, 0.0), 20.0, 1.0, 0.7) - 0.5).abs() < 1e-12);
    assert_eq!(radial_t(40.0, 0.0, (0.0, 0.0), 20.0, 1.0, 0.0), 1.0);
    assert_eq!(radial_t(4.0, 0.0, (0.0, 0.0), 0.0, 1.0, 0.0), 0.0);
}

#[test]
fn linear_coordinate_projects_onto_the_axis() {
    let (p0, p1) = ((1.0, 1.0), (5.0, 4.0)); // length 5
    assert!((linear_t(3.0, 2.5, p0, p1) - 0.5).abs() < 1e-12);
    // Perpendicular offset does not move t.
    assert!((linear_t(3.0 - 3.0, 2.5 + 4.0, p0, p1) - 0.5).abs() < 1e-12);
    assert_eq!(linear_t(-10.0, -10.0, p0, p1), 0.0);
    assert_eq!(linear_t(50.0, 50.0, p0, p1), 1.0);
    assert_eq!(linear_t(3.0, 3.0, p0, p0), 0.0);
}

#[test]
fn stop_interpolation_in_both_spaces() {
    let (k, w) = ([0.0f32; 3], [1.0f32; 3]);
    // Half way from black to white in linear light is linear 0.5, sRGB 0.7354.
    let m = lerp_stops(k, w, 0.5, Interp::LinearRgb);
    assert!(close3(m, [0.735_357; 3], 1e-5), "{m:?}");
    let m = lerp_stops(k, w, 0.25, Interp::Srgb);
    assert!(close3(m, [0.25; 3], 1e-7), "{m:?}");
    let c0 = [0.2f32, 0.4, 0.6];
    let c1 = [0.6f32, 0.2, 1.0];
    assert!(close3(lerp_stops(c0, c1, 0.0, Interp::LinearRgb), c0, 1e-6));
    assert!(close3(lerp_stops(c0, c1, 1.0, Interp::LinearRgb), c1, 1e-6));
    // A mid stop at 0.5: t = 0.25 is half way from c0 to the mid, t = 0.75 half way from
    // the mid to c1.
    let mid = [1.0f32, 1.0, 1.0];
    let mids = [(0.5, mid)];
    let a = eval_stops(k, &mids, w, 0.25, Interp::Srgb);
    assert!(close3(a, [0.5; 3], 1e-6), "{a:?}");
    let b = eval_stops([0.0; 3], &mids, [0.0; 3], 0.75, Interp::Srgb);
    assert!(close3(b, [0.5; 3], 1e-6), "{b:?}");
    // Two mid stops.
    let mids = [(0.2, [0.2f32; 3]), (0.6, [0.2f32; 3])];
    let c = eval_stops([0.0; 3], &mids, [1.0; 3], 0.8, Interp::Srgb);
    assert!(close3(c, [0.6; 3], 1e-6), "{c:?}");
    let c = eval_stops([0.0; 3], &mids, [1.0; 3], 0.1, Interp::Srgb);
    assert!(close3(c, [0.1; 3], 1e-6), "{c:?}");
}

#[test]
fn description_lengths() {
    let mids = vec![(0.5, [0.5f32; 3])];
    let radial = |aspect: f64, mids: Vec<(f64, [f32; 3])>| FillModel::Radial {
        c: (0.0, 0.0),
        r: 1.0,
        c0: [0.0; 3],
        c1: [1.0; 3],
        interp: Interp::Srgb,
        aspect,
        angle: 0.0,
        mids,
    };
    assert_eq!(FillModel::Flat([0.0; 3]).params(), 3.0);
    assert_eq!(radial(1.0, vec![]).params(), 9.0);
    assert_eq!(radial(2.0, vec![]).params(), 11.0);
    assert_eq!(radial(2.0, mids.clone()).params(), 15.0);
    assert_eq!(radial(1.0, mids.clone()).params(), 13.0);
    let linear = FillModel::Linear {
        p0: (0.0, 0.0),
        p1: (1.0, 0.0),
        c0: [0.0; 3],
        c1: [1.0; 3],
        interp: Interp::LinearRgb,
        mids,
    };
    assert_eq!(linear.params(), 14.0);
    assert_eq!(radial(2.0, vec![]).kind(), "ellipse/srgb");
    assert_eq!(radial(1.0, vec![]).kind(), "radial/srgb");
    let lin = |aspect: f64| FillModel::Radial {
        c: (0.0, 0.0),
        r: 1.0,
        c0: [0.0; 3],
        c1: [1.0; 3],
        interp: Interp::LinearRgb,
        aspect,
        angle: 0.0,
        mids: vec![],
    };
    assert_eq!(lin(1.0).kind(), "radial/lin");
    assert_eq!(lin(1.5).kind(), "ellipse/lin");
    assert_eq!(linear.kind(), "linear/lin");
    assert_eq!(FillModel::Flat([0.0; 3]).kind(), "flat");
    assert!(!FillModel::Flat([0.0; 3]).is_gradient() && linear.is_gradient());
    // One colour for the whole fill: the midpoint of the end stops.
    let rep = FillModel::Linear {
        p0: (0.0, 0.0),
        p1: (1.0, 0.0),
        c0: [0.2, 0.4, 1.0],
        c1: [0.6, 0.0, 0.5],
        interp: Interp::Srgb,
        mids: vec![],
    }
    .representative();
    assert!(close3(rep, [0.4, 0.2, 0.75], 1e-7), "{rep:?}");
    assert_eq!(FillModel::Flat([0.3; 3]).representative(), [0.3; 3]);
    assert!((bic_lambda(100) - 0.5 * 100f64.ln()).abs() < 1e-12);
    assert!((bic_lambda(0) - 0.5 * 2f64.ln()).abs() < 1e-12);
}

#[test]
fn unmixing_two_flats_uses_their_colours_and_euclidean_separation() {
    let a = FillModel::Flat([1.0, 0.0, 0.0]);
    let b = FillModel::Flat([0.0, 0.0, 1.0]);
    let (pa, pb, sep) = unmix_pair(&a, &b, 3.0, 4.0);
    assert_eq!((pa, pb), ([1.0, 0.0, 0.0], [0.0, 0.0, 1.0]));
    assert!((sep - 2f64.sqrt()).abs() < 1e-6);
    // Against a gradient: the pair that separates the faces more is chosen.
    let g = FillModel::Linear {
        p0: (0.0, 0.0),
        p1: (10.0, 0.0),
        c0: [0.0; 3],
        c1: [1.0, 0.0, 0.0],
        interp: Interp::Srgb,
        mids: vec![],
    };
    // At x = 10 the gradient is pure red, identical to `a`: the local pair separates
    // nothing, the representatives (red vs half red) separate 0.5.
    let (_, _, s) = unmix_pair(&a, &g, 10.0, 0.0);
    assert!((s - 0.5).abs() < 1e-6, "{s}");
    // At x = 0 the gradient is black: the local pair (red vs black) separates 1.
    let (la, lb, s) = unmix_pair(&a, &g, 0.0, 0.0);
    assert!((s - 1.0).abs() < 1e-6 && lb == [0.0; 3] && la == [1.0, 0.0, 0.0]);
}

#[test]
fn interior_count_is_strict_and_treats_the_picture_edge_as_boundary() {
    // A 3x3 block in a 5x5 image: its centre only.
    let block: Vec<usize> = (0..25)
        .filter(|p| (1..4).contains(&(p % 5)) && (1..4).contains(&(p / 5)))
        .collect();
    let member = |p: usize| block.contains(&p);
    assert_eq!(interior_count(&block, 5, 5, member), 1);
    // The whole image: only the 3x3 not touching the edge.
    let all: Vec<usize> = (0..25).collect();
    assert_eq!(interior_count(&all, 5, 5, |_| true), 9);
    let s = collect_samples(&[[0.5; 3]; 25], 5, 5, &all, |_| true, |_| true, true);
    assert_eq!(s.px, vec![6, 7, 8, 11, 12, 13, 16, 17, 18]);
    assert_eq!((s.x[4], s.y[4]), (2.0, 2.0));
}

// ------------------------------------------------------------------ residual machinery

#[test]
fn the_cached_residual_is_the_1d_least_squares_residual() {
    let s = exact_samples(20, 20, disc(9.5, 9.5, 9.0), |x, y| {
        // Deliberately not a function of any one distance: a real residual.
        [0.1 + 0.02 * x, 0.3 + 0.01 * y, 0.5 + 0.001 * x * y]
    });
    let idx: Vec<usize> = (0..s.len()).collect();
    let cols = s.colors(Interp::LinearRgb);
    let mut rz = Resid1d::new(&s, &idx, &cols);
    let c = (4.0, 13.0);
    let rho: Vec<f64> = (0..s.len())
        .map(|i| ((s.x[i] - c.0).powi(2) + (s.y[i] - c.1).powi(2)).sqrt())
        .collect();
    let want = fit_1d(&cols, &rho).0;
    let got = rz.radial(c);
    assert!(
        want > 1e-3,
        "the field must not fit a radial exactly ({want})"
    );
    assert!((got - want).abs() <= 1e-9 * want, "{got} vs {want}");
    // The elliptical frame at 40 degrees, aspect 1.7.
    let (sn, cs) = 40f64.to_radians().sin_cos();
    let ell: Vec<f64> = (0..s.len())
        .map(|i| {
            let (dx, dy) = (s.x[i] - c.0, s.y[i] - c.1);
            let (u, v) = (dx * cs + dy * sn, (-dx * sn + dy * cs) * 1.7);
            (u * u + v * v).sqrt()
        })
        .collect();
    let want = fit_1d(&cols, &ell).0;
    let got = rz.elliptic(c, sn, cs, 1.7);
    assert!((got - want).abs() <= 1e-9 * want, "{got} vs {want}");
    // An exact affine function of the distance leaves nothing.
    let e = exact_samples(20, 20, disc(9.5, 9.5, 9.0), |x, y| {
        let d = ((x - c.0).powi(2) + (y - c.1).powi(2)).sqrt();
        affine([0.1, 0.2, 0.3], [0.01, 0.02, -0.005], d)
    });
    let idx: Vec<usize> = (0..e.len()).collect();
    let cols = e.colors(Interp::LinearRgb);
    let mut rz = Resid1d::new(&e, &idx, &cols);
    assert!(rz.radial(c) < 1e-10);
    assert!(rz.radial((c.0 + 1.0, c.1)) > 1e-6);
}

#[test]
fn fit_1d_recovers_an_exact_line() {
    let t: Vec<f64> = (0..10).map(|i| i as f64 * 0.7 - 1.0).collect();
    let cols: Vec<[f64; 3]> = t
        .iter()
        .map(|&v| affine([0.2, 0.5, 0.9], [0.1, -0.05, 0.0], v))
        .collect();
    let (resid, a, g) = fit_1d(&cols, &t);
    assert!(resid < 1e-12);
    for k in 0..3 {
        assert!((a[k] - [0.2, 0.5, 0.9][k]).abs() < 1e-12);
        assert!((g[k] - [0.1, -0.05, 0.0][k]).abs() < 1e-12);
    }
}

#[test]
fn colour_axes_are_unit_principal_directions() {
    // Colours spread along (1, 2, 2) / 3 -- and only channel 0 varies in the second set.
    let dir = [1.0 / 3.0, 2.0 / 3.0, 2.0 / 3.0];
    let cols: Vec<[f64; 3]> = (0..9)
        .map(|i| affine([0.1, 0.1, 0.1], dir, 0.05 * i as f64))
        .collect();
    let v = color_axis(&cols).expect("an axis");
    let dot = v[0] * dir[0] + v[1] * dir[1] + v[2] * dir[2];
    assert!((dot.abs() - 1.0).abs() < 1e-9, "{v:?}");
    assert!(((v[0] * v[0] + v[1] * v[1] + v[2] * v[2]) - 1.0).abs() < 1e-12);
    let red: Vec<[f64; 3]> = (0..9).map(|i| [0.1 * i as f64, 0.5, 0.5]).collect();
    let v = color_axis(&red).expect("one channel varying is still an axis");
    assert!((v[0].abs() - 1.0).abs() < 1e-9 && v[1].abs() < 1e-9 && v[2].abs() < 1e-9);
    assert!(color_axis(&[[0.3; 3]; 5]).is_none());

    // The sRGB-sample version used by the two-flat test.
    let s = exact_samples(6, 6, |_, _| true, |x, _| [x * 0.1, 0.2 + x * 0.05, 0.3]);
    let idx: Vec<usize> = (0..s.len()).collect();
    let mean = {
        let mut m = [0.0; 3];
        for c in &s.srgb {
            for k in 0..3 {
                m[k] += c[k] as f64 / s.len() as f64;
            }
        }
        m
    };
    let v = dominant_color_axis(&s, &idx, mean).expect("an axis");
    assert!(((v[0] * v[0] + v[1] * v[1] + v[2] * v[2]) - 1.0).abs() < 1e-9);
    assert!(v[2].abs() < 1e-9 && v[0].abs() > v[1].abs(), "{v:?}");
}

#[test]
fn two_flat_residual_matches_a_brute_force_split() {
    // Grey levels, 12 at 0.1, 6 at 0.45, 10 at 0.9: along the grey axis the best 2-split
    // is {0.1, 0.45} | {0.9}. The oracle tries every split of the sorted values.
    let levels: Vec<f32> = [vec![0.1f32; 12], vec![0.45; 6], vec![0.9; 10]].concat();
    let n = levels.len();
    let s = Samples {
        px: (0..n).collect(),
        x: (0..n).map(|i| i as f64).collect(),
        y: vec![0.0; n],
        srgb: levels.iter().map(|&v| [v; 3]).collect(),
        lin: levels
            .iter()
            .map(|&v| [srgb_to_linear(v) as f64; 3])
            .collect(),
    };
    let sigma = 2.0 / 255.0;
    let mut sorted: Vec<f64> = levels.iter().map(|&v| v as f64).collect();
    sorted.sort_by(f64::total_cmp);
    let mut best = (f64::INFINITY, 0usize);
    for k in 1..n {
        let (lo, hi) = sorted.split_at(k);
        let sse = |g: &[f64]| {
            let m = g.iter().sum::<f64>() / g.len() as f64;
            g.iter().map(|v| (v - m).powi(2)).sum::<f64>()
        };
        let e = sse(lo) + sse(hi);
        if e < best.0 {
            best = (e, k);
        }
    }
    let (lo, hi) = sorted.split_at(best.1);
    let chi = |g: &[f64]| {
        let m = g.iter().sum::<f64>() / g.len() as f64;
        g.iter()
            .map(|v| ((v - m).abs() - QUANT_HALF_STEP).max(0.0).powi(2))
            .sum::<f64>()
    };
    // Three channels, all equal.
    let want = 3.0 * (chi(lo) + chi(hi)) / (sigma * sigma);
    let got = chi2_two_flats(&s, sigma);
    assert!((got - want).abs() <= 1e-6 * want, "{got} vs {want}");
    // Exactly two levels leave nothing beyond the dead zone.
    let two: Vec<f32> = [vec![0.2f32; 7], vec![0.7; 9]].concat();
    let s2 = Samples {
        px: (0..16).collect(),
        x: (0..16).map(|i| i as f64).collect(),
        y: vec![0.0; 16],
        srgb: two.iter().map(|&v| [v; 3]).collect(),
        lin: two.iter().map(|&v| [v as f64; 3]).collect(),
    };
    assert_eq!(chi2_two_flats(&s2, sigma), 0.0);
    // One level cannot be split.
    let one = Samples {
        px: (0..8).collect(),
        x: (0..8).map(|i| i as f64).collect(),
        y: vec![0.0; 8],
        srgb: vec![[0.4; 3]; 8],
        lin: vec![[0.13; 3]; 8],
    };
    assert_eq!(chi2_two_flats(&one, sigma), f64::INFINITY);
}

// ------------------------------------------------------------------ fits

#[test]
fn linear_fit_recovers_axis_endpoints_and_stops_exactly() {
    // Several axes -- 100 and 60 degrees are where a wrong PCA seed would sit more than the
    // +-45 degree scan away from the truth -- on a disc so the extremes are well defined.
    for deg in [100.0, 60.0, 20.0] {
        linear_fit_case(deg);
    }
}

fn linear_fit_case(deg: f64) {
    let theta = f64::to_radians(deg);
    let (dc, ds) = (theta.cos(), theta.sin());
    let base = [0.4, 0.45, 0.5];
    let slope = [0.02, 0.012, -0.018];
    let field = |x: f64, y: f64| affine(base, slope, (x - 20.0) * dc + (y - 21.0) * ds);
    let s = exact_samples(44, 44, disc(21.3, 20.6, 15.0), field);
    let cols = s.colors(Interp::LinearRgb);
    let Some(FillModel::Linear {
        p0,
        p1,
        c0,
        c1,
        interp,
        mids,
    }) = fit_linear(&s, &cols, Interp::LinearRgb)
    else {
        panic!("no linear fit");
    };
    assert_eq!(interp, Interp::LinearRgb);
    assert!(mids.is_empty());
    let got = (p1.1 - p0.1).atan2(p1.0 - p0.0);
    assert!(
        axis_diff_deg(got, theta) < 1e-3,
        "axis {}",
        got.to_degrees()
    );
    // Endpoints: the extreme samples' projections onto the true axis through the centroid.
    let (xc, yc) = s.centroid();
    let proj: Vec<f64> = (0..s.len())
        .map(|i| (s.x[i] - xc) * dc + (s.y[i] - yc) * ds)
        .collect();
    let (smin, smax) = proj
        .iter()
        .fold((f64::MAX, f64::MIN), |(a, b), &v| (a.min(v), b.max(v)));
    let ends = [
        (xc + smin * dc, yc + smin * ds),
        (xc + smax * dc, yc + smax * ds),
    ];
    let (e0, e1) = if (p0.0 - ends[0].0).hypot(p0.1 - ends[0].1) < 1.0 {
        (ends[0], ends[1])
    } else {
        (ends[1], ends[0])
    };
    for (got, want) in [(p0, e0), (p1, e1)] {
        assert!(
            (got.0 - want.0).hypot(got.1 - want.1) < 1e-3,
            "end {got:?} vs {want:?}"
        );
    }
    // Stops are the field's own colour at the ends.
    assert!(close3(c0, to_srgb(field(e0.0, e0.1)), 1e-4), "{c0:?}");
    assert!(close3(c1, to_srgb(field(e1.0, e1.1)), 1e-4), "{c1:?}");
    // The model reproduces every sample.
    let model = fit_linear(&s, &cols, Interp::LinearRgb).unwrap();
    for i in (0..s.len()).step_by(7) {
        assert!(close3(model.color_at(s.x[i], s.y[i]), s.srgb[i], 2e-4));
    }
}

/// The fitted linear axis, the least-squares axis found by brute force, and the PCA seed,
/// in degrees, for a region `50 x h` whose colour channels vary along different directions
/// (channel 1 by `c1y` per pixel down y). With positions that are not isotropic the seed is
/// not the least-squares axis, and the scan and the golden-section refinement have to find
/// it. The oracle scans every 0.1 degree, then every 0.0005 degree around the best, with a
/// from-scratch 1-D least-squares fit.
fn anisotropic_linear_case(h: usize, c1y: f64) -> (f64, f64, f64) {
    let s = exact_samples(
        60,
        h + 10,
        |x, y| (5.0..=54.0).contains(&x) && (5.0..(5 + h) as f64).contains(&y),
        |x, y| [0.1 + 0.01 * x, 0.2 + c1y * y, 0.5 + 0.004 * x - 0.01 * y],
    );
    let cols = s.colors(Interp::LinearRgb);
    let Some(FillModel::Linear { p0, p1, .. }) = fit_linear(&s, &cols, Interp::LinearRgb) else {
        panic!("no linear fit");
    };
    let got = (p1.1 - p0.1).atan2(p1.0 - p0.0);
    let resid_at = |th: f64| {
        let t: Vec<f64> = (0..s.len())
            .map(|i| s.x[i] * th.cos() + s.y[i] * th.sin())
            .collect();
        fit_1d(&cols, &t).0
    };
    let scan = |from: f64, step: f64, n: usize| {
        (0..n)
            .map(|k| (from + k as f64 * step).to_radians())
            .map(|th| (th, resid_at(th)))
            .fold((0.0, f64::INFINITY), |a, b| if b.1 < a.1 { b } else { a })
    };
    let coarse = scan(0.0, 0.1, 1800);
    let (best, _) = scan(coarse.0.to_degrees() - 0.2, 0.0005, 800);
    // The seed: the principal direction of the least-squares slope matrix.
    let (xc, yc) = s.centroid();
    let (mut sxx, mut sxy, mut syy) = (0.0, 0.0, 0.0);
    let mut sc = [[0.0; 2]; 3];
    let cbar = mean3(&cols);
    for (i, c) in cols.iter().enumerate() {
        let (dx, dy) = (s.x[i] - xc, s.y[i] - yc);
        sxx += dx * dx;
        sxy += dx * dy;
        syy += dy * dy;
        for k in 0..3 {
            sc[k][0] += dx * (c[k] - cbar[k]);
            sc[k][1] += dy * (c[k] - cbar[k]);
        }
    }
    let det = sxx * syy - sxy * sxy;
    let mut btb = [0.0; 3];
    for row in sc {
        let bx = (syy * row[0] - sxy * row[1]) / det;
        let by = (sxx * row[1] - sxy * row[0]) / det;
        btb[0] += bx * bx;
        btb[1] += bx * by;
        btb[2] += by * by;
    }
    let seed = 0.5 * (2.0 * btb[1]).atan2(btb[0] - btb[2]);
    (got, best, seed)
}

#[test]
fn linear_fit_searches_past_a_poor_seed_to_the_least_squares_axis() {
    // 50 x 30: the seed is 32 degrees off the least-squares axis, inside the scan's reach.
    let (got, best, seed) = anisotropic_linear_case(30, 0.01);
    assert!(
        axis_diff_deg(seed, best) > 20.0,
        "precondition: seed {}",
        seed.to_degrees()
    );
    assert!(
        axis_diff_deg(got, best) < 0.01,
        "fit {} vs best {}",
        got.to_degrees(),
        best.to_degrees()
    );
}

#[test]
#[ignore = "BUG: the PCA seed ignores the positions' covariance (gradient.rs:702-711), and on             a long thin region it can land 60 degrees from the least-squares axis, beyond the             +-45 degree scan (gradient.rs:728-736): the fit returns -41.4 deg, residual 4.080,             where -27.4 deg gives 4.040"]
fn linear_fit_finds_the_least_squares_axis_on_a_long_thin_region() {
    // 50 x 10: the seed lands 60 degrees off, at the residual's maximum.
    let (got, best, seed) = anisotropic_linear_case(10, 0.03);
    assert!(
        axis_diff_deg(seed, best) > 45.0,
        "precondition: seed {}",
        seed.to_degrees()
    );
    assert!(
        axis_diff_deg(got, best) < 0.01,
        "fit {} vs best {}",
        got.to_degrees(),
        best.to_degrees()
    );
}

#[test]
fn linear_fit_refuses_collinear_or_tiny_regions() {
    let line = exact_samples(40, 3, |_, y| y == 1.0, |x, _| [x * 0.02; 3]);
    assert!(fit_linear(&line, &line.colors(Interp::Srgb), Interp::Srgb).is_none());
    let tiny = exact_samples(3, 3, |_, _| true, |x, _| [x * 0.2; 3]);
    assert!(fit_linear(&tiny, &tiny.colors(Interp::Srgb), Interp::Srgb).is_none());
}

#[test]
fn radial_fit_recovers_the_centre_and_stops() {
    let centre = (18.3, 23.6);
    let base = [0.9, 0.7, 0.2];
    let slope = [-0.02, -0.015, 0.01];
    let field = |x: f64, y: f64| affine(base, slope, (x - centre.0).hypot(y - centre.1));
    let s = exact_samples(48, 48, disc(22.0, 22.0, 17.0), field);
    let cols = s.colors(Interp::LinearRgb);
    let Some(FillModel::Radial {
        c,
        r,
        c0,
        c1,
        aspect,
        angle,
        interp,
        ..
    }) = fit_radial(&s, &cols, Interp::LinearRgb, 48)
    else {
        panic!("no radial fit");
    };
    assert_eq!((aspect, angle, interp), (1.0, 0.0, Interp::LinearRgb));
    let err = (c.0 - centre.0).hypot(c.1 - centre.1);
    assert!(err < 0.05, "centre {c:?} off by {err}");
    // The radius is the farthest sample from the fitted centre.
    let far = (0..s.len())
        .map(|i| (s.x[i] - c.0).hypot(s.y[i] - c.1))
        .fold(0.0, f64::max);
    assert!((r - far).abs() < 1e-9);
    assert!(close3(c0, to_srgb(base), 2e-3), "centre colour {c0:?}");
    assert!(
        close3(c1, to_srgb(affine(base, slope, r)), 2e-3),
        "rim colour {c1:?}"
    );
}

#[test]
fn radial_fit_finds_a_centre_outside_the_region() {
    // The region is a band to the right of the centre: the gradient lines converge
    // outside it, within the one-width allowance.
    let centre = (4.0, 20.0);
    let field = |x: f64, y: f64| {
        affine(
            [0.1, 0.2, 0.3],
            [0.012, 0.01, 0.008],
            (x - centre.0).hypot(y - centre.1),
        )
    };
    let s = exact_samples(
        40,
        40,
        |x, y| (14.0..=30.0).contains(&x) && (6.0..=34.0).contains(&y),
        field,
    );
    let cols = s.colors(Interp::LinearRgb);
    let Some(FillModel::Radial { c, .. }) = fit_radial(&s, &cols, Interp::LinearRgb, 40) else {
        panic!("no radial fit");
    };
    let err = (c.0 - centre.0).hypot(c.1 - centre.1);
    assert!(err < 0.1, "centre {c:?} off by {err}");
}

#[test]
fn elliptical_fit_recovers_centre_orientation_and_aspect() {
    let (centre, angle, aspect) = ((24.3, 26.1), 35f64.to_radians(), 2.2);
    let (sn, cs) = angle.sin_cos();
    let rho = move |x: f64, y: f64| {
        let (dx, dy) = (x - centre.0, y - centre.1);
        let (u, v) = (dx * cs + dy * sn, (-dx * sn + dy * cs) * aspect);
        (u * u + v * v).sqrt()
    };
    let base = [0.8, 0.3, 0.1];
    let slope = [-0.01, 0.012, 0.015];
    let field = |x: f64, y: f64| affine(base, slope, rho(x, y));
    let s = exact_samples(50, 50, disc(25.0, 25.0, 18.0), field);
    let cols = s.colors(Interp::LinearRgb);
    let circ = fit_radial(&s, &cols, Interp::LinearRgb, 50).expect("circular seed");
    let Some(FillModel::Radial {
        c,
        r,
        c0,
        c1,
        aspect: got_aspect,
        angle: got_angle,
        ..
    }) = fit_radial_elliptic(&s, &cols, Interp::LinearRgb, &circ)
    else {
        panic!("no elliptical fit");
    };
    let err = (c.0 - centre.0).hypot(c.1 - centre.1);
    assert!(err < 0.1, "centre {c:?} off by {err}");
    assert!(
        (got_aspect / aspect - 1.0).abs() < 0.01,
        "aspect {got_aspect}"
    );
    assert!(
        axis_diff_deg(got_angle, angle) < 0.5,
        "angle {}",
        got_angle.to_degrees()
    );
    let far = (0..s.len())
        .map(|i| rho(s.x[i], s.y[i]))
        .fold(0.0, f64::max);
    assert!((r / far - 1.0).abs() < 0.01, "radius {r} vs {far}");
    assert!(close3(c0, to_srgb(base), 3e-3), "centre colour {c0:?}");
    assert!(
        close3(c1, to_srgb(affine(base, slope, r)), 3e-3),
        "rim colour {c1:?}"
    );
    // A circle stays a circle.
    let round = exact_samples(50, 50, disc(25.0, 25.0, 18.0), |x, y| {
        affine(base, slope, (x - 24.0).hypot(y - 25.5))
    });
    let rc = round.colors(Interp::LinearRgb);
    let seed = fit_radial(&round, &rc, Interp::LinearRgb, 50).unwrap();
    assert!(fit_radial_elliptic(&round, &rc, Interp::LinearRgb, &seed).is_none());
    // Not a radial seed, or too few samples: nothing to refine.
    assert!(
        fit_radial_elliptic(&s, &cols, Interp::LinearRgb, &FillModel::Flat([0.0; 3])).is_none()
    );
}

/// Renders an elliptical ramp the way `tests/gradient.rs` does -- in linear light, then
/// sRGB, then 8 bits -- and checks the public selection keeps the ellipse.
#[test]
fn an_elliptical_ramp_is_chosen_as_an_ellipse_end_to_end() {
    let (w, h) = (80usize, 80usize);
    let (centre, angle, aspect) = ((40.5, 38.0), 25f64.to_radians(), 2.0);
    let (sn, cs) = angle.sin_cos();
    let c0 = [1.0f32, 0.95, 0.8];
    let c1 = [0.55f32, 0.15, 0.05];
    let (l0, l1) = (to_lin(c0), to_lin(c1));
    let mut rgb = vec![[0.95f32; 3]; w * h];
    let mut labels = vec![0u16; w * h];
    for p in 0..w * h {
        let (x, y) = ((p % w) as f64, (p / w) as f64);
        let (dx, dy) = (x - centre.0, y - centre.1);
        let (u, v) = (dx * cs + dy * sn, (-dx * sn + dy * cs) * aspect);
        let rho = (u * u + v * v).sqrt();
        if rho > 36.0 {
            continue;
        }
        let t = rho / 40.0;
        let lin = [
            l0[0] + (l1[0] - l0[0]) * t,
            l0[1] + (l1[1] - l0[1]) * t,
            l0[2] + (l1[2] - l0[2]) * t,
        ];
        let c = to_srgb(lin);
        rgb[p] = [
            (c[0] * 255.0).round() / 255.0,
            (c[1] * 255.0).round() / 255.0,
            (c[2] * 255.0).round() / 255.0,
        ];
        labels[p] = 1;
    }
    let n = labels.iter().filter(|&&l| l == 1).count();
    let cands = fit_candidates(&rgb, w, h, &labels, 1, 1.0 / 255.0, bic_lambda(n));
    let ell = cands
        .iter()
        .find(|f| f.model.kind() == "ellipse/lin")
        .unwrap_or_else(|| {
            panic!(
                "no elliptical candidate among {:?}",
                cands.iter().map(|f| f.model.kind()).collect::<Vec<_>>()
            )
        });
    let FillModel::Radial {
        c,
        aspect: a,
        angle: g,
        ..
    } = ell.model
    else {
        unreachable!()
    };
    assert!((c.0 - centre.0).hypot(c.1 - centre.1) < 0.3, "centre {c:?}");
    assert!((a / aspect - 1.0).abs() < 0.03, "aspect {a}");
    assert!(axis_diff_deg(g, angle) < 1.5, "angle {}", g.to_degrees());
    // And it wins: every circle and every line costs more.
    let best = select(cands.clone());
    assert_eq!(best.model.kind(), "ellipse/lin", "chose {:?}", best.model);
}

#[test]
fn contrast_and_support_of_a_ramp() {
    // Black to white across x = 0..=10 in sRGB: range 1 in every channel. Against the mean
    // (mid-grey), a sample is "shaded" when it is more than a quarter of the contrast away
    // in RGB distance, |v - 0.5| * sqrt(3) > 0.25: x = 0..=3 and 7..=10, 8 of 11.
    let s = exact_samples(11, 1, |_, _| true, |x, _| [x / 10.0; 3]);
    let ramp = FillModel::Linear {
        p0: (0.0, 0.0),
        p1: (10.0, 0.0),
        c0: [0.0; 3],
        c1: [1.0; 3],
        interp: Interp::Srgb,
        mids: vec![],
    };
    let contrast = visible_contrast(&ramp, &s);
    assert!((contrast - 1.0).abs() < 1e-6, "{contrast}");
    let support = ramp_support(&ramp, &s, [0.5; 3], contrast);
    assert!((support - 8.0 / 11.0).abs() < 1e-12, "{support}");
    assert_eq!(visible_contrast(&FillModel::Flat([0.3; 3]), &s), 0.0);
}

#[test]
fn a_lost_feature_at_one_end_leaves_the_region_flat() {
    // Solid near-black, lightening over its last two columns only (a lost feature, not
    // shading): the region stays flat, at its median rather than a mean the outliers
    // pull up.
    let (w, h) = (40usize, 40usize);
    let mut rgb = vec![[0.95f32; 3]; w * h];
    let mut labels = vec![0u16; w * h];
    for y in 4..36 {
        for x in 4..36 {
            let p = y * w + x;
            let v = if x >= 33 {
                0.02 + 0.2 * (x - 32) as f32
            } else {
                0.02
            };
            rgb[p] = [v; 3];
            labels[p] = 1;
        }
    }
    let n = labels.iter().filter(|&&l| l == 1).count();
    let cands = fit_candidates(&rgb, w, h, &labels, 1, 1.0 / 255.0, bic_lambda(n));
    assert_eq!(
        cands.len(),
        1,
        "{:?}",
        cands.iter().map(|f| f.model.kind()).collect::<Vec<_>>()
    );
    assert!(matches!(cands[0].model, FillModel::Flat(c) if close3(c, [0.02; 3], 1e-6)));
}
