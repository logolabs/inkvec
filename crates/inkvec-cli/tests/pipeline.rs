//! End-to-end tests of the public library entry points: raster in, SVG text out.
//!
//! These cover the layer between traced geometry and bytes on disk (argument defaults, the
//! colour and bilevel emitters, post-processing, transparency, and how the pipeline reports a
//! stop), which the tracer's own tests never reach.

use inkvec_cli::{post_process, trace_image, trace_image_sized, Args, Stop};
use inkvec_trace::Rgba;

fn image(w: usize, h: usize, px: impl Fn(usize, usize) -> [f32; 4]) -> Rgba {
    let mut data = Vec::with_capacity(w * h * 4);
    for y in 0..h {
        for x in 0..w {
            data.extend_from_slice(&px(x, y));
        }
    }
    Rgba {
        width: w,
        height: h,
        data,
    }
}

/// A black 32x32 square centred on a white 64x64 canvas.
fn square() -> Rgba {
    image(64, 64, |x, y| {
        if (16..48).contains(&x) && (16..48).contains(&y) {
            [0.0, 0.0, 0.0, 1.0]
        } else {
            [1.0, 1.0, 1.0, 1.0]
        }
    })
}

fn traced_svg(img: Rgba, args: &Args) -> String {
    let t = trace_image(img, args).expect("trace succeeds");
    post_process(args, t.svg, t.width, t.height)
}

/// Shape elements the emitter writes: fitted paths, and the primitives it prefers when a
/// face is exactly one.
fn shapes(svg: &str) -> usize {
    ["<path", "<rect", "<circle", "<ellipse", "<polygon"]
        .iter()
        .map(|tag| svg.matches(tag).count())
        .sum()
}

fn fills(svg: &str) -> std::collections::BTreeSet<String> {
    svg.match_indices("fill=\"")
        .filter_map(|(i, m)| {
            let rest = &svg[i + m.len()..];
            rest.find('"').map(|end| rest[..end].to_ascii_lowercase())
        })
        .filter(|f| f != "none")
        .map(|f| expand_hex(&f))
        .collect()
}

/// `#abc` written out as `#aabbcc`: two spellings of one colour have to compare equal,
/// because `--minify` picks whichever is shorter.
fn expand_hex(v: &str) -> String {
    match v.strip_prefix('#') {
        Some(b) if b.len() == 3 || b.len() == 4 => {
            let mut out = String::from("#");
            for c in b.chars() {
                out.push(c);
                out.push(c);
            }
            out
        }
        _ => v.to_string(),
    }
}

#[test]
fn a_square_on_white_becomes_a_well_formed_svg_with_both_colours() {
    let args = Args::default();
    let t = trace_image(square(), &args).expect("trace succeeds");
    assert_eq!((t.width, t.height), (64, 64));
    let svg = post_process(&args, t.svg, t.width, t.height);
    assert!(svg.contains("<svg"), "{svg}");
    assert!(svg.trim_end().ends_with("</svg>"), "{svg}");
    assert!(
        shapes(&svg) >= 2,
        "a shape for the square and one for the canvas: {svg}"
    );
    assert!(fills(&svg).len() >= 2, "{svg}");
}

#[test]
fn no_background_leaves_out_the_canvas() {
    let plain = traced_svg(square(), &Args::default());
    let knocked = traced_svg(
        square(),
        &Args {
            no_background: true,
            ..Args::default()
        },
    );
    assert!(
        shapes(&knocked) < shapes(&plain),
        "knock-out should remove the canvas face:\n{plain}\n{knocked}"
    );
    assert!(shapes(&knocked) >= 1, "{knocked}");
}

#[test]
fn minify_is_smaller_and_keeps_the_geometry() {
    let plain = traced_svg(square(), &Args::default());
    let small = traced_svg(
        square(),
        &Args {
            minify: true,
            ..Args::default()
        },
    );
    assert!(small.len() < plain.len());
    assert_eq!(shapes(&small), shapes(&plain));
    assert_eq!(fills(&small), fills(&plain));
}

#[test]
fn bilevel_mode_traces_the_shape() {
    let svg = traced_svg(
        square(),
        &Args {
            bilevel: true,
            ..Args::default()
        },
    );
    assert!(shapes(&svg) >= 1, "{svg}");
}

