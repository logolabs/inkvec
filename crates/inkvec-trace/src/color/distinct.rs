//! The image as its distinct colours: each pixel's colour id, where each colour's sampled
//! pixels sit, and the palette's spatial tests evaluated through them.
//!
//! # Why
//!
//! Every per-pixel quantity the palette reads -- OKLab, sRGB, linear light, the distance to a
//! candidate or to the nearest accepted ink -- is a pure function of the pixel's colour bits.
//! Vector art repeats a few colours many times: the 246-icon screen set has 232 distinct
//! colours in 16 384 pixels at the median (70 pixels per colour), a 2048 px flat render 52 in
//! 4.2 million. So every such quantity is computed once per distinct colour and read back
//! through the pixel's id, and the result is bit-identical because it is the same f32
//! function of the same f32 bits.
//!
//! This is the unique-colour reduction of Celebi's weighted k-means colour quantiser:
//! M. E. Celebi, "Improving the Performance of K-Means for Color Quantization", *Image and
//! Vision Computing* 29(4):260-271, 2011, doi:10.1016/j.imavis.2010.10.002
//! (arXiv:1101.0395) -- sample the pixels with unique colours through a hash table and weight
//! each by its frequency. Adapted: the weights here are the counts of *sampled* pixels
//! (the palette's statistical passes visit every `stride_px`-th pixel), and the means that
//! become inks are still summed in pixel order, because floating-point sums over a histogram
//! are not provably the sums over the pixels.
//!
//! The spatial tests (interior, straddle) need pixel *positions* as well as colours, so each
//! colour also keeps the list of its sampled pixels, and a per-colour decision is mapped back
//! onto the pixels through the id image. That is histogram backprojection: M. J. Swain and
//! D. H. Ballard, "Color Indexing", *IJCV* 7(1):11-32, 1991 -- replace each pixel by the value
//! its colour's histogram bin holds.
//!
//! # The claimed set, computed once per candidate
//!
//! A candidate `c` *claims* the pixels strictly nearer to it than to every ink accepted so far:
//! the bichromatic reverse-nearest-neighbour set of `c` against the accepted inks, weighted by
//! pixel count. F. Korn and S. Muthukrishnan, "Influence Sets Based on Reverse Nearest Neighbor
//! Queries", SIGMOD 2000, pp. 201-212, doi:10.1145/335191.335415; the weighted-client form is
//! R. C.-W. Wong, M. T. Özsu, P. S. Yu, A. W.-C. Fu and L. Liu, "Efficient Method for
//! Maximizing Bichromatic Reverse Nearest Neighbor", PVLDB 2(1), 2009. Nothing changes the
//! nearest-ink distances until a candidate is accepted, so the set is the same for the claim
//! count, the spread, the interior test and every straddle pair: [`Claim`] holds it once.
//!
//! Not from the literature: each claimed pixel's 3x3 neighbourhood is gathered once per
//! candidate as colour ids ([`Neighbourhoods`]), so a blend pair classifies each colour once
//! and each pixel reads nine bytes, instead of projecting nine pixels onto the pair's axis.

use std::collections::HashMap;
use std::hash::{BuildHasherDefault, Hash, Hasher};

use rayon::prelude::*;

/// A multiplicative word hash (the one `rustc-hash`'s `FxHasher` uses). The keys are raw
/// float bits, so a fast non-cryptographic mix is all that is needed; the table exists only
/// to number the colours, and the numbering does not reach any result.
#[derive(Default, Clone, Copy)]
pub(crate) struct WordHasher(u64);

impl WordHasher {
    const SEED: u64 = 0x51_7c_c1_b7_27_22_0a_95;
    #[inline]
    fn add(&mut self, x: u64) {
        self.0 = (self.0.rotate_left(5) ^ x).wrapping_mul(Self::SEED);
    }
}

impl Hasher for WordHasher {
    fn write(&mut self, bytes: &[u8]) {
        for &b in bytes {
            self.add(u64::from(b));
        }
    }
    fn write_u32(&mut self, x: u32) {
        self.add(u64::from(x));
    }
    fn write_u64(&mut self, x: u64) {
        self.add(x);
    }
    fn write_usize(&mut self, x: usize) {
        self.add(x as u64);
    }
    fn finish(&self) -> u64 {
        self.0
    }
}

