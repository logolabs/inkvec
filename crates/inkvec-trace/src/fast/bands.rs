//! Fast mode's gradients: posterised ramps put back together, one fit per ramp.
//!
//! A smooth gradient has no flat colour, so the palette quantises it into bands, and each
//! band is a face with a boundary of its own: on a gradient-filled icon that is most of the
//! document. Quality mode merges bands pairwise under its MDL objective, refitting after
//! every merge; this is the one-shot version. Adjacent faces whose inks are a small step
//! apart are joined into clusters, each cluster is fitted once with the quality fitter's
//! own gradient models (linear, radial, elliptical), and the fit is kept only when it
//! explains the cluster's pixels at least about as well as the flat bands did. A step
//! between two genuinely different inks cannot be fitted by a ramp, fails that test and
//! stays two faces.

use crate::color::{rgb_to_oklab, Palette};
use crate::gradient::{self, FillFit, FillModel};
use std::collections::HashMap;

/// Largest OKLab distance between two adjacent faces' inks for them to be two bands of
/// one ramp. The palette's own merge distance is 0.035, so bands of a ramp sit one or two
/// of those apart.
const RAMP_STEP: f32 = 0.09;
/// Pixels a boundary must share before it counts as adjacency rather than a corner touch.
const MIN_CONTACT: u32 = 3;
/// Smallest cluster, in pixels, worth a gradient.
const MIN_PIXELS: usize = 64;
/// Most pixels the acceptance test samples per cluster.
const CHECK_SAMPLES: usize = 4096;
/// A gradient is kept when its RMS residual is at most this multiple of the flat bands'
/// residual, plus [`SLACK`]: it replaces every band boundary in the cluster with one fill,
/// so it may explain the colour slightly worse and still be the better document.
const RATIO: f64 = 1.25;
/// Flat bands whose RMS residual is at most this, in sRGB units, are left as they are.
const FLAT_ENOUGH: f64 = 1.5 / 255.0;
/// Absolute slack on the acceptance test, in sRGB units (one display level).
const SLACK: f64 = 1.0 / 255.0;

fn find(parent: &mut [usize], mut x: usize) -> usize {
    while parent[x] != x {
        parent[x] = parent[parent[x]];
        x = parent[x];
    }
    x
}

/// Contact length between every pair of adjacent faces.
fn contacts(labels: &[u16], w: usize, h: usize) -> HashMap<(u16, u16), u32> {
    let mut out: HashMap<(u16, u16), u32> = HashMap::new();
    let mut touch = |a: u16, b: u16| {
        if a != b {
            *out.entry((a.min(b), a.max(b))).or_insert(0) += 1;
        }
    };
    for y in 0..h {
        for x in 0..w {
            let p = y * w + x;
            if x + 1 < w {
                touch(labels[p], labels[p + 1]);
            }
            if y + 1 < h {
                touch(labels[p], labels[p + w]);
            }
        }
    }
    out
}

/// RMS residual over sampled pixels of the cluster's interior: of `model` (zero without
/// one), and of each pixel's own flat band colour.
fn residuals(
    rgb: &[[f32; 3]],
    w: usize,
    pixels: &[usize],
    model: Option<&FillModel>,
    band: impl Fn(usize) -> [f32; 3],
) -> (f64, f64) {
    let step = (pixels.len() / CHECK_SAMPLES).max(1);
    let (mut g, mut f, mut n) = (0.0f64, 0.0f64, 0usize);
    for &p in pixels.iter().step_by(step) {
        let c = rgb[p];
        let m = model.map_or([0.0; 3], |m| m.color_at((p % w) as f64, (p / w) as f64));
        let b = band(p);
        for k in 0..3 {
            g += ((c[k] - m[k]) as f64).powi(2);
            f += ((c[k] - b[k]) as f64).powi(2);
        }
        n += 3;
    }
    let n = n.max(1) as f64;
    ((g / n).sqrt(), (f / n).sqrt())
}

