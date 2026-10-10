use super::*;

const BLUE: [f32; 4] = [0.235, 0.533, 0.761, 1.0];
const WHITE: [f32; 4] = [1.0, 1.0, 1.0, 1.0];
/// The two halo inks Fast founded on `twemoji/1f1fa` (web tier): #3f7eae and #bee1ff.
const DARK_HALO: [f32; 4] = [0.247, 0.494, 0.682, 1.0];
const LIGHT_HALO: [f32; 4] = [0.745, 0.882, 1.0, 1.0];
const BLACK: [f32; 4] = [0.0, 0.0, 0.0, 1.0];

/// Brute force: is any pixel of component `c` deep (its window of radius `reach` holds only
/// its label, the outside counting as its own)?
fn deep_by_pixels(labels: &[u16], w: usize, h: usize, reach: usize) -> Vec<bool> {
    let mut runs = RunLabels::new(labels, w, h);
    runs.components();
    let mut comp = vec![0u32; w * h];
    for y in 0..h {
        for r in runs.row_start[y]..runs.row_start[y + 1] {
            let Run { x0, x1, .. } = runs.runs[r];
            for x in x0..x1 {
                comp[y * w + x as usize] = runs.run_comp[r];
            }
        }
    }
    let mut deep = vec![false; runs.size.len()];
    for y in 0..h {
        for x in 0..w {
            let l = labels[y * w + x];
            let ok = (y.saturating_sub(reach)..(y + reach + 1).min(h)).all(|yy| {
                (x.saturating_sub(reach)..(x + reach + 1).min(w)).all(|xx| labels[yy * w + xx] == l)
            });
            if ok {
                deep[comp[y * w + x] as usize] = true;
            }
        }
    }
    deep
}

/// A small deterministic generator (xorshift), so the random images are the same every run.
fn xorshift(s: &mut u64) -> u64 {
    *s ^= *s << 13;
    *s ^= *s >> 7;
    *s ^= *s << 17;
    *s
}

#[test]
fn deep_components_match_the_pixel_windows() {
    let mut seed = 0x9e37_79b9_7f4a_7c15u64;
    for case in 0..200 {
        let (w, h) = (1 + case % 17, 1 + (case * 7) % 13);
        let k = 2 + (case % 3) as u64;
        // Blocky random labels, so some components are deep and some are not.
        let labels: Vec<u16> = (0..w * h)
            .map(|p| {
                let (x, y) = (p % w, p / w);
                let v = xorshift(&mut seed);
                if v.is_multiple_of(4) {
                    (v % k) as u16
                } else {
                    (((x / 3) + (y / 4) * 2) as u64 % k) as u16
                }
            })
            .collect();
        for reach in 1..=3u32 {
            let mut runs = RunLabels::new(&labels, w, h);
            runs.components();
            let got = runs.deep_components(reach);
            assert_eq!(
                got,
                deep_by_pixels(&labels, w, h, reach as usize),
                "case {case}, {w} x {h}, reach {reach}"
            );
        }
    }
}

#[test]
fn ycc_is_jpegs_full_range_transform() {
    let w = ycc(WHITE);
    assert!((w[0] - 1.0).abs() < 1e-5 && w[1].abs() < 1e-5 && w[2].abs() < 1e-5);
    assert_eq!(ycc(BLACK), [0.0, 0.0, 0.0]);
    let red = ycc([1.0, 0.0, 0.0, 1.0]);
    assert!((red[0] - 0.299).abs() < 1e-6 && (red[2] - 0.5).abs() < 1e-6);
}

#[test]
fn a_halo_lies_in_the_hull_and_off_the_line() {
    let (b, wh) = (ycc(BLUE), ycc(WHITE));
    for halo in [DARK_HALO, LIGHT_HALO] {
        assert!(in_hull(ycc(halo), b, wh));
        // ... which the sRGB blend test does not see.
        let (_, d) = super::super::blend_of(halo, BLUE, WHITE).unwrap();
        assert!(d > super::super::BLEND_TOL * super::super::BLEND_TOL);
    }
    // A black outline between them is no halo; nor is red between black and white.
    assert!(!in_hull(ycc(BLACK), b, wh));
    assert!(!in_hull(ycc([0.85, 0.1, 0.1, 1.0]), ycc(BLACK), wh));
    assert_eq!(halo_pair(ycc(DARK_HALO), &[0, 1], &[b, wh]), Some((0, 1)));
    assert_eq!(halo_pair(ycc(BLACK), &[0, 1], &[b, wh]), None);
    assert_eq!(coverage(b, b, b), None);
}

/// Columns: blue, a dark halo, a light halo, white -- the band a JPEG leaves -- and,
/// lower down, a black hairline between blue and white. `widths` are the column counts.
fn band_image(h: usize) -> (Vec<u16>, Vec<[f32; 3]>, usize) {
    let cols: [(u16, usize); 4] = [(0, 8), (2, 2), (3, 2), (1, 8)];
    let w: usize = cols.iter().map(|c| c.1).sum();
    let inks = [BLUE, WHITE, DARK_HALO, LIGHT_HALO, BLACK];
    let mut labels = Vec::with_capacity(w * h);
    for y in 0..h {
        for &(l, n) in &cols {
            let l = if y >= h / 2 && (l == 2 || l == 3) {
                4
            } else {
                l
            };
            labels.extend(std::iter::repeat_n(l, n));
        }
    }
    let rgb = labels
        .iter()
        .map(|&l| {
            let c = inks[l as usize];
            [c[0], c[1], c[2]]
        })
        .collect();
    (labels, rgb, w)
}

