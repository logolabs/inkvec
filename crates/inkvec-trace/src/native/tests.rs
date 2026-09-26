//! Unit tests for the native-alpha palette. Every reference value below comes from the
//! defining formula written out independently, from an identity (a blend of two inks is
//! found at its coverage, alpha read back from two grounds is the alpha composited with),
//! or from a few-pixel fixture whose inks are known by construction.

use super::*;
use crate::color::{de00, rgb_to_oklab, Oklab, Palette, PaletteEvidence};
use crate::gradient::{FillModel, Interp};

const RED: [f32; 3] = [0.9, 0.1, 0.1];
const BLUE: [f32; 3] = [0.1, 0.2, 0.9];
const GREEN: [f32; 3] = [0.1, 0.9, 0.1];
/// The same two opaque inks as `[W, a]`.
const A4: [f32; 4] = [0.9, 0.1, 0.1, 1.0];
const B4: [f32; 4] = [0.1, 0.2, 0.9, 1.0];

/// A straight colour `s` at opacity `a`, seen over white.
fn on_white(s: [f32; 3], a: f32) -> [f32; 3] {
    s.map(|v| v * a + 1.0 - a)
}

/// A straight colour at opacity `a` as the two-ground point a pixel of it is.
fn ink(s: [f32; 3], a: f32) -> Ink2 {
    pixel_points(&[on_white(s, a)], &[a])[0]
}

fn mix<const N: usize>(a: [f32; N], b: [f32; N], t: f32) -> [f32; N] {
    std::array::from_fn(|k| a[k] * (1.0 - t) + b[k] * t)
}

fn close<const N: usize>(got: [f32; N], want: [f32; N], tol: f32) -> bool {
    got.iter().zip(&want).all(|(g, w)| (g - w).abs() <= tol)
}

// ---------------------------------------------------------------------------------------
// The two-ground point
// ---------------------------------------------------------------------------------------

#[test]
fn over_black_takes_the_uncovered_share_of_the_grey_ground_off() {
    // Mid-grey second ground: W - (1 - a)·0.5, floored at black.
    let w = [0.8f32, 0.5, 0.1];
    assert!(close(over_black(w, 0.6), [0.6, 0.3, 0.0], 1e-6));
    assert_eq!(over_black(w, 1.0), w);
    assert_eq!(over_black(w, 1.7), w, "alpha clamps to 1");
    assert!(
        close(over_black(w, -3.0), [0.3, 0.0, 0.0], 1e-6),
        "alpha clamps to 0"
    );
}

#[test]
fn ink2_measures_the_worse_ground_both_as_distance_and_as_de00() {
    let o = |l: f32| Oklab { l, a: 0.0, b: 0.0 };
    let (p, q) = (
        Ink2 {
            w: o(0.5),
            k: o(0.2),
        },
        Ink2 {
            w: o(0.6),
            k: o(0.5),
        },
    );
    assert!((p.dist(q) - 0.3).abs() < 1e-6);
    let (p, q) = (
        Ink2 {
            w: o(0.2),
            k: o(0.5),
        },
        Ink2 {
            w: o(0.5),
            k: o(0.6),
        },
    );
    assert!((p.dist(q) - 0.3).abs() < 1e-6);
    // White paint and the clear ground agree over white; over the grey ground they are
    // white against grey, and that is the whole perceptual difference.
    let (paint, ground) = (ink([1.0; 3], 1.0), ink([1.0; 3], 0.0));
    let want = de00([1.0; 3], [0.5; 3]);
    assert!(
        (paint.de00(ground) - want).abs() < 0.05,
        "{}",
        paint.de00(ground)
    );
    assert!(want > 10.0);
    // Opaque: plain CIEDE2000.
    let (r, b) = (ink(RED, 1.0), ink(BLUE, 1.0));
    assert!((r.de00(b) - de00(RED, BLUE)).abs() < 0.05);
}

#[test]
fn palette_entries_and_pixels_carry_their_opacity_into_four_channels() {
    let wash = on_white(BLUE, 0.4);
    let pal = Palette {
        colors: vec![rgb_to_oklab(RED), rgb_to_oklab(wash), rgb_to_oklab(GREEN)],
        rgb: vec![RED, wash, GREEN],
        weight: vec![0.3; 3],
        alpha: vec![1.0, 0.4], // the third entry has no alpha: opaque
    };
    assert_eq!(
        ink_rgba_w(&pal),
        vec![
            [0.9, 0.1, 0.1, 1.0],
            [wash[0], wash[1], wash[2], 0.4],
            [0.1, 0.9, 0.1, 1.0]
        ]
    );
    let pts = ink_points(&pal);
    assert_eq!(pts[0], Ink2::opaque(rgb_to_oklab(RED)));
    assert_eq!(pts[2], Ink2::opaque(rgb_to_oklab(GREEN)));
    assert_eq!(pts[1].w, rgb_to_oklab(wash));
    assert_eq!(pts[1].k, rgb_to_oklab(over_black(wash, 0.4)));
    assert!((pts[1].alpha() - 0.4).abs() < 0.01);
    assert_eq!(
        rgba_w(&[RED, RED], &[1.5, -0.2]),
        vec![[0.9, 0.1, 0.1, 1.0], [0.9, 0.1, 0.1, 0.0]]
    );
}

#[test]
fn same_opacity_joins_inks_within_five_hundredths() {
    let pal = Palette {
        colors: vec![rgb_to_oklab(RED); 4],
        rgb: vec![RED; 4],
        weight: vec![0.25; 4],
        alpha: vec![1.0, 0.97, 0.5],
    };
    let same = same_opacity(&pal);
    assert!(same(0, 1));
    assert!(same(0, 3), "a missing alpha is opaque");
    assert!(!same(0, 2));
    assert!(!same(2, 3));
}

// ---------------------------------------------------------------------------------------
// Palette helpers
// ---------------------------------------------------------------------------------------

