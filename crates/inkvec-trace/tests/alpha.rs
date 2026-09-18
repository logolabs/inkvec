//! Translucent layer recovery.
//!
//! Every case here is built from the compositing algebra rather than from an image: the
//! question is whether `alpha::decompose` inverts `c_F = a·C + (1-a)·c_G` from a face
//! partition, and an image would only add rasterisation noise to a question that has an
//! exact answer. The one thing an image *would* add — measurement error — is added
//! directly instead, as a bounded 2/255 perturbation on every face colour.
//!
//! The test that matters most is [`unrelated_flat_colours_yield_no_layers`]. A missed
//! layer costs parameters; an invented one unions faces that are not one shape and
//! paints them a colour that appears nowhere in the image. The gates are set so that the
//! second failure is the rare one, and [`noise_makes_the_minimal_case_conservative`]
//! measures the price: with only two backgrounds, noisy input is sometimes declared
//! opaque rather than guessed at.

use inkvec_trace::alpha::{composite, decompose, decompose_with, AlphaOptions, Space};
use inkvec_trace::color::{rgb_to_oklab, to_hex};

const WHITE: [f32; 3] = [1.0, 1.0, 1.0];
const RED: [f32; 3] = [
    0xe5 as f32 / 255.0,
    0x3e as f32 / 255.0,
    0x3e as f32 / 255.0,
];
const BLUE: [f32; 3] = [
    0x31 as f32 / 255.0,
    0x82 as f32 / 255.0,
    0xce as f32 / 255.0,
];
const GREEN: [f32; 3] = [
    0x38 as f32 / 255.0,
    0xa1 as f32 / 255.0,
    0x69 as f32 / 255.0,
];

fn max_channel_err(a: [f32; 3], b: [f32; 3]) -> f32 {
    (0..3).fold(0.0f32, |m, k| m.max((a[k] - b[k]).abs()))
}

/// A face partition ready for [`decompose`]: `F` face colours with their areas, and `E`
/// adjacency pairs.
type Scene<const F: usize, const E: usize> = ([[f32; 3]; F], [usize; F], [(usize, usize); E]);

/// Perceptual distance. A JND is around 0.01-0.02 here.
fn oklab_err(a: [f32; 3], b: [f32; 3]) -> f32 {
    rgb_to_oklab(a).dist(rgb_to_oklab(b))
}

/// xorshift64*, so "noise" is reproducible and the tests cannot pass by luck of the day.
struct Rng(u64);
impl Rng {
    fn unit(&mut self) -> f32 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        (self.0 >> 40) as f32 / (1u64 << 24) as f32
    }
    /// Uniform in `[-amp, amp]` on every channel.
    fn jitter(&mut self, c: [f32; 3], amp: f32) -> [f32; 3] {
        [
            (c[0] + (self.unit() * 2.0 - 1.0) * amp).clamp(0.0, 1.0),
            (c[1] + (self.unit() * 2.0 - 1.0) * amp).clamp(0.0, 1.0),
            (c[2] + (self.unit() * 2.0 - 1.0) * amp).clamp(0.0, 1.0),
        ]
    }
}

// --- the minimal determined case -----------------------------------------------------

/// Faces: `0` white background, `1` navy background, `2` the layer over white, `3` the
/// layer over navy. The two backgrounds meet, and the layer straddles where they meet.
fn two_background_case(layer: [f32; 3], alpha: f32) -> Scene<4, 4> {
    let navy = [0.08, 0.11, 0.24];
    (
        [
            WHITE,
            navy,
            composite(layer, alpha, WHITE),
            composite(layer, alpha, navy),
        ],
        [4000, 4000, 2000, 2000],
        [(0, 1), (0, 2), (1, 3), (2, 3)],
    )
}

#[test]
fn one_layer_over_two_backgrounds_is_recovered() {
    let (rgb, area, adj) = two_background_case(BLUE, 0.85);
    let res = decompose(&rgb, &area, &adj);

    assert_eq!(res.layers.len(), 1, "expected exactly one layer");
    let l = &res.layers[0];
    assert!(
        (l.alpha - 0.85).abs() <= 0.02,
        "alpha {} not within 0.02 of 0.85",
        l.alpha
    );
    assert!(
        max_channel_err(l.color, BLUE) * 255.0 <= 3.0,
        "colour off by {}/255",
        max_channel_err(l.color, BLUE) * 255.0
    );
    assert_eq!(l.faces, vec![2, 3]);
    assert_eq!(res.opaque_faces, vec![0, 1]);

    // Peeling put the backgrounds back where they belong.
    assert!(max_channel_err(res.base_rgb[2], WHITE) * 255.0 <= 1.0);
    assert!(max_channel_err(res.base_rgb[3], rgb[1]) * 255.0 <= 1.0);
}

