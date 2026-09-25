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

/// `--restore on` asked for the restorer outright: without one it is still an error.
#[test]
fn restore_on_without_a_restorer_is_an_error() {
    let args = no_restorer(inkvec_restore::Mode::On);
    assert!(trace_image(square(), &args).is_err());
}
