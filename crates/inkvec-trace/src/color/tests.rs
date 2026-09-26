//! Reference-value tests for the colour science and the palette's decisions.
//!
//! Every expected number here comes from outside this module: Sharma, Wu and Dalal's
//! published CIEDE2000 table, Ottosson's OKLab values for the sRGB primaries, CIELAB of the
//! primaries (cross-checked with `skimage.color`), or a fixture whose answer follows from
//! how it was built.

use super::*;

/// Sharma, Wu and Dalal (2005), "The CIEDE2000 color-difference formula: implementation
/// notes, supplementary test data, and mathematical observations", Table 1: all 34 pairs,
/// `(L1, a1, b1, L2, a2, b2, dE00)`. The table was cross-checked against
/// `skimage.color.deltaE_ciede2000` (worst disagreement 5e-5, i.e. the table's rounding).
const SHARMA: [[f64; 7]; 34] = [
    [50.0000, 2.6772, -79.7751, 50.0000, 0.0000, -82.7485, 2.0425],
    [50.0000, 3.1571, -77.2803, 50.0000, 0.0000, -82.7485, 2.8615],
    [50.0000, 2.8361, -74.0200, 50.0000, 0.0000, -82.7485, 3.4412],
    [
        50.0000, -1.3802, -84.2814, 50.0000, 0.0000, -82.7485, 1.0000,
    ],
    [
        50.0000, -1.1848, -84.8006, 50.0000, 0.0000, -82.7485, 1.0000,
    ],
    [
        50.0000, -0.9009, -85.5211, 50.0000, 0.0000, -82.7485, 1.0000,
    ],
    [50.0000, 0.0000, 0.0000, 50.0000, -1.0000, 2.0000, 2.3669],
    [50.0000, -1.0000, 2.0000, 50.0000, 0.0000, 0.0000, 2.3669],
    [50.0000, 2.4900, -0.0010, 50.0000, -2.4900, 0.0009, 7.1792],
    [50.0000, 2.4900, -0.0010, 50.0000, -2.4900, 0.0010, 7.1792],
    [50.0000, 2.4900, -0.0010, 50.0000, -2.4900, 0.0011, 7.2195],
    [50.0000, 2.4900, -0.0010, 50.0000, -2.4900, 0.0012, 7.2195],
    [50.0000, -0.0010, 2.4900, 50.0000, 0.0009, -2.4900, 4.8045],
    [50.0000, -0.0010, 2.4900, 50.0000, 0.0010, -2.4900, 4.8045],
    [50.0000, -0.0010, 2.4900, 50.0000, 0.0011, -2.4900, 4.7461],
    [50.0000, 2.5000, 0.0000, 50.0000, 0.0000, -2.5000, 4.3065],
    [50.0000, 2.5000, 0.0000, 73.0000, 25.0000, -18.0000, 27.1492],
    [50.0000, 2.5000, 0.0000, 61.0000, -5.0000, 29.0000, 22.8977],
    [50.0000, 2.5000, 0.0000, 56.0000, -27.0000, -3.0000, 31.9030],
    [50.0000, 2.5000, 0.0000, 58.0000, 24.0000, 15.0000, 19.4535],
    [50.0000, 2.5000, 0.0000, 50.0000, 3.1736, 0.5854, 1.0000],
    [50.0000, 2.5000, 0.0000, 50.0000, 3.2972, 0.0000, 1.0000],
    [50.0000, 2.5000, 0.0000, 50.0000, 1.8634, 0.5757, 1.0000],
    [50.0000, 2.5000, 0.0000, 50.0000, 3.2592, 0.3350, 1.0000],
    [
        60.2574, -34.0099, 36.2677, 60.4626, -34.1751, 39.4387, 1.2644,
    ],
    [
        63.0109, -31.0961, -5.8663, 62.8187, -29.7946, -4.0864, 1.2630,
    ],
    [61.2901, 3.7196, -5.3901, 61.4292, 2.2480, -4.9620, 1.8731],
    [35.0831, -44.1164, 3.7933, 35.0232, -40.0716, 1.5901, 1.8645],
    [
        22.7233, 20.0904, -46.6940, 23.0331, 14.9730, -42.5619, 2.0373,
    ],
    [36.4612, 47.8580, 18.3852, 36.2715, 50.5065, 21.2231, 1.4146],
    [90.8027, -2.0831, 1.4410, 91.1528, -1.6435, 0.0447, 1.4441],
    [90.9257, -0.5406, -0.9208, 88.6381, -0.8985, -0.7239, 1.5381],
    [6.7747, -0.2908, -2.4247, 5.8714, -0.0985, -2.2286, 0.6377],
    [2.0776, 0.0795, -1.1350, 0.9033, -0.0636, -0.5514, 0.9082],
];