#[test]
fn a_range_of_opacities_is_recovered() {
    for &a in &[0.15f32, 0.35, 0.5, 0.7, 0.85, 0.95] {
        let (rgb, area, adj) = two_background_case(BLUE, a);
        let res = decompose(&rgb, &area, &adj);
        assert_eq!(res.layers.len(), 1, "no layer at alpha {a}");
        assert!(
            (res.layers[0].alpha - a).abs() <= 0.02,
            "alpha {} not within 0.02 of {a}",
            res.layers[0].alpha
        );
        assert!(max_channel_err(res.layers[0].color, BLUE) * 255.0 <= 3.0);
    }
}

// --- the under-determined case must be refused ---------------------------------------

#[test]
fn a_layer_over_a_single_background_is_rejected() {
    // Faces: 0 white, 1 the layer over white, 2 an unrelated flat region also on white.
    // Three equations, four unknowns. Any (a, C) on the ray from white through face 1
    // fits exactly, so there is no answer to give — only a guess, which would be a
    // visible error whenever it is wrong.
    let rgb = [WHITE, composite(BLUE, 0.85, WHITE), [0.1, 0.1, 0.12]];
    let area = [6000usize, 2000, 2000];
    let adj = [(0, 1), (0, 2)];
    let res = decompose(&rgb, &area, &adj);
    assert!(
        res.layers.is_empty(),
        "invented a layer from one background: {:?}",
        res.layers
    );
    assert_eq!(res.opaque_faces, vec![0, 1, 2]);
}

#[test]
fn a_second_background_that_the_layer_never_crosses_does_not_help() {
    // The layer sits on white only; navy exists elsewhere in the image. Seeing two
    // backgrounds somewhere is not the same as seeing the layer over both of them.
    let navy = [0.08, 0.11, 0.24];
    let rgb = [WHITE, navy, composite(BLUE, 0.85, WHITE)];
    let area = [6000usize, 3000, 2000];
    let adj = [(0, 1), (0, 2)];
    assert!(decompose(&rgb, &area, &adj).layers.is_empty());
}

// --- the false-positive test ---------------------------------------------------------

/// 4-connected `w x h` grid of faces — a genuine planar map, and the topology in which
/// the layer hypothesis is realisable: `F1, F2` side by side over `G1, G2` side by side,
/// with the diagonals correctly absent.
fn grid_adjacency(w: usize, h: usize) -> Vec<(usize, usize)> {
    let mut a = Vec::new();
    for y in 0..h {
        for x in 0..w {
            let i = y * w + x;
            if x + 1 < w {
                a.push((i, i + 1));
            }
            if y + 1 < h {
                a.push((i, i + w));
            }
        }
    }
    a
}

#[test]
fn unrelated_flat_colours_yield_no_layers() {
    // A flat 4x3 mosaic with nothing translucent in it: twelve saturated, mutually
    // unrelated inks of the kind flat art is actually made of. Every one of the grid's
    // quads is a candidate layer, so the algebra is offered plenty of chances.
    let palette: Vec<[f32; 3]> = vec![
        [1.0, 1.0, 1.0],
        [0.05, 0.06, 0.09],
        [0.90, 0.24, 0.24],
        [0.19, 0.51, 0.81],
        [0.22, 0.63, 0.41],
        [0.96, 0.79, 0.22],
        [0.55, 0.36, 0.72],
        [0.98, 0.55, 0.31],
        [0.13, 0.70, 0.67],
        [0.71, 0.11, 0.42],
        [0.42, 0.45, 0.50],
        [0.78, 0.90, 0.31],
    ];
    let area = vec![3000usize; 12];
    let res = decompose(&palette, &area, &grid_adjacency(4, 3));
    assert!(
        res.layers.is_empty(),
        "invented {} layer(s) in flat art: {:?}",
        res.layers.len(),
        res.layers
            .iter()
            .map(|l| (l.alpha, to_hex(l.color), l.faces.clone()))
            .collect::<Vec<_>>()
    );
    assert_eq!(res.opaque_faces.len(), 12);
    // Nothing was peeled, so every face's base colour is the colour it came in with
    // (up to the sRGB -> linear -> sRGB round trip).
    for (i, &c) in palette.iter().enumerate() {
        assert!(max_channel_err(res.base_rgb[i], c) * 255.0 <= 0.5);
    }
}