/// A hash map keyed by float bits.
pub(crate) type BitsMap<K, V> = HashMap<K, V, BuildHasherDefault<WordHasher>>;

/// Pixels numbered per parallel task when the ids are built.
const ID_CHUNK: usize = 1 << 16;

/// Each pixel's colour id, and one pixel carrying each colour.
///
/// Ids are numbered in order of first occurrence in the image, so the numbering is the same
/// on every run and every thread count; nothing downstream depends on it beyond that.
pub(crate) struct ColourIds {
    /// Pixel `i`'s colour id.
    pub(crate) cid: Vec<u32>,
    /// The first pixel carrying each id, in id order.
    pub(crate) reps: Vec<usize>,
}

impl ColourIds {
    /// Ids of opaque colours: the three channels' bits.
    pub(crate) fn of_rgb(rgb: &[[f32; 3]]) -> Self {
        Self::build(rgb.len(), |i| {
            let c = rgb[i];
            (
                u64::from(c[0].to_bits()) | u64::from(c[1].to_bits()) << 32,
                u64::from(c[2].to_bits()),
            )
        })
    }

    /// Ids of colours with an opacity: the three channels' bits and the alpha's. `n` is the
    /// shorter of the two slices, as every four-channel pass in `native` zips them.
    pub(crate) fn of_rgba(rgb: &[[f32; 3]], alpha: &[f32]) -> Self {
        Self::build(rgb.len().min(alpha.len()), |i| {
            let c = rgb[i];
            (
                u64::from(c[0].to_bits()) | u64::from(c[1].to_bits()) << 32,
                u64::from(c[2].to_bits()) | u64::from(alpha[i].to_bits()) << 32,
            )
        })
    }

    /// Number of distinct colours.
    pub(crate) fn len(&self) -> usize {
        self.reps.len()
    }

    /// Number the `n` pixels by `key`. Each task numbers a chunk with its own table (a
    /// pixel equal to its left neighbour, 91 % of an icon's pixels, skips the table); the
    /// chunks' tables are then merged in chunk order, which reproduces the numbering a
    /// single left-to-right pass would give.
    pub(crate) fn build<K>(n: usize, key: impl Fn(usize) -> K + Sync) -> Self
    where
        K: Hash + Eq + Copy + Send + Sync,
    {
        type Local<K> = (Vec<K>, Vec<usize>, Vec<u32>);
        let chunks: Vec<Local<K>> = (0..n.div_ceil(ID_CHUNK))
            .into_par_iter()
            .map(|c| {
                let (lo, hi) = (c * ID_CHUNK, ((c + 1) * ID_CHUNK).min(n));
                let mut map: BitsMap<K, u32> = BitsMap::default();
                let (mut keys, mut first) = (Vec::new(), Vec::new());
                let mut local = Vec::with_capacity(hi - lo);
                let mut prev: Option<(K, u32)> = None;
                for i in lo..hi {
                    let k = key(i);
                    let id = match prev {
                        Some((pk, pid)) if pk == k => pid,
                        _ => {
                            let id = *map.entry(k).or_insert_with(|| {
                                keys.push(k);
                                first.push(i);
                                (keys.len() - 1) as u32
                            });
                            prev = Some((k, id));
                            id
                        }
                    };
                    local.push(id);
                }
                (keys, first, local)
            })
            .collect();
        let mut map: BitsMap<K, u32> = BitsMap::default();
        let mut reps = Vec::new();
        let remaps: Vec<Vec<u32>> = chunks
            .iter()
            .map(|(keys, first, _)| {
                keys.iter()
                    .zip(first)
                    .map(|(&k, &f)| {
                        *map.entry(k).or_insert_with(|| {
                            reps.push(f);
                            (reps.len() - 1) as u32
                        })
                    })
                    .collect()
            })
            .collect();
        let mut cid = vec![0u32; n];
        cid.par_chunks_mut(ID_CHUNK)
            .zip(chunks.par_iter().zip(remaps.par_iter()))
            .for_each(|(out, ((_, _, local), remap))| {
                for (o, &l) in out.iter_mut().zip(local) {
                    *o = remap[l as usize];
                }
            });
        ColourIds { cid, reps }
    }
}

