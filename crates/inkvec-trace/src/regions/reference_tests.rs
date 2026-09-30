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