#[test]
fn the_false_positive_rate_on_flat_mosaics_stays_low() {
    // The single-palette test above shows the gates hold for one arrangement. This one
    // measures the property they are actually claiming, over 500 random 5x5 mosaics of
    // twenty-five *uniformly random* colours each — a far harsher palette than real
    // flat art, where inks are few and repeat.
    //
    // Every layer reported here is spurious by construction. The measured rate is well
    // under 1%; the bound is set at 2% so that the test fails on a regression in the
    // gates rather than on the run-to-run wobble of a rate this small.
    let adj = grid_adjacency(5, 5);
    let area = vec![1200usize; 25];
    let mut rng = Rng(0x0BAD_F00D_1234_5678);
    let mut hits = 0;
    let mut multi = 0;
    let trials = 500;
    for _ in 0..trials {
        let rgb: Vec<[f32; 3]> = (0..25)
            .map(|_| [rng.unit(), rng.unit(), rng.unit()])
            .collect();
        let res = decompose(&rgb, &area, &adj);
        if !res.layers.is_empty() {
            hits += 1;
            if res.layers.iter().any(|l| l.faces.len() >= 3) {
                multi += 1;
            }
        }
    }
    assert!(
        hits * 50 <= trials,
        "spurious layers on {hits}/{trials} random flat mosaics (bound: 2%)"
    );
    // Three or more faces agreeing on one (a, C) by chance is a different order of
    // improbability, and the ranking prefers those — so a layer that survives with real
    // support is the one we can stand behind.
    assert!(
        multi * 250 <= trials,
        "spurious layers with >=3 faces on {multi}/{trials} mosaics (bound: 0.2%)"
    );
}

#[test]
fn a_flat_gradient_ramp_is_not_a_stack_of_layers() {
    // Six steps of one ramp: every face colour is a blend of the two ends, so every
    // triple is exactly collinear and the parallelism test is satisfied by construction.
    // This is the worst possible input for the algebra, and it must still come back
    // empty: `1-a` is recovered but the *same* (a, C) never explains two faces, and
    // where it would, the two "backgrounds" are not far enough apart to condition it.
    let a = [0.95f32, 0.93, 0.88];
    let b = [0.12f32, 0.20, 0.45];
    let rgb: Vec<[f32; 3]> = (0..6)
        .map(|i| {
            let t = i as f32 / 5.0;
            [
                a[0] + (b[0] - a[0]) * t,
                a[1] + (b[1] - a[1]) * t,
                a[2] + (b[2] - a[2]) * t,
            ]
        })
        .collect();
    let area = vec![3000usize; 6];
    let adj: Vec<(usize, usize)> = (0..5).map(|i| (i, i + 1)).collect();
    let res = decompose(&rgb, &area, &adj);
    assert!(
        res.layers.is_empty(),
        "read a ramp as layers: {:?}",
        res.layers
    );
}

// --- stacked layers ------------------------------------------------------------------

/// The benchmark's `stack_overlap`: red opaque, blue at 0.85, green at 0.85, all on
/// white, all three discs mutually overlapping. Eight flat regions; the numbering is
///
/// ```text
///   0 white   1 red            2 blue/white   3 green/white
///   4 blue/red   5 green/red   6 green/blue/white   7 green/blue/red
/// ```
///
/// and two regions are adjacent exactly when their disc-membership triples differ in one
/// bit and the arc between them is non-degenerate.
fn stack_overlap() -> Scene<8, 12> {
    let bw = composite(BLUE, 0.85, WHITE);
    let br = composite(BLUE, 0.85, RED);
    let rgb = [
        WHITE,
        RED,
        bw,
        composite(GREEN, 0.85, WHITE),
        br,
        composite(GREEN, 0.85, RED),
        composite(GREEN, 0.85, bw),
        composite(GREEN, 0.85, br),
    ];
    // Pixel-centre counts for the three r=66 discs at (96,110), (160,110), (128,166) on
    // a 256x256 canvas.
    let area = [37834usize, 5846, 5846, 5902, 2318, 2262, 2262, 3266];
    let adj = [
        (0, 1),
        (0, 2),
        (0, 3),
        (1, 4),
        (1, 5),
        (2, 4),
        (2, 6),
        (3, 5),
        (3, 6),
        (4, 7),
        (5, 7),
        (6, 7),
    ];
    (rgb, area, adj)
}