#[test]
fn ciede2000_matches_all_34_sharma_pairs() {
    for (k, r) in SHARMA.iter().enumerate() {
        let d = de00_lab([r[0], r[1], r[2]], [r[3], r[4], r[5]]);
        // The table is printed to four decimals.
        assert!(
            (d - r[6]).abs() < 6e-5,
            "pair {}: dE00 {d:.6}, Sharma et al. {:.4}",
            k + 1,
            r[6]
        );
        // The formula is symmetric in its two arguments.
        let back = de00_lab([r[3], r[4], r[5]], [r[0], r[1], r[2]]);
        assert!((back - d).abs() < 1e-9, "pair {} not symmetric", k + 1);
    }
}

#[test]
fn ciede2000_of_srgb_colours_matches_an_independent_implementation() {
    // skimage.color.deltaE_ciede2000(rgb2lab(a), rgb2lab(b)). skimage's sRGB matrix differs
    // from ours in the fourth decimal, hence the tolerance.
    let cases: [([f32; 3], [f32; 3], f32); 5] = [
        ([1.0, 0.0, 0.0], [0.0, 0.0, 1.0], 52.8814),
        ([0.0, 0.0, 0.0], [1.0, 1.0, 1.0], 100.0),
        ([0.2, 0.4, 0.6], [0.6, 0.4, 0.2], 40.1182),
        ([1.0, 1.0, 0.0], [0.0, 1.0, 1.0], 41.9714),
        ([0.9, 0.1, 0.5], [0.85, 0.15, 0.5], 2.3185),
    ];
    for (a, b, want) in cases {
        let d = de00(a, b);
        assert!(
            (d - want).abs() < 0.02,
            "de00({a:?}, {b:?}) = {d}, want {want}"
        );
    }
}

#[test]
fn srgb_to_cielab_matches_reference_values() {
    // CIELAB (D65, 2 degree) of sRGB colours, as given by skimage.color.rgb2lab and
    // Bruce Lindbloom's calculator.
    let cases: [([f32; 3], [f32; 3]); 6] = [
        ([1.0, 0.0, 0.0], [53.2406, 80.0923, 67.2028]),
        ([0.0, 1.0, 0.0], [87.7351, -86.1830, 83.1797]),
        ([0.0, 0.0, 1.0], [32.2957, 79.1856, -107.8573]),
        ([1.0, 1.0, 1.0], [100.0, 0.0, 0.0]),
        ([0.5, 0.5, 0.5], [53.3890, 0.0, 0.0]),
        ([0.2, 0.4, 0.6], [42.0080, -0.1540, -32.8429]),
    ];
    for (rgb, want) in cases {
        let lab = srgb_to_lab(rgb);
        for k in 0..3 {
            assert!(
                (lab[k] - want[k]).abs() < 0.03,
                "Lab of {rgb:?} = {lab:?}, want {want:?}"
            );
        }
    }
}

/// Ottosson's OKLab coordinates of the sRGB primaries and white (also the CSS Color 4
/// sample values).
const OKLAB_REF: [([f32; 3], [f32; 3]); 4] = [
    ([1.0, 0.0, 0.0], [0.627_955, 0.224_863, 0.125_846]),
    ([0.0, 1.0, 0.0], [0.866_440, -0.233_888, 0.179_498]),
    ([0.0, 0.0, 1.0], [0.452_014, -0.032_457, -0.311_528]),
    ([1.0, 1.0, 1.0], [1.0, 0.0, 0.0]),
];

#[test]
fn oklab_matches_ottossons_reference_values_both_ways() {
    for (rgb, lab) in OKLAB_REF {
        let got = rgb_to_oklab(rgb);
        let tol = 1e-3;
        assert!(
            (got.l - lab[0]).abs() < tol
                && (got.a - lab[1]).abs() < tol
                && (got.b - lab[2]).abs() < tol,
            "OKLab of {rgb:?} = {got:?}, want {lab:?}"
        );
        let back = oklab_to_rgb(Oklab {
            l: lab[0],
            a: lab[1],
            b: lab[2],
        });
        for k in 0..3 {
            assert!(
                (back[k] - rgb[k]).abs() < 2e-3,
                "sRGB of {lab:?} = {back:?}, want {rgb:?}"
            );
        }
    }
}

#[test]
fn oklab_round_trips_across_the_cube() {
    let mut worst = 0.0f32;
    for r in 0..=8 {
        for g in 0..=8 {
            for b in 0..=8 {
                let c = [r as f32 / 8.0, g as f32 / 8.0, b as f32 / 8.0];
                let back = oklab_to_rgb(rgb_to_oklab(c));
                for k in 0..3 {
                    worst = worst.max((back[k] - c[k]).abs());
                }
            }
        }
    }
    assert!(worst < 1e-4, "round trip error {worst}");
}

