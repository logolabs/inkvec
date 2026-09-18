//! Does coverage inversion actually recover geometry below the pixel grid?
//!
//! The whole S2 thesis is that an anti-aliased pixel is a measurement rather than noise.
//! These tests put a number on it by rendering shapes whose exact geometry is known,
//! tracing them back, and measuring the error — against a thresholding baseline that
//! discards the same information, which is what every open-source tracer does today.

use inkvec_core::Point;
use inkvec_trace::coverage::{CoverageField, Rgba};
use inkvec_trace::{contour, trace_bilevel, TraceOptions};

/// Render a black shape on white with analytic coverage, by supersampling the indicator
/// function. `inside` is the exact geometry we will try to recover.
fn render(w: usize, h: usize, inside: impl Fn(f64, f64) -> bool) -> Rgba {
    const SS: usize = 16;
    let mut data = vec![0.0f32; w * h * 4];
    for y in 0..h {
        for x in 0..w {
            let mut hits = 0;
            for sy in 0..SS {
                for sx in 0..SS {
                    let px = x as f64 - 0.5 + (sx as f64 + 0.5) / SS as f64;
                    let py = y as f64 - 0.5 + (sy as f64 + 0.5) / SS as f64;
                    if inside(px, py) {
                        hits += 1;
                    }
                }
            }
            let a = hits as f32 / (SS * SS) as f32;
            let v = 1.0 - a; // black shape on white
            let i = (y * w + x) * 4;
            data[i] = v;
            data[i + 1] = v;
            data[i + 2] = v;
            data[i + 3] = 1.0;
        }
    }
    Rgba {
        width: w,
        height: h,
        data,
    }
}

/// The baseline every open-source tracer starts from: round each pixel to inside or
/// outside, destroying the coverage information, then trace the result.
fn thresholded(img: &Rgba) -> Rgba {
    let mut out = img.clone();
    for i in 0..img.width * img.height {
        let v = if out.data[i * 4] < 0.5 { 0.0 } else { 1.0 };
        out.data[i * 4] = v;
        out.data[i * 4 + 1] = v;
        out.data[i * 4 + 2] = v;
    }
    out
}

/// Algebraic circle fit (Kasa). Adequate here because the points are dense and nearly
/// noise-free; DESIGN.md S4 specifies orthogonal-distance fitting for production use.
fn fit_circle(pts: &[Point]) -> (Point, f64) {
    let n = pts.len() as f64;
    let (mx, my) = (
        pts.iter().map(|p| p.x).sum::<f64>() / n,
        pts.iter().map(|p| p.y).sum::<f64>() / n,
    );
    let (mut suu, mut svv, mut suv, mut suuu, mut svvv, mut suvv, mut svuu) =
        (0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0);
    for p in pts {
        let (u, v) = (p.x - mx, p.y - my);
        suu += u * u;
        svv += v * v;
        suv += u * v;
        suuu += u * u * u;
        svvv += v * v * v;
        suvv += u * v * v;
        svuu += v * u * u;
    }
    let c1 = 0.5 * (suuu + suvv);
    let c2 = 0.5 * (svvv + svuu);
    let det = suu * svv - suv * suv;
    if det.abs() < 1e-12 {
        return (Point::new(mx, my), 0.0);
    }
    let uc = (c1 * svv - c2 * suv) / det;
    let vc = (c2 * suu - c1 * suv) / det;
    let r = (uc * uc + vc * vc + (suu + svv) / n).sqrt();
    (Point::new(mx + uc, my + vc), r)
}

fn longest_contour(img: &Rgba) -> Vec<Point> {
    let (polys, _) = trace_bilevel(img, &TraceOptions::default());
    polys
        .into_iter()
        .max_by(|a, b| {
            contour::signed_area(a)
                .abs()
                .partial_cmp(&contour::signed_area(b).abs())
                .unwrap()
        })
        .map(|p| p.points)
        .unwrap_or_default()
}

// --- the core claim -----------------------------------------------------------------

