//! 4-connected components of a label map, found by runs and union-find.
//!
//! The label-map clean-up (despeckle, sliver absorption) used to flood-fill every component
//! and keep a list of every component's pixels, although it acts only on a few small or
//! thin ones: on a 2048 px flat render 99.4 % of pixels continue their left neighbour's run,
//! and the whole image is a few hundred components of which a few hundred pixels matter.
//!
//! This is two-pass connected-component labelling with a union-find over provisional
//! labels, the scheme K. Wu, E. Otoo and K. Suzuki, "Optimizing Two-Pass
//! Connected-Component Labeling Algorithms", *Pattern Analysis and Applications*
//! 12(2):117-135, 2009 (LBNL-59102) optimise, applied to runs rather than pixels: the first
//! pass cuts each row into runs of one label and unites every run with the runs of the same
//! label that overlap it in the row above (4-connectivity); the second resolves the unions.
//! Adapted in two ways. The image is multi-label rather than binary, so "same component"
//! means "same label and connected". And components are numbered in raster order of their
//! first pixel, as the flood fill numbered them: provisional labels are handed out in
//! raster order, a component's first pixel always starts a run with a fresh label, and the
//! union keeps the smaller label as the root, so each root is its component's first run.
//!
//! Member lists are built only for the components a caller selects.

/// The components of a `w x h` label map.
pub(crate) struct Components {
    /// Each pixel's component id, numbered in raster order of the component's first pixel.
    pub(crate) comp: Vec<u32>,
    /// Each component's pixel count.
    pub(crate) size: Vec<u32>,
    /// Every run: first pixel, length, component, in raster order.
    runs: Vec<(u32, u32, u32)>,
}

/// Root of `x` with path halving.
fn find(parent: &mut [u32], mut x: u32) -> u32 {
    while parent[x as usize] != x {
        let p = parent[x as usize];
        parent[x as usize] = parent[p as usize];
        x = p;
    }
    x
}

impl Components {
    /// Label the first `w * h` entries of `labels` (row-major).
    pub(crate) fn of(labels: &[u16], w: usize, h: usize) -> Self {
        let n = w * h;
        // Pass 1: runs with provisional labels, united with the overlapping runs above.
        let mut parent: Vec<u32> = Vec::new();
        let mut runs: Vec<(u32, u32, u32)> = Vec::new();
        let mut run_label: Vec<u16> = Vec::new();
        let mut above = 0..0;
        for y in 0..h {
            let row = y * w;
            let first = runs.len();
            let mut j = above.start;
            let mut x = 0;
            while x < w {
                let l = labels[row + x];
                let s = x;
                while x < w && labels[row + x] == l {
                    x += 1;
                }
                let prov = parent.len() as u32;
                parent.push(prov);
                while j < above.end && {
                    let (ps, pl, _) = runs[j];
                    ps as usize - (row - w) + pl as usize <= s
                } {
                    j += 1;
                }
                let mut k = j;
                while k < above.end && (runs[k].0 as usize - (row - w)) < x {
                    if run_label[k] == l {
                        let (a, b) = (find(&mut parent, prov), find(&mut parent, runs[k].2));
                        let (lo, hi) = (a.min(b), a.max(b));
                        parent[hi as usize] = lo;
                    }
                    k += 1;
                }
                runs.push(((row + s) as u32, (x - s) as u32, prov));
                run_label.push(l);
            }
            above = first..runs.len();
        }
        // Pass 2: roots in increasing order are the components in raster order.
        let mut final_id = vec![0u32; parent.len()];
        let mut next = 0u32;
        for p in 0..parent.len() as u32 {
            let root = find(&mut parent, p);
            final_id[p as usize] = if root == p {
                next += 1;
                next - 1
            } else {
                final_id[root as usize]
            };
        }
        let mut comp = vec![0u32; n];
        let mut size = vec![0u32; next as usize];
        for run in runs.iter_mut() {
            let id = final_id[run.2 as usize];
            run.2 = id;
            let (s, l) = (run.0 as usize, run.1 as usize);
            comp[s..s + l].fill(id);
            size[id as usize] += run.1;
        }
        Components { comp, size, runs }
    }

    /// Number of components.
    pub(crate) fn len(&self) -> usize {
        self.size.len()
    }