#[test]
fn a_halo_band_goes_back_to_its_two_inks_and_a_hairline_stays() {
    let h = 12;
    let (labels, rgb, w) = band_image(h);
    let inks = [BLUE, WHITE, DARK_HALO, LIGHT_HALO, BLACK];
    let mut runs = RunLabels::new(&labels, w, h);
    let px = Pixels {
        rgb: &rgb,
        alpha: None,
    };
    let (first, _) = runs.absorb_halos(px, &inks);
    assert_eq!(first, 2, "the dark and the light halo");
    let out = runs.to_labels();
    for y in 0..h {
        let row = &out[y * w..(y + 1) * w];
        if y < h / 2 {
            // The dark halo's pixels are mostly blue (t < 1/2), the light one's mostly white.
            assert_eq!(&row[8..10], &[0, 0], "row {y}");
            assert_eq!(&row[10..12], &[1, 1], "row {y}");
        } else {
            assert_eq!(
                &row[8..12],
                &[4, 4, 4, 4],
                "row {y}: the hairline is no halo"
            );
        }
    }
    runs.assert_canonical();
}

#[test]
fn a_thick_band_is_no_halo() {
    // The light halo's colour, but five pixels wide: an ink with an interior of radius 2.
    let (w, h) = (20, 9);
    let labels: Vec<u16> = (0..w * h)
        .map(|p| match p % w {
            0..8 => 0,
            8..13 => 3,
            _ => 1,
        })
        .collect();
    let inks = [BLUE, WHITE, DARK_HALO, LIGHT_HALO];
    let rgb: Vec<[f32; 3]> = labels
        .iter()
        .map(|&l| {
            [
                inks[l as usize][0],
                inks[l as usize][1],
                inks[l as usize][2],
            ]
        })
        .collect();
    let mut runs = RunLabels::new(&labels, w, h);
    let px = Pixels {
        rgb: &rgb,
        alpha: None,
    };
    assert_eq!(runs.absorb_halos(px, &inks), (0, 0));
    assert_eq!(runs.to_labels(), labels);
}

#[test]
fn halo_specks_inside_one_ink_go_to_it() {
    // A band (dark halo) along the blue-white edge, and two specks of the same ink inside
    // the blue, away from the edge: with the band a halo, the specks follow it.
    let (w, h) = (24, 12);
    let mut labels: Vec<u16> = (0..w * h)
        .map(|p| match p % w {
            0..10 => 0,
            10..12 => 2,
            _ => 1,
        })
        .collect();
    labels[3 * w + 3] = 2;
    labels[8 * w + 5] = 2;
    labels[8 * w + 6] = 2;
    let inks = [BLUE, WHITE, DARK_HALO];
    let rgb: Vec<[f32; 3]> = labels
        .iter()
        .map(|&l| {
            [
                inks[l as usize][0],
                inks[l as usize][1],
                inks[l as usize][2],
            ]
        })
        .collect();
    let mut runs = RunLabels::new(&labels, w, h);
    let px = Pixels {
        rgb: &rgb,
        alpha: None,
    };
    assert_eq!(runs.absorb_halos(px, &inks), (1, 2));
    let out = runs.to_labels();
    assert!(!out.contains(&2));
    assert_eq!(out[3 * w + 3], 0);
}

/// A strip of a real ink -- one that owns an area elsewhere -- between two others whose
/// box it lies in but whose segment it is far from (`twemoji/1faa3`'s #55ACEE rim between
/// its navy and white, whose pixels the JPEG gives a mean of #62A9DD, 0.054 from the
/// segment) is no halo; a strip of an ink that owns no area is, at the same distance.
#[test]
fn a_strip_of_an_ink_with_an_area_must_lie_near_the_segment() {
    const NAVY: [f32; 4] = [0.0, 0.29, 0.467, 1.0];
    const RIM: [f32; 4] = [0.384, 0.665, 0.868, 1.0];
    let (n, r, wh) = (ycc(NAVY), ycc(RIM), ycc(WHITE));
    assert!(in_hull(r, n, wh));
    let (_, d) = coverage(r, n, wh).unwrap();
    assert!(d.sqrt() > HALO_LINE_TOL, "{}", d.sqrt());
    // Navy | a two-pixel strip of the rim's ink | white, over rows 0..6; below, a block of
    // the rim's ink with an interior (so the ink owns an area) when `area` is set.
    let inks = [NAVY, WHITE, RIM];
    for area in [true, false] {
        let (w, h) = (20, 12);
        let labels: Vec<u16> = (0..w * h)
            .map(|p| {
                let (x, y) = (p % w, p / w);
                match (x, y) {
                    (_, 6..) if area && (3..=14).contains(&x) => 2,
                    (0..8, _) => 0,
                    (8..10, 0..6) => 2,
                    _ => 1,
                }
            })
            .collect();
        let rgb: Vec<[f32; 3]> = labels
            .iter()
            .map(|&l| {
                [
                    inks[l as usize][0],
                    inks[l as usize][1],
                    inks[l as usize][2],
                ]
            })
            .collect();
        let mut runs = RunLabels::new(&labels, w, h);
        let px = Pixels {
            rgb: &rgb,
            alpha: None,
        };
        let (first, _) = runs.absorb_halos(px, &inks);
        assert_eq!(first, usize::from(!area), "area {area}");
    }
}