#[test]
fn bin_is_a_base_24_index_of_lightness_then_a_then_b() {
    let o = |l, a, b| Oklab { l, a, b };
    let idx = |l: u64, a: u64, b: u64| l * 576 + a * 24 + b;
    assert_eq!(bin(o(0.0, -0.4, -0.4)), 0);
    assert_eq!(bin(o(1.0, 0.4, 0.4)), 24 * 24 * 24 - 1);
    // l 0.5 -> 11.5 -> 12; a 0.2 -> 0.75 of the range -> 17.25 -> 17; b -0.2 -> 5.75 -> 6.
    assert_eq!(bin(o(0.5, 0.2, -0.2)), idx(12, 17, 6));
    // b 0.1 -> 0.625 of the range -> 14.375 -> 14; a -0.3 -> 0.125 -> 2.875 -> 3.
    assert_eq!(bin(o(0.25, -0.3, 0.1)), idx(6, 3, 14));
    // Out of range clamps to the edge bins.
    assert_eq!(bin(o(1.5, 0.9, -0.9)), idx(23, 23, 0));
}

#[test]
fn claim_spread_counts_strictly_nearer_pixels_and_reports_the_median_spread() {
    let c = Ink2::opaque(Oklab {
        l: 0.5,
        a: 0.0,
        b: 0.0,
    });
    let px: Vec<Ink2> = [0.0f32, 0.01, 0.02, 0.03, 0.2]
        .iter()
        .map(|&d| {
            Ink2::opaque(Oklab {
                l: 0.5 + d,
                a: 0.0,
                b: 0.0,
            })
        })
        .collect();
    let d: Vec<f32> = px.iter().map(|p| p.dist(c)).collect();
    // Pixel 3 is exactly as near an accepted ink already: a tie is not a claim.
    let nearest = vec![
        f32::INFINITY,
        f32::INFINITY,
        f32::INFINITY,
        d[3],
        f32::INFINITY,
    ];
    let (n, spread) = claim_spread(&px, &nearest, c, 0.05, 1);
    assert_eq!(n, 4);
    // Within tolerance: 0, 0.01, 0.02; the median is the middle one.
    assert_eq!(spread, d[1]);
    // Sampled at every other pixel, the count is scaled back up.
    let (n, spread) = claim_spread(&px, &nearest, c, 0.05, 2);
    assert_eq!((n, spread), (6, d[2]));
    // Nothing within tolerance: no spread.
    assert_eq!(claim_spread(&px[4..], &nearest[4..], c, 0.05, 1), (1, 0.0));
}

/// A 5 x 4 image with ink `A` where `on(x, y)` and `B` elsewhere.
fn two_ink_image(on: impl Fn(usize, usize) -> bool) -> Vec<Ink2> {
    let (a, b) = (ink(RED, 1.0), ink(BLUE, 1.0));
    (0..20)
        .map(|i| if on(i % 5, i / 5) { a } else { b })
        .collect()
}

#[test]
fn interior_fraction_counts_claimed_pixels_with_four_claimed_neighbours() {
    let c = ink(RED, 1.0);
    // Red claims its own pixels and not blue's.
    let nearest = vec![0.1; 20];
    assert!(ink(BLUE, 1.0).dist(c) > 0.1);
    // A 3 x 3 block against the top edge: the outside counts as the block's own, so the
    // interior is its middle column's top two pixels.
    let px = two_ink_image(|x, y| (1..4).contains(&x) && y < 3);
    assert_eq!(interior_fraction(&px, 5, 4, c, &nearest, 1), 2.0 / 9.0);
    // The two right-hand columns: the edge column is interior, the other is not.
    let px = two_ink_image(|x, _| x >= 3);
    assert_eq!(interior_fraction(&px, 5, 4, c, &nearest, 1), 0.5);
    // A single row.
    let px = two_ink_image(|_, y| y == 1);
    assert_eq!(interior_fraction(&px, 5, 4, c, &nearest, 1), 0.0);
    // Nothing claimed; and inputs it cannot measure.
    assert_eq!(interior_fraction(&px, 5, 4, c, &[0.0; 20], 1), 0.0);
    assert_eq!(interior_fraction(&px, 0, 4, c, &nearest, 1), 1.0);
    assert_eq!(interior_fraction(&px[..10], 5, 4, c, &nearest, 1), 1.0);
}

#[test]
fn six_is_the_colour_over_both_grounds_and_from_six_inverts_it() {
    let c = ink(BLUE, 0.4); // over white 0.64 0.68 0.96; over the grey ground 0.3 lower
    let want = [0.64f32, 0.68, 0.96, 0.34, 0.38, 0.66];
    assert!(close(six(c, false), want, 1e-4), "{:?}", six(c, false));
    let lin = |v: f32| ((v + 0.055) / 1.055).powf(2.4);
    assert!(
        close(six(c, true), want.map(lin), 1e-4),
        "{:?}",
        six(c, true)
    );
    for linear in [false, true] {
        assert!(from_six(six(c, linear), linear).dist(c) < 1e-4);
    }
}

// ---------------------------------------------------------------------------------------
// Blend tests
// ---------------------------------------------------------------------------------------

#[test]
fn blend_pairs_finds_a_mix_of_two_inks_and_only_that_pair() {
    let (r, b, g) = (ink(RED, 1.0), ink(BLUE, 1.0), ink(GREEN, 1.0));
    let acc = [r, b, g];
    // 30/70 in sRGB: on the red-blue chord in sRGB space, nowhere else.
    let c = ink(mix(RED, BLUE, 0.7), 1.0);
    let pairs = blend_pairs(c, &acc, 0.01, 0.04);
    assert!(pairs.iter().all(|p| (p.0, p.1) == (0, 1)), "{pairs:?}");
    let srgb = pairs.iter().find(|p| !p.2).expect("found in sRGB");
    assert!(srgb.3 < 1e-3, "{srgb:?}");
    // The same mix in linear light is found in linear space.
    let lin = |v: f32| ((v + 0.055) / 1.055).powf(2.4);
    let unlin = |v: f32| 1.055 * v.powf(1.0 / 2.4) - 0.055;
    let c_lin = ink(mix(RED.map(lin), BLUE.map(lin), 0.7).map(unlin), 1.0);
    let pairs = blend_pairs(c_lin, &acc, 0.01, 0.04);
    let l = pairs.iter().find(|p| p.2).expect("found in linear light");
    assert_eq!((l.0, l.1), (0, 1));
    assert!(l.3 < 1e-3, "{l:?}");
    // An accepted ink is no blend of the others, and one ink alone has no pairs.
    assert!(blend_pairs(g, &acc, 0.01, 0.04).is_empty());
    assert!(blend_pairs(c, &acc[..1], 1.0, 0.04).is_empty());
    // Too near an end of the chord to be a blend.
    let near_end = ink(mix(RED, BLUE, 0.01), 1.0);
    assert!(blend_pairs(near_end, &acc[..2], 0.01, 0.04).is_empty());
    // A repeated ink has no chord with itself.
    let pairs = blend_pairs(c, &[r, r, b], 0.01, 0.04);
    assert!(!pairs.is_empty());
    assert!(pairs.iter().all(|p| p.1 == 2), "{pairs:?}");
}