#[test]
fn a_small_transparent_hole_stays_open_under_cutout() {
    // A black 36x36 block on a transparent canvas, with a 12x12 hole whose one-pixel rim is
    // half-transparent, as an anti-aliased hole is. The rim is a large share of so small a
    // hole, which once kept it from counting as transparent and painted it in the matte.
    let img = image(64, 64, |x, y| {
        let block = (14..50).contains(&x) && (14..50).contains(&y);
        let hole = (26..38).contains(&x) && (26..38).contains(&y);
        let hole_rim = hole && (x == 26 || x == 37 || y == 26 || y == 37);
        if !block {
            [0.0, 0.0, 0.0, 0.0]
        } else if hole_rim {
            [0.0, 0.0, 0.0, 0.5]
        } else if hole {
            [0.0, 0.0, 0.0, 0.0]
        } else {
            [0.0, 0.0, 0.0, 1.0]
        }
    });
    let args = Args {
        cutout: true,
        no_background: true,
        ..Args::default()
    };
    let svg = traced_svg(img, &args);
    let rendered = inkvec_sr::detect::render_svg(&svg, 64, 64).expect("the SVG renders");
    let alpha_at = |x: usize, y: usize| rendered.data[(y * 64 + x) * 4 + 3];
    assert!(
        alpha_at(32, 32) < 0.5,
        "the hole should be transparent: {svg}"
    );
    assert!(alpha_at(18, 18) > 0.5, "the block should be painted: {svg}");
    assert!(
        alpha_at(4, 4) < 0.5,
        "the canvas should be transparent: {svg}"
    );
}

#[test]
fn a_flat_image_is_a_stop_only_under_strict() {
    let flat = || image(32, 32, |_, _| [0.4, 0.5, 0.6, 1.0]);
    assert!(trace_image(flat(), &Args::default()).is_ok());
    let err = trace_image(
        flat(),
        &Args {
            strict: true,
            ..Args::default()
        },
    )
    .expect_err("strict refuses a flat image");
    assert!(
        matches!(err.downcast_ref::<Stop>(), Some(Stop::FlatInput)),
        "{err}"
    );
}

#[test]
fn degenerate_sizes_and_full_transparency_do_not_fail() {
    for (w, h) in [(1, 1), (1, 9), (9, 1), (2, 2)] {
        let t = trace_image(
            image(w, h, |x, _| [x as f32 / 9.0, 0.2, 0.2, 1.0]),
            &Args::default(),
        );
        assert!(t.is_ok(), "{w}x{h}: {:?}", t.err().map(|e| e.to_string()));
    }
    let clear = image(16, 16, |_, _| [0.0, 0.0, 0.0, 0.0]);
    assert!(trace_image(clear, &Args::default()).is_ok());
}

/// Three pie slices side by side on white, anti-aliased as a renderer would draw them.
const SLICES: [[f32; 3]; 3] = [[0.9, 0.1, 0.1], [0.1, 0.2, 0.9], [0.1, 0.7, 0.2]];

fn pie3() -> Rgba {
    const SS: usize = 4;
    image(64, 64, |x, y| {
        let mut acc = [0.0f32; 3];
        for sy in 0..SS {
            for sx in 0..SS {
                let px = x as f32 + (sx as f32 + 0.5) / SS as f32 - 32.0;
                let py = y as f32 + (sy as f32 + 0.5) / SS as f32 - 32.0;
                let c = if px.hypot(py) > 26.0 {
                    [1.0; 3]
                } else {
                    // Spokes at 10, 130 and 250 degrees, so none runs along a pixel row.
                    let a = (py.atan2(px).to_degrees() - 10.0).rem_euclid(360.0);
                    SLICES[(a / 120.0) as usize % 3]
                };
                for k in 0..3 {
                    acc[k] += c[k] / (SS * SS) as f32;
                }
            }
        }
        [acc[0], acc[1], acc[2], 1.0]
    })
}