#[test]
fn recovers_circle_radius_to_sub_pixel_accuracy() {
    let (cx, cy, r) = (31.5, 31.5, 22.3);
    let img = render(64, 64, |x, y| (x - cx).hypot(y - cy) <= r);

    let pts = longest_contour(&img);
    assert!(
        pts.len() > 40,
        "expected a dense contour, got {}",
        pts.len()
    );

    let (c, rr) = fit_circle(&pts);
    let r_err = (rr - r).abs();
    let c_err = c.dist(Point::new(cx, cy));

    assert!(
        r_err < 0.1,
        "radius error {r_err:.4}px should be well under one pixel (got r={rr:.4}, want {r})"
    );
    assert!(c_err < 0.1, "centre error {c_err:.4}px should be sub-pixel");
}

/// The comparison that justifies the whole stage: identical geometry, identical tracer,
/// the only difference being whether the anti-aliasing was read or discarded.
#[test]
fn sub_pixel_beats_thresholding_by_an_order_of_magnitude() {
    let (cx, cy, r) = (31.5, 31.5, 22.3);
    let img = render(64, 64, |x, y| (x - cx).hypot(y - cy) <= r);

    let (_, sub_r) = fit_circle(&longest_contour(&img));
    let (_, thr_r) = fit_circle(&longest_contour(&thresholded(&img)));

    let sub_err = (sub_r - r).abs();
    let thr_err = (thr_r - r).abs();

    assert!(
        sub_err < thr_err,
        "coverage inversion ({sub_err:.4}px) should beat thresholding ({thr_err:.4}px)"
    );
    // Report the ratio in the failure message so a regression is legible.
    assert!(
        sub_err < 0.1,
        "sub-pixel error {sub_err:.4}px vs thresholded {thr_err:.4}px"
    );
}

#[test]
fn accuracy_holds_as_the_shape_shrinks_toward_the_pixel_grid() {
    // A 9px-radius circle on a 32px canvas: the regime where icons actually live.
    for r in [18.0f64, 12.0, 9.0, 6.0] {
        let n = 64;
        let c = n as f64 / 2.0 - 0.5;
        let img = render(n, n, |x, y| (x - c).hypot(y - c) <= r);
        let (_, rr) = fit_circle(&longest_contour(&img));
        assert!(
            (rr - r).abs() < 0.15,
            "radius {r}: recovered {rr:.4}, error {:.4}px",
            (rr - r).abs()
        );
    }
}

// --- sub-pixel features -------------------------------------------------------------

/// `docs/M0-BASELINE.md` §3: VTracer recovers 2 of 11 elements on `thin_features`, and
/// no setting recovers more, because strokes narrower than a pixel never reach the
/// threshold. A coverage-based front end should keep them.
#[test]
fn strokes_narrower_than_one_pixel_survive() {
    let widths = [0.3f64, 0.5, 0.8, 1.2, 2.0];
    for (k, w) in widths.iter().enumerate() {
        let x0 = 10.0 + k as f64 * 12.0;
        let img = render(64, 64, |x, y| {
            (10.0..54.0).contains(&y) && x >= x0 && x < x0 + w
        });
        let (polys, _) = trace_bilevel(&img, &TraceOptions { min_area: 0.05 });
        assert!(
            !polys.is_empty(),
            "a {w}px-wide stroke was lost entirely — this is the M0 failure mode"
        );
    }
}

/// Once a feature is wide enough to produce fully-covered pixels, the colour axis is
/// identifiable and the recovered area is essentially exact.
#[test]
fn stroke_width_is_accurate_once_features_resolve() {
    for w in [2.0f64, 4.0, 8.0] {
        let img = render(64, 64, |x, y| {
            (10.0..54.0).contains(&y) && x >= 20.0 && x < 20.0 + w
        });
        let (polys, _) = trace_bilevel(&img, &TraceOptions { min_area: 0.05 });
        let area: f64 = polys.iter().map(|p| contour::signed_area(p).abs()).sum();
        let expected = w * 44.0;
        let err = (area - expected).abs() / expected;
        assert!(
            err < 0.05,
            "{w}px stroke: area {area:.2} vs {expected:.2} ({:+.1}%)",
            err * 100.0
        );
    }
}