/// Distinct colours at or above which a per-colour pass runs on every core.
pub(crate) const PAR_COLOURS: usize = 1 << 14;

/// Most pixels the spread's median is taken over: pixel `i` votes when `i` is a multiple of
/// `max(n / SPREAD_SAMPLES, 1)`, which bounds the sort at any image size.
const SPREAD_SAMPLES: usize = 8192;

/// The image's geometry and its sampled pixels, grouped by colour.
///
/// The palette's statistical passes visit pixels `0, s, 2s, ...` (`s = stride_px`,
/// [`super::stat_stride`]). Here the visited pixels are stored per colour (a compressed row
/// per id), so a pass over "the visited pixels a candidate claims" touches exactly those.
pub(crate) struct DistinctImage<'a> {
    /// Pixel `i`'s colour id.
    pub(crate) cid: &'a [u32],
    /// Row width.
    pub(crate) width: usize,
    /// Row count.
    pub(crate) height: usize,
    /// Number of pixels the claim visits (`0..n`; the spatial tests stop at `width * height`).
    n: usize,
    /// Stride of the statistical passes.
    pub(crate) stride_px: usize,
    /// Visited pixels per colour, and the compressed rows of their indices.
    count: Vec<u32>,
    off: Vec<u32>,
    px: Vec<u32>,
    /// Per colour, the visited pixels that also fall on the spread's sub-sample
    /// (`i % max(n / 8192, 1) == 0`).
    spread_count: Vec<u32>,
}

impl<'a> DistinctImage<'a> {
    /// Index `ids` (over `n = ids.cid.len()` pixels) for a `width x height` image, visiting
    /// every [`super::stat_stride`]-th pixel.
    pub(crate) fn new(ids: &'a ColourIds, width: usize, height: usize) -> Self {
        let stride_px = super::stat_stride(ids.cid.len(), width);
        Self::with_stride(ids, width, height, stride_px)
    }

    /// [`Self::new`] with the visiting stride given (at least 1).
    pub(crate) fn with_stride(
        ids: &'a ColourIds,
        width: usize,
        height: usize,
        stride_px: usize,
    ) -> Self {
        let stride_px = stride_px.max(1);
        let n = ids.cid.len();
        let d = ids.len();
        let spread_stride = (n / SPREAD_SAMPLES).max(1);
        let mut count = vec![0u32; d];
        let mut spread_count = vec![0u32; d];
        for i in (0..n).step_by(stride_px) {
            let id = ids.cid[i] as usize;
            count[id] += 1;
            if i % spread_stride == 0 {
                spread_count[id] += 1;
            }
        }
        let mut off = vec![0u32; d + 1];
        for k in 0..d {
            off[k + 1] = off[k] + count[k];
        }
        let mut fill = off.clone();
        let mut px = vec![0u32; off[d] as usize];
        for i in (0..n).step_by(stride_px) {
            let id = ids.cid[i] as usize;
            px[fill[id] as usize] = i as u32;
            fill[id] += 1;
        }
        DistinctImage {
            cid: &ids.cid,
            width,
            height,
            n,
            stride_px,
            count,
            off,
            px,
            spread_count,
        }
    }

    /// Number of distinct colours.
    pub(crate) fn colours(&self) -> usize {
        self.count.len()
    }

    /// Number of pixels.
    pub(crate) fn pixels(&self) -> usize {
        self.n
    }

    /// Whether the spatial tests have a full `width x height` grid to read.
    pub(crate) fn has_geometry(&self) -> bool {
        self.width != 0 && self.height != 0 && self.n >= self.width * self.height
    }