#[test]
fn an_anti_aliased_rim_is_a_blend_of_the_paint_and_the_clear_ground() {
    let paint = ink([1.0; 3], 1.0);
    let ground = ink([1.0; 3], 0.0);
    let rim = ink([1.0; 3], 0.3);
    let pairs = blend_pairs(rim, &[paint, ground], 0.01, 0.04);
    let p = pairs.iter().find(|p| !p.2).expect("an sRGB blend");
    assert_eq!((p.0, p.1), (0, 1));
    assert!(p.3 < 1e-3);
}

/// Pixels of a row-major image of red `A`, blue `B` and their sRGB midpoint `M`, with the
/// caches `straddle_fraction` reads.
fn straddle_image(row: &str, rows: usize) -> (Vec<Ink2>, Vec<[f32; 6]>, Vec<[f32; 6]>) {
    let (a, b, m) = (ink(RED, 1.0), ink(BLUE, 1.0), ink(mix(RED, BLUE, 0.5), 1.0));
    let px: Vec<Ink2> = row
        .chars()
        .map(|ch| match ch {
            'A' => a,
            'B' => b,
            _ => m,
        })
        .collect::<Vec<_>>()
        .repeat(rows);
    let s6 = px.iter().map(|&p| six(p, false)).collect();
    let l6 = px.iter().map(|&p| six(p, true)).collect();
    (px, s6, l6)
}

#[test]
fn straddle_fraction_is_the_share_of_the_blend_between_both_of_its_inks() {
    let (a, b, m) = (ink(RED, 1.0), ink(BLUE, 1.0), ink(mix(RED, BLUE, 0.5), 1.0));
    // Of the three midpoint pixels only the first has red on one side and blue on the
    // other; the other two each touch one end only.
    let (px, s6, l6) = straddle_image("AMBBMMA", 1);
    let near = vec![0.05; px.len()];
    for linear in [false, true] {
        let f = straddle_fraction(&px, &s6, &l6, 7, 1, m, &near, a, b, linear, 1);
        assert!((f - 1.0 / 3.0).abs() < 1e-6, "linear={linear}: {f}");
    }
    // At the right edge the row does not wrap onto the next one.
    let (px, s6, l6) = straddle_image("BAM", 2);
    let near = vec![0.05; px.len()];
    assert_eq!(
        straddle_fraction(&px, &s6, &l6, 3, 2, m, &near, a, b, false, 1),
        0.0
    );
    // Nothing claimed: nothing speaks against the blend.
    let none = vec![0.0; px.len()];
    assert_eq!(
        straddle_fraction(&px, &s6, &l6, 3, 2, m, &none, a, b, false, 1),
        1.0
    );
    // Degenerate inputs say "no straddle".
    assert_eq!(
        straddle_fraction(&px, &s6, &l6, 0, 2, m, &near, a, b, false, 1),
        0.0
    );
    assert_eq!(
        straddle_fraction(&px[..4], &s6, &l6, 3, 2, m, &near, a, b, false, 1),
        0.0
    );
    assert_eq!(
        straddle_fraction(&px, &s6, &l6, 3, 2, m, &near, a, a, false, 1),
        0.0
    );
}

// ---------------------------------------------------------------------------------------
// Mixtures and blend absorption
// ---------------------------------------------------------------------------------------

#[test]
fn mixture_returns_the_distance_to_the_simplex_and_the_heaviest_ink() {
    let cols = [
        [1.0f32, 0.0, 0.0, 1.0],
        [0.0, 1.0, 0.0, 1.0],
        [0.0, 0.0, 1.0, 1.0],
    ];
    let at = |w: [f32; 3], lift: f32| -> [f32; 4] { [w[0], w[1], w[2], 1.0 + lift] };
    // (weights, lift off the plane, residual, dominant). Inside the triangle the residual is
    // the lift alone; outside it, the distance to the nearest edge.
    let d = 0.06f32.sqrt();
    let cases: [([f32; 3], f32, f32, usize); 8] = [
        ([0.5, 0.3, 0.2], 0.0, 0.0, 0),
        ([0.2, 0.5, 0.3], 0.0, 0.0, 1),
        ([0.2, 0.3, 0.5], 0.0, 0.0, 2),
        ([0.25, 0.35, 0.4], 0.1, 0.1, 2),
        // Outside across each edge: nearest point (0.6, 0.4, 0) and the like.
        ([0.7, 0.5, -0.2], 0.0, d, 0),
        ([0.7, -0.2, 0.5], 0.0, d, 0),
        ([-0.2, 0.7, 0.5], 0.0, d, 1),
        // Beyond an end of an edge: the end ink itself.
        ([-0.5, 1.5, 0.0], 0.0, 0.5 * 2.0f32.sqrt(), 1),
    ];
    for (w, lift, r, who) in cases {
        let (got_r, got_who) = mixture(at(w, lift), &cols).unwrap();
        assert!((got_r - r).abs() < 1e-5, "{w:?}: residual {got_r} want {r}");
        assert_eq!(got_who, who, "{w:?}");
    }
    // Two inks: the nearer end dominates.
    let (r, who) = mixture([0.3f32, 0.7, 0.0, 1.0], &cols[..2]).unwrap();
    assert!(r < 1e-6);
    assert_eq!(who, 1);
    assert_eq!(mixture([0.5f32; 4], &cols[..1]), None);
    assert_eq!(mixture([0.5f32; 4], &[cols[0], cols[0]]), None);
}