#[test]
fn transfer_functions_are_the_srgb_standard() {
    // IEC 61966-2-1: the linear segment below 0.04045, the 2.4 power above.
    assert!((srgb_to_linear(0.04) - 0.04 / 12.92).abs() < 1e-7);
    assert!((srgb_to_linear(0.5) - 0.214_041_14).abs() < 1e-6);
    assert!((srgb_to_linear(1.0) - 1.0).abs() < 1e-6);
    assert!((linear_to_srgb(0.214_041_14) - 0.5).abs() < 1e-6);
    assert!((linear_to_srgb(0.002) - 0.002 * 12.92).abs() < 1e-7);
    assert_eq!(to_hex([1.0, 0.5, 0.0]), "#ff8000");
    assert_eq!(to_hex([-0.2, 1.3, 0.2]), "#00ff33");
}

// ------------------------------------------------------------------ blends

fn lab(c: [f32; 3]) -> Oklab {
    rgb_to_oklab(c)
}

/// The colour a fraction `t` of the way from `a` to `b`, mixed in linear light.
fn mix_linear(a: [f32; 3], b: [f32; 3], t: f32) -> [f32; 3] {
    let mut out = [0.0; 3];
    for k in 0..3 {
        let (x, y) = (srgb_to_linear(a[k]), srgb_to_linear(b[k]));
        out[k] = linear_to_srgb(x + (y - x) * t);
    }
    out
}

const RED: [f32; 3] = [0.9, 0.1, 0.1];
const BLUE: [f32; 3] = [0.1, 0.2, 0.9];
const GREEN: [f32; 3] = [0.1, 0.8, 0.2];

#[test]
fn a_linear_light_mixture_is_found_as_a_blend_of_its_two_inks() {
    let accepted = [lab(RED), lab(GREEN), lab(BLUE)];
    let c = lab(mix_linear(RED, BLUE, 0.3));
    let pairs = blend_pairs(c, &accepted, 0.05, 0.04);
    let lin: Vec<_> = pairs.iter().filter(|p| p.2).collect();
    assert_eq!(lin.len(), 1, "linear-light pairs {pairs:?}");
    let &&(i, j, _, off) = lin.first().unwrap();
    assert_eq!((i, j), (0, 2), "the mixture is red + blue");
    assert!(off < 2e-3, "a mixture lies on the chord, off {off}");
    // Every reported pair is within tolerance, and no pair ever involves green.
    assert!(pairs.iter().all(|p| p.3 <= 0.05 && p.0 != 1 && p.1 != 1));
}

#[test]
fn an_srgb_mixture_is_found_in_the_srgb_space() {
    let accepted = [lab(RED), lab(BLUE)];
    let mid = [
        0.5 * (RED[0] + BLUE[0]),
        0.5 * (RED[1] + BLUE[1]),
        0.5 * (RED[2] + BLUE[2]),
    ];
    let pairs = blend_pairs(lab(mid), &accepted, 0.01, 0.04);
    let srgb = pairs.iter().find(|p| !p.2).expect("found in sRGB");
    assert!(
        srgb.3 < 2e-3,
        "sRGB midpoint off the sRGB chord by {}",
        srgb.3
    );
    // The sRGB midpoint is not a linear-light mixture of the two: it is darker than the
    // linear chord by far more than 0.01.
    assert!(pairs.iter().all(|p| !p.2), "{pairs:?}");
}

#[test]
fn blend_pairs_rejects_non_mixtures_and_chord_ends() {
    let accepted = [lab(RED), lab(BLUE)];
    assert!(blend_pairs(lab(GREEN), &accepted, 0.05, 0.04).is_empty());
    // Too near one end: 1 % of the way along (in sRGB, which is under 4 % in linear light
    // too) is the ink itself, not a blend.
    let near_end = |t: f32| {
        lab([
            RED[0] + (BLUE[0] - RED[0]) * t,
            RED[1] + (BLUE[1] - RED[1]) * t,
            RED[2] + (BLUE[2] - RED[2]) * t,
        ])
    };
    assert!(blend_pairs(near_end(0.01), &accepted, 0.05, 0.04).is_empty());
    assert!(blend_pairs(near_end(0.99), &accepted, 0.05, 0.04).is_empty());
    // ... while 10 % of the way is.
    assert!(!blend_pairs(near_end(0.10), &accepted, 0.05, 0.04).is_empty());
    // Fewer than two inks: nothing to blend.
    assert!(blend_pairs(lab(RED), &accepted[..1], 0.05, 0.04).is_empty());
    // Two inks a hair apart have no chord to lie on, even for their exact midpoint.
    let g = [0.5f32, 0.5, 0.5];
    let g2 = [0.500_01f32, 0.500_01, 0.500_01];
    let tight = [lab(g), lab(g2)];
    assert!(blend_pairs(lab(mix_linear(g, g2, 0.5)), &tight, 0.05, 0.04).is_empty());
}