#[test]
fn rasterised_lens_tips_do_not_defeat_the_quad_rule() {
    // Traced from the real 128px `stack_overlap.png`, the planar map has *seventeen*
    // adjacencies, not twelve. The five extras are the lens tips — points where one
    // circle's arc crosses another's, which the label grid renders as a shared boundary
    // two or three pixel corners long. One of them, white touching blue-over-red, is a
    // diagonal of the very quad that recovers the blue layer.
    //
    // These faces are also composited in **sRGB**, because that is what an SVG renderer
    // does; see `the_compositing_space_is_a_parameter_not_an_assumption`.
    let bw = inkvec_trace::alpha::composite_in(BLUE, 0.85, WHITE, Space::Srgb);
    let br = inkvec_trace::alpha::composite_in(BLUE, 0.85, RED, Space::Srgb);
    let srgb = |c, a, u| inkvec_trace::alpha::composite_in(c, a, u, Space::Srgb);
    let rgb = [
        WHITE,
        RED,
        bw,
        srgb(GREEN, 0.85, WHITE),
        br,
        srgb(GREEN, 0.85, RED),
        srgb(GREEN, 0.85, bw),
        srgb(GREEN, 0.85, br),
    ];
    let (_, area, ideal_adj) = stack_overlap();
    let mut adj = ideal_adj.to_vec();
    adj.extend_from_slice(&[(0, 4), (0, 5), (0, 6), (1, 7), (3, 7)]);

    let opt = AlphaOptions {
        space: Space::Srgb,
        ..Default::default()
    };
    let res = decompose_with(&rgb, &area, &adj, &opt);
    assert_eq!(
        res.layers.len(),
        2,
        "lens tips cost us a layer: {:?}",
        res.layers
            .iter()
            .map(|l| (l.alpha, to_hex(l.color), l.faces.clone()))
            .collect::<Vec<_>>()
    );
    assert!(oklab_err(res.layers[0].color, GREEN) <= 0.01);
    assert!(oklab_err(res.layers[1].color, BLUE) <= 0.02);
    assert!((res.layers[0].alpha - 0.85).abs() <= 0.02);
    assert!((res.layers[1].alpha - 0.85).abs() <= 0.02);
    assert_eq!(res.layers[0].faces, vec![3, 5, 6, 7]);
    assert_eq!(res.layers[1].faces, vec![2, 4, 6, 7]);
}

#[test]
fn stack_overlap_peels_green_then_blue() {
    let (rgb, area, adj) = stack_overlap();
    let res = decompose(&rgb, &area, &adj);

    assert_eq!(
        res.layers.len(),
        2,
        "expected two layers, got {:?}",
        res.layers
    );

    // Green is frontmost and must come off first: while it is on, faces 6 and 7 are
    // green-over-blue, not blue-over-anything, so blue is only visible on two faces.
    // Peeling green turns them into blue faces and blue then has four.
    let g = &res.layers[0];
    assert!((g.alpha - 0.85).abs() <= 0.02, "green alpha {}", g.alpha);
    assert!(
        max_channel_err(g.color, GREEN) * 255.0 <= 3.0,
        "green colour off by {}/255",
        max_channel_err(g.color, GREEN) * 255.0
    );
    assert_eq!(g.faces, vec![3, 5, 6, 7]);

    let b = &res.layers[1];
    assert!((b.alpha - 0.85).abs() <= 0.02, "blue alpha {}", b.alpha);
    assert!(
        max_channel_err(b.color, BLUE) * 255.0 <= 3.0,
        "blue colour off by {}/255",
        max_channel_err(b.color, BLUE) * 255.0
    );
    assert_eq!(b.faces, vec![2, 4, 6, 7]);

    // Nothing translucent covers white or red.
    assert_eq!(res.opaque_faces, vec![0, 1]);

    // What survives peeling is the two opaque shapes: the white ground under faces
    // 0/2/3/6 and the red disc under faces 1/4/5/7. Three circles, not eight patches.
    for f in [0usize, 2, 3, 6] {
        assert!(
            max_channel_err(res.base_rgb[f], WHITE) * 255.0 <= 2.0,
            "face {f} should peel back to white, got {:?}",
            res.base_rgb[f]
        );
    }
    for f in [1usize, 4, 5, 7] {
        assert!(
            max_channel_err(res.base_rgb[f], RED) * 255.0 <= 2.0,
            "face {f} should peel back to red, got {:?}",
            res.base_rgb[f]
        );
    }
}