/// Distance from `p` to the nearest blend of two slice colours.
fn off_blends(p: [f32; 3]) -> f32 {
    let mut best = f32::INFINITY;
    for (i, a) in SLICES.iter().enumerate() {
        for b in &SLICES[i + 1..] {
            let ab: Vec<f32> = (0..3).map(|k| b[k] - a[k]).collect();
            let den: f32 = ab.iter().map(|v| v * v).sum();
            let t = ((0..3).map(|k| (p[k] - a[k]) * ab[k]).sum::<f32>() / den).clamp(0.0, 1.0);
            let d = (0..3)
                .map(|k| (p[k] - a[k] - t * ab[k]).powi(2))
                .sum::<f32>()
                .sqrt();
            best = best.min(d);
        }
    }
    best
}

/// Two fills that share an edge and are painted side by side each cover half of the pixel
/// the edge crosses, and the renderer composites those halves one after the other, so a
/// quarter of whatever lies beneath -- here the white canvas -- shows through as a pale
/// hairline along every spoke. Along the spokes, clear of the six pixels at each end over
/// which the fix tapers in from the junctions, every rendered pixel has to be a blend of two
/// slice colours, at any zoom.
#[test]
fn pie_slices_side_by_side_show_no_background_at_their_shared_edges() {
    let svg = traced_svg(pie3(), &Args::default());
    for size in [64usize, 88, 150, 192] {
        let r = inkvec_sr::detect::render_svg(&svg, size, size).expect("the SVG renders");
        let s = size as f32 / 64.0;
        let mut worst = (0.0f32, 0, 0);
        for y in 0..size {
            for x in 0..size {
                // Pixel centre in traced-image coordinates (the viewBox starts at -0.5).
                let px = (x as f32 + 0.5) / s - 0.5 - 31.5;
                let py = (y as f32 + 0.5) / s - 0.5 - 31.5;
                let rad = px.hypot(py);
                if !(7.0..19.0).contains(&rad) {
                    continue;
                }
                let i = (y * size + x) * 4;
                let d = off_blends([r.data[i], r.data[i + 1], r.data[i + 2]]);
                if d > worst.0 {
                    worst = (d, x, y);
                }
            }
        }
        // Unmended the worst pixel is 0.29 off at every size. At the traced size the
        // half-pixel reach leaves a sixteenth of the canvas there (0.07); at 1.4x it leaves
        // 0.03, and from 2x up nothing.
        let limit = if size == 64 { 0.12 } else { 0.05 };
        assert!(
            worst.0 < limit,
            "at {size}px the canvas shows through a spoke: {:.3} off every blend at {:?}\n{svg}",
            worst.0,
            (worst.1, worst.2)
        );
    }
}

/// `--max-dim` writes the geometry in capped space but presents it at the arrival size: the
/// `viewBox` shrinks to the capped raster while `width`/`height` keep the size that arrived.
#[test]
fn max_dim_cap_keeps_arrival_size_in_attributes() {
    let img = image(128, 96, |x, _| {
        if x < 64 {
            [0.0, 0.0, 0.0, 1.0]
        } else {
            [1.0, 1.0, 1.0, 1.0]
        }
    });

    // 128x96 capped at 64: longest side halves, so the capped raster is 64x48.
    let args = Args {
        max_dim: 64,
        ..Args::default()
    };
    let t = trace_image_sized(img.clone(), &args, Some((128, 96))).expect("trace succeeds");
    let svg = post_process(&args, t.svg, t.width, t.height);
    assert!(svg.contains("width=\"128\" height=\"96\""), "{svg}");
    assert!(svg.contains("viewBox=\"-0.5 -0.5 64 48\""), "{svg}");

    // `max_dim = 0` keeps meaning "no cap": the geometry stays at the arrival size.
    let no_cap = Args {
        max_dim: 0,
        ..Args::default()
    };
    let t0 = trace_image_sized(img, &no_cap, Some((128, 96))).expect("trace succeeds");
    let svg0 = post_process(&no_cap, t0.svg, t0.width, t0.height);
    assert!(svg0.contains("viewBox=\"-0.5 -0.5 128 96\""), "{svg0}");
}

/// A black disc of `area` px² centred on a transparent `n × n` canvas, its rim at the
/// pixel's covered share.
fn lone_disc(n: usize, area: f32) -> Rgba {
    let r = (area / std::f32::consts::PI).sqrt();
    let c = n as f32 / 2.0;
    image(n, n, |x, y| {
        let d = ((x as f32 - c).powi(2) + (y as f32 - c).powi(2)).sqrt();
        [0.0, 0.0, 0.0, (r + 0.5 - d).clamp(0.0, 1.0)]
    })
}

