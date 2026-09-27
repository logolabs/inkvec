//! Speckle removal and faces for fast mode, in two linear passes over row runs.
//!
//! `regions::despeckle` and `regions::split_components` flood-fill pixel by pixel and keep
//! every component's pixel list; at 2048 px that is most of a second of allocation. Here a
//! component is found by union-find over horizontal runs of one label -- a run is joined to
//! the runs above it that overlap it and carry the same label -- so the work is one pass
//! over the pixels plus one over the runs. Components smaller than `min_size` take the
//! label they share the longest border with (the smallest label on a tie), and the faces
//! are the components of the result.

/// Components of equal labels (4-connected): the component of every pixel, each
/// component's size and label, in order of first appearance in scan order.
pub(crate) struct Components {
    /// Component id per pixel.
    pub comp: Vec<u32>,
    /// Pixels per component.
    pub size: Vec<usize>,
    /// Label of each component.
    pub label: Vec<u16>,
}

fn find(parent: &mut [u32], mut x: u32) -> u32 {
    while parent[x as usize] != x {
        let p = parent[x as usize];
        parent[x as usize] = parent[p as usize];
        x = p;
    }
    x
}

pub(crate) fn components(labels: &[u16], w: usize, h: usize) -> Components {
    // Runs: (x0, x1 exclusive, label), and where each row's runs start.
    let mut runs: Vec<(u32, u32, u16)> = Vec::new();
    let mut row_start = Vec::with_capacity(h + 1);
    for y in 0..h {
        row_start.push(runs.len());
        let row = &labels[y * w..(y + 1) * w];
        let mut x = 0;
        while x < w {
            let l = row[x];
            let x0 = x;
            while x < w && row[x] == l {
                x += 1;
            }
            runs.push((x0 as u32, x as u32, l));
        }
    }
    row_start.push(runs.len());
    let mut parent: Vec<u32> = (0..runs.len() as u32).collect();
    for y in 1..h {
        let (mut i, mut j) = (row_start[y - 1], row_start[y]);
        let (ie, je) = (row_start[y], row_start[y + 1]);
        while i < ie && j < je {
            let (a, b) = (runs[i], runs[j]);
            if a.0 < b.1 && b.0 < a.1 && a.2 == b.2 {
                let (ra, rb) = (find(&mut parent, i as u32), find(&mut parent, j as u32));
                if ra != rb {
                    // The earlier run is the root, so ids follow scan order.
                    parent[ra.max(rb) as usize] = ra.min(rb);
                }
            }
            if a.1 <= b.1 {
                i += 1;
            } else {
                j += 1;
            }
        }
    }
    let mut id_of_root = vec![u32::MAX; runs.len()];
    let mut size = Vec::new();
    let mut label = Vec::new();
    let mut run_comp = vec![0u32; runs.len()];
    for r in 0..runs.len() {
        let root = find(&mut parent, r as u32) as usize;
        if id_of_root[root] == u32::MAX {
            id_of_root[root] = size.len() as u32;
            size.push(0);
            label.push(runs[root].2);
        }
        let id = id_of_root[root];
        run_comp[r] = id;
        size[id as usize] += (runs[r].1 - runs[r].0) as usize;
    }
    let mut comp = vec![0u32; w * h];
    for y in 0..h {
        for r in row_start[y]..row_start[y + 1] {
            let (x0, x1, _) = runs[r];
            comp[y * w + x0 as usize..y * w + x1 as usize].fill(run_comp[r]);
        }
    }
    Components { comp, size, label }
}

/// Relabel every component smaller than `min_size` pixels with the neighbouring label it
/// shares the longest border with.
pub(crate) fn despeckle(labels: &mut [u16], w: usize, h: usize, min_size: usize) {
    if min_size <= 1 {
        return;
    }
    let c = components(labels, w, h);
    if c.size.iter().all(|&s| s >= min_size) {
        return;
    }
    let mut tally: std::collections::HashMap<u32, Vec<(u16, u32)>> = Default::default();
    for p in 0..w * h {
        let id = c.comp[p];
        if c.size[id as usize] >= min_size {
            continue;
        }
        let (x, y) = (p % w, p / w);
        let t = tally.entry(id).or_default();
        let mut vote = |q: usize| {
            if c.comp[q] != id {
                let l = labels[q];
                match t.iter_mut().find(|e| e.0 == l) {
                    Some(e) => e.1 += 1,
                    None => t.push((l, 1)),
                }
            }
        };
        if x > 0 {
            vote(p - 1);
        }
        if x + 1 < w {
            vote(p + 1);
        }
        if y > 0 {
            vote(p - w);
        }
        if y + 1 < h {
            vote(p + w);
        }
    }
    // New label per component; `u16::MAX` keeps its own.
    let mut to = vec![u16::MAX; c.size.len()];
    for (id, t) in tally {
        if let Some((l, _)) = t
            .into_iter()
            .max_by_key(|&(l, n)| (n, std::cmp::Reverse(l)))
        {
            to[id as usize] = l;
        }
    }
    for (l, &id) in labels.iter_mut().zip(&c.comp) {
        let t = to[id as usize];
        if t != u16::MAX {
            *l = t;
        }
    }
}

