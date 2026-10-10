//! Unit tests for lib.rs intake normalization, pricing, error recovery, and CLI exit codes.

use super::*;
use std::path::PathBuf;
use std::process::ExitCode;

const TINY_PNG: &[u8] = &[
    0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, // PNG signature
    0x00, 0x00, 0x00, 0x0d, 0x49, 0x48, 0x44, 0x52, // IHDR
    0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, // 1x1
    0x08, 0x02, 0x00, 0x00, 0x00, 0x90, 0x77, 0x53, 0xde, 0x00, 0x00, 0x00, 0x0c, 0x49, 0x44, 0x41,
    0x54, // IDAT
    0x08, 0xd7, 0x63, 0x60, 0x60, 0x60, 0x00, 0x00, 0x00, 0x04, 0x00, 0x01, 0x27, 0x34, 0x27, 0x0a,
    0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4e, 0x44, 0xae, 0x42, 0x60, 0x82, // IEND
];

/// The `--sr-command` that `build_upscaler` answers with [`Nearest`], in process.
pub(crate) const NEAREST_UPSCALER: &str = "<test: nearest x4>";

/// A stand-in for the SR network: nearest-neighbour replication by 4, the factor
/// `build_upscaler` gives an external command.
pub(crate) struct Nearest;

impl inkvec_sr::Upscaler for Nearest {
    fn scale(&self) -> usize {
        4
    }

    fn upscale(
        &self,
        img: &inkvec_trace::Rgba,
    ) -> Result<inkvec_trace::Rgba, Box<dyn std::error::Error>> {
        let (w, h) = (img.width * 4, img.height * 4);
        let data = (0..w * h)
            .flat_map(|i| img.pixel((i % w) / 4, (i / w) / 4))
            .collect();
        Ok(inkvec_trace::Rgba {
            width: w,
            height: h,
            data,
        })
    }

    fn describe(&self) -> String {
        "nearest x4 (test)".into()
    }
}

/// The `--restore-command` that `build_restorer` answers with [`Identity`], in process.
pub(crate) const IDENTITY_RESTORER: &str = "<test: identity>";

/// A stand-in for the restorer that hands the image back unchanged.
pub(crate) struct Identity;

impl inkvec_restore::Restore for Identity {
    fn restore(
        &self,
        rgb: &[f32],
        _width: usize,
        _height: usize,
    ) -> Result<Vec<f32>, Box<dyn std::error::Error>> {
        Ok(rgb.to_vec())
    }

    fn describe(&self) -> String {
        "identity (test)".into()
    }
}

fn make_square() -> inkvec_trace::Rgba {
    let mut data = vec![1.0f32; 64 * 64 * 4];
    for y in 16..48 {
        for x in 16..48 {
            let i = (y * 64 + x) * 4;
            data[i] = 0.0;
            data[i + 1] = 0.0;
            data[i + 2] = 0.0;
            data[i + 3] = 1.0;
        }
    }
    inkvec_trace::Rgba {
        width: 64,
        height: 64,
        data,
    }
}

fn make_blurred_step(w: usize, h: usize, edge: f64, sigma: f64) -> inkvec_trace::Rgba {
    let mut data = vec![1.0f32; w * h * 4];
    for y in 0..h {
        for x in 0..w {
            let d = (x as f64 - edge) / sigma;
            let t = (0.5 * (1.0 + (0.8 * d).tanh())) as f32;
            let i = (y * w + x) * 4;
            data[i] = t;
            data[i + 1] = t;
            data[i + 2] = t;
            data[i + 3] = 1.0;
        }
    }
    inkvec_trace::Rgba {
        width: w,
        height: h,
        data,
    }
}