#[test]
fn blend_tmin_defaults_to_four_percent() {
    assert_eq!(BLEND_TMIN, 0.04);
}

/// A `w x h` image from a column painter, with the caches `extract_palette_mdl` builds.
struct Fixture {
    w: usize,
    h: usize,
    lab: Vec<Oklab>,
    srgb: Vec<[f32; 3]>,
    lin: Vec<[f32; 3]>,
}

fn fixture(w: usize, h: usize, col: impl Fn(usize) -> [f32; 3]) -> Fixture {
    let lab: Vec<Oklab> = (0..w * h).map(|i| rgb_to_oklab(col(i % w))).collect();
    let srgb: Vec<[f32; 3]> = lab.iter().map(|&p| oklab_to_rgb(p)).collect();
    let lin = srgb
        .iter()
        .map(|r| {
            [
                srgb_to_linear(r[0]),
                srgb_to_linear(r[1]),
                srgb_to_linear(r[2]),
            ]
        })
        .collect();
    Fixture {
        w,
        h,
        lab,
        srgb,
        lin,
    }
}

impl Fixture {
    fn nearest(&self, inks: &[Oklab]) -> Vec<f32> {
        self.lab
            .iter()
            .map(|p| {
                inks.iter()
                    .map(|&q| p.dist(q))
                    .fold(f32::INFINITY, f32::min)
            })
            .collect()
    }
    fn straddle(&self, c: Oklab, a: Oklab, b: Oklab, nearest: &[f32]) -> f32 {
        straddle_fraction(
            &self.lab, &self.srgb, &self.lin, self.w, self.h, c, nearest, a, b, true, 1,
        )
    }
}

#[test]
fn a_one_pixel_blend_band_straddles_and_a_two_pixel_band_does_not() {
    let c = mix_linear(RED, BLUE, 0.5);
    let (a, b) = (lab(RED), lab(BLUE));
    // Anti-aliasing: red | blend | blue. Every blend pixel has red on one side and blue on
    // the other.
    let aa = fixture(7, 3, |x| match x {
        0..=2 => RED,
        3 => c,
        _ => BLUE,
    });
    let near = aa.nearest(&[a, b]);
    assert_eq!(aa.straddle(lab(c), a, b, &near), 1.0);
    // A band two pixels wide: each of its pixels touches one ink only.
    let band = fixture(8, 3, |x| match x {
        0..=2 => RED,
        3 | 4 => c,
        _ => BLUE,
    });
    let near = band.nearest(&[a, b]);
    assert_eq!(band.straddle(lab(c), a, b, &near), 0.0);
    // Half of each: a one-pixel band on the top rows, a two-pixel one below.
    let mixed_lab: Vec<Oklab> = (0..8 * 4)
        .map(|i| {
            let (x, y) = (i % 8, i / 8);
            let one = y < 2;
            lab(match x {
                0..=2 => RED,
                3 => c,
                4 if !one => c,
                _ => BLUE,
            })
        })
        .collect();
    let srgb: Vec<[f32; 3]> = mixed_lab.iter().map(|&p| oklab_to_rgb(p)).collect();
    let lin: Vec<[f32; 3]> = srgb
        .iter()
        .map(|r| {
            [
                srgb_to_linear(r[0]),
                srgb_to_linear(r[1]),
                srgb_to_linear(r[2]),
            ]
        })
        .collect();
    let near: Vec<f32> = mixed_lab.iter().map(|p| p.dist(a).min(p.dist(b))).collect();
    let f = straddle_fraction(&mixed_lab, &srgb, &lin, 8, 4, lab(c), &near, a, b, true, 1);
    // The oracle does not project anything onto an axis: a blend pixel straddles when its
    // 3x3 neighbourhood holds a pixel of ink A and a pixel of ink B.
    let mut want = 0;
    let mut total = 0;
    for y in 0..4usize {
        for x in 0..8usize {
            let i = y * 8 + x;
            if mixed_lab[i].dist(lab(c)) >= near[i] {
                continue;
            }
            total += 1;
            let (mut lo, mut hi) = (false, false);
            for (dx, dy) in (-1i32..=1).flat_map(|dx| (-1i32..=1).map(move |dy| (dx, dy))) {
                let (nx, ny) = (x as i32 + dx, y as i32 + dy);
                if nx < 0 || ny < 0 || nx >= 8 || ny >= 4 {
                    continue;
                }
                let q = mixed_lab[ny as usize * 8 + nx as usize];
                lo |= q == a;
                hi |= q == b;
            }
            want += (lo && hi) as i32;
        }
    }
    assert_eq!(total, 6);
    assert!(
        (f - want as f32 / total as f32).abs() < 1e-6,
        "{f} vs {want}/{total}"
    );
}