/// Labels of a `w`-wide image whose every row is `row` (ink indices).
fn rows(row: &[u16], h: usize) -> Vec<u16> {
    row.repeat(h)
}

/// Pixels of a `w`-wide image with column `sliver` coloured `s` and the rest by label.
fn sliver_px(labels: &[u16], w: usize, sliver: usize, s: [f32; 4]) -> Vec<[f32; 4]> {
    labels
        .iter()
        .enumerate()
        .map(|(i, &l)| {
            if i % w == sliver {
                s
            } else if l == 0 {
                A4
            } else {
                B4
            }
        })
        .collect()
}

#[test]
fn a_blend_sliver_between_two_inks_goes_to_the_ink_it_is_mostly() {
    let inks = [A4, B4, mix(A4, B4, 0.5)];
    let mut labels = rows(&[0, 0, 2, 1, 1], 3);
    let px = sliver_px(&labels, 5, 2, mix(A4, B4, 0.3));
    assert_eq!(
        absorb_blend_slivers(&mut labels, &px, 5, 3, &inks, 0.001),
        1
    );
    assert_eq!(labels, rows(&[0, 0, 0, 1, 1], 3));
}

#[test]
fn a_translucent_sliver_is_explained_with_the_clear_ground() {
    // Red at 60 % over the clear ground, between red and blue paint, with no clear ink in
    // the palette: only the clear ground explains the pixel.
    let inks = [A4, B4, mix(A4, CLEAR, 0.5)];
    let s = mix(A4, CLEAR, 0.4);
    let mut labels = rows(&[0, 0, 2, 1, 1], 3);
    let px = sliver_px(&labels, 5, 2, s);
    assert_eq!(
        absorb_blend_slivers(&mut labels, &px, 5, 3, &inks, 0.001),
        1
    );
    assert_eq!(labels, rows(&[0, 0, 0, 1, 1], 3));
    // Touching one ink only, the same sliver is left alone.
    let mut labels = rows(&[0, 0, 2], 3);
    let px = sliver_px(&labels, 3, 2, s);
    assert_eq!(
        absorb_blend_slivers(&mut labels, &px, 3, 3, &inks, 0.001),
        0
    );
    assert_eq!(labels, rows(&[0, 0, 2], 3));
}

#[test]
fn slivers_that_are_not_blends_and_blend_areas_with_an_interior_stay() {
    let inks = [A4, B4, [0.1, 0.9, 0.1, 1.0]];
    let mut labels = rows(&[0, 0, 2, 1, 1], 3);
    let px = sliver_px(&labels, 5, 2, inks[2]);
    assert_eq!(
        absorb_blend_slivers(&mut labels, &px, 5, 3, &inks, 0.001),
        0
    );
    assert_eq!(labels, rows(&[0, 0, 2, 1, 1], 3));
    // Three columns of blend: the middle one is interior, so this is an area, not a rim.
    let inks = [A4, B4, mix(A4, B4, 0.3)];
    let mut labels = rows(&[0, 0, 2, 2, 2, 1, 1], 5);
    let px: Vec<[f32; 4]> = labels.iter().map(|&l| inks[l as usize]).collect();
    assert_eq!(
        absorb_blend_slivers(&mut labels, &px, 7, 5, &inks, 0.001),
        0
    );
    assert_eq!(labels, rows(&[0, 0, 2, 2, 2, 1, 1], 5));
}

#[test]
fn reassign_moves_blend_pixels_to_the_ink_they_are_mostly() {
    let (w, h) = (6, 4);
    let inks = [A4, B4];
    let mut labels = rows(&[0, 0, 0, 1, 1, 1], h);
    let mut px: Vec<[f32; 4]> = labels.iter().map(|&l| inks[l as usize]).collect();
    // Mostly blue, labelled red.
    px[w + 2] = mix(A4, B4, 0.8);
    // 30 % red over the clear ground, labelled blue: the clear ground is the heaviest part,
    // and of the inks around it red explains it.
    px[2 * w + 3] = mix(A4, CLEAR, 0.7);
    // Green among red and blue: no mixture of them, so it stays.
    px[3 * w + 2] = [0.1, 0.9, 0.1, 1.0];
    assert_eq!(
        reassign_blend_pixels(&mut labels, &px, w, h, &inks, 0.001),
        2
    );
    let mut want = rows(&[0, 0, 0, 1, 1, 1], h);
    want[w + 2] = 1;
    want[2 * w + 3] = 0;
    assert_eq!(labels, want);
}

// ---------------------------------------------------------------------------------------
// Fades
// ---------------------------------------------------------------------------------------

fn linear_model(c0: [f32; 3], mids: Vec<(f64, [f32; 3])>, c1: [f32; 3], len: f64) -> FillModel {
    FillModel::Linear {
        p0: (0.0, 0.0),
        p1: (len, 0.0),
        c0,
        c1,
        interp: Interp::Srgb,
        mids,
    }
}

#[test]
fn model_stops_and_alpha_params_read_a_profile() {
    let c = [0.2f32, 0.3, 0.4];
    assert_eq!(model_stops(&FillModel::Flat(c)), vec![(0.0, c)]);
    let m = linear_model([0.1; 3], vec![(0.3, c)], [0.9; 3], 10.0);
    assert_eq!(
        model_stops(&m),
        vec![(0.0, [0.1; 3]), (0.3, c), (1.0, [0.9; 3])]
    );
    assert_eq!(alpha_params(&FillModel::Flat(c)), 1.0);
    assert_eq!(alpha_params(&linear_model(c, vec![], c, 1.0)), 6.0);
    assert_eq!(alpha_params(&m), 8.0);
    let radial = |aspect: f64, mids: Vec<(f64, [f32; 3])>| FillModel::Radial {
        c: (0.0, 0.0),
        r: 1.0,
        c0: c,
        c1: c,
        interp: Interp::Srgb,
        aspect,
        angle: 0.0,
        mids,
    };
    assert_eq!(alpha_params(&radial(1.0, vec![(0.5, c)])), 7.0);
    assert_eq!(alpha_params(&radial(1.5, vec![])), 7.0);
    assert_eq!(alpha_params(&radial(1.5, vec![(0.2, c), (0.6, c)])), 11.0);
}