/// Largest distance (sRGB and opacity, Euclidean) from a pixel to the line between two
/// neighbouring inks for the pixel to count as a blend of them.
pub(crate) const BLEND_TOL: f32 = 0.04;

/// `col` read as a blend of inks `a` and `b` (sRGB and opacity): how much of `b` it holds,
/// clamped to [0, 1], and its squared distance from the line between them. `None` when the
/// two inks are one colour.
pub(crate) fn blend_of(col: [f32; 4], a: [f32; 4], b: [f32; 4]) -> Option<(f32, f32)> {
    let ab: [f32; 4] = std::array::from_fn(|k| b[k] - a[k]);
    let l2: f32 = ab.iter().map(|v| v * v).sum();
    if l2 < 1e-9 {
        return None;
    }
    let t = ((0..4).map(|k| (col[k] - a[k]) * ab[k]).sum::<f32>() / l2).clamp(0.0, 1.0);
    let d = (0..4)
        .map(|k| (col[k] - (a[k] + t * ab[k])).powi(2))
        .sum::<f32>();
    Some((t, d))
}

/// Whether `col` is a blend of inks `a` and `b`: within [`BLEND_TOL`] of the line between
/// them.
pub(crate) fn is_blend(col: [f32; 4], a: [f32; 4], b: [f32; 4]) -> bool {
    blend_of(col, a, b).is_some_and(|(_, d)| d <= BLEND_TOL * BLEND_TOL)
}

/// Put anti-aliasing slivers back where they belong.
///
/// The palette sends a blend to a neighbour's ink only when the blend's colour is not an
/// ink itself; a rim between white and a gradient crosses the gradient's own light bands,
/// and comes out as one-pixel strips of those inks, each a face with a boundary. A
/// component with no interior pixel (none whose four neighbours share its label) whose
/// pixels are each a blend of two inks that meet across it -- or simply one of them -- is
/// such a strip: every pixel takes the nearer of those inks. A hairline is not a blend of
/// what lies either side of it, and stays.
pub(crate) fn absorb_slivers(
    labels: &mut [u16],
    px: &[[f32; 4]],
    inks: &[[f32; 4]],
    w: usize,
    h: usize,
) {
    let c = components(labels, w, h);
    let interior = interiors(&c, w, h);
    let d2 = |a: [f32; 4], b: [f32; 4]| (0..4).map(|k| (a[k] - b[k]).powi(2)).sum::<f32>();
    let src = labels.to_vec();
    for p in 0..w * h {
        let id = c.comp[p] as usize;
        if interior[id] {
            continue;
        }
        let (x, y) = (p % w, p / w);
        let mut around: Vec<u16> = Vec::with_capacity(4);
        for q in [
            (x > 0).then(|| p - 1),
            (x + 1 < w).then(|| p + 1),
            (y > 0).then(|| p - w),
            (y + 1 < h).then(|| p + w),
        ]
        .into_iter()
        .flatten()
        {
            if interior[c.comp[q] as usize] && src[q] != src[p] && !around.contains(&src[q]) {
                around.push(src[q]);
            }
        }
        let col = px[p];
        let mut best: Option<(u16, f32)> = None;
        let mut consider = |l: u16, d: f32| {
            if d <= BLEND_TOL * BLEND_TOL && best.is_none_or(|(_, e)| d < e) {
                best = Some((l, d));
            }
        };
        for (i, &a) in around.iter().enumerate() {
            let ia = inks[a as usize];
            consider(a, d2(col, ia));
            for &b in &around[i + 1..] {
                let Some((t, d)) = blend_of(col, ia, inks[b as usize]) else {
                    continue;
                };
                // The blend goes to the ink it is mostly made of.
                consider(if t < 0.5 { a } else { b }, d);
            }
        }
        if let Some((l, _)) = best {
            labels[p] = l;
        }
    }
}

