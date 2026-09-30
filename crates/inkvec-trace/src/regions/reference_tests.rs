//! The flood-fill label clean-up as it shipped before [`super::Components`], kept as the
//! oracle the run-based rewrite is tested against (compiled only for tests).

use super::*;
use crate::color::reference_tests::{random_art, Lcg};

/// The shipped `despeckle`.
fn old_despeckle(labels: &mut [u16], w: usize, h: usize, min_size: usize) {
    if min_size <= 1 {
        return;
    }
    let (comp, members) = label_components(labels, w, h);

    for (id, group) in members.iter().enumerate() {
        if group.len() >= min_size {
            continue;
        }
        let mut tally: std::collections::HashMap<u16, usize> = std::collections::HashMap::new();
        for &p in group {
            let (x, y) = (p % w, p / w);
            for (nx, ny) in [
                (x as isize - 1, y as isize),
                (x as isize + 1, y as isize),
                (x as isize, y as isize - 1),
                (x as isize, y as isize + 1),
            ] {
                if nx < 0 || ny < 0 || nx >= w as isize || ny >= h as isize {
                    continue;
                }
                let q = ny as usize * w + nx as usize;
                if comp[q] != id as u32 {
                    *tally.entry(labels[q]).or_insert(0) += 1;
                }
            }
        }
        if let Some((&best, _)) = tally
            .iter()
            .max_by_key(|(&lab, &c)| (c, std::cmp::Reverse(lab)))
        {
            for &p in group {
                labels[p] = best;
            }
        }
    }
}

/// The shipped `absorb_blend_slivers` round loop, over flood-filled components.
fn old_absorb(
    labels: &mut [u16],
    rgb: &[[f32; 3]],
    alpha: &[f32],
    w: usize,
    h: usize,
    pal: &Palette,
    sigma_noise: f64,
) -> usize {
    let scene = SliverScene {
        rgb,
        alpha,
        w,
        h,
        pal,
        tol: (3.0 * sigma_noise).max(0.025) as f32,
        debug: false,
    };
    let mut absorbed = 0usize;
    for _round in 0..2 {
        let (comp, members) = label_components(labels, w, h);
        let mut changed = 0usize;
        let n_labels = labels
            .iter()
            .copied()
            .max()
            .map_or(1, |m| m as usize + 1)
            .max(pal.rgb.len());
        let mut contacts: Vec<usize> = vec![0; n_labels];
        for group in &members {
            if absorb_sliver(&scene, group, &comp, labels, &mut contacts) {
                changed += 1;
            }
        }
        absorbed += changed;
        if changed == 0 {
            break;
        }
    }
    absorbed
}

/// A random image with a palette recovered from it and its nearest-ink labels, plus
/// speckle, and an alpha that is partly translucent.
fn scene(rng: &mut Lcg, w: usize, h: usize) -> (Vec<[f32; 3]>, Vec<f32>, Palette, Vec<u16>) {
    let rgb = random_art(rng, w, h);
    let pal = crate::color::extract_palette(&rgb, w, h, crate::color::DEFAULT_MERGE_DISTANCE, 64);
    let mut labels = crate::color::label_image(&rgb, &pal);
    for l in labels.iter_mut() {
        if rng.below(15) == 0 {
            *l = rng.below(pal.len() as u64) as u16;
        }
    }
    let alpha = (0..w * h)
        .map(|_| if rng.below(6) == 0 { 0.5 } else { 1.0 })
        .collect();
    (rgb, alpha, pal, labels)
}

#[test]
fn run_based_despeckle_and_absorption_equal_the_flood_fill_ones() {
    let mut rng = Lcg(0xdec0);
    for case in 0..120 {
        let (w, h) = (2 + rng.below(50) as usize, 2 + rng.below(50) as usize);
        let (rgb, alpha, pal, labels) = scene(&mut rng, w, h);
        let min = [1, 2, 4, 9][case % 4];
        let (mut a, mut b) = (labels.clone(), labels.clone());
        old_despeckle(&mut a, w, h, min);
        despeckle(&mut b, w, h, min);
        assert_eq!(a, b, "despeckle case {case} {w}x{h} min {min}");
        let sigma = [0.0, 0.004, 0.02][case % 3];
        let na = old_absorb(&mut a, &rgb, &alpha, w, h, &pal, sigma);
        let nb = absorb_blend_slivers(&mut b, &rgb, &alpha, w, h, &pal, sigma);
        assert_eq!((na, &a), (nb, &b), "absorb case {case} {w}x{h}");
    }
}

#[test]
fn despeckle_leaves_a_one_label_image_alone_and_handles_empty_ones() {
    let mut one = vec![3u16; 12];
    despeckle(&mut one, 4, 3, 100);
    assert_eq!(one, vec![3u16; 12]);
    let mut empty: Vec<u16> = Vec::new();
    despeckle(&mut empty, 0, 0, 4);
    assert!(empty.is_empty());
}