    /// The visited pixels of colour `d` that lie on the `width x height` grid.
    fn visited(&self, d: usize) -> impl Iterator<Item = usize> + '_ {
        let grid = self.width * self.height;
        self.px[self.off[d] as usize..self.off[d + 1] as usize]
            .iter()
            .map(|&i| i as usize)
            .take_while(move |&i| i < grid)
    }

    /// Mark the colours candidate `c` claims: those strictly nearer to `c` (`dist(d)`) than
    /// to the nearest accepted ink (`nearest[d]`). Returns the claim in pixels, the count of
    /// claimed visited pixels scaled back up by the stride (the shipped estimate of a full
    /// count).
    pub(crate) fn claim(
        &self,
        claim: &mut Claim,
        nearest: &[f32],
        dist: impl Fn(usize) -> f32 + Sync,
    ) -> usize {
        let visited = if self.colours() >= PAR_COLOURS {
            claim
                .dist
                .par_iter_mut()
                .zip(claim.claimed.par_iter_mut())
                .enumerate()
                .map(|(d, (dd, cl))| {
                    *dd = dist(d);
                    *cl = *dd < nearest[d];
                    if *cl {
                        self.count[d] as usize
                    } else {
                        0
                    }
                })
                .sum::<usize>()
        } else {
            let mut visited = 0usize;
            for d in 0..self.colours() {
                let dd = dist(d);
                claim.dist[d] = dd;
                claim.claimed[d] = dd < nearest[d];
                if claim.claimed[d] {
                    visited += self.count[d] as usize;
                }
            }
            visited
        };
        visited * self.stride_px
    }

    /// The lower median distance from the candidate of its claimed *members*: claimed
    /// visited pixels within `tol` of it, on the spread sub-sample. 0 when there are none.
    /// A weighted median over colours; the same element as sorting the pixels' distances.
    ///
    /// Members, not territory: territory is whatever has no closer ink yet, which for the
    /// first candidate is the whole image, and a spread over that rejected every colour after
    /// the first (screen set 0.4328 -> 1.2461 before it was restricted to `tol`). The median,
    /// not the mean: an anti-aliased edge puts a ramp of blend pixels inside `tol` on a clean
    /// image, and a mean is pulled up by them (2 % on the screen set). This is the scale at
    /// which the image itself says "these pixels are the same colour"; a global noise estimate
    /// cannot supply it, because an icon is mostly empty and its median Laplacian is zero.
    pub(crate) fn spread(&self, claim: &Claim, tol: f32) -> f32 {
        let mut items: Vec<(f32, u32)> = (0..self.colours())
            .filter(|&d| claim.claimed[d] && claim.dist[d] < tol && self.spread_count[d] > 0)
            .map(|d| (claim.dist[d], self.spread_count[d]))
            .collect();
        weighted_lower_median(&mut items)
    }

    /// One step of 4-neighbour erosion of the claimed set, as a fraction of the claimed
    /// visited pixels: `|{i claimed : its in-image 4-neighbours are claimed}| / |claimed|`.
    /// A border pixel has no neighbour outside the image to disqualify it: the outside
    /// counts as claimed, so a region touching the edge is not penalised. 1 without a full
    /// grid (the caller then falls back on its other evidence), 0 when nothing is claimed.
    ///
    /// The set is the candidate's claim -- the pixels it would take from the palette as it
    /// stands -- and not a ball around it. A ball is the obvious choice and it is wrong: an
    /// anti-aliased colour sits close to one end of its ramp, so a ball around it swallows
    /// the solid region as well as the band, and the band then measures as solid (tried: the
    /// green-circle case went from 29 faces to 40). Near zero for an anti-aliased band, near
    /// one for a filled region.
    pub(crate) fn interior(&self, claim: &Claim) -> f32 {
        if !self.has_geometry() {
            return 1.0;
        }
        let (w, h) = (self.width, self.height);
        let mask = |j: usize| claim.claimed[self.cid[j] as usize];
        let (mut total, mut interior) = (0u32, 0u32);
        for d in (0..self.colours()).filter(|&d| claim.claimed[d]) {
            for i in self.visited(d) {
                let (x, y) = (i % w, i / w);
                let ok = (x == 0 || mask(i - 1))
                    && (x + 1 == w || mask(i + 1))
                    && (y == 0 || mask(i - w))
                    && (y + 1 == h || mask(i + w));
                total += 1;
                interior += u32::from(ok);
            }
        }
        if total == 0 {
            return 0.0;
        }
        interior as f32 / total as f32
    }

    /// Every claimed visited pixel's 3x3 neighbourhood as colour ids, gathered once per
    /// candidate for all its blend pairs.
    pub(crate) fn neighbourhoods(&self, claim: &Claim) -> Neighbourhoods {
        let outside = self.colours() as u32;
        let mut hoods = Neighbourhoods {
            ids: Vec::new(),
            colours: Vec::new(),
            outside,
            geometry: self.has_geometry(),
        };
        if !hoods.geometry {
            return hoods;
        }
        let (w, h) = (self.width, self.height);
        let claimed: usize = (0..self.colours())
            .filter(|&d| claim.claimed[d])
            .map(|d| self.count[d] as usize)
            .sum();
        hoods.ids.reserve(9 * claimed);
        let mut seen = vec![false; self.colours() + 1];
        seen[outside as usize] = true;
        for d in (0..self.colours()).filter(|&d| claim.claimed[d]) {
            for i in self.visited(d) {
                let (x, y) = (i % w, i / w);
                for ny in [y.wrapping_sub(1), y, y + 1] {
                    for nx in [x.wrapping_sub(1), x, x + 1] {
                        let id = if nx < w && ny < h {
                            self.cid[ny * w + nx]
                        } else {
                            outside
                        };
                        if !seen[id as usize] {
                            seen[id as usize] = true;
                            hoods.colours.push(id);
                        }
                        hoods.ids.push(id);
                    }
                }
            }
        }
        hoods
    }
}

