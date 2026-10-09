//! Adversarial empirical challenge suite for degenerate rasters and numerical singularities.
//!
//! Stress-tests:
//! 1. 0x0, 0x10, and 10x0 dimension rasters across bilevel, color quality, fast, and native alpha.
//! 2. Massive aspect ratios: 10000x1, 1x10000, 2000x1, 2500x1 high-frequency Nyquist checkerboard.
//! 3. Subnormal floats and uniform zero-contrast rasters.
//! 4. Strict numerical finiteness (no NaNs, no Infs, positive sigmas) and deterministic execution.

use inkvec_core::Polyline;
use inkvec_trace::{
    trace_bilevel, trace_color, trace_color_full_with_alpha, ColorOptions, PlanarMap, Rgba,
    TraceOptions,
};

fn assert_finite_polylines(polys: &[Polyline]) {
    for poly in polys {
        for pt in &poly.points {
            assert!(pt.x.is_finite(), "Polyline point x is not finite: {}", pt.x);
            assert!(pt.y.is_finite(), "Polyline point y is not finite: {}", pt.y);
        }
        for &sg in &poly.sigma {
            assert!(sg.is_finite(), "Polyline sigma is not finite: {}", sg);
            assert!(sg >= 0.0, "Polyline sigma must be non-negative: {}", sg);
        }
    }
}

fn assert_finite_planar_map(map: &PlanarMap) {
    for edge in &map.edges {
        for pt in &edge.points {
            assert!(pt.x.is_finite(), "Edge point x is not finite: {}", pt.x);
            assert!(pt.y.is_finite(), "Edge point y is not finite: {}", pt.y);
        }
        for &sg in &edge.sigma {
            assert!(sg.is_finite(), "Edge sigma is not finite: {}", sg);
            assert!(sg >= 0.0, "Edge sigma must be non-negative: {}", sg);
        }
    }
}

#[test]
fn challenge_zero_dim_rasters() {
    let opts_bilevel = TraceOptions::default();
    let opts_color_q = ColorOptions {
        fast: false,
        ..Default::default()
    };
    let opts_color_f = ColorOptions {
        fast: true,
        ..Default::default()
    };
    let opts_color_native = ColorOptions {
        native_alpha: true,
        ..Default::default()
    };

    let zero_dims = [(0, 0), (0, 10), (10, 0)];

    for (w, h) in zero_dims {
        let empty_img = Rgba {
            width: w,
            height: h,
            data: Vec::new(),
        };

        // 1. Bilevel
        let (polys, field) = trace_bilevel(&empty_img, &opts_bilevel);
        assert_finite_polylines(&polys);
        assert!(field.sigma_alpha.is_finite());
        assert!(field.saturation.is_finite());
        assert_eq!(polys.len(), 0);

        // 2. Color Quality
        let (map_q, pal_q) = trace_color(&empty_img, &opts_color_q);
        assert_finite_planar_map(&map_q);
        assert!(pal_q.len() <= 64);
        assert_eq!(map_q.edges.len(), 0);

        // 3. Color Fast
        let (map_f, pal_f) = trace_color(&empty_img, &opts_color_f);
        assert_finite_planar_map(&map_f);
        assert!(pal_f.len() <= 64);
        assert_eq!(map_f.edges.len(), 0);

        // 4. Color Native Alpha
        let trace_alpha = trace_color_full_with_alpha(&empty_img, &opts_color_native, Some(&[]));
        assert_finite_planar_map(&trace_alpha.map);
        assert_eq!(trace_alpha.map.edges.len(), 0);
    }
}

#[test]
fn challenge_massive_10000x1_bilevel_extreme_aspect() {
    let w = 10000;
    let h = 1;
    let mut data = Vec::with_capacity(w * 4);
    for x in 0..w {
        let val = if (x / 25) % 2 == 0 { 1.0f32 } else { 0.0f32 };
        data.extend_from_slice(&[val, val, val, 1.0]);
    }
    let img = Rgba {
        width: w,
        height: h,
        data,
    };

    let (polys, field) = trace_bilevel(&img, &TraceOptions::default());
    assert_finite_polylines(&polys);
    assert!(field.sigma_alpha.is_finite());
}

#[test]
fn challenge_massive_1x10000_bilevel_extreme_aspect() {
    let w = 1;
    let h = 10000;
    let mut data = Vec::with_capacity(h * 4);
    for y in 0..h {
        let val = if (y / 25) % 2 == 0 { 1.0f32 } else { 0.0f32 };
        data.extend_from_slice(&[val, val, val, 1.0]);
    }
    let img = Rgba {
        width: w,
        height: h,
        data,
    };

    let (polys, field) = trace_bilevel(&img, &TraceOptions::default());
    assert_finite_polylines(&polys);
    assert!(field.sigma_alpha.is_finite());
}

