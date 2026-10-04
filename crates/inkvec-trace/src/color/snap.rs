//! One artist ink, one hex: flat faces of one ink whose fitted colours differ by less than a
//! viewer could see are painted one colour, the colour of the ink's best-evidenced face.
//!
//! # The problem
//!
//! The palette (`color/mdl.rs`, `native/palette.rs`) decides which inks the drawing has.
//! The fills are decided later and separately: after band merging and the carve, every face
//! inherits a fill fitted from pixels (`gradient::bands::Merger::write_back`, the carve's
//! `fit_pixels`). A flat component with an interior of its own keeps its own colour, which is
//! right for a real plateau (the lizard's belly, 17 levels off its ink) and wrong for two
//! faces of one ink, whose medians land a level or two apart. The r2-palette research
//! counted 26 such duplicate fills under 1 dE00 in 21 of the 362 icons of screen + held_a at
//! 128 px, and 683 in 147 icons of the same icons upscaled 2x with Lanczos; the Studio crest,
//! one gold, came out as `#b08a4a` and `#b18b4b` (0.35 dE00 apart).
//!
//! # The rule
//!
//! 1. **Each ink's colour.** The fill of its *representative face*: the flat face of that
//!    ink with the most strictly interior pixels (not on the picture edge, all four
//!    neighbours in the face: the samples `gradient::fit_pixels` fits first) whose fill is
//!    within [`REP_DE00`] (1.5, the palette's same-ink floor [`super::SAME_INK_DE00`]) of the
//!    palette entry, ties to the lower face index. An ink with no such face is left alone.
//!    Not the palette entry's own colour: that is the mean of every pixel within the merge
//!    radius of the ink (`refine_to_members`), which takes in the anti-aliasing and, on a
//!    resampled raster, the overshoot rim. On the crest the entry is `#b28c4c` while the
//!    gold's interior reads `#b08a4a`, and painting the faces the entry cost dE00
//!    0.1192 -> 0.1323 (measured 2026-10-03). A face fill is the median of its own interior.
//! 2. **Snap.** A flat face whose fill `c` is within [`SNAP_DE00`] (0.5) of its ink's colour
//!    `r` is painted `r`. Everything else keeps its fill: gradient faces, faces the caller
//!    skips (the native path's fades), faces whose ink index is out of range, and every face
//!    farther than 0.5 from its ink's colour.
//!
//! Only the colour changes; `chi2`, `params` and `cost` are left as fitted, since nothing after
//! this point reads them for a flat fill. The call sites run this before the boundary stages,
//! so sub-pixel refinement and the boundary solve place the edges against the colour that is
//! written.
//!
//! # Why 0.5, and what was dropped (gate v2 against v0.2.5, 2026-10-03)
//!
//! The research proposed snapping within 1.5 dE00, and always when a face has no interior
//! pixel. Both were measured and both fail the gate:
//!
//! * within 1.5 or 1.0: quality-512ssop dE00 +1.31 % / +1.30 % (upper bound 2.99 % against a
//!   2.25 % margin), worst `noto-emoji/emoji_u2b05` 0.0084 -> 0.1302: the white page
//!   (`#ffffff`) and the white arrow (`#fafafa`) are one palette ink under the same-ink floor,
//!   and painting the page the arrow's colour tints the whole canvas. Two colours the artist
//!   wrote 5 levels apart are not a duplicate, however close the palette calls them;
//! * a face with no interior, painted its ink when its colour lies on a chord between its ink
//!   and another (the palette's blend test): quality-128ss dE00 +0.97 % (48 icons, 30 worse),
//!   e.g. `twemoji/1f646-1f3fb-200d-2640-fe0f` repainted a thin face `#c1694f` as `#df1f32`
//!   (13.5 dE00) and `simple-icons/ebox` +0.045 on held_a. A thin face of one ink's colour
//!   blended with another is often a stroke of its own, and the boundary stages cannot shrink
//!   a one-pixel face to the coverage a full-strength ink would need;
//! * within 0.5, no interior rule: quality-128ss +0.02 %, quality-512ssop +0.28 %, both
//!   within noise. That is what ships. 0.5 is half the 1.0 the research used to call two
//!   fills duplicates; a level or two of 8-bit sRGB on a mid colour reads 0.3-0.7.
//!
//! # Cost
//!
//! One pass over the face map for the interior counts, O(w·h); then at most two CIEDE2000
//! per flat face.
//!
//! # Literature
//!
//! Inspired by: J. Yang, N. Vining, S. Kheradmand, N. Carr, L. Sigal, A. Sheffer (2023),
//! "Subpixel Deblurring of Anti-Aliased Raster Clip-Art", *Computer Graphics Forum* 42(2),
//! doi:10.1111/cgf.14744: their labelling energy carries a distinctiveness term that
//! penalises adjacent regions with similar but not identical colours, so one ink comes out as
//! one colour, and their palette colours come from patches, never from edge pixels. Here the
//! same outcome is a post-fit choice between near-equal fitted colours, because the labels
//! are already fixed. Fast mode writes every face in its ink by construction
//! (`fast/palette.rs`).