#[test]
fn a_fade_over_white_composites_each_stop_at_its_own_opacity() {
    let alpha = linear_model([0.2; 3], vec![(0.5, [0.6; 3])], [0.9; 3], 10.0);
    let fade = Fade {
        color: linear_model(
            [0.8, 0.2, 0.1],
            vec![(0.5, [0.4; 3])],
            [0.1, 0.3, 0.9],
            10.0,
        ),
        alpha: alpha.clone(),
    };
    let want = [
        [0.96f32, 0.84, 0.82],
        [0.64, 0.64, 0.64],
        [0.19, 0.37, 0.91],
    ];
    let got = model_stops(&fade.over_white());
    assert_eq!(got.len(), 3);
    for ((o, g), (wo, w)) in got.iter().zip([0.0, 0.5, 1.0].iter().zip(want)) {
        assert_eq!(o, wo);
        assert!(close(*g, w, 1e-6), "{g:?} vs {w:?}");
    }
    assert!(matches!(fade.over_white(), FillModel::Linear { p1, .. } if p1 == (10.0, 0.0)));
    assert!((fade.rim_alpha() - 0.2).abs() < 1e-7);
    // A flat colour is the colour at every stop.
    let black = Fade {
        color: FillModel::Flat([0.0; 3]),
        alpha,
    };
    let got: Vec<f32> = model_stops(&black.over_white())
        .iter()
        .map(|s| s.1[0])
        .collect();
    assert!(
        close([got[0], got[1], got[2]], [0.8, 0.4, 0.1], 1e-6),
        "{got:?}"
    );
}

#[test]
fn solve_returns_the_exact_solution_and_refuses_a_singular_system() {
    // The first pivot is zero: this needs the row swap.
    let a = vec![
        vec![0.0, 2.0, 1.0],
        vec![1.0, 1.0, 1.0],
        vec![2.0, 1.0, 3.0],
    ];
    let x = [[1.0, 2.0, 3.0], [-1.0, 0.5, 2.0], [0.25, -3.0, 1.0]];
    let b: Vec<[f64; 3]> = a
        .iter()
        .map(|row| std::array::from_fn(|c| (0..3).map(|k| row[k] * x[k][c]).sum()))
        .collect();
    let got = solve(a, b).expect("regular");
    for (g, w) in got.iter().zip(&x) {
        for c in 0..3 {
            assert!((g[c] - w[c]).abs() < 1e-12, "{got:?}");
        }
    }
    let singular = vec![vec![1.0, 2.0], vec![2.0, 4.0]];
    assert_eq!(solve(singular, vec![[1.0; 3], [2.0; 3]]), None);
}

/// The colour a three-stop fade was painted with at `t`.
const STOPS: [[f32; 3]; 3] = [[0.9, 0.2, 0.1], [0.3, 0.7, 0.2], [0.1, 0.3, 0.8]];
fn painted(t: f32) -> [f32; 3] {
    if t <= 0.5 {
        mix(STOPS[0], STOPS[1], t / 0.5)
    } else {
        mix(STOPS[1], STOPS[2], (t - 0.5) / 0.5)
    }
}

/// An 11 x 3 fade: colour by column along three stops, opacity rising along both axes.
fn fade_fixture() -> (Vec<[f32; 3]>, Vec<f32>) {
    let (mut rgb, mut alpha) = (Vec::new(), Vec::new());
    for y in 0..3 {
        for x in 0..11 {
            let a = 0.25 + 0.05 * x as f32 + 0.1 * y as f32;
            rgb.push(on_white(painted(x as f32 / 10.0), a));
            alpha.push(a);
        }
    }
    (rgb, alpha)
}

#[test]
fn fit_colour_stops_recovers_the_colours_a_fade_was_painted_with() {
    let (rgb, alpha) = fade_fixture();
    let model = linear_model([0.0; 3], vec![(0.5, [0.5; 3])], [1.0; 3], 10.0);
    let px: Vec<usize> = (0..33).collect();
    let got = model_stops(&fit_colour_stops(&model, &px, &rgb, &alpha, 11));
    for (g, want) in got.iter().zip(STOPS) {
        assert!(close(g.1, want, 2e-3), "{:?} vs {want:?}", g.1);
    }
}

#[test]
fn a_stop_no_pixel_testifies_about_is_the_fades_mean_colour() {
    // Only the left half: the last stop has no evidence and is held to the opacity-weighted
    // mean colour, `Σ s·a / Σ a`.
    let (mut rgb, mut alpha) = fade_fixture();
    let mut px: Vec<usize> = (0..33).filter(|p| p % 11 <= 5).collect();
    let (mut num, mut den) = ([0.0f64; 3], 0.0f64);
    for &p in &px {
        let s = painted((p % 11) as f32 / 10.0);
        for k in 0..3 {
            num[k] += (s[k] * alpha[p]) as f64;
        }
        den += alpha[p] as f64;
    }
    // A pixel too faint to see must not steer anything, whatever its colour.
    rgb.push([0.0; 3]);
    alpha.push(0.0009);
    px.push(33);
    let model = linear_model([0.0; 3], vec![(0.5, [0.5; 3])], [1.0; 3], 10.0);
    let got = model_stops(&fit_colour_stops(&model, &px, &rgb, &alpha, 11));
    let mean = num.map(|v| (v / den) as f32);
    assert!(close(got[2].1, mean, 1e-5), "{:?} vs {mean:?}", got[2].1);
    assert!(close(got[0].1, STOPS[0], 2e-3), "{:?}", got[0].1);
}