    /// Per component: how many of its pixels have all their in-image 4-neighbours in it,
    /// and how many (pixel, neighbour) pairs cross into another component. These are what
    /// [`super::tally_contacts`] counts for one component, for all of them in one pass.
    pub(crate) fn shape(&self, w: usize, h: usize) -> (Vec<u32>, Vec<u32>) {
        let mut interior = vec![0u32; self.len()];
        let mut foreign = vec![0u32; self.len()];
        let comp = &self.comp;
        for y in 0..h {
            for x in 0..w {
                let i = y * w + x;
                let c = comp[i];
                let mut out = 0u32;
                if x > 0 && comp[i - 1] != c {
                    out += 1;
                }
                if x + 1 < w && comp[i + 1] != c {
                    out += 1;
                }
                if y > 0 && comp[i - w] != c {
                    out += 1;
                }
                if y + 1 < h && comp[i + w] != c {
                    out += 1;
                }
                foreign[c as usize] += out;
                interior[c as usize] += u32::from(out == 0);
            }
        }
        (interior, foreign)
    }

    /// The pixels of every component `keep` selects, in raster order.
    pub(crate) fn members(&self, keep: impl Fn(usize) -> bool) -> Members {
        let mut off = vec![0u32; self.len() + 1];
        for c in 0..self.len() {
            off[c + 1] = off[c] + if keep(c) { self.size[c] } else { 0 };
        }
        let mut fill = off.clone();
        let mut px = vec![0usize; off[self.len()] as usize];
        for &(s, l, c) in &self.runs {
            let c = c as usize;
            if off[c + 1] > off[c] {
                let at = fill[c] as usize;
                for (k, p) in (s as usize..(s + l) as usize).enumerate() {
                    px[at + k] = p;
                }
                fill[c] += l;
            }
        }
        Members { off, px }
    }
}

/// Member lists of selected components.
pub(crate) struct Members {
    off: Vec<u32>,
    px: Vec<usize>,
}

impl Members {
    /// The pixels of component `c` (empty when it was not selected).
    pub(crate) fn of(&self, c: usize) -> &[usize] {
        &self.px[self.off[c] as usize..self.off[c + 1] as usize]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::regions::label_components;

    fn check(labels: &[u16], w: usize, h: usize) {
        let (comp, members) = label_components(labels, w, h);
        let c = Components::of(labels, w, h);
        assert_eq!(c.comp, comp, "{w}x{h}");
        assert_eq!(c.len(), members.len());
        let all = c.members(|_| true);
        for (id, group) in members.iter().enumerate() {
            assert_eq!(c.size[id] as usize, group.len());
            let mut g = group.clone();
            g.sort_unstable();
            assert_eq!(all.of(id), &g[..]);
            // The per-component counts equal the per-group tally.
            let mut contacts = vec![0usize; 1 << 16];
            let (interior, foreign) = crate::regions::tally_contacts(
                group,
                id as u32,
                &comp,
                labels,
                w,
                h,
                &mut contacts,
            );
            let (ci, cf) = c.shape(w, h);
            assert_eq!((ci[id] as usize, cf[id] as usize), (interior, foreign));
        }
    }

    #[test]
    fn components_equal_the_flood_fill_on_random_and_degenerate_maps() {
        let mut s = 0x9e37_79b9_7f4a_7c15u64;
        let mut next = move |n: u64| {
            s ^= s << 13;
            s ^= s >> 7;
            s ^= s << 17;
            s % n
        };
        check(&[], 0, 0);
        check(&[3], 1, 1);
        check(&[1, 1, 2, 2, 1, 1], 6, 1);
        check(&[1, 1, 2, 2, 1, 1], 1, 6);
        for _ in 0..300 {
            let (w, h) = (1 + next(24) as usize, 1 + next(24) as usize);
            let k = 1 + next(4) as u16;
            // Blobs of a few labels with speckle, so runs meet in every way.
            let mut labels = vec![0u16; w * h];
            for y in 0..h {
                for x in 0..w {
                    labels[y * w + x] = if next(7) == 0 {
                        next(k as u64) as u16
                    } else {
                        (((x / 3) ^ (y / 2)) as u16) % k
                    };
                }
            }
            check(&labels, w, h);
        }
        // A spiral: one component whose runs join only far down the image.
        let (w, h) = (9, 9);
        let mut sp = vec![0u16; w * h];
        for y in 0..h {
            for x in 0..w {
                let ring = x.min(y).min(w - 1 - x).min(h - 1 - y);
                sp[y * w + x] = (ring % 2) as u16;
            }
        }
        check(&sp, w, h);
    }
}
