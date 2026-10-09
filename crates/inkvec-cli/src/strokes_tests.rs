//! Unit tests for the `--strokes` line-art pipeline.

use super::*;
use inkvec_core::Point;
use inkvec_trace::centerline::Stroke;
use inkvec_trace::coverage::CoverageField;

#[test]
fn test_run_strokes_declines_on_flat_image() {
    let img = inkvec_trace::Rgba {
        width: 16,
        height: 16,
        data: vec![1.0; 16 * 16 * 4],
    };
    let args = Args {
        strokes: true,
        ..Args::default()
    };
    let cfg = FitConfig::default();
    assert!(run_strokes(&img, &args, &cfg).is_none());
}

#[test]
fn test_check_ink_balance_singularities_and_closed_cap() {
    let s_closed = Stroke {
        path: vec![
            Point::new(0.0, 0.0),
            Point::new(10.0, 0.0),
            Point::new(0.0, 0.0),
        ],
        sigma: vec![0.1, 0.1, 0.1],
        width: 2.0,
        width_sigma: 0.1,
        closed: true,
        label: 1,
    };
    let cov = CoverageField {
        width: 10,
        height: 10,
        data: vec![0.001; 100], // ink_have <= 1.0
        sigma_alpha: 0.05,
        sigma_model: 0.05,
        fg: [0.0, 0.0, 0.0],
        bg: [1.0, 1.0, 1.0],
        saturation: 1.0,
    };
    let args = Args {
        quiet: false,
        ..Args::default()
    };
    // ink_have <= 1.0 returns true immediately
    assert!(check_ink_balance(
        std::slice::from_ref(&s_closed),
        &cov,
        0,
        &args,
        10,
        10
    ));

    // ink_have > 1.0 with severe out of balance and quiet=false exercises diagnostic logging
    let cov_high = CoverageField {
        data: vec![1.0; 100],
        ..cov
    };
    let s_tiny = Stroke {
        path: vec![Point::new(0.0, 0.0), Point::new(1.0, 0.0)],
        sigma: vec![0.1, 0.1],
        width: 0.1,
        width_sigma: 0.01,
        closed: false,
        label: 1,
    };
    // Exercise open stroke cap calculation
    assert!(!check_ink_balance(&[s_tiny], &cov_high, 0, &args, 10, 10));
    // Exercise closed stroke cap calculation
    assert!(!check_ink_balance(&[s_closed], &cov_high, 0, &args, 10, 10));
}

#[test]
fn test_shared_width_and_stroke_paths_non_shared() {
    let s1 = Stroke {
        path: vec![Point::new(0.0, 0.0), Point::new(10.0, 0.0)],
        sigma: vec![0.1, 0.1],
        width: 1.0,
        width_sigma: 0.01,
        closed: false,
        label: 1,
    };
    let s2 = Stroke {
        path: vec![Point::new(0.0, 0.0), Point::new(10.0, 0.0)],
        sigma: vec![0.1, 0.1],
        width: 5.0,
        width_sigma: 0.01,
        closed: false,
        label: 2,
    };
    let (med, shared) = shared_width(&[s1.clone(), s2.clone()]);
    assert!(!shared);
    assert_eq!(med, 5.0);

    let cfg = FitConfig::default();
    let (body, params) = stroke_paths(&[s1, s2], &cfg, false, 2, 0.0);
    assert!(body.contains("stroke-width=\"1.00\""));
    assert!(body.contains("stroke-width=\"5.00\""));
    assert!(params > 2.0);

    // Empty segments continue check
    let s_empty = Stroke {
        path: vec![Point::new(0.0, 0.0)],
        sigma: vec![0.1],
        width: 1.0,
        width_sigma: 0.01,
        closed: false,
        label: 3,
    };
    let (empty_body, _) = stroke_paths(&[s_empty], &cfg, false, 2, 0.0);
    assert!(empty_body.is_empty());
}

#[test]
fn test_residual_fills_with_sufficient_ink() {
    let w = 20;
    let h = 20;
    let mut data = vec![1.0f32; w * h * 4];
    let mut cov_data = vec![0.0f32; w * h];
    let labels = vec![0u16; w * h];

    // Paint a 10x10 black square with cov = 1.0 (100 pixels >= 8 threshold)
    for y in 5..15 {
        for x in 5..15 {
            let i = y * w + x;
            cov_data[i] = 1.0;
            data[i * 4] = 0.0;
            data[i * 4 + 1] = 0.0;
            data[i * 4 + 2] = 0.0;
        }
    }
    let img = inkvec_trace::Rgba {
        width: w,
        height: h,
        data,
    };
    let cov = CoverageField {
        width: w,
        height: h,
        data: cov_data,
        sigma_alpha: 0.05,
        sigma_model: 0.05,
        fg: [0.0, 0.0, 0.0],
        bg: [1.0, 1.0, 1.0],
        saturation: 1.0,
    };
    let args = Args::default();
    let cfg = FitConfig::default();
    let (fills, fill_params, residual_ink) =
        residual_fills(&img, &cov, &labels, &[], &args, &cfg, "#000000");
    assert_eq!(residual_ink, 100);
    assert!(!fills.is_empty());
    assert!(fill_params > 0.0);
}

#[test]
fn test_run_strokes_declines_when_residual_prunes_all_strokes() {
    let mut data = vec![1.0f32; 64 * 64 * 4];
    for y in 16..48 {
        for x in 30..34 {
            let i = (y * 64 + x) * 4;
            data[i] = 0.0;
            data[i + 1] = 0.0;
            data[i + 2] = 0.0;
        }
    }
    let img = inkvec_trace::Rgba {
        width: 64,
        height: 64,
        data,
    };
    // stroke_residual: -1.0 guarantees all strokes are dropped
    let args = Args {
        strokes: true,
        stroke_residual: -1.0,
        ..Args::default()
    };
    let cfg = FitConfig::default();
    assert!(run_strokes(&img, &args, &cfg).is_none());
}

#[test]
fn test_run_strokes_declines_on_ink_imbalance() {
    let mut data = vec![1.0f32; 64 * 64 * 4];
    for y in 16..48 {
        for x in 30..34 {
            let i = (y * 64 + x) * 4;
            data[i] = 0.0;
            data[i + 1] = 0.0;
            data[i + 2] = 0.0;
        }
    }
    let img = inkvec_trace::Rgba {
        width: 64,
        height: 64,
        data,
    };
    let args = Args {
        strokes: true,
        stroke_balance: 0.99999,
        quiet: false,
        ..Args::default()
    };
    let cfg = FitConfig::default();
    assert!(run_strokes(&img, &args, &cfg).is_none());
}

#[test]
fn test_run_strokes_with_refine_and_monochrome_no_bg() {
    let mut data = vec![1.0f32; 64 * 64 * 4];
    for y in 16..48 {
        for x in 30..34 {
            let i = (y * 64 + x) * 4;
            data[i] = 0.0;
            data[i + 1] = 0.0;
            data[i + 2] = 0.0;
        }
    }
    let img = inkvec_trace::Rgba {
        width: 64,
        height: 64,
        data,
    };
    let args = Args {
        strokes: true,
        stroke_refine: 2,
        monochrome: true,
        no_background: true,
        quiet: false,
        ..Args::default()
    };
    let cfg = FitConfig::default();
    let res = run_strokes(&img, &args, &cfg);
    assert!(res.is_some());
    let (svg, stats) = res.unwrap();
    assert!(svg.contains("<svg"));
    assert!(!stats.is_empty());
}