#[test]
fn test_normalise_intake_branches() {
    // 1. Sharp raster: scale < INTAKE_SCALE_FLOOR (1.5)
    let sharp = inkvec_trace::Rgba {
        width: 16,
        height: 16,
        data: vec![1.0; 16 * 16 * 4],
    };
    let (out_sharp, did_sharp) = normalise_intake(sharp.clone(), true);
    assert!(!did_sharp);
    assert_eq!(out_sharp.width, 16);

    // 2. Small raster where max(8.0) clamp prevents shrinking (nw >= img.width)
    let tiny = inkvec_trace::Rgba {
        width: 8,
        height: 8,
        data: vec![0.5; 8 * 8 * 4],
    };
    let (_out_tiny, did_tiny) = normalise_intake(tiny, false);
    assert!(!did_tiny);

    // 3. Blurred raster with quiet = false: exercises downsample_to and diag::stage
    let blurred = make_blurred_step(64, 64, 32.0, 3.0);
    let (out_blur, did_blur) = normalise_intake(blurred, false);
    assert!(did_blur);
    assert!(out_blur.width < 64);
}

#[test]
fn test_reduce_intake_variations() {
    let img = inkvec_trace::Rgba {
        width: 16,
        height: 16,
        data: vec![1.0; 16 * 16 * 4],
    };
    // Replicated early return
    let (r_img, rep, soft) = reduce_intake(img.clone(), &Args::default(), true);
    assert!(rep);
    assert!(!soft);
    assert_eq!(r_img.width, 16);

    // SR active early return
    let args_sr = Args {
        sr: inkvec_sr::Mode::Auto,
        ..Args::default()
    };
    let (_, _, soft_sr) = reduce_intake(img.clone(), &args_sr, false);
    assert!(!soft_sr);

    // Fast mode or no_soft_intake early return
    let args_fast = Args {
        mode: TraceMode::Fast,
        ..Args::default()
    };
    let (_, did_fast, _) = reduce_intake(img.clone(), &args_fast, false);
    assert!(!did_fast);

    let args_no_soft = Args {
        no_soft_intake: true,
        ..Args::default()
    };
    let (_, did_no_soft, _) = reduce_intake(img.clone(), &args_no_soft, false);
    assert!(!did_no_soft);

    // intake_scale flag
    let args_scale = Args {
        intake_scale: true,
        quiet: false,
        ..Args::default()
    };
    let blurred = make_blurred_step(64, 64, 32.0, 3.0);
    let (out_sc, did_sc, _) = reduce_intake(blurred, &args_scale, false);
    assert!(did_sc);
    assert!(out_sc.width < 64);
}

#[test]
fn test_intake_and_present_stretching() {
    let img = inkvec_trace::Rgba {
        width: 16,
        height: 16,
        data: vec![1.0; 16 * 16 * 4],
    };
    let args = Args {
        quiet: false,
        ..Args::default()
    };
    let intk = intake(img, &args, Some((32, 32)));
    assert_eq!(intk.display, (32, 32));

    let svg_test = "<svg viewBox=\"-0.5 -0.5 16 16\"><rect width=\"16\" height=\"16\"/></svg>";
    let presented_normal = present(svg_test, 16, 16, false);
    assert!(presented_normal.contains("viewBox"));
    let presented_stretched = present(svg_test, 32, 24, true);
    assert!(presented_stretched.contains("viewBox"));
}

#[test]
fn test_price_in_raster_units_variations() {
    let img = inkvec_trace::Rgba {
        width: 256,
        height: 256,
        data: vec![1.0; 256 * 256 * 4],
    };
    // Fast mode
    let args_fast = Args {
        mode: TraceMode::Fast,
        ..Args::default()
    };
    let priced_fast = price_in_raster_units(&img, &args_fast);
    assert_eq!(priced_fast.precision, args_fast.precision);

    // Quiet = false with over_reference > REF_EXTENT (128)
    let args_q = Args {
        quiet: false,
        ..Args::default()
    };
    let priced_q = price_in_raster_units(&img, &args_q);
    assert_eq!(priced_q.precision, args_q.precision);

    // Blurred image to trigger oversample / redundancy scaling
    let blurred_large = make_blurred_step(256, 256, 128.0, 4.0);
    let priced_blur = price_in_raster_units(&blurred_large, &args_q);
    assert!(priced_blur.min_area >= args_q.min_area);
}