/// The colours a candidate claims, and each colour's distance from it.
pub(crate) struct Claim {
    /// Per colour: strictly nearer to the candidate than to every accepted ink.
    pub(crate) claimed: Vec<bool>,
    /// Per colour: distance to the candidate (read only where claimed).
    pub(crate) dist: Vec<f32>,
}

impl Claim {
    /// Room for `colours` distinct colours.
    pub(crate) fn new(colours: usize) -> Self {
        Claim {
            claimed: vec![false; colours],
            dist: vec![0.0; colours],
        }
    }
}

/// The element at index `total / 2` of the multiset `items` (value, multiplicity) in
/// ascending order: the lower median `d_in[d_in.len() / 2]` of the pixels' values, taken
/// per distinct value. 0 for an empty multiset. Values must be comparable (no NaN).
pub(crate) fn weighted_lower_median(items: &mut [(f32, u32)]) -> f32 {
    let total: u64 = items.iter().map(|&(_, m)| u64::from(m)).sum();
    if total == 0 {
        return 0.0;
    }
    items.sort_unstable_by(|a, b| a.0.total_cmp(&b.0));
    let target = total / 2;
    let mut seen = 0u64;
    for &(v, m) in items.iter() {
        seen += u64::from(m);
        if seen > target {
            return v;
        }
    }
    items[items.len() - 1].0
}

/// The straddle test's pixels: each claimed visited pixel's nine neighbourhood ids (an
/// out-of-image neighbour is the id `outside`, whose side is always 0), and every colour
/// that occurs among them.
pub(crate) struct Neighbourhoods {
    /// Nine ids per claimed pixel, row by row.
    ids: Vec<u32>,
    /// Every colour that occurs in `ids`, `outside` excluded.
    colours: Vec<u32>,
    /// The id standing for "outside the image": the number of distinct colours.
    outside: u32,
    /// Whether the image had a full grid to read.
    geometry: bool,
}

impl Neighbourhoods {
    /// The straddle fraction along one blend axis. `side(d)` says where colour `d` sits on
    /// it: bit 0 set when it is below the candidate by more than the step, bit 1 when above.
    /// A pixel straddles when its neighbourhood has both. `scratch` must have one entry per
    /// distinct colour plus one; only the colours that occur here are written and read.
    ///
    /// Returns `straddling / claimed`, and 1 when nothing is claimed, as the per-pixel test
    /// did. The caller has already returned 0 for a missing grid or a degenerate axis.
    pub(crate) fn straddle(&self, side: impl Fn(usize) -> u8, scratch: &mut [u8]) -> f32 {
        let total = (self.ids.len() / 9) as u32;
        if total == 0 {
            return 1.0;
        }
        for &d in &self.colours {
            scratch[d as usize] = side(d as usize);
        }
        scratch[self.outside as usize] = 0;
        let (hoods, _) = self.ids.as_chunks::<9>();
        let straddle = hoods
            .iter()
            .filter(|hood| hood.iter().fold(0u8, |acc, &d| acc | scratch[d as usize]) == 3)
            .count() as u32;
        straddle as f32 / total as f32
    }

