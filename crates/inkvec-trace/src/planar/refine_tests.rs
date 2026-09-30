//! Tests of the parallel sub-pixel refinement against the serial loop it replaced
//! (2026-09-30): bit-for-bit equal points and sigmas on anti-aliased scenes, with flat and
//! gradient fills, with and without the alpha channel, on one pixel, one row and on edges
//! long enough for the vertex-parallel path.

use super::*;

/// The refinement as it ran until 2026-09-30: one edge after another on the calling
/// thread, each written back before the next. The reference `measure_subpixel` is held
/// to.
fn refine_serial(
    map: &mut PlanarMap,
    rgb: &[[f32; 3]],
    face_fill: &[FillModel],
    sigma_noise: f64,
    src_alpha: Option<(&[f32], &[f32])>,
) {
    let ctx = RefineCtx {
        src: Source {
            rgb,
            alpha: src_alpha.map(|(img_a, _)| img_a),
            w: map.width,
            h: map.height,
        },
        face_alpha: src_alpha.map(|(_, face_a)| face_a),
        sigma_noise,
        min_contrast: (3.0 * sigma_noise).max(MIN_UNMIX_CONTRAST),
        simplify_faint: false,
        debug: false,
        dump: None,
    };
    for e in &mut map.edges {
        if e.left == u16::MAX || e.right == u16::MAX {
            continue;
        }
        let (Some(fa), Some(fb)) = (
            face_fill.get(e.left as usize),
            face_fill.get(e.right as usize),
        ) else {
            continue;
        };
        let (moved, sigmas): (Vec<Point>, Vec<f64>) = (0..e.points.len())
            .map(|k| refine_vertex(&ctx, fa, fb, e.left, e.right, &e.points, k))
            .unzip();
        let closed = e.closed;
        let sigmas: Vec<f64> = (0..moved.len())
            .map(|k| crate::contour::inflate_for_curvature(&moved, k, sigmas[k], closed))
            .collect();
        e.points = moved;
        e.sigma = sigmas;
    }
}

/// A `w x h` scene of three shapes (a large disc, a bar, a small disc) over a ground
/// (face 0; from 320 px wide on, wavy bands of faces 0 and 3), rendered with 4x4
/// supersampling so the boundaries are anti-aliased, plus a little deterministic noise.
/// Returns the labels (the face at each pixel centre), the image, and the four face
/// colours.
fn scene(w: usize, h: usize, seed: u64) -> (Vec<u16>, Vec<[f32; 3]>, Vec<[f32; 3]>) {
    let colors = vec![
        [0.95f32, 0.93, 0.90],
        [0.10, 0.20, 0.60],
        [0.80, 0.30, 0.20],
        [0.20, 0.70, 0.30],
    ];
    let (cx, cy, r) = (w as f64 * 0.45, h as f64 * 0.5, w.min(h) as f64 * 0.38);
    let bands = w >= 320;
    let face = |x: f64, y: f64| -> u16 {
        if (x - w as f64 * 0.75).hypot(y - h as f64 * 0.3) < w as f64 * 0.08 {
            3
        } else if (y - h as f64 * 0.62).abs() < 2.3 && x > w as f64 * 0.2 && x < w as f64 * 0.9 {
            2
        } else if (x - cx).hypot(y - cy) < r {
            1
        } else if !bands || ((y + 2.0 * (x / 7.0).sin()) / 5.0).floor().rem_euclid(2.0) == 0.0 {
            // From 320 px on, wavy bands over the ground: many long boundaries, so the
            // scene has enough vertices for the parallel schedule.
            0
        } else {
            3
        }
    };
    let mut s = seed | 1;
    let mut noise = move || {
        s ^= s << 13;
        s ^= s >> 7;
        s ^= s << 17;
        ((s >> 40) as f32 / (1u64 << 24) as f32 - 0.5) * 0.01
    };
    let mut labels = Vec::with_capacity(w * h);
    let mut rgb = Vec::with_capacity(w * h);
    for y in 0..h {
        for x in 0..w {
            labels.push(face(x as f64, y as f64));
            let mut acc = [0.0f32; 3];
            for sy in 0..4 {
                for sx in 0..4 {
                    let c = colors[face(
                        x as f64 - 0.375 + 0.25 * sx as f64,
                        y as f64 - 0.375 + 0.25 * sy as f64,
                    ) as usize];
                    for k in 0..3 {
                        acc[k] += c[k] / 16.0;
                    }
                }
            }
            rgb.push([acc[0] + noise(), acc[1] + noise(), acc[2] + noise()]);
        }
    }
    (labels, rgb, colors)
}

fn assert_same(a: &PlanarMap, b: &PlanarMap) {
    assert_eq!(a.edges.len(), b.edges.len());
    for (ea, eb) in a.edges.iter().zip(&b.edges) {
        assert_eq!(ea.points.len(), eb.points.len());
        for (p, q) in ea.points.iter().zip(&eb.points) {
            assert_eq!(
                (p.x.to_bits(), p.y.to_bits()),
                (q.x.to_bits(), q.y.to_bits())
            );
        }
        for (s, t) in ea.sigma.iter().zip(&eb.sigma) {
            assert_eq!(s.to_bits(), t.to_bits());
        }
    }
}

#[test]
fn parallel_refinement_equals_the_serial_loop() {
    for (w, h, seed) in [
        (96usize, 80usize, 3u64),
        (160, 150, 11),
        (320, 300, 13),
        (1, 1, 5),
        (9, 1, 7),
    ] {
        let (labels, rgb, colors) = scene(w, h, seed);
        let base = build(&labels, w, h, colors.len());
        // Flat fills, then one face given a gradient so the root-find path runs too;
        // and with and without the alpha channel.
        let flat: Vec<FillModel> = colors.iter().map(|&c| FillModel::Flat(c)).collect();
        let mut graded = flat.clone();
        graded[1] = FillModel::Linear {
            p0: (0.0, 0.0),
            p1: (w as f64, h as f64),
            c0: [0.05, 0.15, 0.55],
            c1: [0.15, 0.25, 0.65],
            interp: crate::gradient::Interp::Srgb,
            mids: Vec::new(),
        };
        let img_alpha: Vec<f32> = (0..w * h)
            .map(|p| if labels[p] == 0 { 0.0 } else { 1.0 })
            .collect();
        let face_alpha = [0.0f32, 1.0, 1.0, 0.6];
        for fills in [&flat, &graded] {
            for alpha in [None, Some((&img_alpha[..], &face_alpha[..]))] {
                let mut serial = base.clone();
                refine_serial(&mut serial, &rgb, fills, 0.004, alpha);
                let mut par = base.clone();
                refine_subpixel_alpha(&mut par, &rgb, fills, 0.004, false, alpha);
                assert_same(&serial, &par);
            }
        }
        if w >= 96 {
            // An edge long enough for the vertex-parallel path (used from 320 px on).
            assert!(base
                .edges
                .iter()
                .any(|e| e.points.len() >= 2 * PAR_VERTICES));
        }
        // The larger scenes take the parallel schedule, the tiny ones the serial one.
        assert_eq!(refine_in_parallel(&base), w >= 96, "{w}x{h}");
    }
}