#[test]
fn test_sr_and_restore_error_recovery_paths() {
    let sq = make_square();

    // Restore Auto with threshold -1.0 forcing Restore decision; since tools/inkvec_restore does not exist, recovers gracefully
    let args_restore = Args {
        restore: inkvec_restore::Mode::Auto,
        restore_threshold: -1.0,
        // A lossy container, the sign of compression `auto` needs besides the residual.
        lossy: inkvec_sr::Mode::On,
        ..Args::default()
    };
    let res_rest = trace_image(sq, &args_restore);
    assert!(
        res_rest.is_ok(),
        "Restore auto should recover gracefully when restorer unavailable"
    );
    let stats_rest = res_rest.unwrap().stats;
    assert!(stats_rest
        .iter()
        .any(|s| s.contains("no restorer is available")));
}

#[test]
fn test_build_upscaler_and_restorer_command_parsing() {
    // Valid sr_command returns Ok
    let valid_sr = Args {
        sr_command: Some("my_tool {in} {out}".into()),
        ..Args::default()
    };
    assert!(build_upscaler(&valid_sr).is_ok());

    // When no restore_command is given, restore_tools_dir returns error because tools/inkvec_restore does not exist in checkout
    let default_res = Args::default();
    assert!(build_restorer(&default_res).is_err());

    // Valid restore_command returns Ok
    let valid_res = Args {
        restore_command: Some("my_tool {in} {out}".into()),
        ..Args::default()
    };
    assert!(build_restorer(&valid_res).is_ok());
}

#[test]
fn test_run_cli_exit_codes() {
    // 1. FlatInput stop under strict mode yields exit code 2
    let tmp = std::env::temp_dir().join("test_flat_exit.png");
    std::fs::write(&tmp, TINY_PNG).expect("write tiny png");
    let args_flat = Args {
        input: tmp.clone(),
        strict: true,
        ..Args::default()
    };
    assert_eq!(run_cli(&args_flat), ExitCode::from(2));
    let _ = std::fs::remove_file(tmp);

    // `INKVEC_DUMP_MAP`'s stop is not exercised here: `inkvec_core::env` caches each
    // variable for the whole process the first time it is read, so setting it in one test
    // either does nothing or turns every later trace in this binary into a map dump.

    // 4. cli_main_from invalid args returns FAILURE
    let code = cli_main_from(["inkvec".into(), "--unknown-arg-12345".into()].into_iter());
    assert_eq!(code, ExitCode::FAILURE);
}

#[test]
fn test_trace_bordered_and_hypotheses_conditions() {
    let img = inkvec_trace::Rgba {
        width: 16,
        height: 16,
        data: vec![1.0; 16 * 16 * 4],
    };
    let args = Args {
        quiet: false,
        ..Args::default()
    };
    let res = trace_bordered(img, &args);
    assert!(res.is_ok());

    // Check hypotheses_apply flags
    let mut ha_args = Args::default();
    assert!(hypotheses_apply(&ha_args));
    ha_args.monochrome = true;
    assert!(!hypotheses_apply(&ha_args));
    ha_args.monochrome = false;
    ha_args.uncertainty = Some(PathBuf::from("unc.svg"));
    assert!(!hypotheses_apply(&ha_args));
}

fn lcg(s: &mut u64) -> u64 {
    *s = s.wrapping_mul(6364136223846793005).wrapping_add(1);
    *s >> 32
}

fn make_art_upscale(k: usize) -> inkvec_trace::Rgba {
    let (w, h) = (160, 128);
    let mut s = 3u64;
    let mut data = vec![1.0f32; w * h * 4];
    for _ in 0..6 {
        let (x0, y0) = ((lcg(&mut s) as usize) % w, (lcg(&mut s) as usize) % h);
        let (x1, y1) = (
            (x0 + 1 + (lcg(&mut s) as usize) % w).min(w),
            (y0 + 1 + (lcg(&mut s) as usize) % h).min(h),
        );
        let c = [0, 1, 2, 3].map(|_| (lcg(&mut s) % 256) as f32 / 255.0);
        for y in y0..y1 {
            for x in x0..x1 {
                data[(y * w + x) * 4..(y * w + x) * 4 + 4].copy_from_slice(&c);
            }
        }
    }
    for _ in 0..60 {
        let p = (lcg(&mut s) as usize) % (w * h);
        data[p * 4] = (lcg(&mut s) % 256) as f32 / 255.0;
    }
    let (uw, uh) = (w * k, h * k);
    let mut up_data = Vec::with_capacity(uw * uh * 4);
    for y in 0..uh {
        for x in 0..uw {
            let p = (y / k) * w + x / k;
            up_data.extend_from_slice(&data[p * 4..p * 4 + 4]);
        }
    }
    inkvec_trace::Rgba {
        width: uw,
        height: uh,
        data: up_data,
    }
}