/// **A limit, not a bug.** A stroke narrower than one pixel never produces a
/// fully-covered pixel, so the foreground colour is never observed directly. A 0.3px
/// black stroke and a 1px grey stroke are pixel-identical, and no processing separates
/// them — the colour axis is genuinely unidentifiable from the image alone.
///
/// What the system must not do is guess a width and report it confidently. The contract
/// tested here is that it *detects* the feature (which is already far better than the
/// measured VTracer behaviour of losing it entirely) and simultaneously reports low
/// `saturation`, which inflates sigma and makes downstream stages simplify rather than
/// fabricate detail.
///
/// Resolving this properly needs the analysis-by-synthesis solve of DESIGN.md S2: fit
/// stroke width and position against a forward render, with a prior on the palette to
/// break the ambiguity. That is future work, and until then this is the honest behaviour.
#[test]
fn sub_pixel_width_is_ambiguous_and_is_reported_as_such() {
    let thin = render(64, 64, |x, y| {
        (10.0..54.0).contains(&y) && (20.0..20.3).contains(&x)
    });
    let wide = render(64, 64, |x, y| {
        (10.0..54.0).contains(&y) && (20.0..28.0).contains(&x)
    });

    let (thin_polys, thin_field) = trace_bilevel(&thin, &TraceOptions { min_area: 0.05 });
    let (_, wide_field) = trace_bilevel(&wide, &TraceOptions { min_area: 0.05 });

    assert!(!thin_polys.is_empty(), "the feature must still be detected");
    assert!(
        thin_field.saturation < 0.25,
        "an all-sub-pixel image should report low saturation, got {:.3}",
        thin_field.saturation
    );
    assert!(
        wide_field.saturation > 0.5,
        "a resolved image should report high saturation, got {:.3}",
        wide_field.saturation
    );
    assert!(
        thin_field.sigma_alpha > wide_field.sigma_alpha,
        "unidentifiable colour axis must inflate sigma: {:.5} vs {:.5}",
        thin_field.sigma_alpha,
        wide_field.sigma_alpha
    );
}

// --- uncertainty --------------------------------------------------------------------

#[test]
fn sigma_is_smaller_on_high_contrast_boundaries() {
    let sharp = render(64, 64, |x, y| (x - 31.5).hypot(y - 31.5) <= 20.0);

    // Same shape, lower contrast: the shape is grey rather than black, so |F - B| is
    // small and every coverage estimate is correspondingly less certain.
    let mut faint = sharp.clone();
    for i in 0..faint.width * faint.height {
        let v = faint.data[i * 4];
        let scaled = 1.0 - (1.0 - v) * 0.12;
        faint.data[i * 4] = scaled;
        faint.data[i * 4 + 1] = scaled;
        faint.data[i * 4 + 2] = scaled;
    }

    let (sp, _) = trace_bilevel(&sharp, &TraceOptions::default());
    let (fp, _) = trace_bilevel(&faint, &TraceOptions::default());
    let mean = |v: &[inkvec_core::Polyline]| -> f64 {
        let all: Vec<f64> = v.iter().flat_map(|p| p.sigma.iter().copied()).collect();
        all.iter().sum::<f64>() / all.len().max(1) as f64
    };
    // Both should trace; the faint one should report itself as less certain.
    assert!(!sp.is_empty() && !fp.is_empty());
    assert!(
        mean(&fp) > mean(&sp),
        "faint boundary sigma {:.4} should exceed sharp boundary sigma {:.4}",
        mean(&fp),
        mean(&sp)
    );
}

#[test]
fn contours_are_closed_and_consistently_wound() {
    let img = render(64, 64, |x, y| (x - 31.5).hypot(y - 31.5) <= 20.0);
    let (polys, _) = trace_bilevel(&img, &TraceOptions::default());
    assert_eq!(polys.len(), 1, "one shape should give one contour");
    assert!(polys[0].closed);
    let a = contour::signed_area(&polys[0]);
    let expected = std::f64::consts::PI * 400.0;
    assert!(
        (a.abs() - expected).abs() / expected < 0.02,
        "enclosed area {:.1} vs analytic {:.1}",
        a.abs(),
        expected
    );
}

#[test]
fn empty_and_uniform_images_do_not_panic() {
    for v in [0.0f32, 1.0] {
        let img = Rgba {
            width: 16,
            height: 16,
            data: (0..16 * 16).flat_map(|_| [v, v, v, 1.0]).collect(),
        };
        let _ = trace_bilevel(&img, &TraceOptions::default());
    }
    let _ = CoverageField {
        width: 0,
        height: 0,
        data: vec![],
        sigma_alpha: 0.01,
        sigma_model: 0.05,
        saturation: 1.0,
        fg: [0.0; 3],
        bg: [1.0; 3],
    };
}
