//! Locating a tangential junction from the region that tapers to nothing there.
//!
//! The quantity under test is the intercept: where the tapering region vanishes. The
//! slope, and the radius it implies, are a check on the model rather than a result — the
//! quadratic is a contact approximation and drifts once the region is thick.

use inkvec_trace::taper;

/// Width of the gap between a line and a circle of radius `r` tangent to it, at distance
/// `u` from the tangent point. This is the exact form the fit approximates.
fn gap(r: f64, u: f64) -> f64 {
    if u.abs() >= r {
        return r;
    }
    r - (r * r - u * u).sqrt()
}

#[test]
fn it_finds_the_tangent_point_of_a_circle_on_a_line() {
    let (r, x0) = (14.22, 13.72);
    // Sampled the way an image samples it: one column per pixel, to the left of the
    // tangent point, measuring the gap that opens up as we move away.
    let samples: Vec<(f64, f64)> = (0..14).map(|x| (x as f64, gap(r, x0 - x as f64))).collect();
    let t = taper::fit(&samples).expect("a taper");
    assert!(
        (t.vanish - x0).abs() < 0.3,
        "vanish {:.2} should be near {x0:.2}",
        t.vanish
    );
    // The intercept must also be trustworthy, not merely close by luck.
    assert!(t.sigma < 0.5, "sigma {:.2} too loose", t.sigma);
}

#[test]
fn the_intercept_survives_measurement_noise() {
    let (r, x0) = (14.22, 13.72);
    // Deterministic pseudo-noise at the scale of a coverage measurement, about 0.02px of
    // area per column.
    let noise = |i: usize| {
        let h = ((i as f64 * 12.9898).sin() * 43758.5453).fract();
        0.04 * (h - 0.5)
    };
    let samples: Vec<(f64, f64)> = (0..14)
        .map(|x| (x as f64, (gap(r, x0 - x as f64) + noise(x)).max(0.0)))
        .collect();
    let t = taper::fit(&samples).expect("a taper");
    assert!(
        (t.vanish - x0).abs() < 0.5,
        "vanish {:.2} should survive noise near {x0:.2}",
        t.vanish
    );
}

/// The direction the samples run in must not matter.
#[test]
fn it_does_not_care_which_way_the_samples_run() {
    let (r, x0) = (10.0, 20.0);
    let fwd: Vec<(f64, f64)> = (10..20)
        .map(|x| (x as f64, gap(r, x0 - x as f64)))
        .collect();
    let mut rev = fwd.clone();
    rev.reverse();
    let a = taper::fit(&fwd).expect("forward");
    let b = taper::fit(&rev).expect("reversed");
    assert!((a.vanish - b.vanish).abs() < 1e-9);
}

/// A region of constant width is not a taper and must be refused rather than extrapolated
/// to some far-off intercept.
#[test]
fn a_parallel_gap_is_not_a_taper() {
    let samples: Vec<(f64, f64)> = (0..12).map(|x| (x as f64, 2.0)).collect();
    assert!(taper::fit(&samples).is_none());
}

/// Two boundaries crossing at an angle give a *linear* taper, and there the intersection
/// method already works. This must not quietly return a tangency estimate for one.
#[test]
fn a_transversal_wedge_is_not_mistaken_for_a_tangency() {
    let theta = 0.5f64; // radians, a wide crossing
    let x0 = 12.0;
    let samples: Vec<(f64, f64)> = (0..12)
        .map(|x| (x as f64, (theta * (x0 - x as f64)).max(0.0)))
        .collect();
    match taper::fit(&samples) {
        None => {}
        Some(t) => assert!(
            t.tangency_defect > 1.0,
            "a wedge came back as a clean tangency: defect {:.3}, vanish {:.2}",
            t.tangency_defect,
            t.vanish
        ),
    }
}

/// The measurement that motivated all of this, kept as data.
///
/// Transparent thickness per column along the top edge of twemoji `1f17e`, taken from the
/// alpha channel — the sub-pixel area that labelling cannot see. Ground truth for that
/// icon is a 36-unit box at 128px with corner radius 4, so the tangent point is at pixel
/// index 13.72. The planar map puts the junction at 10.5.
#[test]
fn it_recovers_a_real_tangent_point_that_the_planar_map_missed() {
    let measured: [(f64, f64); 13] = [
        (0.0, 10.929),
        (1.0, 8.000),
        (2.0, 6.188),
        (3.0, 4.937),
        (4.0, 3.875),
        (5.0, 3.063),
        (6.0, 2.251),
        (7.0, 1.749),
        (8.0, 1.251),
        (9.0, 0.875),
        (10.0, 0.565),
        (11.0, 0.251),
        (12.0, 0.251),
    ];
    let t = taper::fit(&measured).expect("a taper");
    assert!(
        (t.vanish - 13.72).abs() < 0.6,
        "vanish {:.2} should be near the true 13.72, not the planar map's 10.5",
        t.vanish
    );
    assert!(
        (t.vanish - 10.5).abs() > 1.5,
        "the estimate must actually move off the junction it is replacing"
    );
    assert!(
        (t.implied_radius - 14.22).abs() < 5.0,
        "implied radius {:.2} should be recognisably the true 14.22",
        t.implied_radius
    );
}

#[test]
fn too_few_usable_samples_is_refused() {
    let samples = [(0.0, 1.0), (1.0, 0.5)];
    assert!(taper::fit(&samples).is_none());
}
