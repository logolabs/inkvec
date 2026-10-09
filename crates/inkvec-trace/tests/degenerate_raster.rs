//! Integration tests for degenerate raster inputs (1x1, blank, extreme aspect ratios).
//!
//! Verifies zero panics, zero unhandled zero divisions, and zero NaN propagation across:
//! - 1x1 white and black rasters through bilevel and color pipelines (Quality, Fast, Native Alpha)
//! - 64x64 all-zero transparent rasters through all pipelines
//! - Extreme aspect ratios (1000x1 and 1x1000 striped rasters) through bilevel and color pipelines

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
        }
    }
}

#[test]
fn test_one_by_one_raster_bilevel_and_color() {
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

    // 1. 1x1 White Raster
    let white = Rgba {
        width: 1,
        height: 1,
        data: vec![1.0, 1.0, 1.0, 1.0],
    };

    let (polys_w, field_w) = trace_bilevel(&white, &opts_bilevel);
    assert_finite_polylines(&polys_w);
    assert!(field_w.sigma_alpha.is_finite());
    assert!(field_w.saturation.is_finite());

    let (map_w_q, pal_w_q) = trace_color(&white, &opts_color_q);
    assert_finite_planar_map(&map_w_q);
    assert!(pal_w_q.len() <= 64);

    let (map_w_f, pal_w_f) = trace_color(&white, &opts_color_f);
    assert_finite_planar_map(&map_w_f);
    assert!(pal_w_f.len() <= 64);

    let trace_w_alpha = trace_color_full_with_alpha(&white, &opts_color_native, Some(&[1.0]));
    assert_finite_planar_map(&trace_w_alpha.map);

    // 2. 1x1 Black Raster
    let black = Rgba {
        width: 1,
        height: 1,
        data: vec![0.0, 0.0, 0.0, 1.0],
    };

    let (polys_b, field_b) = trace_bilevel(&black, &opts_bilevel);
    assert_finite_polylines(&polys_b);
    assert!(field_b.sigma_alpha.is_finite());
    assert!(field_b.saturation.is_finite());

    let (map_b_q, pal_b_q) = trace_color(&black, &opts_color_q);
    assert_finite_planar_map(&map_b_q);
    assert!(pal_b_q.len() <= 64);

    let (map_b_f, pal_b_f) = trace_color(&black, &opts_color_f);
    assert_finite_planar_map(&map_b_f);
    assert!(pal_b_f.len() <= 64);

    let trace_b_alpha = trace_color_full_with_alpha(&black, &opts_color_native, Some(&[1.0]));
    assert_finite_planar_map(&trace_b_alpha.map);
}

#[test]
fn test_all_zero_blank_raster() {
    let blank = Rgba {
        width: 64,
        height: 64,
        data: vec![0.0; 64 * 64 * 4],
    };

    // Bilevel
    let (polys, field) = trace_bilevel(&blank, &TraceOptions::default());
    assert_finite_polylines(&polys);
    assert!(field.sigma_alpha.is_finite());
    assert!(field.saturation.is_finite());

    // Color Quality
    let (map_q, pal_q) = trace_color(
        &blank,
        &ColorOptions {
            fast: false,
            ..Default::default()
        },
    );
    assert_finite_planar_map(&map_q);
    assert!(pal_q.len() <= 64);

    // Color Fast
    let (map_f, pal_f) = trace_color(
        &blank,
        &ColorOptions {
            fast: true,
            ..Default::default()
        },
    );
    assert_finite_planar_map(&map_f);
    assert!(pal_f.len() <= 64);

    // Native Alpha
    let alpha = vec![0.0f32; 64 * 64];
    let trace_alpha = trace_color_full_with_alpha(
        &blank,
        &ColorOptions {
            native_alpha: true,
            ..Default::default()
        },
        Some(&alpha),
    );
    assert_finite_planar_map(&trace_alpha.map);
}

#[test]
fn test_extreme_aspect_ratios_1000x1_and_1x1000() {
    // 1000x1 striped raster
    let mut data_1000x1 = Vec::with_capacity(1000 * 4);
    for x in 0..1000 {
        let val = if (x / 20) % 2 == 0 { 1.0f32 } else { 0.0f32 };
        data_1000x1.extend_from_slice(&[val, val, val, 1.0]);
    }
    let img_1000x1 = Rgba {
        width: 1000,
        height: 1,
        data: data_1000x1,
    };

    let (polys_row, field_row) = trace_bilevel(&img_1000x1, &TraceOptions::default());
    assert_finite_polylines(&polys_row);
    assert!(field_row.sigma_alpha.is_finite());

    let (map_row_q, _) = trace_color(
        &img_1000x1,
        &ColorOptions {
            fast: false,
            ..Default::default()
        },
    );
    assert_finite_planar_map(&map_row_q);

    let (map_row_f, _) = trace_color(
        &img_1000x1,
        &ColorOptions {
            fast: true,
            ..Default::default()
        },
    );
    assert_finite_planar_map(&map_row_f);

    // 1x1000 striped raster
    let mut data_1x1000 = Vec::with_capacity(1000 * 4);
    for y in 0..1000 {
        let val = if (y / 20) % 2 == 0 { 1.0f32 } else { 0.0f32 };
        data_1x1000.extend_from_slice(&[val, val, val, 1.0]);
    }
    let img_1x1000 = Rgba {
        width: 1,
        height: 1000,
        data: data_1x1000,
    };

    let (polys_col, field_col) = trace_bilevel(&img_1x1000, &TraceOptions::default());
    assert_finite_polylines(&polys_col);
    assert!(field_col.sigma_alpha.is_finite());

    let (map_col_q, _) = trace_color(
        &img_1x1000,
        &ColorOptions {
            fast: false,
            ..Default::default()
        },
    );
    assert_finite_planar_map(&map_col_q);

    let (map_col_f, _) = trace_color(
        &img_1x1000,
        &ColorOptions {
            fast: true,
            ..Default::default()
        },
    );
    assert_finite_planar_map(&map_col_f);
}