#[test]
fn test_intake_unblock_upscale_and_max_dim() {
    let up = make_art_upscale(3);
    let args = Args {
        max_dim: 64,
        quiet: false,
        ..Args::default()
    };
    let intk = intake(up, &args, None);
    assert!(intk.replicated);
    assert!(intk.normalised);
    assert!(intk.img.width <= 64 && intk.img.height <= 64);
}

#[test]
fn test_sr_prepass_execution() {
    let img = make_square();
    let args_sr = Args {
        sr: inkvec_sr::Mode::On,
        sr_command: Some(NEAREST_UPSCALER.into()),
        sr_no_recolour: true,
        quiet: false,
        ..Args::default()
    };
    let res = trace_image(img, &args_sr);
    assert!(res.is_ok());
    let stats = res.unwrap().stats;
    assert!(stats
        .iter()
        .any(|s| s.contains("cleaned to") && s.contains("no recolour")));
}

#[test]
fn test_restore_prepass_execution() {
    let img = make_square();
    let args_res = Args {
        restore: inkvec_restore::Mode::On,
        restore_command: Some(IDENTITY_RESTORER.into()),
        quiet: false,
        ..Args::default()
    };
    let res = trace_image(img, &args_res);
    assert!(res.is_ok());
    let stats = res.unwrap().stats;
    assert!(stats.iter().any(|s| s.contains("restored")));
}

#[test]
fn test_sr_tools_dir_and_build_upscaler_default() {
    let _ = sr_tools_dir();
    let def_args = Args::default();
    let _ = build_upscaler(&def_args);
}

#[test]
fn test_trace_bordered_canvas_edge() {
    // 32x32 transparent canvas with 16x16 black square touching canvas edge
    let mut data = vec![0.0f32; 32 * 32 * 4];
    for y in 0..16 {
        for x in 0..16 {
            let i = (y * 32 + x) * 4;
            data[i] = 0.0;
            data[i + 1] = 0.0;
            data[i + 2] = 0.0;
            data[i + 3] = 1.0;
        }
    }
    let img = inkvec_trace::Rgba {
        width: 32,
        height: 32,
        data,
    };
    let args = Args {
        quiet: false,
        ..Args::default()
    };
    assert!(trace_bordered(img, &args).is_ok());
}

#[test]
fn test_cli_main_from_valid_args() {
    let test_img_path =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../bindings/contract/tiny.png");
    // Without `-o` the SVG would be written beside the input, into the source tree.
    let out = std::env::temp_dir().join(format!("cli_main_from_{}.svg", std::process::id()));
    let code = cli_main_from(
        [
            "inkvec".into(),
            test_img_path.to_str().unwrap().into(),
            "-o".into(),
            out.to_str().unwrap().into(),
            "--quiet".into(),
        ]
        .into_iter(),
    );
    assert_eq!(code, ExitCode::SUCCESS);
    assert!(out.exists());
    let _ = std::fs::remove_file(out);
}

#[test]
fn test_restore_auto_kept_probe_with_normalisation() {
    let img = make_square();
    let args = Args {
        restore: inkvec_restore::Mode::Auto,
        restore_threshold: 1000.0, // High threshold -> Decision::Keep
        max_dim: 32,               // Forces normalised = true
        sr: inkvec_sr::Mode::Off,
        // A sign of compression, without which `auto` makes no probe to keep.
        lossy: inkvec_sr::Mode::On,
        ..Args::default()
    };
    let res = trace_image(img, &args);
    assert!(res.is_ok());
}