#[test]
fn fade_chi2_charges_opacity_and_premultiplied_colour_beyond_half_a_level() {
    let s = [0.2f32, 0.4, 0.6];
    let (rgb, alpha) = (vec![on_white(s, 0.5)], vec![0.5f32]);
    let sigma = 0.01;
    // Exact, and within the dead zone: nothing to pay.
    assert_eq!(fade_chi2(&[0], &rgb, &alpha, sigma, |_| (s, 0.5)), 0.0);
    assert_eq!(fade_chi2(&[0], &rgb, &alpha, sigma, |_| (s, 0.501)), 0.0);
    // Opacity 0.1 too high: that error, and premultiplied colour s·0.1 short per channel.
    let dead = 0.5 / 255.0;
    let beyond = |e: f64| ((e - dead) / sigma).powi(2);
    let want = beyond(0.1) + beyond(0.02) + beyond(0.04) + beyond(0.06);
    let got = fade_chi2(&[0, 0], &rgb, &alpha, sigma, |_| (s, 0.6));
    assert!(
        (got - 2.0 * want).abs() < 1e-3 * want,
        "{got} vs {}",
        2.0 * want
    );
}

#[test]
fn fit_opacity_prices_the_evidence_once_and_each_stop_at_one_number() {
    // A clean linear ramp in opacity along x.
    let (w, h) = (12usize, 4usize);
    let alpha: Vec<f32> = (0..w * h).map(|p| 0.1 + 0.07 * (p % w) as f32).collect();
    let grey: Vec<[f32; 3]> = alpha.iter().map(|&a| [a; 3]).collect();
    let px: Vec<usize> = (0..w * h).collect();
    let (sigma, lambda) = (1.0 / 255.0, 2.0);
    let (m, cost) = fit_opacity(&grey, w, h, &px, |_| true, sigma, lambda, false);
    assert!(m.is_gradient(), "{m:?}");
    // A ramp is explained exactly: the cost is the geometry and two opacity stops.
    assert!(
        (cost - lambda * alpha_params(&m)).abs() < 1e-3,
        "{cost} for {m:?}"
    );
    assert_eq!(alpha_params(&m), 6.0);
    // Flat only: one number, and the grey fitter's misfit counted once, not once per
    // channel it repeats the alpha in.
    let (m, cost) = fit_opacity(&grey, w, h, &px, |_| true, sigma, lambda, true);
    let flat = gradient::fit_pixels(&grey, w, h, &px, |_| true, |_| true, sigma, lambda)
        .into_iter()
        .find(|f| !f.model.is_gradient())
        .expect("a flat candidate");
    assert_eq!(m, flat.model);
    assert!(
        flat.chi2 > 1000.0,
        "a ramp does not fit flat: {}",
        flat.chi2
    );
    let want = 0.5 * (flat.chi2 / 3.0) + lambda;
    assert!((cost - want).abs() < 1e-9 * want, "{cost} vs {want}");
}

/// A `w` x `h` glow: a straight colour whose opacity ramps along x, in `bands` palette bands.
fn glow(
    w: usize,
    h: usize,
    bands: usize,
    s: [f32; 3],
) -> (Vec<[f32; 3]>, Vec<f32>, Vec<u16>, Palette) {
    let a_at = |x: usize| 0.1 + 0.75 * x as f32 / (w - 1) as f32;
    let band = |x: usize| (x * bands / w) as u16;
    let alpha: Vec<f32> = (0..w * h).map(|p| a_at(p % w)).collect();
    let rgb: Vec<[f32; 3]> = alpha.iter().map(|&a| on_white(s, a)).collect();
    let labels: Vec<u16> = (0..w * h).map(|p| band(p % w)).collect();
    let band_alpha: Vec<f32> = (0..bands)
        .map(|b| {
            let xs: Vec<usize> = (0..w).filter(|&x| band(x) as usize == b).collect();
            xs.iter().map(|&x| a_at(x)).sum::<f32>() / xs.len() as f32
        })
        .collect();
    let pal_rgb: Vec<[f32; 3]> = band_alpha.iter().map(|&a| on_white(s, a)).collect();
    let pal = Palette {
        colors: pal_rgb.iter().map(|&c| rgb_to_oklab(c)).collect(),
        rgb: pal_rgb,
        weight: vec![1.0 / bands as f32; bands],
        alpha: band_alpha,
    };
    (rgb, alpha, labels, pal)
}

/// [`merge_fades`] from no fills yet, at a noise of one level: `(fades, fills, label_ink)`.
fn fades_of(
    labels: &mut [u16],
    rgb: &[[f32; 3]],
    alpha: &[f32],
    (w, h): (usize, usize),
    pal: &Palette,
    lambda: f64,
) -> (Vec<Option<Fade>>, Vec<gradient::FillFit>, Vec<usize>) {
    let (mut fills, mut ink) = (Vec::new(), Vec::new());
    let sigma = 1.0 / 255.0;
    let fades = merge_fades(
        labels, &mut fills, &mut ink, rgb, alpha, w, h, pal, sigma, lambda,
    );
    (fades, fills, ink)
}

#[test]
fn bands_of_one_glow_become_one_fade() {
    let s = [0.8f32, 0.3, 0.1];
    let (w, h) = (16, 4);
    let (rgb, alpha, mut labels, pal) = glow(w, h, 4, s);
    let lambda = gradient::bic_lambda(w * h);
    let (fades, fills, label_ink) = fades_of(&mut labels, &rgb, &alpha, (w, h), &pal, lambda);
    // One new label, 4, for the whole glow.
    assert_eq!(fades.len(), 5);
    assert!(fades[..4].iter().all(Option::is_none));
    let fade = fades[4].as_ref().expect("a fade");
    assert!(labels.iter().all(|&l| l == 4), "{labels:?}");
    assert_eq!(label_ink[4], 0, "the fade keeps its first band's ink");
    // The opacity profile is the ramp it was painted with, in one colour throughout. (The
    // fitter places the ends on the interior columns and pads the edge ones.)
    for x in 1..w - 1 {
        let a = fade.alpha.color_at(x as f64, 1.5)[0];
        assert!((a - alpha[x]).abs() < 0.01, "x={x}: {a} vs {}", alpha[x]);
    }
    for (_, c) in model_stops(&fade.color) {
        assert!(close(c, s, 0.01), "{c:?}");
    }
    assert_eq!(fills[4].model, fade.over_white());
    // The padding is each label's palette colour.
    for (fill, &c) in fills.iter().zip(&pal.rgb) {
        assert_eq!(fill.model, FillModel::Flat(c));
    }
}