/// Merge ramps of bands into gradient faces. `labels` are face ids; the three per-face
/// vectors are rewritten with the merged faces renumbered. Returns the number of gradient
/// faces made.
pub(crate) fn merge_ramps(
    rgb: &[[f32; 3]],
    w: usize,
    h: usize,
    pal: &Palette,
    labels: &mut [u16],
    face_fill: &mut Vec<FillFit>,
    face_color: &mut Vec<usize>,
) -> usize {
    use rayon::prelude::*;
    let n = face_color.len();
    if n < 2 {
        return 0;
    }
    let lab: Vec<_> = face_color
        .iter()
        .map(|&c| rgb_to_oklab(pal.rgb.get(c).copied().unwrap_or([0.0; 3])))
        .collect();
    let mut parent: Vec<usize> = (0..n).collect();
    let mut pairs: Vec<((u16, u16), u32)> = contacts(labels, w, h).into_iter().collect();
    pairs.sort_unstable();
    for ((a, b), len) in pairs {
        let (a, b) = (a as usize, b as usize);
        if a >= n || b >= n || len < MIN_CONTACT || lab[a].dist(lab[b]) > RAMP_STEP {
            continue;
        }
        let (ra, rb) = (find(&mut parent, a), find(&mut parent, b));
        if ra != rb {
            parent[ra.max(rb)] = ra.min(rb);
        }
    }
    let root: Vec<usize> = (0..n).map(|f| find(&mut parent, f)).collect();
    let mut members: HashMap<usize, usize> = HashMap::new();
    for &r in &root {
        *members.entry(r).or_insert(0) += 1;
    }
    // Pixels per cluster that has more than one face.
    let mut cluster_px: HashMap<usize, Vec<usize>> = HashMap::new();
    for (p, &l) in labels.iter().enumerate() {
        let r = root[l as usize];
        if members[&r] > 1 {
            cluster_px.entry(r).or_default().push(p);
        }
    }
    let mut clusters: Vec<(usize, Vec<usize>)> = cluster_px
        .into_iter()
        .filter(|(_, px)| px.len() >= MIN_PIXELS)
        .collect();
    clusters.sort_unstable_by_key(|c| c.0);
    let sigma = crate::coverage::NOISE_FLOOR;
    let lambda = gradient::bic_lambda(w * h);
    let labels_ro: &[u16] = labels;
    let fits: Vec<Option<(usize, FillFit)>> = clusters
        .par_iter()
        .map(|(r, px)| {
            let member = |p: usize| root[labels_ro[p] as usize] == *r;
            let interior: Vec<usize> = px
                .iter()
                .copied()
                .filter(|&p| {
                    let (x, y) = (p % w, p / w);
                    x > 0 && y > 0 && x + 1 < w && y + 1 < h && {
                        [p - 1, p + 1, p - w, p + w].iter().all(|&q| member(q))
                    }
                })
                .collect();
            let band = |p: usize| {
                let c = face_color[labels_ro[p] as usize];
                pal.rgb.get(c).copied().unwrap_or([0.0; 3])
            };
            // Bands that already explain their pixels are flat inks side by side, not a
            // quantised ramp: nothing for a gradient to win, and the fit is the costly part.
            let (_, f) = residuals(rgb, w, &interior, None, band);
            if f <= FLAT_ENOUGH {
                return None;
            }
            let best = gradient::select(gradient::fit_pixels(
                rgb,
                w,
                h,
                px,
                member,
                |_| true,
                sigma,
                lambda,
            ));
            if !best.model.is_gradient() {
                return None;
            }
            let (g, _) = residuals(rgb, w, &interior, Some(&best.model), band);
            (g <= RATIO * f + SLACK).then_some((*r, best))
        })
        .collect();
    let mut made = 0;
    let mut fill_of_root: HashMap<usize, FillFit> = HashMap::new();
    for (r, fit) in fits.into_iter().flatten() {
        fill_of_root.insert(r, fit);
        made += 1;
    }
    if made == 0 {
        return 0;
    }
    // Renumber: a merged cluster becomes its root's face; everything else keeps its own.
    let target = |f: usize| {
        if fill_of_root.contains_key(&root[f]) {
            root[f]
        } else {
            f
        }
    };
    let mut new_id = vec![u16::MAX; n];
    let mut next = 0u16;
    let (mut fills, mut colors) = (Vec::new(), Vec::new());
    for f in 0..n {
        let t = target(f);
        if new_id[t] == u16::MAX {
            new_id[t] = next;
            next += 1;
            fills.push(
                fill_of_root
                    .get(&t)
                    .cloned()
                    .unwrap_or_else(|| face_fill[t].clone()),
            );
            colors.push(face_color[t]);
        }
    }
    for l in labels.iter_mut() {
        *l = new_id[target(*l as usize)];
    }
    *face_fill = fills;
    *face_color = colors;
    made
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pal(colors: &[[f32; 3]]) -> Palette {
        Palette {
            colors: colors.iter().map(|&c| rgb_to_oklab(c)).collect(),
            rgb: colors.to_vec(),
            weight: vec![1.0 / colors.len() as f32; colors.len()],
            alpha: vec![1.0; colors.len()],
        }
    }

    fn flat(c: [f32; 3]) -> FillFit {
        FillFit {
            model: FillModel::Flat(c),
            chi2: 0.0,
            params: gradient::PARAMS_FLAT,
            cost: 0.0,
        }
    }

    #[test]
    fn a_banded_ramp_becomes_one_gradient_face() {
        let (w, h) = (64, 32);
        let rgb: Vec<[f32; 3]> = (0..w * h)
            .map(|p| {
                let t = (p % w) as f32 / (w - 1) as f32;
                [0.2 + 0.6 * t, 0.3, 0.8 - 0.5 * t]
            })
            .collect();
        // Four bands, each labelled with its mean colour.
        let bands: Vec<[f32; 3]> = (0..4)
            .map(|b| {
                let t = (b as f32 + 0.5) / 4.0;
                [0.2 + 0.6 * t, 0.3, 0.8 - 0.5 * t]
            })
            .collect();
        let p = pal(&bands);
        let mut labels: Vec<u16> = (0..w * h).map(|q| ((q % w) / 16) as u16).collect();
        let mut fills: Vec<FillFit> = bands.iter().map(|&c| flat(c)).collect();
        let mut colors: Vec<usize> = (0..4).collect();
        let made = merge_ramps(&rgb, w, h, &p, &mut labels, &mut fills, &mut colors);
        assert_eq!(made, 1);
        assert_eq!(fills.len(), 1);
        assert!(fills[0].model.is_gradient());
        assert!(labels.iter().all(|&l| l == 0));
    }

    #[test]
    fn a_step_between_two_close_inks_stays_two_faces() {
        let (w, h) = (64, 32);
        let a = [0.80f32, 0.40, 0.30];
        let b = [0.74f32, 0.36, 0.28];
        let rgb: Vec<[f32; 3]> = (0..w * h).map(|p| if p % w < 32 { a } else { b }).collect();
        let p = pal(&[a, b]);
        let mut labels: Vec<u16> = (0..w * h).map(|q| u16::from(q % w >= 32)).collect();
        let mut fills = vec![flat(a), flat(b)];
        let mut colors = vec![0, 1];
        assert_eq!(
            merge_ramps(&rgb, w, h, &p, &mut labels, &mut fills, &mut colors),
            0
        );
        assert_eq!(fills.len(), 2);
    }
}