/// A lone 50 px² shape covers 0.3 % of a 128 px canvas, under the palette's rarity floor;
/// it was traced to an empty SVG. It must be drawn, in its own colour.
#[test]
fn a_lone_small_shape_on_a_transparent_canvas_is_drawn() {
    let svg = traced_svg(lone_disc(128, 50.0), &Args::default());
    assert!(shapes(&svg) >= 1, "{svg}");
    assert!(fills(&svg).contains("#000000"), "{svg}");
}

/// A near-empty 144 px raster, a 38 px² black disc on white, was read as eight times
/// more pixels than detail (the round trip's mean error is diluted by the empty canvas),
/// so the speckle floor rose 64-fold to 128 px² and the disc was removed. It must be drawn.
#[test]
fn a_lone_small_shape_on_a_near_empty_canvas_is_drawn() {
    let r = (38.0f32 / std::f32::consts::PI).sqrt();
    let img = image(144, 144, |x, y| {
        let d = ((x as f32 - 72.0).powi(2) + (y as f32 - 72.0).powi(2)).sqrt();
        let v = 1.0 - (r + 0.5 - d).clamp(0.0, 1.0);
        [v, v, v, 1.0]
    });
    let svg = traced_svg(img, &Args::default());
    // Drawn in (near) black: the opaque palette leaves a shape this rare to the carve
    // stage, which paints it its pixels' median colour.
    let dark = |f: &String| {
        f.len() == 7
            && (1..7)
                .step_by(2)
                .all(|i| u8::from_str_radix(&f[i..i + 2], 16).is_ok_and(|v| v < 0x20))
    };
    assert!(fills(&svg).iter().any(dark), "{svg}");
}

/// Weights that cannot be loaded: no restorer for a build without the network, and a failed
/// load for one with it, so both builds take the same path.
fn no_restorer(mode: inkvec_restore::Mode) -> Args {
    Args {
        restore: mode,
        // Below any residual, so `auto` always decides to restore.
        restore_threshold: -1.0,
        restore_weights: Some("no-such-dir/restorer.onnx".into()),
        ..Args::default()
    }
}

/// `--restore auto` asks for the restorer only where it helps, so a binary that cannot
/// restore traces the input as it is and says why, instead of failing the whole trace.
#[test]
fn restore_auto_without_a_restorer_traces_directly() {
    let args = no_restorer(inkvec_restore::Mode::Auto);
    let t = trace_image(square(), &args).expect("auto falls back to tracing directly");
    let note = t
        .stats
        .iter()
        .find(|l| l.starts_with("restore"))
        .expect("a restore line");
    assert!(note.contains("no restorer is available"), "{note}");
    assert!(note.contains("traced directly"), "{note}");
    let direct = trace_image(square(), &Args::default()).expect("trace succeeds");
    assert_eq!(t.svg, direct.svg, "the fallback is the plain trace");
}

/// Upscaler flags that cannot give an upscaler: an empty `--sr-command`, the same failure a
/// missing `tools/inkvec_sr` gives, without depending on what is installed beside the test.
fn no_upscaler(mode: inkvec_sr::Mode) -> Args {
    Args {
        sr: mode,
        // Below any residual, so `auto` always decides to clean.
        sr_threshold: -1.0,
        sr_command: Some(String::new()),
        ..Args::default()
    }
}

/// `--sr auto` asks for the clean-up only where it helps, so a machine without the
/// upscaler traces the input as it is and says why, as `--restore auto` does. It used to
/// fail the whole trace ("the packaged SR pre-pass ... was not found").
#[test]
fn sr_auto_without_an_upscaler_traces_directly() {
    let args = no_upscaler(inkvec_sr::Mode::Auto);
    let t = trace_image(square(), &args).expect("auto falls back to tracing directly");
    let note = t
        .stats
        .iter()
        .find(|l| l.starts_with("sr "))
        .expect("an sr line");
    assert!(note.contains("no upscaler is available"), "{note}");
    assert!(note.contains("traced directly"), "{note}");
    let direct = trace_image(square(), &Args::default()).expect("trace succeeds");
    assert_eq!(t.svg, direct.svg, "the fallback is the plain trace");
    // Monochrome: the probe is a colour trace, so the fallback traces again as asked.
    let mono = Args {
        monochrome: true,
        ..no_upscaler(inkvec_sr::Mode::Auto)
    };
    let t = trace_image(square(), &mono).expect("auto falls back under monochrome too");
    assert!(t
        .stats
        .iter()
        .any(|l| l.contains("no upscaler is available")));
    let plain_mono = Args {
        monochrome: true,
        ..Args::default()
    };
    assert_eq!(
        t.svg,
        trace_image(square(), &plain_mono).expect("trace").svg
    );
}