    /// Whether the image had a full grid (the straddle test's precondition).
    pub(crate) fn has_geometry(&self) -> bool {
        self.geometry
    }
}

/// Where a colour sits along a blend axis, as the straddle test classifies it: bit 0 when
/// `t < lo`, bit 1 when `t > hi`.
#[inline]
pub(crate) fn side_of(t: f32, lo: f32, hi: f32) -> u8 {
    u8::from(t < lo) | u8::from(t > hi) << 1
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_number_colours_by_first_occurrence_across_chunks() {
        // Three chunks' worth of pixels cycling through five colours in a shifted order.
        let n = 3 * ID_CHUNK + 17;
        let col = |i: usize| [((i / 7 + 3) % 5) as f32, 0.5, (i % 2) as f32];
        let rgb: Vec<[f32; 3]> = (0..n).map(col).collect();
        let ids = ColourIds::of_rgb(&rgb);
        // Reference: one sequential pass.
        let mut seen: Vec<[u32; 3]> = Vec::new();
        for (i, c) in rgb.iter().enumerate() {
            let bits = c.map(f32::to_bits);
            let id = match seen.iter().position(|&s| s == bits) {
                Some(p) => p,
                None => {
                    seen.push(bits);
                    assert_eq!(ids.reps[seen.len() - 1], i);
                    seen.len() - 1
                }
            };
            assert_eq!(ids.cid[i] as usize, id);
        }
        assert_eq!(ids.len(), seen.len());
    }

    #[test]
    fn ids_tell_signed_zeros_and_alphas_apart() {
        let rgb = [[0.0, 0.0, 0.0], [-0.0, 0.0, 0.0], [0.0, 0.0, 0.0]];
        let ids = ColourIds::of_rgb(&rgb);
        assert_eq!(ids.cid, vec![0, 1, 0]);
        let ids = ColourIds::of_rgba(&rgb, &[1.0, 1.0, 0.5]);
        assert_eq!(ids.cid, vec![0, 1, 2]);
        // The shorter slice decides the pixel count.
        assert_eq!(ColourIds::of_rgba(&rgb, &[1.0]).cid.len(), 1);
        assert_eq!(ColourIds::of_rgb(&[]).len(), 0);
    }

    #[test]
    fn weighted_median_is_the_sorted_pixels_lower_median() {
        let mut rng = 7u64;
        for _ in 0..500 {
            let k = (rng % 9) as usize + 1;
            let mut items = Vec::new();
            let mut pixels = Vec::new();
            for _ in 0..k {
                rng = rng
                    .wrapping_mul(6364136223846793005)
                    .wrapping_add(1442695040888963407);
                let v = ((rng >> 33) % 13) as f32 * 0.25;
                let m = ((rng >> 20) % 4) as u32;
                items.push((v, m));
                pixels.extend(std::iter::repeat_n(v, m as usize));
            }
            pixels.sort_by(|a, b| a.partial_cmp(b).unwrap());
            let want = if pixels.is_empty() {
                0.0
            } else {
                pixels[pixels.len() / 2]
            };
            assert_eq!(weighted_lower_median(&mut items), want);
        }
    }

    #[test]
    fn side_bits_follow_the_strict_comparisons() {
        assert_eq!(side_of(0.1, 0.2, 0.8), 1);
        assert_eq!(side_of(0.9, 0.2, 0.8), 2);
        assert_eq!(side_of(0.5, 0.2, 0.8), 0);
        assert_eq!(side_of(0.2, 0.2, 0.8), 0);
        assert_eq!(side_of(0.8, 0.2, 0.8), 0);
        assert_eq!(side_of(f32::NAN, 0.2, 0.8), 0);
    }
}