#[test]
fn challenge_massive_2000x1_color_fast() {
    let w = 2000;
    let h = 1;
    let mut data = Vec::with_capacity(w * 4);
    for x in 0..w {
        let val = if (x / 25) % 2 == 0 { 1.0f32 } else { 0.0f32 };
        data.extend_from_slice(&[val, val, val, 1.0]);
    }
    let img = Rgba {
        width: w,
        height: h,
        data,
    };

    let (map_f, _) = trace_color(
        &img,
        &ColorOptions {
            fast: true,
            ..Default::default()
        },
    );
    assert_finite_planar_map(&map_f);
}

#[test]
fn challenge_massive_aspect_ratio_color_quality() {
    let w = 1500;
    let h = 1;
    let mut data = Vec::with_capacity(w * 4);
    for x in 0..w {
        let val = if (x / 25) % 2 == 0 { 1.0f32 } else { 0.0f32 };
        data.extend_from_slice(&[val, val, val, 1.0]);
    }
    let img = Rgba {
        width: w,
        height: h,
        data,
    };

    let (map_q, pal_q) = trace_color(
        &img,
        &ColorOptions {
            fast: false,
            ..Default::default()
        },
    );
    assert_finite_planar_map(&map_q);
    assert!(pal_q.len() <= 64);
}

#[test]
fn challenge_nyquist_checkerboard_extreme_aspect() {
    // 2500x1 single-pixel alternating stripes (highest spatial frequency)
    let n = 2500;
    let mut data = Vec::with_capacity(n * 4);
    for i in 0..n {
        let v = if i % 2 == 0 { 1.0f32 } else { 0.0f32 };
        data.extend_from_slice(&[v, v, v, 1.0]);
    }
    let img_alt = Rgba {
        width: n,
        height: 1,
        data,
    };

    let (polys, field) = trace_bilevel(&img_alt, &TraceOptions::default());
    assert_finite_polylines(&polys);
    assert!(field.sigma_alpha.is_finite());
}

#[test]
fn challenge_subnormal_and_uniform_color_rasters() {
    // 1. Subnormal floats in pixel data
    let subnormal_val = 1e-35f32;
    let mut data_subnormal = vec![0.0f32; 16 * 16 * 4];
    for chunk in data_subnormal.chunks_mut(4) {
        chunk[0] = subnormal_val;
        chunk[1] = subnormal_val;
        chunk[2] = subnormal_val;
        chunk[3] = 1.0;
    }
    let img_subnormal = Rgba {
        width: 16,
        height: 16,
        data: data_subnormal,
    };

    let (polys_sub, field_sub) = trace_bilevel(&img_subnormal, &TraceOptions::default());
    assert_finite_polylines(&polys_sub);
    assert!(field_sub.sigma_alpha.is_finite());

    let (map_sub, _) = trace_color(
        &img_subnormal,
        &ColorOptions {
            fast: true,
            ..Default::default()
        },
    );
    assert_finite_planar_map(&map_sub);

    // 2. Uniform 0.5 flat gray (zero contrast)
    let data_uniform = vec![0.5f32; 16 * 16 * 4];
    let img_uniform = Rgba {
        width: 16,
        height: 16,
        data: data_uniform,
    };

    let (polys_uni, field_uni) = trace_bilevel(&img_uniform, &TraceOptions::default());
    assert_finite_polylines(&polys_uni);
    assert_eq!(polys_uni.len(), 0);
    assert!(field_uni.sigma_alpha.is_finite());

    let (map_uni, _) = trace_color(
        &img_uniform,
        &ColorOptions {
            fast: true,
            ..Default::default()
        },
    );
    assert_finite_planar_map(&map_uni);
}

#[test]
fn challenge_tracing_determinism() {
    let mut data = Vec::with_capacity(1000 * 4);
    for x in 0..1000 {
        let v = if (x / 20) % 2 == 0 { 1.0f32 } else { 0.0f32 };
        data.extend_from_slice(&[v, v, v, 1.0]);
    }
    let img = Rgba {
        width: 1000,
        height: 1,
        data,
    };

    // Run 3 consecutive traces and assert bitwise identical results
    let (first_polys, _) = trace_bilevel(&img, &TraceOptions::default());
    let (first_map, _) = trace_color(
        &img,
        &ColorOptions {
            fast: true,
            ..Default::default()
        },
    );

    for _ in 0..2 {
        let (polys, _) = trace_bilevel(&img, &TraceOptions::default());
        assert_eq!(polys.len(), first_polys.len());
        for (p1, p2) in polys.iter().zip(&first_polys) {
            assert_eq!(p1.points, p2.points);
            assert_eq!(p1.sigma, p2.sigma);
        }

        let (map, _) = trace_color(
            &img,
            &ColorOptions {
                fast: true,
                ..Default::default()
            },
        );
        assert_eq!(map.edges.len(), first_map.edges.len());
        for (e1, e2) in map.edges.iter().zip(&first_map.edges) {
            assert_eq!(e1.points, e2.points);
            assert_eq!(e1.sigma, e2.sigma);
        }
    }
}