/// Which components have an interior pixel: one whose four neighbours are all in it.
fn interiors(c: &Components, w: usize, h: usize) -> Vec<bool> {
    let mut interior = vec![false; c.size.len()];
    for p in 0..w * h {
        let (x, y) = (p % w, p / w);
        let id = c.comp[p];
        if x > 0
            && y > 0
            && x + 1 < w
            && y + 1 < h
            && [p - 1, p + 1, p - w, p + w]
                .iter()
                .all(|&q| c.comp[q] == id)
        {
            interior[id as usize] = true;
        }
    }
    interior
}

/// Put the rims [`absorb_slivers`] cannot reach back where they belong.
///
/// A component with no interior pixel is a strip at most two pixels wide. When its ink is a
/// blend of two of its neighbours' inks, it is the anti-aliased rim between them, and each
/// of its pixels goes to the side it covers more of. [`absorb_slivers`] only reads
/// neighbours with an interior of their own, so the grey rim between small text and the
/// paper, both strips, stayed a face of its own and cut every glyph's outline into pieces
/// at the junctions it made. A hairline still stays: its ink is no blend of what lies either
/// side of it.
pub(crate) fn absorb_rims(
    labels: &mut [u16],
    px: &[[f32; 4]],
    inks: &[[f32; 4]],
    w: usize,
    h: usize,
) {
    let c = components(labels, w, h);
    let interior = interiors(&c, w, h);
    let n_inks = inks.len();
    // The labels each strip borders.
    let mut around: Vec<Vec<u16>> = vec![Vec::new(); c.size.len()];
    for p in 0..w * h {
        let id = c.comp[p];
        if interior[id as usize] {
            continue;
        }
        let (x, y) = (p % w, p / w);
        for q in [
            (x > 0).then(|| p - 1),
            (x + 1 < w).then(|| p + 1),
            (y > 0).then(|| p - w),
            (y + 1 < h).then(|| p + w),
        ]
        .into_iter()
        .flatten()
        {
            let l = labels[q];
            let t = &mut around[id as usize];
            if c.comp[q] != id && (l as usize) < n_inks && !t.contains(&l) {
                t.push(l);
            }
        }
    }
    // Per strip, the two inks it is a rim between: the pair its own ink lies nearest the
    // line between, within the blend tolerance.
    let pair: Vec<Option<(u16, u16)>> = around
        .iter()
        .enumerate()
        .map(|(id, t)| {
            let own = c.label[id] as usize;
            if own >= n_inks || t.len() < 2 {
                return None;
            }
            let mut best: Option<(u16, u16, f32)> = None;
            for (i, &a) in t.iter().enumerate() {
                for &b in &t[i + 1..] {
                    let (lo, hi) = (a.min(b), a.max(b));
                    if let Some((_, d)) = blend_of(inks[own], inks[lo as usize], inks[hi as usize])
                    {
                        let better =
                            best.is_none_or(|(l0, h0, e)| d < e || (d == e && (lo, hi) < (l0, h0)));
                        if d <= BLEND_TOL * BLEND_TOL && better {
                            best = Some((lo, hi, d));
                        }
                    }
                }
            }
            best.map(|(a, b, _)| (a, b))
        })
        .collect();
    for (p, l) in labels.iter_mut().enumerate() {
        if let Some((a, b)) = pair[c.comp[p] as usize] {
            *l = match blend_of(px[p], inks[a as usize], inks[b as usize]) {
                Some((t, _)) if t >= 0.5 => b,
                _ => a,
            };
        }
    }
}