use super::{de00, Palette};
use crate::gradient::{FillFit, FillModel};

/// CIEDE2000 within which a face's flat fill is painted its ink's colour. See the module
/// docs for the values measured above it.
pub(crate) const SNAP_DE00: f32 = 0.5;

/// CIEDE2000 within which a face counts as its palette entry's when the ink's colour is
/// chosen: the palette's own same-ink floor.
pub(crate) const REP_DE00: f32 = super::SAME_INK_DE00;

/// How many strictly interior pixels each face owns: not on the picture edge and with all
/// four neighbours in the same face (the samples `gradient::fit_pixels` fits first).
///
/// `faces` is the face map, one id per pixel, row-major `w × h`; ids at or above `n_faces`
/// are ignored. One pass, O(w·h). An image narrower or shorter than 3 px has no interior.
fn interior_counts(faces: &[u16], w: usize, h: usize, n_faces: usize) -> Vec<u32> {
    let mut count = vec![0u32; n_faces];
    if w < 3 || h < 3 || faces.len() < w * h {
        return count;
    }
    for y in 1..h - 1 {
        let row = y * w;
        for x in 1..w - 1 {
            let p = row + x;
            let f = faces[p];
            if (f as usize) < n_faces
                && faces[p - 1] == f
                && faces[p + 1] == f
                && faces[p - w] == f
                && faces[p + w] == f
            {
                count[f as usize] += 1;
            }
        }
    }
    count
}