#[test]
fn a_blend_near_one_end_is_judged_by_the_room_left_on_that_side() {
    // An anti-aliased pixel 90 % of the way from red to blue: blue sits only 0.1 further
    // along the axis, less than the full 0.12 step, and still counts as the far side.
    let (a, b) = (lab(RED), lab(BLUE));
    for t in [0.9f32, 0.1] {
        let c = mix_linear(RED, BLUE, t);
        let fx = fixture(7, 3, |x| match x {
            0..=2 => RED,
            3 => c,
            _ => BLUE,
        });
        let near = fx.nearest(&[a, b]);
        assert_eq!(fx.straddle(lab(c), a, b, &near), 1.0, "t = {t}");
    }
}

#[test]
fn straddle_is_zero_for_a_degenerate_axis_even_with_no_territory() {
    // No geometry: nothing is known to straddle.
    let fx0 = fixture(3, 3, |_| RED);
    let none0 = vec![f32::INFINITY; 9];
    assert_eq!(
        straddle_fraction(
            &fx0.lab,
            &fx0.srgb,
            &fx0.lin,
            0,
            3,
            lab(GREEN),
            &none0,
            lab(RED),
            lab(BLUE),
            true,
            1
        ),
        0.0
    );
    let fx = fixture(3, 3, |_| RED);
    let none = vec![0.0f32; 9];
    let a = lab(RED);
    assert_eq!(fx.straddle(a, a, a, &none), 0.0);
    // A real axis but no pixel the candidate would claim: nothing to protect.
    assert_eq!(fx.straddle(lab(GREEN), a, lab(BLUE), &none), 1.0);
}

#[test]
fn interior_fraction_counts_pixels_whose_four_neighbours_are_claimed() {
    // A 3x3 block of `c` in a 5x5 field of red: only its centre is interior.
    let c = [0.2f32, 0.6, 0.3];
    let mut labv = vec![lab(RED); 25];
    for y in 1..4 {
        for x in 1..4 {
            labv[y * 5 + x] = lab(c);
        }
    }
    let near: Vec<f32> = labv.iter().map(|p| p.dist(lab(RED))).collect();
    let f = interior_fraction(&labv, 5, 5, lab(c), &near, 1);
    assert!((f - 1.0 / 9.0).abs() < 1e-6, "{f}");
    // The top two rows: off-image counts as claimed, so row 0 is interior (its only
    // missing neighbours are off-image) and row 1 is not (red below).
    let mut top = vec![lab(RED); 25];
    for p in top.iter_mut().take(10) {
        *p = lab(c);
    }
    let near: Vec<f32> = top.iter().map(|p| p.dist(lab(RED))).collect();
    let f = interior_fraction(&top, 5, 5, lab(c), &near, 1);
    assert!((f - 0.5).abs() < 1e-6, "{f}");
    // The right two columns: off-image counts as claimed on the right, so x = 4 is
    // interior on every row and x = 3 (red to its left) on none.
    let mut right = vec![lab(RED); 25];
    for y in 0..5 {
        right[y * 5 + 3] = lab(c);
        right[y * 5 + 4] = lab(c);
    }
    let near_r: Vec<f32> = right.iter().map(|p| p.dist(lab(RED))).collect();
    let f = interior_fraction(&right, 5, 5, lab(c), &near_r, 1);
    assert!((f - 0.5).abs() < 1e-6, "{f}");
    // Nothing claimed: zero. No geometry: solid.
    let f0 = interior_fraction(&top, 5, 5, lab(GREEN), &vec![0.0; 25], 1);
    assert_eq!(f0, 0.0);
    assert_eq!(interior_fraction(&top, 0, 0, lab(c), &near, 1), 1.0);
}

#[test]
fn claim_spread_counts_territory_and_takes_the_median_member_distance() {
    // Ten pixels at OKLab distances 0, 0.001, ..., 0.009 from the candidate along L, and
    // nothing accepted yet (everything is territory).
    let c = Oklab {
        l: 0.5,
        a: 0.0,
        b: 0.0,
    };
    let px: Vec<Oklab> = (0..10)
        .map(|k| Oklab {
            l: 0.5 + 0.001 * k as f32,
            a: 0.0,
            b: 0.0,
        })
        .collect();
    let inf = vec![f32::INFINITY; 10];
    let (n, spread) = claim_spread(&px, &inf, c, 1.0, 1);
    assert_eq!(n, 10);
    // Median of ten sorted distances is element 5: 0.005.
    assert!((spread - 0.005).abs() < 1e-6, "{spread}");
    // Members are those within `tol`: 0..=0.0035 -> four of them, median element 2.
    let (_, spread) = claim_spread(&px, &inf, c, 0.0035, 1);
    assert!((spread - 0.002).abs() < 1e-6, "{spread}");
    // A strided visit sees every other pixel and scales the count back up.
    let (n2, _) = claim_spread(&px, &inf, c, 1.0, 2);
    assert_eq!(n2, 10);
    // Pixels already closer to an accepted ink are not territory.
    let mut taken = inf.clone();
    for d in taken.iter_mut().skip(6) {
        *d = 0.0;
    }
    assert_eq!(claim_spread(&px, &taken, c, 1.0, 1).0, 6);
    // No members within tol: zero spread.
    assert_eq!(claim_spread(&px, &inf, c, 0.0, 1).1, 0.0);
}