/// Join each component to a larger neighbour whose ink it cannot be told apart from.
///
/// The fast palette keeps two inks closer than [`crate::color::SAME_INK_DE00`] apart when
/// they are far enough apart in OKLab, which is widest near black: its thin-text ink beside
/// the flat black of large type, 1.3 dE00 from it, which
/// tiled every glyph of a masthead's body text into patches of the two, each patch an
/// outline and each corner of one a junction. Components are taken largest first; each
/// joins the neighbour it shares most border with among the larger ones whose (settled)
/// ink is within the threshold of its own, so no pixel's colour moves further than that
/// however the joins chain -- a ramp of close bands is thinned, not flattened, and the
/// ramp pass after this still sees its bands.
pub(crate) fn merge_same_inks(labels: &mut [u16], inks: &[[f32; 4]], w: usize, h: usize) {
    let n_inks = inks.len();
    let same: Vec<bool> = (0..n_inks * n_inks)
        .map(|k| {
            let (ia, ib) = (inks[k / n_inks], inks[k % n_inks]);
            k / n_inks != k % n_inks
                && (ia[3] - ib[3]).abs() < SAME_ALPHA
                && crate::color::de00([ia[0], ia[1], ia[2]], [ib[0], ib[1], ib[2]])
                    < crate::color::SAME_INK_DE00
        })
        .collect();
    if !same.contains(&true) {
        return;
    }
    let c = components(labels, w, h);
    let n = c.size.len();
    // Border shared by each pair of components whose inks are one ink to the eye.
    let mut border: std::collections::HashMap<(u32, u32), u32> = Default::default();
    let mut touch = |a: u32, b: u32| {
        let (la, lb) = (c.label[a as usize] as usize, c.label[b as usize] as usize);
        if a != b && la < n_inks && lb < n_inks && same[la * n_inks + lb] {
            *border.entry((a.min(b), a.max(b))).or_insert(0) += 1;
        }
    };
    for y in 0..h {
        for x in 0..w {
            let p = y * w + x;
            if x + 1 < w {
                touch(c.comp[p], c.comp[p + 1]);
            }
            if y + 1 < h {
                touch(c.comp[p], c.comp[p + w]);
            }
        }
    }
    if border.is_empty() {
        return;
    }
    let mut nbrs: Vec<Vec<(u32, u32)>> = vec![Vec::new(); n];
    let mut pairs: Vec<((u32, u32), u32)> = border.into_iter().collect();
    pairs.sort_unstable();
    for ((a, b), len) in pairs {
        nbrs[a as usize].push((b, len));
        nbrs[b as usize].push((a, len));
    }
    let mut order: Vec<u32> = (0..n as u32).collect();
    order.sort_unstable_by_key(|&k| (std::cmp::Reverse(c.size[k as usize]), k));
    let mut rank = vec![0u32; n];
    for (r, &k) in order.iter().enumerate() {
        rank[k as usize] = r as u32;
    }
    let mut to: Vec<u16> = c.label.clone();
    for &k in &order {
        let own = c.label[k as usize] as usize;
        let best = nbrs[k as usize]
            .iter()
            .filter(|&&(o, _)| rank[o as usize] < rank[k as usize])
            .filter(|&&(o, _)| {
                let t = to[o as usize] as usize;
                t != own && t < n_inks && same[own * n_inks + t]
            })
            .max_by_key(|&&(o, len)| (len, std::cmp::Reverse(rank[o as usize])));
        if let Some(&(o, _)) = best {
            to[k as usize] = to[o as usize];
        }
    }
    for (l, &id) in labels.iter_mut().zip(&c.comp) {
        *l = to[id as usize];
    }
}

/// Largest opacity difference between two inks that can be one ink.
const SAME_ALPHA: f32 = 0.02;