/// Paint near-duplicate flat faces of one ink that ink's colour (see the module docs).
///
/// * `faces`: the face map (`split_components` ids), row-major `w × h`;
/// * `face_fill`, `face_color`: per face, its fill (edited in place) and its palette index;
/// * `pal`: the palette, `pal.rgb` over white (as the fills are);
/// * `skip`: faces to leave alone (the native path's fades). A skipped face is not a
///   representative either.
///
/// Returns how many faces were repainted (a face already at its ink's colour is not
/// counted). Deterministic: every representative is chosen before any fill changes.
pub(crate) fn snap_flat_fills(
    faces: &[u16],
    w: usize,
    h: usize,
    face_fill: &mut [FillFit],
    face_color: &[usize],
    pal: &Palette,
    skip: impl Fn(usize) -> bool,
) -> usize {
    let n = face_fill.len().min(face_color.len());
    if n == 0 || pal.is_empty() {
        return 0;
    }
    let interior = interior_counts(faces, w, h, n);
    // Face `f`'s (ink, flat colour), when it is a flat face this pass may touch.
    let flat = |fills: &[FillFit], f: usize| match fills[f].model {
        FillModel::Flat(c) if face_color[f] < pal.len() && !skip(f) => Some((face_color[f], c)),
        _ => None,
    };
    // Each ink's representative: (interior pixels, colour) of its best-evidenced face.
    let mut rep: Vec<Option<(u32, [f32; 3])>> = vec![None; pal.len()];
    for f in 0..n {
        let Some((ink, c)) = flat(face_fill, f) else {
            continue;
        };
        if interior[f] == 0 || de00(c, pal.rgb[ink]) >= REP_DE00 {
            continue;
        }
        if rep[ink].is_none_or(|(k, _)| interior[f] > k) {
            rep[ink] = Some((interior[f], c));
        }
    }
    let mut snapped = 0;
    for f in 0..n {
        let Some((ink, c)) = flat(face_fill, f) else {
            continue;
        };
        let Some((_, r)) = rep[ink] else {
            continue;
        };
        if c != r && de00(c, r) < SNAP_DE00 {
            face_fill[f].model = FillModel::Flat(r);
            snapped += 1;
        }
    }
    snapped
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::color::rgb_to_oklab;
    use crate::gradient::PARAMS_FLAT;

    fn flat(c: [f32; 3]) -> FillFit {
        FillFit {
            model: FillModel::Flat(c),
            chi2: 1.0,
            params: PARAMS_FLAT,
            cost: 2.0,
        }
    }

    fn palette(inks: &[[f32; 3]]) -> Palette {
        Palette {
            colors: inks.iter().map(|&c| rgb_to_oklab(c)).collect(),
            rgb: inks.to_vec(),
            weight: vec![1.0 / inks.len() as f32; inks.len()],
            alpha: vec![1.0; inks.len()],
        }
    }

    const WHITE: [f32; 3] = [1.0, 1.0, 1.0];
    const BLACK: [f32; 3] = [0.0, 0.0, 0.0];
    const RED: [f32; 3] = [0.9, 0.1, 0.1];
    /// One level off black on two channels: under `SNAP_DE00`.
    const NEAR_BLACK: [f32; 3] = [1.0 / 255.0, 1.0 / 255.0, 2.0 / 255.0];

    /// A 12 x 12 face map: face 0 the ground, face 1 a 6 x 6 block (16 interior pixels),
    /// face 2 a one-pixel line (none), face 3 a 3 x 3 block (one).
    fn faces() -> (Vec<u16>, usize, usize) {
        let (w, h) = (12, 12);
        let f = (0..w * h)
            .map(|p| {
                let (x, y) = (p % w, p / w);
                if (2..8).contains(&x) && (2..8).contains(&y) {
                    1
                } else if x == 10 && (2..10).contains(&y) {
                    2
                } else if (2..5).contains(&x) && (9..12).contains(&y) {
                    3
                } else {
                    0
                }
            })
            .collect();
        (f, w, h)
    }

    #[test]
    fn interior_is_four_neighbours_and_off_the_picture_edge() {
        let (f, w, h) = faces();
        let c = interior_counts(&f, w, h, 4);
        assert_eq!(&c[1..], &[16, 0, 1], "{c:?}");
        assert!(c[0] > 0);
        // Too small to have any interior at all; and ids past `n_faces` are ignored.
        assert_eq!(interior_counts(&[0; 4], 2, 2, 1), vec![0]);
        assert_eq!(interior_counts(&f, w, h, 1).len(), 1);
    }

    #[test]
    fn faces_of_one_ink_take_the_colour_of_the_face_with_most_interior() {
        let (f, w, h) = faces();
        assert!(de00(NEAR_BLACK, BLACK) < SNAP_DE00);
        // The palette entry is NEAR_BLACK; the big block was fitted to BLACK, the line and
        // the small block to NEAR_BLACK. All are painted BLACK, the big block's colour.
        let pal = palette(&[WHITE, NEAR_BLACK]);
        let mut fills = vec![flat(WHITE), flat(BLACK), flat(NEAR_BLACK), flat(NEAR_BLACK)];
        let got = snap_flat_fills(&f, w, h, &mut fills, &[0, 1, 1, 1], &pal, |_| false);
        assert_eq!(got, 2);
        for k in 1..4 {
            assert_eq!(fills[k].model, FillModel::Flat(BLACK), "face {k}");
        }
        // Only the colour moves.
        assert_eq!((fills[2].chi2, fills[2].cost), (1.0, 2.0));
    }

    #[test]
    fn colours_the_artist_kept_apart_stay_apart() {
        let (f, w, h) = faces();
        // Two light greys a few levels apart: one palette ink (under the same-ink floor),
        // but above the snap: the page and the arrow of noto-emoji/emoji_u2b05.
        let (page, arrow) = (WHITE, [0.98, 0.98, 0.98]);
        let d = de00(page, arrow);
        assert!((SNAP_DE00..REP_DE00).contains(&d), "{d}");
        let pal = palette(&[WHITE, BLACK]);
        let mut fills = vec![flat(page), flat(BLACK), flat(BLACK), flat(arrow)];
        assert_eq!(
            snap_flat_fills(&f, w, h, &mut fills, &[0, 1, 1, 0], &pal, |_| false),
            0
        );
        assert_eq!(fills[3].model, FillModel::Flat(arrow));
    }

    #[test]
    fn an_ink_without_an_interior_face_and_a_plateau_are_left_alone() {
        let (f, w, h) = faces();
        let pal = palette(&[WHITE, BLACK]);
        // Only the line is black's, and it has no interior: nothing to snap to.
        let mut fills = vec![flat(WHITE), flat(RED), flat(NEAR_BLACK), flat(RED)];
        snap_flat_fills(&f, w, h, &mut fills, &[0, 9, 1, 9], &pal, |_| false);
        assert_eq!(fills[2].model, FillModel::Flat(NEAR_BLACK));
        assert_eq!(fills[1].model, FillModel::Flat(RED));
        // A plateau far from its entry keeps its colour and does not represent the ink.
        let grey = [0.3, 0.3, 0.3];
        let mut fills = vec![flat(WHITE), flat(grey), flat(NEAR_BLACK), flat(BLACK)];
        assert_eq!(
            snap_flat_fills(&f, w, h, &mut fills, &[0, 1, 1, 1], &pal, |_| false),
            1
        );
        assert_eq!(fills[1].model, FillModel::Flat(grey));
        assert_eq!(fills[2].model, FillModel::Flat(BLACK));
    }

    #[test]
    fn a_thin_face_of_anti_aliasing_keeps_its_fill() {
        let (f, w, h) = faces();
        let pal = palette(&[WHITE, BLACK]);
        // Half coverage of black over white: measured and dropped (module docs).
        let half = [0.735, 0.735, 0.735];
        let mut fills = vec![flat(WHITE), flat(BLACK), flat(half), flat(BLACK)];
        assert_eq!(
            snap_flat_fills(&f, w, h, &mut fills, &[0, 1, 1, 1], &pal, |_| false),
            0
        );
        assert_eq!(fills[2].model, FillModel::Flat(half));
    }

    #[test]
    fn gradients_skipped_faces_and_bad_indices_are_left_alone() {
        let (f, w, h) = faces();
        let pal = palette(&[WHITE, BLACK]);
        let grad = FillFit {
            model: FillModel::Linear {
                p0: (0.0, 0.0),
                p1: (1.0, 0.0),
                c0: BLACK,
                c1: WHITE,
                interp: crate::gradient::Interp::LinearRgb,
                mids: Vec::new(),
            },
            chi2: 0.0,
            params: 10.0,
            cost: 0.0,
        };
        let mut fills = vec![
            grad.clone(),
            flat(NEAR_BLACK),
            flat(NEAR_BLACK),
            flat(BLACK),
        ];
        let got = snap_flat_fills(&f, w, h, &mut fills, &[0, 1, 9, 1], &pal, |k| k == 1);
        // Face 1 is skipped (and so does not represent black); face 3 represents black and
        // is already black; face 2's ink is out of range.
        assert_eq!(got, 0);
        assert_eq!(fills[0].model, grad.model);
        assert_eq!(fills[1].model, FillModel::Flat(NEAR_BLACK));
        assert_eq!(fills[2].model, FillModel::Flat(NEAR_BLACK));
        // Empty inputs.
        let empty = Palette {
            colors: vec![],
            rgb: vec![],
            weight: vec![],
            alpha: vec![],
        };
        assert_eq!(
            snap_flat_fills(&f, w, h, &mut fills, &[0, 1, 1, 1], &empty, |_| false),
            0
        );
    }
}