/// `--sr on` asked for the clean-up outright: without an upscaler it is still an error.
#[test]
fn sr_on_without_an_upscaler_is_an_error() {
    assert!(trace_image(square(), &no_upscaler(inkvec_sr::Mode::On)).is_err());
}

/// `--restore on` asked for the restorer outright: without one it is still an error.
#[test]
fn restore_on_without_a_restorer_is_an_error() {
    let args = no_restorer(inkvec_restore::Mode::On);
    assert!(trace_image(square(), &args).is_err());
}

#[test]
fn strokes_mode_traces_line_art_as_stroked_paths() {
    let img = image(64, 64, |x, y| {
        let on_h = (16..48).contains(&x) && (30..34).contains(&y);
        let on_v = (30..34).contains(&x) && (16..48).contains(&y);
        if on_h || on_v {
            [0.0, 0.0, 0.0, 1.0]
        } else {
            [1.0, 1.0, 1.0, 1.0]
        }
    });
    let args = Args {
        strokes: true,
        ..Args::default()
    };
    let t = trace_image(img, &args).expect("trace succeeds");
    assert!(
        t.svg.contains("stroke-width") || t.svg.contains("stroke="),
        "should emit strokes: {}",
        t.svg
    );
}

#[test]
fn pipeline_with_detect_strokes_and_use_symbols() {
    let img = image(64, 64, |x, y| {
        let in_box1 = (8..24).contains(&x) && (8..24).contains(&y);
        let in_box2 = (40..56).contains(&x) && (40..56).contains(&y);
        if in_box1 || in_box2 {
            [0.2, 0.4, 0.8, 1.0]
        } else {
            [1.0, 1.0, 1.0, 1.0]
        }
    });
    let args = Args {
        detect_strokes: true,
        use_symbols: true,
        ..Args::default()
    };
    let t = trace_image(img, &args).expect("trace succeeds");
    assert!(!t.svg.is_empty());
}

/// A black ring of width 10 round a white middle, on a white page: anti-aliased by 4x4
/// supersampling.
fn ring_on_white() -> Rgba {
    image(128, 128, |x, y| {
        let mut ink = 0.0;
        for sy in 0..4 {
            for sx in 0..4 {
                let (px, py) = (
                    x as f32 + (sx as f32 + 0.5) / 4.0,
                    y as f32 + (sy as f32 + 0.5) / 4.0,
                );
                let r = ((px - 64.0).powi(2) + (py - 64.0).powi(2)).sqrt();
                if (r - 36.0).abs() <= 5.0 {
                    ink += 1.0 / 16.0;
                }
            }
        }
        let v = 1.0 - ink;
        [v, v, v, 1.0]
    })
}

#[test]
fn a_ring_on_white_becomes_one_stroke_and_its_middle_leaves_with_it() {
    let args = Args {
        detect_strokes: true,
        ..Args::default()
    };
    let svg = traced_svg(ring_on_white(), &args);
    assert!(
        svg.contains("stroke=\"#000000\"") || svg.contains("stroke=\"#000\""),
        "{svg}"
    );
    // The page and the stroke: the white middle is the page showing through, not a white
    // disc painted over a black one.
    assert_eq!(svg.matches("fill=\"#fff").count(), 1, "{svg}");
    assert_eq!(shapes(&svg), 2, "{svg}");
}