/// Faces: the components of `labels`, as a face id per pixel and each face's label. Past
/// `u16::MAX - 1` faces the rest are folded into face 0, as `regions::split_components`
/// does.
pub(crate) fn faces(labels: &[u16], w: usize, h: usize) -> (Vec<u16>, Vec<usize>) {
    let c = components(labels, w, h);
    let cap = (u16::MAX - 1) as usize;
    let ids: Vec<u16> = c
        .comp
        .iter()
        .map(|&id| if (id as usize) < cap { id as u16 } else { 0 })
        .collect();
    let face_label = c.label.iter().take(cap).map(|&l| l as usize).collect();
    (ids, face_label)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn faces_match_the_flood_fill() {
        let (w, h) = (7, 5);
        let labels: Vec<u16> = (0..w * h)
            .map(|p| {
                let (x, y) = (p % w, p / w);
                u16::from((x + y) % 3 == 0 || (x == 3 && y > 0))
            })
            .collect();
        let (a, fa) = faces(&labels, w, h);
        let (b, fb) = crate::regions::split_components(&labels, w, h);
        assert_eq!(fa.len(), fb.len());
        // The same partition: two pixels share a face in one exactly when in the other.
        for p in 0..w * h {
            for q in 0..w * h {
                assert_eq!(a[p] == a[q], b[p] == b[q]);
            }
        }
    }

    #[test]
    fn a_lone_pixel_takes_its_surroundings() {
        let mut labels = vec![0u16, 0, 0, 0, 1, 0, 0, 0, 2];
        despeckle(&mut labels, 3, 3, 2);
        assert_eq!(labels, vec![0, 0, 0, 0, 0, 0, 0, 0, 0]);
    }

    #[test]
    fn a_rim_of_a_blend_ink_goes_to_the_inks_either_side_and_a_hairline_stays() {
        // White, a one-pixel strip of light grey (ink 2), black, white, a black hairline
        // (ink 1), white.
        let (w, h) = (8, 18);
        let inks = [
            [1.0f32, 1.0, 1.0, 1.0],
            [0.0, 0.0, 0.0, 1.0],
            [0.6, 0.6, 0.6, 1.0],
        ];
        let mut labels = vec![0u16; w * h];
        for x in 0..w {
            labels[4 * w + x] = 2;
            for y in 5..9 {
                labels[y * w + x] = 1;
            }
            labels[13 * w + x] = 1;
        }
        let px: Vec<[f32; 4]> = labels.iter().map(|&l| inks[l as usize]).collect();
        absorb_slivers(&mut labels, &px, &inks, w, h);
        assert!(
            (0..w).all(|x| labels[4 * w + x] == 0),
            "the grey strip is a blend"
        );
        assert!(
            (0..w).all(|x| labels[13 * w + x] == 1),
            "the hairline stays"
        );
    }

    #[test]
    fn the_grey_rim_of_a_thin_stroke_goes_to_the_stroke_and_the_paper() {
        // White paper, a light grey rim row, a two-pixel black stroke (no interior, so
        // `absorb_slivers` cannot use it), another rim row, paper. Rim pixels a little
        // darker than the rim ink go to the stroke side, the rest to the paper.
        let (w, h) = (10, 8);
        let inks = [
            [1.0f32, 1.0, 1.0, 1.0],
            [0.0, 0.0, 0.0, 1.0],
            [0.93, 0.93, 0.93, 1.0],
        ];
        let mut labels = vec![0u16; w * h];
        let mut px = vec![inks[0]; w * h];
        for x in 0..w {
            labels[2 * w + x] = 2;
            labels[5 * w + x] = 2;
            px[2 * w + x] = inks[2];
            px[5 * w + x] = if x < 5 { [0.3, 0.3, 0.3, 1.0] } else { inks[2] };
            for y in 3..5 {
                labels[y * w + x] = 1;
                px[y * w + x] = inks[1];
            }
        }
        let mut hairline = labels.clone();
        absorb_rims(&mut labels, &px, &inks, w, h);
        assert!(
            (0..w).all(|x| labels[2 * w + x] == 0),
            "the rim above is paper"
        );
        assert!(
            (0..5).all(|x| labels[5 * w + x] == 1),
            "darker rim pixels join the stroke"
        );
        assert!((5..w).all(|x| labels[5 * w + x] == 0));
        assert!(
            (0..w).all(|x| (3..5).all(|y| labels[y * w + x] == 1)),
            "the stroke stays"
        );
        // Without the rims, the black stroke on white is no blend of anything and stays.
        for x in 0..w {
            hairline[2 * w + x] = 0;
            hairline[5 * w + x] = 0;
        }
        let before = hairline.clone();
        absorb_rims(&mut hairline, &px, &inks, w, h);
        assert_eq!(hairline, before);
    }

    #[test]
    fn two_inks_one_eye_cannot_tell_apart_become_one_and_a_step_stays() {
        // Left half ink 0, right half ink 1 (1 dE00 away), and a bar of ink 2 (a clearly
        // different grey) across the bottom.
        let (w, h) = (12, 9);
        let inks = [
            [0.02f32, 0.02, 0.02, 1.0],
            [0.035, 0.035, 0.03, 1.0],
            [0.5, 0.5, 0.5, 1.0],
        ];
        assert!(
            crate::color::de00([0.02, 0.02, 0.02], [0.035, 0.035, 0.03])
                < crate::color::SAME_INK_DE00
        );
        let mut labels: Vec<u16> = (0..w * h)
            .map(|p| if p / w >= 7 { 2 } else { u16::from(p % w >= 5) })
            .collect();
        merge_same_inks(&mut labels, &inks, w, h);
        // The larger of the two near-blacks (ink 1, 7 columns) takes the other.
        assert!(labels[..7 * w].iter().all(|&l| l == 1));
        assert!(labels[7 * w..].iter().all(|&l| l == 2));
    }

    #[test]
    fn a_u_shape_is_one_component() {
        // 1 1 . 1
        // 1 . . 1
        // 1 1 1 1
        let labels = vec![1u16, 1, 0, 1, 1, 0, 0, 1, 1, 1, 1, 1];
        let c = components(&labels, 4, 3);
        assert_eq!(c.size.len(), 2);
        assert_eq!(c.comp[0], c.comp[3]);
    }
}