#[test]
fn recovered_layers_recomposite_to_the_observed_colours() {
    // The point of a layer model is that it renders back. Re-composite every face from
    // its peeled base and the layers that cover it, and the observation must return.
    let (rgb, area, adj) = stack_overlap();
    let res = decompose(&rgb, &area, &adj);
    for (f, &observed) in rgb.iter().enumerate() {
        let mut c = res.base_rgb[f];
        // Layers are frontmost-first, so paint them back on in reverse.
        for l in res.layers.iter().rev() {
            if l.faces.contains(&f) {
                c = composite(l.color, l.alpha, c);
            }
        }
        assert!(
            max_channel_err(c, observed) * 255.0 <= 2.0,
            "face {f} recomposited to {c:?}, observed {observed:?}"
        );
    }
}

// --- noise ---------------------------------------------------------------------------

#[test]
fn two_over_255_noise_does_not_break_stacked_recovery() {
    // Bounded +/- 2/255 on every channel of every face colour, over 200 seeds. Both
    // layers must still come off, in order, with the right faces.
    //
    // Colour is checked in OKLab rather than per sRGB channel, because a per-channel
    // bound here would be measuring the wrong thing. `C = (c_F - (1-a)c_G)/a` is solved
    // in linear light, and a linear-light error of fixed size is a *large* sRGB error
    // wherever the channel is dark: blue's red channel is 0.19 in sRGB but 0.031 in
    // linear, so the same 0.02 of linear uncertainty is 5/255 on a bright channel and
    // over 20/255 there. That is a real property of the estimator and not a bug, and
    // OKLab distance is the measure that says how visible it is.
    let (clean, area, adj) = stack_overlap();
    let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
    for seed in 0..200 {
        let rgb: Vec<[f32; 3]> = clean.iter().map(|&c| rng.jitter(c, 2.0 / 255.0)).collect();
        let res = decompose(&rgb, &area, &adj);
        assert!(
            res.layers.len() >= 2,
            "seed {seed}: only {} layer(s)",
            res.layers.len()
        );
        let (g, b) = (&res.layers[0], &res.layers[1]);
        assert!(
            (g.alpha - 0.85).abs() <= 0.03,
            "seed {seed}: green alpha {}",
            g.alpha
        );
        assert!(
            (b.alpha - 0.85).abs() <= 0.03,
            "seed {seed}: blue alpha {}",
            b.alpha
        );
        assert!(
            oklab_err(g.color, GREEN) <= 0.03,
            "seed {seed}: green colour {} off by {} in OKLab",
            to_hex(g.color),
            oklab_err(g.color, GREEN)
        );
        assert!(
            oklab_err(b.color, BLUE) <= 0.06,
            "seed {seed}: blue colour {} off by {} in OKLab",
            to_hex(b.color),
            oklab_err(b.color, BLUE)
        );
        assert_eq!(g.faces, vec![3, 5, 6, 7], "seed {seed}: green faces");
        assert_eq!(b.faces, vec![2, 4, 6, 7], "seed {seed}: blue faces");

        // The only bound that really matters: the layered document renders back to what
        // was observed.
        for (f, &observed) in rgb.iter().enumerate() {
            let mut c = res.base_rgb[f];
            for l in res.layers.iter().rev() {
                if l.faces.contains(&f) {
                    c = composite(l.color, l.alpha, c);
                }
            }
            assert!(
                max_channel_err(c, observed) * 255.0 <= 4.0,
                "seed {seed}: face {f} recomposited off by {}/255",
                max_channel_err(c, observed) * 255.0
            );
        }
    }
}