#[test]
fn stat_stride_is_coprime_with_the_row_width() {
    assert_eq!(stat_stride(128 * 128, 128), 1);
    assert_eq!(stat_stride(STAT_PIXELS, 256), 1);
    assert_eq!(stat_stride(1_000_000, 0), 1);
    // 512 x 512 needs a stride of 4, which shares a factor with 512: the next coprime is 5.
    assert_eq!(stat_stride(512 * 512, 512), 5);
    // 300 x 300 needs 2; 2..6 all share a factor with 300, 7 does not.
    assert_eq!(stat_stride(300 * 300, 300), 7);
    assert_eq!(gcd(12, 18), 6);
    assert_eq!(gcd(7, 300), 1);
}

#[test]
fn palette_basics() {
    let empty = Palette {
        colors: vec![],
        rgb: vec![],
        weight: vec![],
        alpha: vec![],
    };
    assert!(empty.is_empty());
    assert_eq!(empty.len(), 0);
    let pal = Palette {
        colors: vec![lab(RED), lab(GREEN), lab(BLUE)],
        rgb: vec![RED, GREEN, BLUE],
        weight: vec![0.3; 3],
        alpha: vec![1.0; 3],
    };
    assert!(!pal.is_empty());
    let (i, d) = pal.nearest(lab([0.15, 0.25, 0.85]));
    assert_eq!(i, 2);
    assert!((d - lab([0.15, 0.25, 0.85]).dist(lab(BLUE))).abs() < 1e-7);
    assert_eq!(
        label_image(&[RED, BLUE, GREEN, [0.8, 0.2, 0.2]], &pal),
        vec![0, 2, 1, 0]
    );
}

// ------------------------------------------------------------------ extraction

/// Vertical stripes of the given inks, `stripe` pixels each, `h` rows.
fn stripes(inks: &[[f32; 3]], stripe: usize, h: usize) -> (Vec<[f32; 3]>, usize) {
    let w = inks.len() * stripe;
    let rgb = (0..w * h).map(|i| inks[(i % w) / stripe]).collect();
    (rgb, w)
}

fn assert_palette_is(pal: &Palette, inks: &[[f32; 3]]) {
    assert_eq!(pal.len(), inks.len(), "palette {:?}", pal.rgb);
    for ink in inks {
        let (i, d) = pal.nearest(lab(*ink));
        assert!(d < 1e-4, "{ink:?} recovered as {:?}", pal.rgb[i]);
    }
}

#[test]
fn flat_inks_are_recovered_exactly_with_their_areas() {
    let inks = [
        [0.95, 0.95, 0.95],
        [0.05, 0.05, 0.08],
        [0.85, 0.15, 0.15],
        [0.15, 0.65, 0.25],
        [0.20, 0.30, 0.85],
        [0.95, 0.80, 0.10],
        [0.60, 0.20, 0.70],
        [0.10, 0.70, 0.75],
    ];
    let (rgb, w) = stripes(&inks, 6, 20);
    let pal = extract_palette(&rgb, w, 20, DEFAULT_MERGE_DISTANCE, 32);
    assert_palette_is(&pal, &inks);
    for (i, wgt) in pal.weight.iter().enumerate() {
        assert!((wgt - 1.0 / 8.0).abs() < 1e-6, "ink {i} weight {wgt}");
    }
    assert!(pal.alpha.iter().all(|&a| a == 1.0));
    // `max_colors` caps the count, most frequent first.
    let (rgb2, w2) = stripes(&[inks[0], inks[0], inks[2], inks[3]], 5, 10);
    let two = extract_palette(&rgb2, w2, 10, DEFAULT_MERGE_DISTANCE, 1);
    assert_eq!(two.len(), 1);
    assert!(two.nearest(lab(inks[0])).1 < 1e-4);
}