#[test]
fn two_separate_glows_get_two_new_labels() {
    let s = [0.8f32, 0.3, 0.1];
    let (rgb1, alpha1, labels1, pal) = glow(16, 4, 4, s);
    // The same glow twice, one above the other, with a row of opaque paint (ink 4) between.
    let w = 16;
    let mut rgb = rgb1.clone();
    let mut alpha = alpha1.clone();
    let mut labels = labels1.clone();
    rgb.extend(vec![s; w]);
    alpha.extend(vec![1.0; w]);
    labels.extend(vec![4u16; w]);
    rgb.extend(rgb1);
    alpha.extend(alpha1);
    labels.extend(labels1);
    let mut pal = pal;
    pal.colors.push(rgb_to_oklab(s));
    pal.rgb.push(s);
    pal.weight.push(0.1);
    pal.alpha.push(1.0);
    let (fades, _, _) = fades_of(&mut labels, &rgb, &alpha, (w, 9), &pal, 3.0);
    assert_eq!(fades.len(), 7);
    assert!(fades[5].is_some() && fades[6].is_some());
    assert!(labels[..64].iter().all(|&l| l == 5));
    assert!(labels[64..80].iter().all(|&l| l == 4));
    assert!(labels[80..].iter().all(|&l| l == 6));
}

#[test]
fn a_flat_wash_stays_a_wash_with_its_colour_over_white() {
    let s = [0.2f32, 0.4, 0.8];
    let (w, h) = (6, 6);
    let a = 0.5f32;
    let rgb = vec![on_white(s, a); w * h];
    let alpha = vec![a; w * h];
    let mut labels = vec![0u16; w * h];
    let pal = Palette {
        colors: vec![rgb_to_oklab(rgb[0])],
        rgb: vec![rgb[0]],
        weight: vec![1.0],
        alpha: vec![0.52], // the palette's opacity, not the pixels'
    };
    let (fades, fills, _) = fades_of(&mut labels, &rgb, &alpha, (w, h), &pal, 2.0);
    assert_eq!(fades, vec![None]);
    assert!(labels.iter().all(|&l| l == 0));
    // The colour from the pixels (exactly s), written over white at the palette opacity.
    let FillModel::Flat(c) = fills[0].model else {
        panic!("{:?}", fills[0].model)
    };
    assert!(close(c, on_white(s, 0.52), 1e-5), "{c:?}");
}

// ---------------------------------------------------------------------------------------
// Golden fixtures: the palette and the whole path
// ---------------------------------------------------------------------------------------

/// A `n` x `n` straight-RGBA image from a per-pixel function.
fn image(n: usize, f: impl Fn(usize, usize) -> ([f32; 3], f32)) -> (Vec<[f32; 3]>, Vec<f32>) {
    (0..n * n)
        .map(|i| {
            let (s, a) = f(i % n, i / n);
            (on_white(s, a), a)
        })
        .unzip()
}

fn evidence(sigma_noise: f64, lambda: f64, same_ink_de00: f32) -> PaletteEvidence {
    PaletteEvidence {
        sigma_noise,
        lambda,
        noise_sigmas: 0.0,
        same_ink_de00,
    }
}

/// A red square on the clear ground with a one-pixel anti-aliased rim at half coverage.
fn red_square(x: usize, y: usize) -> ([f32; 3], f32) {
    let ring = |lo: usize, hi: usize| (lo..hi).contains(&x) && (lo..hi).contains(&y);
    if ring(3, 9) {
        (RED, 1.0)
    } else if ring(2, 10) {
        (RED, 0.5)
    } else {
        ([0.0; 3], 0.0)
    }
}

fn find(pal: &Palette, alpha: f32) -> usize {
    pal.alpha
        .iter()
        .position(|&a| (a - alpha).abs() < 0.01)
        .unwrap_or_else(|| panic!("no ink at {alpha}: {:?}", pal.alpha))
}

#[test]
fn a_painted_square_is_two_inks_and_its_rim_is_neither() {
    let (rgb, alpha) = image(12, red_square);
    let pal = extract_palette(&rgb, &alpha, 12, 12, 0.035, 64, evidence(0.0, 1.0, 1.5));
    assert_eq!(pal.len(), 2, "{:?} {:?}", pal.rgb, pal.alpha);
    let (paint, ground) = (find(&pal, 1.0), find(&pal, 0.0));
    assert_eq!(pal.alpha[paint], 1.0);
    assert_eq!(pal.alpha[ground], 0.0);
    assert!(close(pal.rgb[paint], RED, 0.005), "{:?}", pal.rgb[paint]);
    assert!(close(pal.rgb[ground], [1.0; 3], 0.005));
    assert!((pal.weight[paint] - 36.0 / 144.0).abs() < 1e-6);
    assert!((pal.weight[ground] - 80.0 / 144.0).abs() < 1e-6);
    // The colour cap does not count the clear ground.
    let capped = extract_palette(&rgb, &alpha, 12, 12, 0.035, 1, evidence(0.0, 1.0, 1.5));
    assert_eq!(capped.alpha.len(), 2, "{:?}", capped.alpha);
}

#[test]
fn a_translucent_wash_is_one_ink_with_its_own_opacity() {
    let (rgb, alpha) = image(12, |x, y| {
        let inside = (3..9).contains(&x) && (3..9).contains(&y);
        if inside {
            (BLUE, 0.5)
        } else {
            ([0.0; 3], 0.0)
        }
    });
    let pal = extract_palette(&rgb, &alpha, 12, 12, 0.035, 64, evidence(0.0, 1.0, 1.5));
    assert_eq!(pal.len(), 2, "{:?}", pal.alpha);
    let wash = find(&pal, 0.5);
    assert!((pal.alpha[wash] - 0.5).abs() < 0.01, "{}", pal.alpha[wash]);
    assert!(
        close(pal.rgb[wash], on_white(BLUE, 0.5), 0.005),
        "{:?}",
        pal.rgb[wash]
    );
    find(&pal, 0.0);
}