#[test]
fn noise_makes_the_minimal_case_conservative() {
    // Two backgrounds is the algebraic minimum: six equations, four unknowns, two
    // degrees of over-determination. A residual band wide enough to absorb 2/255 of
    // noise there is also wide enough to admit coincidences, so the band is tight and
    // the failure mode is a *miss*. What must never happen is a wrong answer.
    let (clean, area, adj) = two_background_case(BLUE, 0.85);
    let mut rng = Rng(0x2545_F491_4F6C_DD1D);
    let mut found = 0;
    let trials = 400;
    for _ in 0..trials {
        let rgb: Vec<[f32; 3]> = clean.iter().map(|&c| rng.jitter(c, 2.0 / 255.0)).collect();
        let res = decompose(&rgb, &area, &adj);
        for l in &res.layers {
            // Whatever it reports, it must be right.
            assert!(
                (l.alpha - 0.85).abs() <= 0.03,
                "recovered a wrong alpha {} under noise",
                l.alpha
            );
            assert!(
                max_channel_err(l.color, BLUE) * 255.0 <= 12.0,
                "recovered a wrong colour, off by {}/255",
                max_channel_err(l.color, BLUE) * 255.0
            );
            assert_eq!(l.faces, vec![2, 3]);
        }
        found += usize::from(!res.layers.is_empty());
    }
    assert!(
        found * 2 >= trials,
        "minimal case recovered only {found}/{trials} times under 2/255 noise — the \
         gates have become too strict to be useful"
    );
}

#[test]
fn one_over_255_noise_leaves_the_minimal_case_intact() {
    let (clean, area, adj) = two_background_case(BLUE, 0.85);
    let mut rng = Rng(0xD1B5_4A32_D192_ED03);
    for seed in 0..200 {
        let rgb: Vec<[f32; 3]> = clean.iter().map(|&c| rng.jitter(c, 1.0 / 255.0)).collect();
        let res = decompose(&rgb, &area, &adj);
        assert_eq!(res.layers.len(), 1, "seed {seed}: lost the layer");
        assert!((res.layers[0].alpha - 0.85).abs() <= 0.02);
        assert!(max_channel_err(res.layers[0].color, BLUE) * 255.0 <= 6.0);
    }
}

// --- degenerate input ----------------------------------------------------------------

#[test]
fn degenerate_input_is_handled() {
    assert!(decompose(&[], &[], &[]).layers.is_empty());
    assert!(decompose(&[WHITE], &[100], &[]).layers.is_empty());
    // Mismatched lengths, self-pairs and out-of-range ids must not panic.
    let r = decompose(
        &[WHITE, RED],
        &[100, 100],
        &[(0, 0), (0, 1), (1, 9), (7, 7)],
    );
    assert!(r.layers.is_empty());
    assert_eq!(r.opaque_faces, vec![0, 1]);
}

#[test]
fn an_opaque_overlay_is_not_a_layer() {
    // Red drawn opaquely over white and over navy is two faces of the same colour: no
    // background shows through, so `a = 1` and the algebra correctly finds nothing.
    let navy = [0.08, 0.11, 0.24];
    let rgb = [WHITE, navy, RED, RED];
    let area = [4000usize, 4000, 2000, 2000];
    let adj = [(0, 1), (0, 2), (1, 3), (2, 3)];
    assert!(decompose(&rgb, &area, &adj).layers.is_empty());
}

// --- compositing space ---------------------------------------------------------------

#[test]
fn the_compositing_space_is_a_parameter_not_an_assumption() {
    // A renderer that composites on gamma-encoded values produces face colours that are
    // *not* collinear in linear light, so the linear-light solver rejects them. That is
    // the correct behaviour — a wrong gamma is a systematic error, not noise — and it is
    // why DESIGN.md S0 says to estimate the compositing model rather than assume it.
    let navy = [0.08, 0.11, 0.24];
    let rgb = [
        WHITE,
        navy,
        inkvec_trace::alpha::composite_in(BLUE, 0.85, WHITE, Space::Srgb),
        inkvec_trace::alpha::composite_in(BLUE, 0.85, navy, Space::Srgb),
    ];
    let area = [4000usize, 4000, 2000, 2000];
    let adj = [(0, 1), (0, 2), (1, 3), (2, 3)];

    assert!(
        decompose(&rgb, &area, &adj).layers.is_empty(),
        "linear-light solver accepted sRGB-composited input"
    );

    let opt = AlphaOptions {
        space: Space::Srgb,
        ..Default::default()
    };
    let res = decompose_with(&rgb, &area, &adj, &opt);
    assert_eq!(
        res.layers.len(),
        1,
        "sRGB solver missed sRGB-composited input"
    );
    assert!((res.layers[0].alpha - 0.85).abs() <= 0.02);
    assert!(max_channel_err(res.layers[0].color, BLUE) * 255.0 <= 3.0);
}