#[test]
fn pipeline_with_layers() {
    let img = image(64, 64, |x, y| {
        if (16..48).contains(&x) && (16..48).contains(&y) {
            [0.8, 0.2, 0.2, 0.5]
        } else {
            [1.0, 1.0, 1.0, 1.0]
        }
    });
    let args = Args {
        layers: true,
        ..Args::default()
    };
    let t = trace_image(img, &args).expect("trace succeeds");
    assert!(!t.svg.is_empty());
}

#[test]
fn pipeline_with_select_hypotheses() {
    let img = image(32, 32, |x, y| {
        if (8..24).contains(&x) && (8..24).contains(&y) {
            [0.0, 0.0, 0.0, 1.0]
        } else {
            [1.0, 1.0, 1.0, 1.0]
        }
    });
    let args = Args {
        hypotheses: true,
        ..Args::default()
    };
    let t = trace_image(img, &args).expect("trace succeeds");
    assert!(!t.svg.is_empty());
}

#[test]
fn pipeline_with_restore_auto() {
    let args = Args {
        restore: inkvec_restore::Mode::Auto,
        ..Args::default()
    };
    let t = trace_image(square(), &args).expect("trace succeeds");
    assert!(!t.svg.is_empty());
}

#[test]
fn pipeline_with_editability() {
    let args = Args {
        editability: true,
        ..Args::default()
    };
    let t = trace_image(pie3(), &args).expect("trace succeeds");
    assert!(!t.svg.is_empty());
}

#[test]
fn pipeline_with_monochrome() {
    let args = Args {
        monochrome: true,
        ..Args::default()
    };
    let t = trace_image(pie3(), &args).expect("trace succeeds");
    assert!(!t.svg.is_empty());
}

#[test]
fn pipeline_with_bilevel() {
    let args = Args {
        bilevel: true,
        ..Args::default()
    };
    let t = trace_image(pie3(), &args).expect("trace succeeds");
    assert!(!t.svg.is_empty());
}

#[test]
fn pipeline_with_clean_damage() {
    let args_auto = Args {
        no_gradients: true,
        no_repair: true,
        ..Args::default()
    };
    assert!(trace_image(pie3(), &args_auto).is_ok());

    let args_on = Args {
        intake_scale: true,
        content_units: true,
        simplify_faint: true,
        ..Args::default()
    };
    assert!(trace_image(pie3(), &args_on).is_ok());
}

#[test]
fn pipeline_with_margin_and_strip_alpha() {
    let args = Args {
        margin: 0.1,
        no_background: true,
        cutout: true,
        minify: true,
        ..Args::default()
    };
    let t = trace_image(pie3(), &args).expect("trace succeeds");
    assert!(!t.svg.is_empty());
}

#[test]
fn pipeline_with_uncertainty() {
    let unc_path = std::env::temp_dir().join("test_pipe_unc.svg");
    let args = Args {
        uncertainty: Some(unc_path.clone()),
        uncertainty_k: 2.0,
        ..Args::default()
    };
    let t = trace_image(pie3(), &args).expect("trace succeeds");
    assert!(!t.svg.is_empty());
    assert!(unc_path.exists());
    let _ = std::fs::remove_file(unc_path);
}

#[test]
fn pipeline_with_touches_border() {
    let img = image(32, 32, |x, y| {
        if x < 16 && y < 16 {
            [0.8, 0.2, 0.2, 1.0]
        } else {
            [0.0, 0.0, 0.0, 0.0]
        }
    });
    let args = Args::default();
    let t = trace_image(img, &args).expect("trace succeeds");
    assert!(!t.svg.is_empty());
}

#[test]
fn pipeline_with_lossy_and_quantise() {
    let args = Args {
        lossy: inkvec_sr::Mode::On,
        max_colors: 4,
        ..Args::default()
    };
    let t = trace_image(pie3(), &args).expect("trace succeeds");
    assert!(!t.svg.is_empty());
}

#[test]
fn cli_main_round_trip() {
    let tmp_out = std::env::temp_dir().join("test_cli_main_out.svg");
    let in_path =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../bindings/contract/tiny.png");
    let code = inkvec_cli::cli_main_from(
        [
            "inkvec".to_string(),
            in_path.to_str().unwrap().to_string(),
            "-o".to_string(),
            tmp_out.to_str().unwrap().to_string(),
        ]
        .into_iter(),
    );
    assert_eq!(code, inkvec_cli::ExitCode::SUCCESS);
    let _ = std::fs::remove_file(tmp_out);
}