#[test]
fn a_hairline_needs_to_be_opaque_to_be_an_ink_and_a_clear_gap_is_always_one() {
    // One pixel wide: opaque paint is an ink, a translucent wash of it is not (it has no
    // interior), and a transparent gap through paint is the clear ground.
    let line = |a: f32| {
        image(
            12,
            move |x, _| if x == 5 { (GREEN, a) } else { ([0.0; 3], 0.0) },
        )
    };
    let (rgb, alpha) = line(1.0);
    let pal = extract_palette(&rgb, &alpha, 12, 12, 0.035, 64, evidence(0.0, 1.0, 1.5));
    assert_eq!(pal.alpha, vec![0.0, 1.0]);
    let (rgb, alpha) = line(0.5);
    let pal = extract_palette(&rgb, &alpha, 12, 12, 0.035, 64, evidence(0.0, 1.0, 1.5));
    assert_eq!(pal.alpha, vec![0.0]);
    let (rgb, alpha) = image(12, |x, _| if x == 5 { ([0.0; 3], 0.0) } else { (RED, 1.0) });
    let pal = extract_palette(&rgb, &alpha, 12, 12, 0.035, 64, evidence(0.0, 1.0, 1.5));
    assert_eq!(pal.alpha, vec![1.0, 0.0]);
}

/// Two opaque halves `d` apart in OKLab lightness.
fn halves(d: f32) -> (Vec<[f32; 3]>, Vec<f32>) {
    // 0.58 is bin 13 of lightness, 0.60 and 0.61 bin 14: the halves are two modes.
    let base = Oklab {
        l: 0.58,
        a: 0.05,
        b: -0.05,
    };
    let other = Oklab {
        l: base.l + d,
        ..base
    };
    image(8, move |x, _| {
        (oklab_to_rgb(if x < 4 { base } else { other }), 1.0)
    })
}

#[test]
fn close_inks_split_only_when_the_noise_says_they_are_two() {
    let (rgb, alpha) = halves(0.03);
    let n = |ev| extract_palette(&rgb, &alpha, 8, 8, 0.05, 64, ev).len();
    // Inside the merge radius: 0.5·32·(0.03/σ)² nats against 2·3 for the extra ink.
    assert_eq!(n(evidence(0.002, 2.0, 0.0)), 2);
    assert_eq!(n(evidence(0.1, 2.0, 0.0)), 1);
    assert_eq!(n(evidence(0.0, 2.0, 0.0)), 1, "no noise measured, no split");
    // Outside the merge radius the perceptual floor decides.
    let (rgb, alpha) = halves(0.02);
    let de = de00(rgb[0], rgb[7]);
    let n = |floor| extract_palette(&rgb, &alpha, 8, 8, 0.005, 64, evidence(0.0, 2.0, floor)).len();
    assert_eq!(n(de * 0.8), 2);
    assert_eq!(n(de * 1.2), 1);
}

#[test]
fn an_ink_no_pixel_is_close_to_keeps_its_seed_and_no_weight() {
    // Two pixels in one bin, further apart than the merge distance: one mode, their mean,
    // and neither pixel near enough to it to count.
    let (p, q) = ([0.60f32, 0.40, 0.30], [0.61f32, 0.40, 0.30]);
    let (ip, iq) = (ink(p, 1.0), ink(q, 1.0));
    assert_eq!(bin(ip.w), bin(iq.w));
    let pal = extract_palette(
        &[p, q],
        &[1.0, 1.0],
        2,
        1,
        1e-4,
        64,
        evidence(0.0, 1.0, 1.5),
    );
    assert_eq!(pal.len(), 1);
    assert_eq!(pal.weight, vec![0.0]);
    let mean = Oklab {
        l: (ip.w.l + iq.w.l) / 2.0,
        a: (ip.w.a + iq.w.a) / 2.0,
        b: (ip.w.b + iq.w.b) / 2.0,
    };
    assert!(
        pal.colors[0].dist(mean) < 1e-6,
        "{:?} vs {mean:?}",
        pal.colors[0]
    );
}

fn rgba(n: usize, f: impl Fn(usize, usize) -> ([f32; 3], f32)) -> (Rgba, Vec<f32>) {
    let mut data = Vec::with_capacity(n * n * 4);
    let mut alpha = Vec::with_capacity(n * n);
    for i in 0..n * n {
        let (s, a) = f(i % n, i / n);
        data.extend([s[0], s[1], s[2], a]);
        alpha.push(a);
    }
    let img = Rgba {
        width: n,
        height: n,
        data,
    };
    (img, alpha)
}

#[test]
fn tracing_the_square_keeps_two_inks_and_a_fade_slot_per_face() {
    let (img, alpha) = rgba(12, red_square);
    let tr = trace_color(&img, &ColorOptions::default(), &alpha);
    assert_eq!(tr.palette.len(), 2, "{:?}", tr.palette.alpha);
    let paint = find(&tr.palette, 1.0);
    assert!(close(tr.palette.rgb[paint], RED, 0.005));
    find(&tr.palette, 0.0);
    assert!(!tr.face_color.is_empty());
    assert_eq!(tr.face_fade.len(), tr.face_color.len());
    assert!(tr.face_fade.iter().all(Option::is_none));
}

#[test]
fn tracing_a_glow_hands_its_fade_to_the_face() {
    let (img, alpha) = rgba(16, |x, _| ([0.8, 0.3, 0.1], 0.1 + 0.05 * x as f32));
    let tr = trace_color(&img, &ColorOptions::default(), &alpha);
    assert_eq!(tr.face_fade.len(), tr.face_color.len());
    let fade = tr.face_fade.iter().flatten().next().expect("a fade");
    for x in 1..15 {
        let a = fade.alpha.color_at(x as f64, 7.5)[0];
        assert!((a - alpha[x]).abs() < 0.02, "x={x}: {a} vs {}", alpha[x]);
    }
}