#[test]
fn an_anti_aliased_seam_is_not_an_ink_but_a_wide_band_of_tint_is() {
    let (a, b) = ([0.9f32, 0.1, 0.1], [0.1f32, 0.2, 0.9]);
    let m = mix_linear(a, b, 0.5);
    // 1-px blend at every seam of alternating 8-px stripes: coverage, not an ink.
    let cols: Vec<[f32; 3]> = (0..8)
        .flat_map(|k| {
            let (p, q) = if k % 2 == 0 { (a, b) } else { (b, a) };
            let mut v = vec![p; 7];
            v.push(mix_linear(p, q, 0.5));
            v
        })
        .collect();
    let w = cols.len();
    let rgb: Vec<[f32; 3]> = (0..w * 24).map(|i| cols[i % w]).collect();
    let pal = extract_palette(&rgb, w, 24, DEFAULT_MERGE_DISTANCE, 32);
    assert_palette_is(&pal, &[a, b]);
    // The same colour painted as a 6-px band is a tint the artist chose.
    let (rgb, w) = stripes(&[a, m, b, a, m, b], 6, 24);
    let pal = extract_palette(&rgb, w, 24, DEFAULT_MERGE_DISTANCE, 32);
    assert_palette_is(&pal, &[a, m, b]);
}

#[test]
fn close_inks_split_only_when_the_noise_says_they_can_be_told_apart() {
    // Two greys 0.025 apart in OKLab lightness: inside the merge radius, above the JND,
    // and in different bins of the mode-seeking grid (a bin is 1/23 of lightness wide,
    // and two inks that share one are a single mode whatever the evidence).
    let g1 = oklab_to_rgb(Oklab {
        l: 0.63,
        a: 0.0,
        b: 0.0,
    });
    let g2 = oklab_to_rgb(Oklab {
        l: 0.655,
        a: 0.0,
        b: 0.0,
    });
    let (rgb, w) = stripes(&[g1, g2], 20, 20);
    let merged = extract_palette(&rgb, w, 20, DEFAULT_MERGE_DISTANCE, 8);
    assert_eq!(merged.len(), 1, "no noise estimate: one ink");
    let ev = PaletteEvidence {
        sigma_noise: 0.5 / 255.0,
        lambda: 0.5 * ((w * 20) as f64).ln(),
        noise_sigmas: 0.0,
        same_ink_de00: 0.0,
    };
    let split = extract_palette_mdl(&rgb, w, 20, DEFAULT_MERGE_DISTANCE, 8, ev);
    assert_palette_is(&split, &[g1, g2]);
    // The perceptual floor overrides the evidence, and only when it is above their
    // CIEDE2000 separation.
    let d = de00(g1, g2);
    assert!(d > 1.5 && d < 3.5, "{d}");
    let floor = |f: f32| PaletteEvidence {
        same_ink_de00: f,
        ..ev
    };
    let n = |f: f32| extract_palette_mdl(&rgb, w, 20, DEFAULT_MERGE_DISTANCE, 8, floor(f)).len();
    assert_eq!(n(d + 0.05), 1);
    assert_eq!(n(d - 0.05), 2);
    // A noise guard wider than their separation merges them too: a spread measured on the
    // candidate's own pixels. Clean stripes have zero spread, so the guard changes nothing.
    let guard = PaletteEvidence {
        noise_sigmas: 3.0,
        ..ev
    };
    assert_eq!(
        extract_palette_mdl(&rgb, w, 20, DEFAULT_MERGE_DISTANCE, 8, guard).len(),
        2
    );
}

#[test]
fn the_mdl_escape_splits_where_half_the_residual_outprices_an_ink() {
    // Folding 400 pixels of g2 into g1 (0.025 apart in OKLab) costs 0.5 * 400 * (d/sigma)^2;
    // a new ink costs lambda * 3 = 10.03 here. At sigma 0.125 the fold costs 8.0 and wins;
    // at sigma 0.08 it costs 19.5 and the ink is kept.
    let g1 = oklab_to_rgb(Oklab {
        l: 0.63,
        a: 0.0,
        b: 0.0,
    });
    let g2 = oklab_to_rgb(Oklab {
        l: 0.655,
        a: 0.0,
        b: 0.0,
    });
    let (rgb, w) = stripes(&[g1, g2], 20, 20);
    let lambda = 0.5 * ((w * 20) as f64).ln();
    let d = rgb_to_oklab(g1).dist(rgb_to_oklab(g2)) as f64;
    let inks = |sigma: f64| {
        let ev = PaletteEvidence {
            sigma_noise: sigma,
            lambda,
            noise_sigmas: 0.0,
            same_ink_de00: 0.0,
        };
        extract_palette_mdl(&rgb, w, 20, DEFAULT_MERGE_DISTANCE, 8, ev).len()
    };
    for (sigma, want) in [(0.125, 1), (0.08, 2)] {
        let fold = 0.5 * 400.0 * (d / sigma).powi(2);
        assert_eq!(
            fold > lambda * PARAMS_PER_INK,
            want == 2,
            "precondition at sigma {sigma}"
        );
        assert_eq!(
            inks(sigma),
            want,
            "sigma {sigma}: fold {fold:.2} vs ink {:.2}",
            lambda * 3.0
        );
    }
}