/// The shipped `reassign_blend_pixels`: every pixel, every round.
fn old_reassign(
    labels: &mut [u16],
    rgb: &[[f32; 3]],
    alpha: &[f32],
    w: usize,
    h: usize,
    pal: &Palette,
    sigma_noise: f64,
) -> usize {
    const ROUNDS: usize = 4;
    let n = w * h;
    let tol = (3.0 * sigma_noise).max(0.025) as f32;
    let mut moved_total = 0usize;

    for _ in 0..ROUNDS {
        let snap = labels.to_vec();
        let decide = |p: usize| -> Option<u16> {
            let (x, y) = (p % w, p / w);
            let own = snap[p];
            let mut labs = [own; 4];
            let mut nl = 1usize;
            for (dx, dy) in [
                (-1i32, -1i32),
                (0, -1),
                (1, -1),
                (-1, 0),
                (1, 0),
                (-1, 1),
                (0, 1),
                (1, 1),
            ] {
                let (qx, qy) = (x as i32 + dx, y as i32 + dy);
                if qx < 0 || qy < 0 || qx >= w as i32 || qy >= h as i32 {
                    continue;
                }
                let l = snap[qy as usize * w + qx as usize];
                if !labs[..nl].contains(&l) && nl < 4 {
                    labs[nl] = l;
                    nl += 1;
                }
            }
            if nl < 2 {
                return None;
            }
            let c = rgb[p];
            let col = |l: u16| -> Option<[f32; 3]> { pal.rgb.get(l as usize).copied() };
            let oc = col(own)?;
            let e0 = [c[0] - oc[0], c[1] - oc[1], c[2] - oc[2]];
            let resid_own = (e0[0] * e0[0] + e0[1] * e0[1] + e0[2] * e0[2]).sqrt();

            let mut cols: Vec<[f32; 3]> = Vec::with_capacity(5);
            let mut keep: Vec<Option<u16>> = Vec::with_capacity(5);
            for &l in &labs[..nl] {
                if let Some(cc) = col(l) {
                    cols.push(cc);
                    keep.push(Some(l));
                }
            }
            if alpha[p] < 0.99 && !cols.contains(&BACKDROP) {
                cols.push(BACKDROP);
                keep.push(None);
            }
            let (r, who) = mixture(c, &cols)?;
            let target = match keep[who] {
                Some(l) => l,
                None => {
                    let real = &cols[..cols.len() - 1];
                    match mixture(c, real) {
                        Some((_, w2)) => keep[w2].expect(
                            "only the backdrop entry is None, and it is last, outside `real`",
                        ),
                        None => return None,
                    }
                }
            };
            if target != own && r <= tol && r < 0.5 * resid_own {
                Some(target)
            } else {
                None
            }
        };
        use rayon::prelude::*;
        let decided: Vec<Option<u16>> = (0..n).into_par_iter().map(decide).collect();
        let mut moved = 0usize;
        for (p, d) in decided.into_iter().enumerate() {
            if let Some(t) = d {
                labels[p] = t;
                moved += 1;
            }
        }
        moved_total += moved;
        if moved == 0 {
            break;
        }
    }
    moved_total
}

#[test]
fn active_set_reassignment_equals_deciding_every_pixel_every_round() {
    let mut rng = Lcg(0x7ea5);
    let mut moved_any = 0usize;
    for case in 0..120 {
        let (w, h) = (2 + rng.below(50) as usize, 2 + rng.below(50) as usize);
        let (rgb, alpha, pal, labels) = scene(&mut rng, w, h);
        let sigma = [0.0, 0.004, 0.02][case % 3];
        let (mut a, mut b) = (labels.clone(), labels.clone());
        let na = old_reassign(&mut a, &rgb, &alpha, w, h, &pal, sigma);
        let nb = reassign_blend_pixels(&mut b, &rgb, &alpha, w, h, &pal, sigma);
        assert_eq!((na, &a), (nb, &b), "case {case} {w}x{h}");
        moved_any += na;
    }
    assert!(moved_any > 0);
}

#[test]
fn relabel_rounds_follows_a_cascade_to_the_end() {
    // A rule that moves one pixel per round along a row: each pixel takes label 1 once its
    // left neighbour has it. Only the active set carries the wave forward.
    let (w, h) = (12, 3);
    let mut labels = vec![0u16; w * h];
    labels[w] = 1;
    let decide = |snap: &[u16], p: usize| -> Option<u16> {
        let x = p % w;
        (x > 0 && snap[p] == 0 && snap[p - 1] == 1 && p / w == 1).then_some(1)
    };
    let moved = relabel_rounds(&mut labels, w, h, 5, decide);
    assert_eq!(moved, 5);
    assert_eq!(&labels[w..2 * w], &[1, 1, 1, 1, 1, 1, 0, 0, 0, 0, 0, 0]);
}
