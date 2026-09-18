//! Per-region fill model selection on synthetic 96x96 images with known answers.
//!
//! Every image is generated in linear light, converted to sRGB and quantised to 8 bits,
//! which is what a renderer would hand us. `sigma_noise` is passed as the true value
//! (1/255) and `lambda` as the BIC default, so these exercise the selection exactly as
//! the pipeline would use it.

use inkvec_trace::color::{rgb_to_oklab, Palette};
use inkvec_trace::gradient::{
    bic_lambda, fill_to_svg, fit_fill, merge_gradient_bands, FillModel, Interp,
};

const W: usize = 96;
const H: usize = 96;

fn srgb_to_linear(c: f32) -> f32 {
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

fn linear_to_srgb(c: f32) -> f32 {
    if c <= 0.003_130_8 {
        c * 12.92
    } else {
        1.055 * c.powf(1.0 / 2.4) - 0.055
    }
}

fn quantize(c: [f32; 3]) -> [f32; 3] {
    let q = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() / 255.0;
    [q(c[0]), q(c[1]), q(c[2])]
}

/// Colour at `t` between two sRGB stops, interpolated in linear light.
fn ramp(c0: [f32; 3], c1: [f32; 3], t: f32) -> [f32; 3] {
    let mut out = [0.0; 3];
    for k in 0..3 {
        let (a, b) = (srgb_to_linear(c0[k]), srgb_to_linear(c1[k]));
        out[k] = linear_to_srgb(a + (b - a) * t.clamp(0.0, 1.0));
    }
    out
}

/// Deterministic pseudo-Gaussian noise (sum of 12 uniforms) with unit sigma.
struct Noise(u64);
impl Noise {
    fn next(&mut self) -> f32 {
        let mut s = 0.0f32;
        for _ in 0..12 {
            self.0 = self
                .0
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            s += (self.0 >> 40) as f32 / (1u64 << 24) as f32;
        }
        s - 6.0
    }
}

fn render(mut f: impl FnMut(usize, usize) -> ([f32; 3], u16)) -> (Vec<[f32; 3]>, Vec<u16>) {
    let mut rgb = Vec::with_capacity(W * H);
    let mut labels = Vec::with_capacity(W * H);
    for y in 0..H {
        for x in 0..W {
            let (c, l) = f(x, y);
            rgb.push(quantize(c));
            labels.push(l);
        }
    }
    (rgb, labels)
}

fn close(a: [f32; 3], b: [f32; 3], tol: f32) -> bool {
    (0..3).all(|k| (a[k] - b[k]).abs() <= tol)
}

const SIGMA: f64 = 1.0 / 255.0;
const BG: [f32; 3] = [0.95, 0.95, 0.95];

#[test]
fn noisy_flat_region_stays_flat() {
    let base = [0.35f32, 0.55, 0.7];
    let mut noise = Noise(7);
    let (rgb, labels) = render(|x, y| {
        let inside = (8..88).contains(&x) && (8..88).contains(&y);
        if inside {
            let mut c = base;
            for v in c.iter_mut() {
                *v += noise.next() / 255.0;
            }
            (c, 1)
        } else {
            (BG, 0)
        }
    });
    let n = labels.iter().filter(|&&l| l == 1).count();
    let fit = fit_fill(&rgb, W, H, &labels, 1, SIGMA, bic_lambda(n));
    match fit.model {
        FillModel::Flat(c) => assert!(close(c, base, 1.0 / 255.0), "flat colour drifted: {c:?}"),
        other => panic!("noisy flat region became {other:?}"),
    }
    // Unit-sigma Gaussian noise beyond a half-sigma dead zone: E[((|z|-0.5)+)^2] = 0.42
    // per channel, so about 1.26 per interior pixel.
    let interior = 78 * 78;
    assert!(
        fit.chi2 > 0.9 * interior as f64 && fit.chi2 < 1.7 * interior as f64,
        "chi2 {} for {} interior pixels",
        fit.chi2,
        interior
    );
    assert_eq!(fill_to_svg(&fit.model, "g0").0, "");
}

fn axis_angle_deg(p0: (f64, f64), p1: (f64, f64)) -> f64 {
    let a = (p1.1 - p0.1).atan2(p1.0 - p0.0).to_degrees();
    // Direction modulo 180: a gradient's axis has no sign.
    a.rem_euclid(180.0)
}

fn angle_diff_deg(a: f64, b: f64) -> f64 {
    let d = (a - b).rem_euclid(180.0);
    d.min(180.0 - d)
}

#[test]
fn horizontal_linear_gradient_is_recovered() {
    let (c0, c1) = ([0.17f32, 0.42, 0.69], [0.96f32, 0.68, 0.33]);
    let (rgb, labels) = render(|x, y| {
        let inside = (8..88).contains(&x) && (8..88).contains(&y);
        if inside {
            (ramp(c0, c1, x as f32 / (W - 1) as f32), 1)
        } else {
            (BG, 0)
        }
    });
    let n = labels.iter().filter(|&&l| l == 1).count();
    let fit = fit_fill(&rgb, W, H, &labels, 1, SIGMA, bic_lambda(n));
    let FillModel::Linear {
        p0,
        p1,
        c0: f0,
        c1: f1,
        interp,
        ..
    } = fit.model.clone()
    else {
        panic!("expected Linear, got {:?}", fit.model);
    };
    assert_eq!(interp, Interp::LinearRgb, "generated in linear light");
    assert!(
        angle_diff_deg(axis_angle_deg(p0, p1), 0.0) <= 2.0,
        "axis angle {}",
        axis_angle_deg(p0, p1)
    );
    // Interior spans x = 9..=86; stops sit at the extremes of the interior.
    let (lo, hi) = if p0.0 < p1.0 { (p0, p1) } else { (p1, p0) };
    let (lo_c, hi_c) = if p0.0 < p1.0 { (f0, f1) } else { (f1, f0) };
    assert!(
        (lo.0 - 9.0).abs() < 0.5 && (hi.0 - 86.0).abs() < 0.5,
        "axis ends {lo:?} {hi:?}"
    );
    let want_lo = ramp(c0, c1, 9.0 / 95.0);
    let want_hi = ramp(c0, c1, 86.0 / 95.0);
    assert!(
        close(lo_c, want_lo, 3.0 / 255.0),
        "stop 0 {lo_c:?} vs {want_lo:?}"
    );
    assert!(
        close(hi_c, want_hi, 3.0 / 255.0),
        "stop 1 {hi_c:?} vs {want_hi:?}"
    );

    let (defs, fill) = fill_to_svg(&fit.model, "g1");
    assert!(defs.starts_with("<linearGradient id=\"g1\""));
    assert_eq!(fill, "url(#g1)");
}

#[test]
fn oblique_linear_gradient_axis_is_recovered() {
    let (c0, c1) = ([0.10f32, 0.10, 0.50], [0.90f32, 0.90, 0.20]);
    let ang = 32.0f32.to_radians();
    let (dx, dy) = (ang.cos(), ang.sin());
    let (rgb, labels) = render(|x, y| {
        let inside = (8..88).contains(&x) && (8..88).contains(&y);
        if inside {
            let s = (x as f32 - 47.5) * dx + (y as f32 - 47.5) * dy;
            (ramp(c0, c1, 0.5 + s / 100.0), 1)
        } else {
            (BG, 0)
        }
    });
    let n = labels.iter().filter(|&&l| l == 1).count();
    let fit = fit_fill(&rgb, W, H, &labels, 1, SIGMA, bic_lambda(n));
    let FillModel::Linear { p0, p1, .. } = fit.model else {
        panic!("expected Linear, got {:?}", fit.model);
    };
    assert!(
        angle_diff_deg(axis_angle_deg(p0, p1), 32.0) <= 2.0,
        "axis angle {}",
        axis_angle_deg(p0, p1)
    );
}

#[test]
fn radial_gradient_centre_is_recovered() {
    let (c0, c1) = ([1.0f32, 0.98, 0.90], [0.72f32, 0.47, 0.12]);
    let centre = (50.3f64, 44.7f64);
    let r_true = 70.0f64;
    let (rgb, labels) = render(|x, y| {
        let rho = ((x as f64 - centre.0).powi(2) + (y as f64 - centre.1).powi(2)).sqrt();
        if rho <= 40.0 {
            (ramp(c0, c1, (rho / r_true) as f32), 1)
        } else {
            (BG, 0)
        }
    });
    let n = labels.iter().filter(|&&l| l == 1).count();
    let fit = fit_fill(&rgb, W, H, &labels, 1, SIGMA, bic_lambda(n));
    let FillModel::Radial {
        c,
        r,
        c0: f0,
        c1: f1,
        interp,
        ..
    } = fit.model.clone()
    else {
        panic!("expected Radial, got {:?}", fit.model);
    };
    assert_eq!(interp, Interp::LinearRgb, "generated in linear light");
    let err = ((c.0 - centre.0).powi(2) + (c.1 - centre.1).powi(2)).sqrt();
    assert!(err <= 0.5, "centre {c:?} off by {err}");
    assert!(close(f0, c0, 3.0 / 255.0), "centre colour {f0:?} vs {c0:?}");
    let want_edge = ramp(c0, c1, (r / r_true) as f32);
    assert!(
        close(f1, want_edge, 3.0 / 255.0),
        "edge colour {f1:?} vs {want_edge:?}"
    );
    assert!(fill_to_svg(&fit.model, "g2")
        .0
        .starts_with("<radialGradient id=\"g2\""));
}

#[test]
fn banded_gradient_labels_merge_into_one_linear() {
    let (c0, c1) = ([0.17f32, 0.42, 0.69], [0.96f32, 0.68, 0.33]);
    let (rgb, mut labels) = render(|x, y| {
        let inside = (8..88).contains(&x) && (8..88).contains(&y);
        if inside {
            (ramp(c0, c1, x as f32 / (W - 1) as f32), 1)
        } else {
            (BG, 0)
        }
    });

    // A palette that bands the ramp four ways, plus the background.
    let mut colors = vec![BG];
    for t in [0.125f32, 0.375, 0.625, 0.875] {
        colors.push(ramp(c0, c1, t));
    }
    let pal = Palette {
        colors: colors.iter().map(|&c| rgb_to_oklab(c)).collect(),
        rgb: colors.clone(),
        weight: vec![0.2; 5],
        alpha: vec![1.0; 5],
    };
    // Label the gradient pixels by nearest palette entry, as `label_image` would.
    for (p, l) in labels.iter_mut().enumerate() {
        if *l == 1 {
            *l = pal.nearest(rgb_to_oklab(rgb[p])).0 as u16;
        }
    }
    let mut bands: Vec<u16> = labels.iter().copied().filter(|&l| l != 0).collect();
    bands.sort_unstable();
    bands.dedup();
    assert_eq!(bands.len(), 4, "expected four bands, got {bands:?}");

    let fills = merge_gradient_bands(&mut labels, &rgb, W, H, &pal, SIGMA, bic_lambda(W * H));

    let mut after: Vec<u16> = (0..W * H)
        .filter(|&p| {
            let (x, y) = (p % W, p / W);
            (8..88).contains(&x) && (8..88).contains(&y)
        })
        .map(|p| labels[p])
        .collect();
    after.sort_unstable();
    after.dedup();
    // One fresh label for the merged ramp, past the five palette entries (the background,
    // a flat region with an interior, is given its own label too, so the exact id is not
    // pinned).
    assert_eq!(
        after.len(),
        1,
        "gradient region labels after merge: {after:?}"
    );
    let g = after[0] as usize;
    assert!(g >= 5, "merged ramp kept a palette label {g}");
    assert!(
        matches!(fills[g].model, FillModel::Linear { .. }),
        "merged fill is {:?}",
        fills[g].model
    );
    let bg = labels[0] as usize;
    assert!(
        matches!(fills[bg].model, FillModel::Flat(_)),
        "background became {:?}",
        fills[bg].model
    );
    assert!(labels.iter().filter(|&&l| l as usize == bg).count() == W * H - 80 * 80);

    let FillModel::Linear { p0, p1, .. } = fills[g].model else {
        unreachable!()
    };
    assert!(angle_diff_deg(axis_angle_deg(p0, p1), 0.0) <= 2.0);
}