#[test]
fn a_rare_colour_is_not_an_ink() {
    // One pixel of green in 40x40 red (0.06 % of the image, below MIN_INK_WEIGHT).
    let mut rgb = vec![RED; 1600];
    rgb[820] = GREEN;
    let pal = extract_palette(&rgb, 40, 40, DEFAULT_MERGE_DISTANCE, 8);
    assert_palette_is(&pal, &[RED]);
    // Eight pixels of it (0.5 %) are.
    for p in [820, 821, 822, 823, 860, 861, 862, 863] {
        rgb[p] = GREEN;
    }
    let pal = extract_palette(&rgb, 40, 40, DEFAULT_MERGE_DISTANCE, 8);
    assert_palette_is(&pal, &[RED, GREEN]);
}

// ------------------------------------------------------------------ alpha levels

fn one_ink_palette(c: [f32; 3]) -> Palette {
    Palette {
        colors: vec![lab(c)],
        rgb: vec![c],
        weight: vec![1.0],
        alpha: vec![1.0],
    }
}

#[test]
fn a_panel_drawn_at_a_quarter_opacity_becomes_its_own_ink() {
    let white = [1.0f32; 3];
    let mut pal = one_ink_palette(white);
    // 60 opaque pixels, 40 at 0.25 opacity.
    let alpha: Vec<f32> = (0..100).map(|p| if p < 60 { 1.0 } else { 0.25 }).collect();
    let mut labels = vec![0u16; 100];
    assert_eq!(split_alpha_inks(&mut labels, &mut pal, &alpha), 1);
    assert_eq!(pal.len(), 2);
    assert_eq!(pal.alpha, vec![1.0, 0.25]);
    assert_eq!(pal.rgb[1], white);
    assert_eq!(pal.weight[1], 0.0);
    assert!(labels[..60].iter().all(|&l| l == 0));
    assert!(labels[60..].iter().all(|&l| l == 1));
}

#[test]
fn clear_pixels_are_a_level_and_one_level_just_sets_the_opacity() {
    let mut pal = one_ink_palette([1.0; 3]);
    // Half the ink is the empty ground (alpha ~0.01), half a wash at 0.5.
    let alpha: Vec<f32> = (0..64)
        .map(|p| if p % 2 == 0 { 0.01 } else { 0.5 })
        .collect();
    let mut labels = vec![0u16; 64];
    assert_eq!(split_alpha_inks(&mut labels, &mut pal, &alpha), 1);
    assert_eq!(pal.alpha, vec![0.5, 0.0], "the clear level snaps to 0");
    for (p, &l) in labels.iter().enumerate() {
        assert_eq!(l, if p % 2 == 0 { 1 } else { 0 });
    }
    // A single level: no new ink, the entry takes that opacity.
    let mut pal = one_ink_palette([0.2, 0.3, 0.4]);
    let mut labels = vec![0u16; 32];
    assert_eq!(split_alpha_inks(&mut labels, &mut pal, &[0.7; 32]), 0);
    assert!((pal.alpha[0] - 0.7).abs() < 1e-6);
}

#[test]
fn glows_and_rare_levels_are_left_alone() {
    // A glow: alpha ramps 0..1 in small steps, no tight level.
    let mut pal = one_ink_palette([1.0, 0.8, 0.2]);
    let alpha: Vec<f32> = (0..100).map(|p| p as f32 / 99.0).collect();
    let mut labels = vec![0u16; 100];
    assert_eq!(split_alpha_inks(&mut labels, &mut pal, &alpha), 0);
    assert_eq!(pal.alpha, vec![1.0]);
    // One pixel in a hundred at another level is under MIN_SHARE (2 %); two are not.
    let mut alpha = vec![1.0f32; 100];
    alpha[5] = 0.3;
    let mut labels = vec![0u16; 100];
    assert_eq!(split_alpha_inks(&mut labels, &mut pal, &alpha), 0);
    alpha[6] = 0.3;
    assert_eq!(split_alpha_inks(&mut labels, &mut pal, &alpha), 1);
    // Two levels closer than LEVEL_GAP are one group, and that group is too spread
    // (0.125 > LEVEL_SPREAD) to be a level.
    let mut pal = one_ink_palette([1.0; 3]);
    let alpha: Vec<f32> = (0..40).map(|p| if p < 20 { 0.5 } else { 0.625 }).collect();
    let mut labels = vec![0u16; 40];
    assert_eq!(split_alpha_inks(&mut labels, &mut pal, &alpha), 0);
    // Too few pixels to judge (under 16), and mismatched lengths.
    let mut labels = vec![0u16; 10];
    let mut pal = one_ink_palette([1.0; 3]);
    let alpha: Vec<f32> = (0..10).map(|p| if p < 5 { 1.0 } else { 0.2 }).collect();
    assert_eq!(split_alpha_inks(&mut labels, &mut pal, &alpha), 0);
    assert_eq!(split_alpha_inks(&mut labels, &mut pal, &alpha[..9]), 0);
}