#[test]
fn strokes_mode_with_monochrome_and_refine() {
    let img = image(64, 64, |x, y| {
        let on_h = (16..48).contains(&x) && (30..34).contains(&y);
        let on_v = (30..34).contains(&x) && (16..48).contains(&y);
        if on_h || on_v {
            [0.0, 0.0, 0.0, 1.0]
        } else {
            [1.0, 1.0, 1.0, 1.0]
        }
    });
    let args = Args {
        strokes: true,
        monochrome: true,
        no_background: true,
        stroke_refine: 2,
        ..Args::default()
    };
    let t = trace_image(img, &args).expect("trace succeeds");
    assert!(!t.svg.is_empty());
}

#[test]
fn pipeline_with_no_soft_intake_and_no_unblock() {
    let args = Args {
        no_soft_intake: true,
        no_unblock: true,
        ..Args::default()
    };
    let t = trace_image(pie3(), &args).expect("trace succeeds");
    assert!(!t.svg.is_empty());
}

#[test]
fn pipeline_with_balanced_mode() {
    let args = Args {
        mode: inkvec_cli::TraceMode::Balanced,
        ..Args::default()
    };
    let t = trace_image(pie3(), &args).expect("trace succeeds");
    assert!(!t.svg.is_empty());
}

#[test]
fn pipeline_with_fast_mode() {
    let args = Args {
        mode: inkvec_cli::TraceMode::Fast,
        ..Args::default()
    };
    let t = trace_image(pie3(), &args).expect("trace succeeds");
    assert!(!t.svg.is_empty());
}

#[test]
fn pipeline_with_non_native_alpha() {
    let img = image(48, 48, |x, y| {
        if (12..36).contains(&x) && (12..36).contains(&y) {
            [0.8, 0.2, 0.2, 0.5]
        } else {
            [0.0, 0.0, 0.0, 0.0]
        }
    });
    let args = Args {
        native_alpha: false,
        ..Args::default()
    };
    let t = trace_image(img, &args).expect("trace succeeds");
    assert!(!t.svg.is_empty());
}

#[test]
fn pipeline_with_non_native_alpha_and_cutout() {
    let img = image(48, 48, |x, y| {
        let inside = (12..36).contains(&x) && (12..36).contains(&y);
        let hole = (20..28).contains(&x) && (20..28).contains(&y);
        if inside && !hole {
            [0.2, 0.8, 0.2, 1.0]
        } else {
            [0.0, 0.0, 0.0, 0.0]
        }
    });
    let args = Args {
        native_alpha: false,
        cutout: true,
        ..Args::default()
    };
    let t = trace_image(img, &args).expect("trace succeeds");
    assert!(!t.svg.is_empty());
}

#[test]
fn white_on_clear_with_non_native_alpha() {
    let bytes = std::fs::read(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../bindings/contract/white_on_clear.png"),
    )
    .expect("fixture");
    let (img, _) = inkvec_trace::decode_image_capped(&bytes, 0).expect("decode");
    let args = Args {
        native_alpha: false,
        ..Args::default()
    };
    let t = trace_image(img, &args).expect("trace succeeds");
    assert!(!t.svg.is_empty());
}

#[test]
fn non_flat_under_strict_succeeds() {
    let args = Args {
        strict: true,
        ..Args::default()
    };
    let t = trace_image(pie3(), &args).expect("trace succeeds");
    assert!(!t.svg.is_empty());
}

#[test]
fn pipeline_with_time_budget() {
    let args = Args {
        time_budget: 10.0,
        ..Args::default()
    };
    let t = trace_image(pie3(), &args).expect("trace succeeds");
    assert!(!t.svg.is_empty());
}

#[test]
fn pipeline_with_simplify_faint() {
    let args = Args {
        simplify_faint: true,
        ..Args::default()
    };
    let t = trace_image(pie3(), &args).expect("trace succeeds");
    assert!(!t.svg.is_empty());
}

#[test]
fn pipeline_without_harmonize() {
    let args = Args {
        harmonize: false,
        ..Args::default()
    };
    let t = trace_image(pie3(), &args).expect("trace succeeds");
    assert!(!t.svg.is_empty());
}
