//! Fast mode's palette and labels, in two passes over the pixels.
//!
//! Quality mode recovers inks by minimum description length, with spatial evidence tests for
//! every candidate; it is the single most expensive stage of its front end. This is the
//! cheap version of the same idea. Every pixel is binned (15-bit colour, or 12-bit colour
//! and 4-bit opacity for an image traced with its transparency), and a bin counts as
//! evidence for an ink only through its *flat* pixels -- those whose four neighbours fall in
//! the same bin. An anti-aliased rim is one pixel wide and has almost none, so blends
//! between inks never become inks; a filled region of any size has plenty. Bins are taken in
//! order of flat pixels. One joins the nearest accepted ink when it lies within
//! `merge_distance` of it *and* next to one of that ink's bins (one ink spread over
//! neighbouring bins by noise), or within [`SAME_INK`] outright; otherwise it founds an
//! ink of its own, up to `max_colors`. Two flat colours a few levels apart, in bins that do
//! not touch, stay two inks.
//!
//! Colours are compared as the transparency-aware module compares them
//! ([`crate::native::Ink2`]): over white and over a second ground, so white paint and the
//! clear ground are as far apart as white and black. For an opaque image that is plain OKLab.
//!
//! Labelling is then per bin, not per pixel: each bin maps to its nearest ink once. A pixel
//! whose bin is not itself an ink colour (a blend) takes the nearest of the inks its
//! ink-coloured neighbours carry, or its own nearest ink when it is a blend of that ink and
//! one of theirs (a thin stroke over its ground) -- never an ink its pixels are not made of.
//!
//! Stage 1 of the Fast pipeline. In: the image over white as sRGB 0..1 per pixel, and the
//! source opacity when transparency is traced natively. Out: a [`Palette`] (sRGB, OKLab,
//! opacity and pixel share per ink) and one ink index per pixel. Called once per trace from
//! [`super::front`].
//!
//! # In the field's terms
//!
//! * The histogram is a **colour coherence vector** in its local form: per colour bucket,
//!   the pixels whose 4-cross stays in the bucket (Pass, Zabih & Miller 1996 count coherence
//!   by connected-component size instead).
//! * Founding inks is **leader clustering** (Hartigan 1975) over weighted bins taken in
//!   popularity order, working on distinct colours with counts as Celebi (2011) does.
//! * The per-bin nearest-ink table is an **inverse colour map** (Thomas 1991) restricted to
//!   occupied cells, and labelling sure pixels through it is **histogram backprojection**
//!   (Swain & Ballard 1991).
//! * The parallel histogram is a **privatised generalised histogram** (Podlozhnyuk 2007;
//!   Henriksen et al. 2020), computed on **runs** (Breuel 2007).
//!
//! Full citations sit on the functions that use each method.
//!
//! # Data layout
//!
//! * `rgb` (`[f32; 3]` per pixel) and `alpha` (`f32` per pixel, native path only) are read
//!   in place, row-major, `p = y · w + x`; a pixel's four-vector is [`pixel`].
//! * `keys`: one `u16` bin key per pixel ([`Grid::key`]), row-major.
//! * [`Bins`]: statistics per *occupied* bin, reached from a key through a 65 536-entry slot
//!   table (a sparse set); B, the number of occupied bins, is 16 at the median 128 px icon
//!   and at most 610 on the 2048 px set.
//! * A **run** is a maximal horizontal stretch of one row with one key (or, while keying, one
//!   colour). Runs are found on the fly and never stored. On the 2048 px set 99.56 % of
//!   pixels continue their left neighbour's run (median), 92 % on the 128 px screen set.
//! * `lut`: per key, the bin's nearest ink and whether the bin *is* that ink ("sure").
//!
//! # The passes
//!
//! 1. **Keys and histogram** ([`keys_and_histogram`]): one pass over row bands, in parallel
//!    on a large image. Each band keys its rows once per colour run ([`key_rows`]), then
//!    walks key runs counting pixels, paired and flat pixels and their f64 colour sums
//!    ([`histogram_rows`]); band histograms are merged. Merging is exact because 8-bit
//!    values make every such sum exact ([`in_exact_set`]); anything else is counted
//!    serially in raster order.
//! 2. **Inks** ([`found_inks`], then [`thin_inks`]) from the O(B) bin table, then opacity
//!    snapping. Microscopic: at most 36 candidates and 36 inks measured.
//! 3. **Lookup table**: each occupied bin's nearest ink, once.
//! 4. **Labels** ([`label_rows`]): sure pixels straight from the table, the rest (0.29 % at
//!    2048 px, 4.4 % at 128 px) through the neighbourhood rule ([`Blends::blend_label`]);
//!    ink shares counted per label run ([`ink_shares`]).
//!
//! Below [`PARALLEL_MIN_PIXELS`] everything runs on the calling thread.
//!
//! # Exactness
//!
//! This module was rewritten for speed with its output held fixed: every ink to the bit
//! and every label identical to the implementation shipped at 55ee4e0, which the tests keep
//! as an oracle (`palette/reference.rs`) and compare on degenerate, 8-bit, native-alpha
//! and resampled images. Each function that replaced an old one says in its comment why its
//! result is the same. Measured at 2048 px before the rewrite: 42.6 ms, 99 % of it in the
//! four-channel copy (6.9), the keys (4.3), the serial histogram (17.8), labelling (3.3) and
//! an unread running share (9.8); the decisions themselves cost 0.16 ms.
//!
//! # Considered and not used
//!
//! * Accelerated nearest-ink search (Elkan's and Hamerly's triangle-inequality k-means
//!   bounds): the lookup table needs at most 11 590 distances on any measured image, 0.05 ms.
//! * A distance transform (Rosenfeld & Pfaltz 1966) for a blend pixel's nearest sure
//!   neighbours: the first ring already holds one for every blend pixel of the median image.
//! * Memoising blend labels by (colour, neighbour inks): exact, and 0.66 % of blend pixels
//!   are distinct at 2048 px, but once sure pixels are labelled inline the blend work is too
//!   small to repay a hash table.
//! * VTracer's clustering (visioncortex `color_clusters`, <https://github.com/visioncortex/vtracer>):
//!   merges same-colour neighbours into clusters with running sums and has no global
//!   palette; adopting it would change the output, and this rewrite had to keep it.

use crate::color::{rgb_to_oklab, Palette};
use crate::native::{over_black, snap_alpha, Ink2, OPAQUE};

/// The palette as it shipped before the exact speed-ups, kept verbatim as the tests' oracle.
#[cfg(test)]
mod reference;

/// Bins: 16-bit keys.
const BINS: usize = 1 << 16;
/// Flat pixels a bin needs before it can found an ink.
const MIN_FLAT: u32 = 3;
/// Inks this close (OKLab) are one ink whether or not their bins touch.
const SAME_INK: f32 = 0.012;
/// Most candidates [`thin_inks`] weighs (the blend test is cubic in them).
const MAX_THIN_CANDIDATES: usize = 48;
/// Paired pixels a bin needs to be a thin ink: a stroke 8 px long.
const MIN_PAIRED: u32 = 8;
/// And the share of the image they must be: 8 px on a 128 px icon, 2100 on 2048 px.
const THIN_SHARE: f32 = 0.0005;

/// How pixels are binned: bits per colour channel and for opacity.
///
/// An opaque image uses 5 bits per sRGB channel and none for opacity (15-bit keys); an
/// image traced with its transparency uses 4 + 4 + 4 colour bits and 4 opacity bits. Both
/// fit the 16-bit key space of [`BINS`].
#[derive(Clone, Copy)]
struct Grid {
    bits: u32,
    alpha_bits: u32,
}

impl Grid {
    /// The bin of a colour `c = [r, g, b, a]` (sRGB 0..1 and opacity 0..1).
    ///
    /// Each channel is clamped to 0..1 and rounded to the nearest of `2^bits` levels,
    /// `q(v) = round(v · (2^bits − 1))` ([`level`]); the key packs `q(r) q(g) q(b)` from the
    /// most significant end, followed by `q(a)` on `alpha_bits` when opacity is binned.
    /// Rounding (not truncation) puts a pure colour at the centre of its bin, so noise
    /// around it spreads into both neighbours evenly. A NaN channel lands in level 0.
    #[inline]
    fn key(self, c: [f32; 4]) -> usize {
        let (b, ab) = (self.bits, self.alpha_bits);
        let mut k = (level(c[0], b) << (2 * b)) | (level(c[1], b) << b) | level(c[2], b);
        if ab > 0 {
            k = (k << ab) | level(c[3], ab);
        }
        k
    }

    /// The inverse of [`Grid::key`] on the quantised levels: `[q(r), q(g), q(b), q(a)]`,
    /// with `q(a) = 0` when opacity is not binned. Signed, so that [`Grid::touch`] can
    /// subtract levels.
    fn parts(self, k: usize) -> [isize; 4] {
        let (b, ab) = (self.bits, self.alpha_bits);
        let m = (1usize << b) - 1;
        let a = k & ((1usize << ab) - 1);
        let c = k >> ab;
        [
            ((c >> (2 * b)) & m) as isize,
            ((c >> b) & m) as isize,
            (c & m) as isize,
            a as isize,
        ]
    }

    /// Two bins are neighbours when no channel differs by more than one step.
    fn touch(self, a: usize, b: usize) -> bool {
        let (p, q) = (self.parts(a), self.parts(b));
        p.iter().zip(q).all(|(x, y)| (x - y).abs() <= 1)
    }
}

/// The largest f32 below one half, `0.5 − 2⁻²⁵`: the addend that makes truncation round
/// half away from zero (see [`level`]).
const BELOW_HALF: f32 = 0.5 - f32::EPSILON / 4.0;

/// The level `q(v) = round(clamp(v, 0, 1) · T)` of one channel on a grid of `T = 2^bits − 1`
/// steps, rounding half away from zero, as `f32::round` does. `bits` is 4 or 5 here, so
/// the level is 0..=15 or 0..=31.
///
/// Computed as `trunc(x + h)` with `x = clamp(v, 0, 1) · T` and `h` = [`BELOW_HALF`],
/// because `f32::round` compiles to a call into libm's `roundf` on the default x86-64
/// target (no SSE4.1 `roundss`): 12 calls in `Grid::key`, 6.7 ns per pixel serial, where
/// this is about half that (micro-benchmark over 4 M pixels, 5.5–7.0 against 13.0–13.5 ns
/// for the 5-bit grid).
///
/// *Why it is the same number* (x ≥ 0, `k = ⌊x⌋`, `f = x − k`):
/// * `f < 1/2`: then `x ≤ k + 1/2 − u` with `u` the spacing of floats at `x`, so
///   `x + h ≤ k + 1 − u − 2⁻²⁵`, which rounds to a float below `k + 1`: `trunc = k`. At
///   `k = 0` the largest such `x` is `1/2 − 2⁻²⁵` and `x + h = 1 − 2⁻²⁴` exactly.
/// * `f ≥ 1/2`: `x + h ≥ k + 1 − 2⁻²⁵`, within half a spacing of `k + 1` (floats below
///   `k + 1 ≥ 1` are at most `2⁻²⁴` apart), so it rounds to `k + 1` -- at `x = 1/2` it is an
///   exact tie that round-half-to-even sends to 1.0, the even neighbour: `trunc = k + 1`.
/// * A plain `+ 0.5` fails the first case: `(1/2 − 2⁻²⁵) + 1/2` ties to 1.0.
///
/// Checked, not only argued: identical to `(x).round() as usize` on every one of the
/// 1 065 353 217 floats in `[0, 1]` for both grids (`rounding_is_exact_on_every_float`).
/// NaN gives 0 either way (`NaN as usize` saturates to 0); `−0.0` clamps to `−0.0` and gives
/// 0 either way.
///
/// Not from the literature: an exactness argument about one IEEE 754 addition, because this
/// is a code-generation workaround, not a method. See also: D. Goldberg, "What Every
/// Computer Scientist Should Know About Floating-Point Arithmetic", ACM Computing Surveys,
/// March 1991, <https://docs.oracle.com/cd/E19957-01/806-3568/ncg_goldberg.html>; Rust's
/// `f32::round` documentation (half away from zero).
#[inline]
fn level(v: f32, bits: u32) -> usize {
    let top = ((1usize << bits) - 1) as f32;
    (v.clamp(0.0, 1.0) * top + BELOW_HALF) as usize
}

/// Per-bin statistics over the *occupied* bins only: all pixels, and flat pixels, with
/// their colour-and-opacity sums.
///
/// # Layout
///
/// A bin is identified by its 16-bit key `k` ([`Grid::key`]) and, once a pixel has fallen
/// in it, by a dense index `id` into the per-bin vectors below (`key[id] = k`). `slot[k]`
/// is `id + 1`, or 0 while the bin is empty. The vectors are as long as the number of
/// occupied bins, B, which is tiny: a median of 16 on the 128 px screen set and at most 610
/// on the 2048 px set, out of 65 536 possible keys.
///
/// The dense layout this replaces kept five 65 536-entry arrays (4.98 MB), plus a filled
/// 1.5 MB `bin_point` table and three scans of all 65 536 keys, whatever the image. At
/// 128 px (16 384 pixels) that was 0.65 of the stage's 1.27 ms: 4 table entries per pixel.
/// `slot` is the only table left sized by the key space, and it is written only where a
/// pixel lands. Everything else is O(B).
///
/// *Why identical:* each bin still receives its pixels' values one at a time in raster
/// order, so every sum is the same sequence of f64 roundings; the ids only rename bins,
/// and every consumer either works bin by bin (the lookup table) or sorts by a key that
/// includes the bin key (the candidate orders), so the order ids were handed out in never
/// shows.
///
/// Method from: P. Briggs, L. Torczon, "An Efficient Representation for Sparse Sets",
/// ACM LOPLAS 2(1–4):59–69, 1993, DOI 10.1145/176454.176484, as described by R. Cox,
/// "Using Uninitialized Memory for Fun and Profit", 2008, <https://research.swtch.com/sparse>:
/// a sparse array from element to position and a dense array of members, so iteration costs
/// the members, not the universe. Adapted: the sparse side is zero-initialised (0 = absent)
/// instead of validated against the dense side, since a zeroed 65 536-entry table is cheap
/// and safe Rust has no uninitialised memory.
///
/// Inspired by: M. E. Celebi, "Improving the Performance of K-Means for Color
/// Quantization", Image and Vision Computing 29:260–271, 2011, DOI
/// 10.1016/j.imavis.2010.10.002 -- working on the distinct colours with their counts
/// rather than on the pixels. Here the "colours" are grid bins with coherence counts.
struct Bins {
    /// Per key: `1 + id` of its bin, or 0 when no pixel has that key. `BINS` entries.
    slot: Vec<u32>,
    /// Per bin: its key.
    key: Vec<u16>,
    /// Per bin: its pixels.
    count: Vec<u32>,
    /// Per bin: its flat pixels (every 4-neighbour inside the image in the same bin).
    flat: Vec<u32>,
    /// Per bin: pixels with at least one of their four neighbours in the same bin: the
    /// bin's pixels that belong to a run of it, not to scattered noise.
    paired: Vec<u32>,
    /// Per bin: the colour-and-opacity sum of its flat pixels.
    flat_sum: Vec<[f64; 4]>,
    /// Per bin: the colour-and-opacity sum of all its pixels.
    all_sum: Vec<[f64; 4]>,
}

impl Bins {
    /// No occupied bin yet. The `slot` table is zeroed by the allocator: an all-zero
    /// `vec!` asks for zeroed memory, which the operating system hands out lazily for large
    /// blocks, so pages no key touches are never written.
    fn new() -> Self {
        Bins {
            slot: vec![0; BINS],
            key: Vec::new(),
            count: Vec::new(),
            flat: Vec::new(),
            paired: Vec::new(),
            flat_sum: Vec::new(),
            all_sum: Vec::new(),
        }
    }

    /// The number of occupied bins, B.
    fn len(&self) -> usize {
        self.key.len()
    }

    /// The id of key `k`'s bin, which must be occupied. O(1).
    #[inline]
    fn id(&self, k: u16) -> usize {
        self.slot[k as usize] as usize - 1
    }

    /// The id of key `k`'s bin, opening an empty one (all counts and sums zero) the first
    /// time the key is seen. O(1) amortised.
    #[inline]
    fn open(&mut self, k: u16) -> usize {
        let s = self.slot[k as usize];
        if s != 0 {
            return s as usize - 1;
        }
        let id = self.key.len();
        // `id + 1 ≤ 65 536`: at most one bin per key, so it always fits a u32.
        self.slot[k as usize] = id as u32 + 1;
        self.key.push(k);
        self.count.push(0);
        self.flat.push(0);
        self.paired.push(0);
        self.flat_sum.push([0.0; 4]);
        self.all_sum.push([0.0; 4]);
        id
    }
}

impl Bins {
    /// Add every bin of `other` into `self`: counts, and the sums channel by channel.
    ///
    /// Used only to merge the row bands of [`keys_and_histogram`], and only when
    /// [`in_exact_set`] held for every value: then each sum is exact (see there), so adding
    /// a band's partial sum equals adding its pixels one by one, in any order. O(B_other).
    fn absorb(&mut self, other: &Bins) {
        for id in 0..other.len() {
            let j = self.open(other.key[id]);
            self.count[j] += other.count[id];
            self.flat[j] += other.flat[id];
            self.paired[j] += other.paired[id];
            for c in 0..4 {
                self.all_sum[j][c] += other.all_sum[id][c];
                self.flat_sum[j][c] += other.flat_sum[id][c];
            }
        }
    }
}

/// Pixel `p`'s colour and opacity, `[r, g, b, a]`: the image over white (sRGB 0..1) and the
/// source's opacity when transparency is traced natively, 1 otherwise.
///
/// Read in place: the palette used to copy the image into a four-channel buffer of its own
/// first (64 MB and 6.9 ms at 2048 px, 16 % of the stage) and read that. The values are the
/// same floats, so everything computed from them is too.
#[inline]
fn pixel(rgb: &[[f32; 3]], alpha: Option<&[f32]>, p: usize) -> [f32; 4] {
    let c = rgb[p];
    [c[0], c[1], c[2], alpha.map_or(1.0, |a| a[p])]
}

/// Most pixels for which [`in_exact_set`] makes the per-bin sums exact: 2²² (2048 × 2048,
/// the default `--max-dim`).
const EXACT_SUM_MAX_PIXELS: usize = 1 << 22;

/// Whether `v` is 0 or lies in `[2⁻⁸, 1]`: the values for which the histogram's f64 sums
/// are exact whatever order they are formed in.
///
/// # The lemma
///
/// Let V be a multiset of f32 values, each 0 or in `[2⁻⁸, 1]`, with `|V| ≤ 2²²`. Then
/// summing V into an f64 in any order, grouped in any way, gives the exact real sum.
///
/// *Proof.* A float `v ≥ 2⁻⁸` has an exponent of at least −8 and a 24-bit significand, so
/// it is an integer multiple of `2⁻⁸⁻²³ = 2⁻³¹`. Every partial sum is then `j · 2⁻³¹` with
/// `0 ≤ j · 2⁻³¹ ≤ |V| ≤ 2²²`, so `j ≤ 2⁵³`, and every such number is representable in
/// binary64 (a 53-bit significand holds every integer up to 2⁵³). IEEE 754 addition is
/// "computed exactly and then rounded"; a representable exact result is returned
/// unrounded. By induction every partial sum is exact, so the final sum is the real sum,
/// independent of order. ∎
///
/// Every 8-bit input that has not been resampled satisfies it: channels are `k / 255`
/// (`k / 255 ≥ 1/255 > 2⁻⁸` for `k ≥ 1`), and a translucent pixel over white is
/// `fl(fl(s·a) + fl(1 − a)) ≥ fl(1 − a) ≥ 1/255` (0 violations on 253 of 253 measured
/// images). Resampled rasters (a `--max-dim` reduction, `--intake-scale`) carry arbitrary
/// box averages and fail it; for them the histogram keeps its serial raster order.
///
/// Not from the literature: the lemma is derived here from IEEE 754's exact rounding,
/// because published reproducible summation (J. Demmel, H. D. Nguyen, "Fast Reproducible
/// Floating-Point Summation", ARITH 2013) makes *any* sum order-independent by pre-rounding
/// to a common grid, which would change the values; this only has to *recognise* inputs
/// whose sums are already exact, and keep the old order for the rest. See also: D.
/// Goldberg, "What Every Computer Scientist Should Know About Floating-Point Arithmetic",
/// ACM Computing Surveys, March 1991,
/// <https://docs.oracle.com/cd/E19957-01/806-3568/ncg_goldberg.html>.
#[inline]
fn in_exact_set(v: f32) -> bool {
    /// 2⁻⁸.
    const LOW: f32 = 1.0 / 256.0;
    // NaN fails both comparisons and so is not in the set.
    v == 0.0 || (LOW..=1.0).contains(&v)
}

/// Write the bin key of pixels `p0 .. p0 + out.len()` into `out`, and report whether every
/// channel value read lies in [`in_exact_set`].
///
/// The key is a pure function of the pixel's four floats, so it is computed once per run
/// of identical colours and copied along the run. On the 2048 px set 99.56 % of pixels
/// repeat their left neighbour's colour (medians; the textured masthead only 33 %), so
/// [`Grid::key`] runs on well under 1 % of them. The set check rides on the same test:
/// a repeated colour was already checked. `f32` equality treats `−0.0` and `0.0` as equal,
/// which is harmless (both give level 0 and both are in the set); a NaN never equals the
/// previous colour and is always keyed afresh. Θ(len) comparisons.
///
/// Inspired by: T. M. Breuel, "Efficient Binary and Run Length Morphology and its
/// Application to Document Image Processing", 2007, <https://arxiv.org/abs/0712.0121> --
/// work per run rather than per pixel, because "an almost blank image takes the same amount
/// of time to process as a highly detailed image" otherwise. Here the runs are found on the
/// fly, not stored.
fn key_rows(
    rgb: &[[f32; 3]],
    alpha: Option<&[f32]>,
    grid: Grid,
    p0: usize,
    out: &mut [u16],
) -> bool {
    let mut exact = true;
    // A colour no pixel equals (NaN != NaN), so the first pixel is always keyed.
    let (mut last, mut key) = ([f32::NAN; 4], 0u16);
    for (i, o) in out.iter_mut().enumerate() {
        let c = pixel(rgb, alpha, p0 + i);
        if c != last {
            key = grid.key(c) as u16;
            exact &= c.iter().all(|&v| in_exact_set(v));
            last = c;
        }
        *o = key;
    }
    exact
}

/// Add rows `y0 ..` of the `w × h` image to `bins`: `band` holds those rows' keys (a whole
/// number of rows), `above` the keys of row `y0 − 1` (`None` at the top of the image) and
/// `below` those of the row after the band (`None` at the bottom).
///
/// Per bin, how many pixels fall in it (`count`, with their colour sums `all_sum`), how
/// many are *paired* (at least one 4-neighbour in the same bin) and how many are *flat*
/// (every 4-neighbour inside the image in the same bin, with their sums `flat_sum`). In
/// morphological terms the flat pixels of bin `k` are the erosion of its indicator set
/// `X_k = {p : key(p) = k}` by the 4-cross, with the image padded by `k`, and the paired
/// pixels are `X_k` without its isolated points. A neighbour outside the image counts as
/// agreeing, so a filled region touching the border keeps its flat pixels there. Sums are
/// in f64 so a 2048 px image's totals keep the low bits of each 0..1 channel.
///
/// # One run at a time
///
/// Each row is walked as maximal runs of one key. Inside a run `[s, e)` a pixel's left
/// neighbour is in the bin exactly when it is not the run's first pixel, and its right one
/// when it is not the last -- a run is maximal, so the pixel before `s` and the one at `e`
/// (if inside the row) hold other keys. Only the rows above and below are read per pixel.
/// A run's additions go into registers loaded from the bin before the run and stored after
/// it: on flat art nine pixels in ten continue their left neighbour's run, and adding each
/// into the bin's memory made every addition wait on the store of the one before
/// (store-to-load forwarding); the dense pixel-by-pixel pass took 17.8 ms at 2048 px, 42 %
/// of the stage.
///
/// *Why identical to the pixel-by-pixel pass:* every bin still receives its pixels' values
/// one at a time in raster order -- runs are visited in raster order, and a run's own pixels
/// left to right -- so every f64 sum is the same sequence of roundings, whatever the values.
/// Counts are integers. Θ(pixels) time, one bin lookup per run.
///
/// Method from: T. M. Breuel, "Efficient Binary and Run Length Morphology and its
/// Application to Document Image Processing", 2007, <https://arxiv.org/abs/0712.0121> -- the
/// erosion is evaluated on the run-length representation, horizontally at run ends and
/// vertically per pixel. Adapted: a greyscale "run" is a run of one bin key, and the erosion
/// only has to be counted, not produced.
///
/// Inspired by: G. Pass, R. Zabih, J. Miller, "Comparing Images Using Color Coherence
/// Vectors", ACM Multimedia '96, pp. 65–73, DOI 10.1145/244130.244148, which splits each
/// colour bucket's pixels into coherent and incoherent by the size of their connected
/// component. Flatness is the local version: a pixel is coherent when its whole 4-cross
/// stays in its bucket, which needs no component labelling.
#[allow(clippy::too_many_arguments)]
fn histogram_rows(
    rgb: &[[f32; 3]],
    alpha: Option<&[f32]>,
    w: usize,
    y0: usize,
    band: &[u16],
    above: Option<&[u16]>,
    below: Option<&[u16]>,
    bins: &mut Bins,
) {
    let rows = band.len() / w;
    for r in 0..rows {
        let at = (y0 + r) * w;
        let row = &band[r * w..(r + 1) * w];
        let up = if r > 0 {
            Some(&band[(r - 1) * w..r * w])
        } else {
            above
        };
        let down = if r + 1 < rows {
            Some(&band[(r + 1) * w..(r + 2) * w])
        } else {
            below
        };
        let mut x = 0;
        while x < w {
            let k = row[x];
            let start = x;
            while x < w && row[x] == k {
                x += 1;
            }
            let id = bins.open(k);
            let (mut all, mut flat_sum) = (bins.all_sum[id], bins.flat_sum[id]);
            let (mut flat, mut paired) = (0u32, 0u32);
            for xx in start..x {
                let c = pixel(rgb, alpha, at + xx);
                for (s, v) in all.iter_mut().zip(c) {
                    *s += v as f64;
                }
                let (l, rr) = (xx > start, xx + 1 < x);
                let u = up.is_some_and(|row| row[xx] == k);
                let d = down.is_some_and(|row| row[xx] == k);
                if l || rr || u || d {
                    paired += 1;
                }
                // Outside the image counts as agreeing: the first column has no left
                // neighbour (start = 0 there), the top row no row above (`up` is None).
                let flat_here = (xx == 0 || l)
                    && (xx + 1 == w || rr)
                    && (up.is_none() || u)
                    && (down.is_none() || d);
                if flat_here {
                    flat += 1;
                    for (s, v) in flat_sum.iter_mut().zip(c) {
                        *s += v as f64;
                    }
                }
            }
            bins.count[id] += (x - start) as u32;
            bins.flat[id] += flat;
            bins.paired[id] += paired;
            bins.all_sum[id] = all;
            bins.flat_sum[id] = flat_sum;
        }
    }
}

/// Fewest rows in a band of [`keys_and_histogram`]: each band keys two rows beyond its
/// own, so a thin band would spend a large share of its work on them.
const MIN_BAND_ROWS: usize = 16;
/// About how many bands an image is cut into, so rayon can balance them over its workers.
const TARGET_BANDS: usize = 64;

/// Every pixel's bin key ([`Grid::key`], row-major) and the per-bin statistics
/// ([`histogram_rows`]), in one pass over the image when it can be done in parallel.
///
/// # Serial
///
/// Below [`PARALLEL_MIN_PIXELS`]: the keys by [`key_rows`], then one band covering the
/// whole image.
///
/// # Parallel row bands
///
/// The image is cut into bands of whole rows. Each band keys the row above it and the row
/// below it into scratch (for the vertical test), then walks its own rows: row `r + 1` is
/// keyed into the output just before row `r` is counted into a band-private [`Bins`], so
/// the count reads pixels the keying has just brought into cache and the image comes from
/// memory about once (keying a whole band before counting it measured 9.5 against 6.8 ms
/// for keys and histogram at 2048 px, loaded machine, pool warm). Rayon's fold then merges
/// the bands' `Bins` ([`Bins::absorb`]).
///
/// *Why identical:* counts are integers, and the merge adds each band's f64 sums in an
/// order the serial pass would not use -- which is exact only because every value was in
/// [`in_exact_set`] and there are at most [`EXACT_SUM_MAX_PIXELS`] pixels (the lemma there):
/// then both the serial sum and any regrouping of it equal the exact real sum. When a band
/// finds a value outside the set (a resampled image), the bands stop counting and the
/// histogram is redone serially over the finished keys, in raster order. Keys are the same
/// either way, being a function of each pixel alone.
///
/// Measured: the run-based parallel histogram gave 0 differing bins on 254 images against
/// the serial one, at 2.4–3.3 ms where the serial pass took 17.8 ms at 2048 px.
///
/// Method from: V. Podlozhnyuk, "Histogram calculation in CUDA", NVIDIA, 2007,
/// <https://developer.download.nvidia.com/compute/cuda/1.1-Beta/x86_website/projects/histogram64/doc/histogram.pdf>,
/// and T. Henriksen, S. Hellfritzsch, P. Sadayappan, C. Oancea, "Compiling Generalized
/// Histograms for GPU", SC20, 2020,
/// <https://hjemmesider.diku.dk/~zgh600/Publications/gen-histo-sc20.pdf> -- privatised
/// sub-histograms, one per worker, merged at the end; the per-bin state (count, flat,
/// paired, sums) is a generalised histogram with an associative, commutative operator.
/// Adapted: the sub-histograms are sparse ([`Bins`]) because the H = 65 536 bins would dwarf
/// N at 128 px, the case Henriksen et al. call inefficient ("when H is close to N"), and
/// associativity of the f64 sums is not assumed but checked per input.
fn keys_and_histogram(
    rgb: &[[f32; 3]],
    alpha: Option<&[f32]>,
    w: usize,
    h: usize,
    grid: Grid,
    parallel: bool,
) -> (Vec<u16>, Bins) {
    use rayon::prelude::*;
    use std::sync::atomic::{AtomicBool, Ordering};
    let n = w * h;
    let mut keys = vec![0u16; n];
    if n == 0 {
        return (keys, Bins::new());
    }
    if !parallel || n > EXACT_SUM_MAX_PIXELS || h < 2 * MIN_BAND_ROWS {
        if parallel {
            keys.par_chunks_mut(w).enumerate().for_each(|(y, row)| {
                key_rows(rgb, alpha, grid, y * w, row);
            });
        } else {
            key_rows(rgb, alpha, grid, 0, &mut keys);
        }
        let mut bins = Bins::new();
        histogram_rows(rgb, alpha, w, 0, &keys, None, None, &mut bins);
        return (keys, bins);
    }
    let band_rows = h.div_ceil(TARGET_BANDS).max(MIN_BAND_ROWS);
    // Set by the first band that meets a value outside the exact set; later bands then
    // only key their rows.
    let inexact = AtomicBool::new(false);
    let key_row = |y: usize| {
        let mut row = vec![0u16; w];
        key_rows(rgb, alpha, grid, y * w, &mut row);
        row
    };
    let bins = keys
        .par_chunks_mut(band_rows * w)
        .enumerate()
        .fold(Bins::new, |mut bins, (b, band)| {
            let y0 = b * band_rows;
            let rows = band.len() / w;
            let above = (y0 > 0).then(|| key_row(y0 - 1));
            let below = (y0 + rows < h).then(|| key_row(y0 + rows));
            // Row by row, each row keyed just before the row above it is counted, so the
            // count reads pixels the keying has just brought into cache.
            if !key_rows(rgb, alpha, grid, y0 * w, &mut band[..w]) {
                inexact.store(true, Ordering::Relaxed);
            }
            for r in 0..rows {
                if r + 1 < rows
                    && !key_rows(
                        rgb,
                        alpha,
                        grid,
                        (y0 + r + 1) * w,
                        &mut band[(r + 1) * w..(r + 2) * w],
                    )
                {
                    inexact.store(true, Ordering::Relaxed);
                }
                if inexact.load(Ordering::Relaxed) {
                    continue;
                }
                let up = if r > 0 {
                    Some(&band[(r - 1) * w..r * w])
                } else {
                    above.as_deref()
                };
                let down = if r + 1 < rows {
                    Some(&band[(r + 1) * w..(r + 2) * w])
                } else {
                    below.as_deref()
                };
                histogram_rows(
                    rgb,
                    alpha,
                    w,
                    y0 + r,
                    &band[r * w..(r + 1) * w],
                    up,
                    down,
                    &mut bins,
                );
            }
            bins
        })
        .reduce(Bins::new, |mut a, b| {
            a.absorb(&b);
            a
        });
    if inexact.load(Ordering::Relaxed) {
        // Resampled values: the sums must keep the serial order.
        let mut bins = Bins::new();
        histogram_rows(rgb, alpha, w, 0, &keys, None, None, &mut bins);
        return (keys, bins);
    }
    (keys, bins)
}

/// The mean colour-and-opacity `s / f` of `f` summed pixels. The caller guarantees
/// `f > 0` (a bin or ink with at least one pixel, or `n.max(1)`).
fn mean(s: [f64; 4], f: f64) -> [f32; 4] {
    [
        (s[0] / f) as f32,
        (s[1] / f) as f32,
        (s[2] / f) as f32,
        (s[3] / f) as f32,
    ]
}

/// A colour over white with its opacity, as the two-ground point inks are compared by.
fn ink2(c: [f32; 4]) -> Ink2 {
    let w = rgb_to_oklab([c[0], c[1], c[2]]);
    if c[3] >= OPAQUE {
        Ink2::opaque(w)
    } else {
        Ink2 {
            w,
            k: rgb_to_oklab(over_black([c[0], c[1], c[2]], c[3])),
        }
    }
}

/// The index of the ink nearest `c` by [`Ink2::dist`] (OKLab distance, the larger over
/// the two grounds), and that distance. The first ink wins a tie. With no inks it returns
/// `(0, ∞)`, which every caller treats as "no ink near enough".
fn nearest(inks: &[Ink2], c: Ink2) -> (usize, f32) {
    let mut best = (0, f32::INFINITY);
    for (i, &k) in inks.iter().enumerate() {
        let d = k.dist(c);
        if d < best.1 {
            best = (i, d);
        }
    }
    best
}

/// One accepted ink while the palette is built: its bins and its flat pixels' sums.
struct Ink {
    bins: Vec<usize>,
    sum: [f64; 4],
    flat: f64,
}

/// Found inks from the candidate bins, most flat pixels first.
///
/// A greedy clustering in a single pass. Candidates are the bins with at least
/// [`MIN_FLAT`] flat pixels, sorted by flat count (bin key breaking ties, so the order is
/// total and the result deterministic). Each candidate's flat mean is compared with the
/// inks founded so far, whose points stay where their founding bin put them:
///
/// * it joins the nearest ink `i` when `d < SAME_INK`, or when `d < merge_distance` and
///   one of `i`'s bins touches it ([`Grid::touch`]): one ink spread over adjacent bins by
///   noise or anti-aliasing;
/// * once `max_colors` inks exist, every further candidate joins its nearest;
/// * otherwise it founds a new ink.
///
/// `d` is [`Ink2::dist`] in OKLab. A joined bin adds its flat sums to the ink, so the
/// ink's final colour is the flat-pixel mean over all its bins, not its founder's colour.
/// Most populous first means the dominant colour of a cluster founds it, which keeps a
/// faint neighbour bin from pulling the ink off its true colour. `Ink::bins` holds keys.
///
/// Candidates are the occupied bins (ids), so the scan is O(B) rather than O(65 536); the
/// sort key `(flat descending, key ascending)` is the one the dense version sorted bin keys
/// by, a total order because keys are distinct, so the candidate sequence -- and with it
/// every join and founding -- is the same. Then O(C · K) distances for C candidates
/// (≤ 36 measured) and K inks.
///
/// Method from: J. A. Hartigan, *Clustering Algorithms*, Wiley, 1975 -- the leader
/// algorithm (one pass; a point joins the nearest leader within a radius or becomes a
/// leader), as described in T. B. Arnold's R package `leaderCluster` 1.5,
/// <https://cran.r-project.org/web/packages/leaderCluster/leaderCluster.pdf>. Adapted: points
/// are weighted bins taken in popularity order, and joining also needs grid adjacency.
fn found_inks(bins: &Bins, grid: Grid, merge_distance: f32, max_colors: usize) -> Vec<Ink> {
    // The key breaks ties so the order is total.
    let mut cands: Vec<usize> = (0..bins.len())
        .filter(|&id| bins.flat[id] >= MIN_FLAT)
        .collect();
    cands.sort_unstable_by_key(|&id| (std::cmp::Reverse(bins.flat[id]), bins.key[id]));
    let max_colors = max_colors.clamp(1, u16::MAX as usize);
    let mut inks: Vec<Ink> = Vec::new();
    let mut points: Vec<Ink2> = Vec::new();
    for &id in &cands {
        let k = bins.key[id] as usize;
        let f = bins.flat[id] as f64;
        let s = bins.flat_sum[id];
        let point = ink2(mean(s, f));
        let (i, d) = nearest(&points, point);
        let joins = !inks.is_empty()
            && (d < SAME_INK
                || (d < merge_distance && inks[i].bins.iter().any(|&b| grid.touch(b, k))));
        if joins || (inks.len() >= max_colors && !inks.is_empty()) {
            // One ink measured twice: its colour is the flat pixels' mean over its bins.
            let ink = &mut inks[i];
            for (t, v) in ink.sum.iter_mut().zip(s) {
                *t += v;
            }
            ink.flat += f;
            ink.bins.push(k);
        } else {
            points.push(point);
            inks.push(Ink {
                bins: vec![k],
                sum: s,
                flat: f,
            });
        }
    }
    inks
}

/// Inks with no flat pixel: small text and hairlines, whose every pixel touches the ground.
///
/// Flatness is the evidence [`found_inks`] reads, and a stroke two or three pixels wide has
/// none, so black text beside a red mark would have no black ink at all: its pixels then
/// take the nearest ink there is, and the text comes out red. Quality mode admits a colour
/// by its share of the image and drops it when it is a blend of two inks; this is the same
/// test on the bins, with the share counted in *paired* pixels (see [`Bins::paired`]) so
/// that a pair of eyes on a 128 px emoji counts and scattered noise does not. A bin with
/// at least [`MIN_PAIRED`] and [`THIN_SHARE`] of the image in paired pixels, not within
/// `merge_distance` of an ink, is a candidate; candidates are then dropped, least
/// populous first, while they lie on the line between two other inks or candidates -- the
/// grey rim of the text is a blend of the text and the paper, the text itself is not.
///
/// `used` is per bin id: the bins [`found_inks`] took. The candidate scan is O(B) over the
/// occupied bins and sorts by `(paired descending, key ascending)`, the dense version's
/// total order, so the picks are the same. Then at most [`MAX_THIN_CANDIDATES`] picks and
/// O(picks · (K + picks)²) blend tests.
///
/// Inspired by: the Quality palette's share-and-blend admission (`color::extract_palette_mdl`);
/// the blend test is `absorb_slivers`' segment distance.
fn thin_inks(
    bins: &Bins,
    used: &[bool],
    inks: &[[f32; 4]],
    n: usize,
    merge_distance: f32,
    max_colors: usize,
) -> Vec<[f32; 4]> {
    if inks.len() >= max_colors || inks.is_empty() {
        return Vec::new();
    }
    let min_paired = ((THIN_SHARE as f64 * n as f64).ceil() as u32).max(MIN_PAIRED);
    let mut cands: Vec<usize> = (0..bins.len())
        .filter(|&id| !used[id] && bins.paired[id] >= min_paired)
        .collect();
    cands.sort_unstable_by_key(|&id| (std::cmp::Reverse(bins.paired[id]), bins.key[id]));
    let mut points: Vec<Ink2> = inks.iter().map(|&c| ink2(c)).collect();
    // Candidate colours, most paired pixels first.
    let mut picked: Vec<[f32; 4]> = Vec::new();
    for &id in &cands {
        let c = mean(bins.all_sum[id], bins.count[id] as f64);
        let p = ink2(c);
        if nearest(&points, p).1 < merge_distance {
            continue;
        }
        points.push(p);
        picked.push(c);
        if picked.len() >= MAX_THIN_CANDIDATES {
            break;
        }
    }
    let mut keep = vec![true; picked.len()];
    for i in (0..picked.len()).rev() {
        let others: Vec<[f32; 4]> = inks
            .iter()
            .chain(
                picked
                    .iter()
                    .enumerate()
                    .filter(|&(j, _)| j != i && keep[j])
                    .map(|(_, c)| c),
            )
            .copied()
            .collect();
        let blend = others.iter().enumerate().any(|(a, &ia)| {
            others[a + 1..]
                .iter()
                .any(|&ib| super::faces::is_blend(picked[i], ia, ib))
        });
        keep[i] = !blend;
    }
    picked
        .into_iter()
        .zip(keep)
        .filter(|&(_, k)| k)
        .map(|(c, _)| c)
        .take(max_colors - inks.len())
        .collect()
}

/// What labelling a blend pixel reads: the pixels, their bins, and the inks.
struct Blends<'a> {
    /// The image over white, sRGB 0..1, read with `alpha` through [`pixel`].
    rgb: &'a [[f32; 3]],
    /// The source's opacity, when transparency is traced natively.
    alpha: Option<&'a [f32]>,
    /// Every pixel's bin key, row-major.
    keys: &'a [u16],
    /// Per key: its bin's nearest ink, and whether the bin is that ink. `BINS` entries,
    /// written only for occupied keys (no pixel reads the others).
    lut: &'a [(u16, bool)],
    /// Per key: `1 + id` of its bin ([`Bins::slot`]).
    slot: &'a [u32],
    /// Per bin id: its mean colour as a two-ground point.
    bin_point: &'a [Ink2],
    inks: &'a [[f32; 4]],
    points: &'a [Ink2],
    /// How near (OKLab) its nearest ink must be for a pixel nothing around explains to
    /// keep it.
    near: f32,
    w: usize,
    h: usize,
}

impl Blends<'_> {
    /// The distinct inks of the sure pixels within `r` of (x, y).
    fn around(&self, x: usize, y: usize, r: usize, out: &mut Vec<u16>) {
        out.clear();
        for yy in y.saturating_sub(r)..(y + r + 1).min(self.h) {
            for xx in x.saturating_sub(r)..(x + r + 1).min(self.w) {
                let (l, ok) = self.lut[self.keys[yy * self.w + xx] as usize];
                if ok && !out.contains(&l) {
                    out.push(l);
                }
            }
        }
    }

    /// The label of a *blend* pixel (x, y): one whose bin is not itself an ink colour, so
    /// the lookup table only gave it its nearest ink `own`. (An ink-coloured, "sure", pixel
    /// keeps its ink; [`label_rows`] settles those inline and never calls this.) The pixel
    /// takes the nearer of its own nearest ink and its ink-coloured neighbours' nearest,
    /// when it is made of that ink: is it, or a blend of it and an ink around. Its own
    /// nearest is how a thin stroke keeps its ink: it has no flat pixel anywhere near, and
    /// its partly covered pixels must not all go to the ground beside it.
    ///
    /// Nearest in colour alone is not enough: the grey rim between black text and white
    /// paper lies nearer a red used elsewhere than either -- or a red touching it -- and is
    /// no blend of red with anything around it. A pixel not made of its nearest ink takes
    /// the ink it is mostly made of ([`Blends::explain`]). A pixel nothing around explains
    /// -- a thin band of an ink too small to be in the palette -- keeps its own nearest ink
    /// when that is near it ([`KEEP_OWN`]), and its neighbours' nearest otherwise.
    ///
    /// Neighbours are the ink-coloured pixels within one pixel, or, when there are none
    /// (small text downscaled is all blends), within up to [`REACH`].
    ///
    /// A pure function of the keys in the 9 × 9 window around (x, y), the tables and the
    /// pixel's own colour: it reads no other label, so pixels may be labelled in any order
    /// and on any thread. Cost: a window of 9 to 81 table reads, then O(|A| + K) distances
    /// and, in `explain`, O(|A| · K) blend tests for K inks and |A| inks around. Blends are
    /// 0.29 % of pixels at 2048 px and 4.4 % on the 128 px screen set (medians).
    #[inline(never)]
    fn blend_label(&self, x: usize, y: usize, own: u16) -> u16 {
        let p = y * self.w + x;
        let mut around = Vec::with_capacity(9);
        for r in 1..=REACH {
            self.around(x, y, r, &mut around);
            if !around.is_empty() {
                break;
            }
        }
        if around.is_empty() {
            return own;
        }
        // Every pixel's key is occupied, so its slot is at least 1.
        let c = self.bin_point[self.slot[self.keys[p] as usize] as usize - 1];
        let mut best = (own, f32::INFINITY);
        for &l in &around {
            let d = self.points[l as usize].dist(c);
            if d < best.1 {
                best = (l, d);
            }
        }
        let col = pixel(self.rgb, self.alpha, p);
        // Whether the pixel is ink `i`, or a blend of it and an ink around.
        let made_of = |i: u16| {
            let ci = self.inks[i as usize];
            let tol2 = super::faces::BLEND_TOL * super::faces::BLEND_TOL;
            (0..4).map(|k| (col[k] - ci[k]).powi(2)).sum::<f32>() <= tol2
                || around
                    .iter()
                    .any(|&l| l != i && super::faces::is_blend(col, ci, self.inks[l as usize]))
        };
        let d_own = self.points[own as usize].dist(c);
        let nearest = if d_own < best.1 { own } else { best.0 };
        if made_of(nearest) {
            return nearest;
        }
        if let Some(l) = self.explain(col, &around) {
            return l;
        }
        if d_own < self.near {
            own
        } else {
            best.0
        }
    }

    /// The ink a pixel of colour `col` is mostly made of, when it is a blend of an ink in
    /// `around` and any ink: of all such pairs the one it lies nearest the line of, and
    /// the side of it that covers more of the pixel -- `absorb_slivers`' rule. A pixel
    /// within the blend tolerance of an ink in `around` is that ink.
    fn explain(&self, col: [f32; 4], around: &[u16]) -> Option<u16> {
        let mut best: Option<(u16, f32)> = None;
        let mut consider = |l: u16, d: f32| {
            if d <= super::faces::BLEND_TOL * super::faces::BLEND_TOL
                && best.is_none_or(|(_, e)| d < e)
            {
                best = Some((l, d));
            }
        };
        for &a in around {
            let ia = self.inks[a as usize];
            consider(a, (0..4).map(|k| (col[k] - ia[k]).powi(2)).sum());
            for (b, &ib) in self.inks.iter().enumerate() {
                if b == a as usize {
                    continue;
                }
                if let Some((t, d)) = super::faces::blend_of(col, ia, ib) {
                    consider(if t < 0.5 { a } else { b as u16 }, d);
                }
            }
        }
        best.map(|(l, _)| l)
    }
}

/// Farthest (in pixels) a blend looks for ink-coloured neighbours.
const REACH: usize = 4;
/// A pixel nothing around explains keeps its nearest ink only within this many
/// `merge_distance`s of it: a thin band of a near-black that missed the palette keeps the
/// black beside it in colour, a grey rim does not become red.
const KEEP_OWN: f32 = 3.0;

/// Below this many pixels (256 × 256) every pass of the palette runs on the calling thread,
/// and so does every pass when rayon has a single worker.
///
/// Every pass here is exact whatever the thread count, so this only decides speed. At
/// 128 px (16 384 pixels) the per-pixel work of a whole pass is a tenth of a millisecond,
/// and handing it to rayon cost more than it saved: building the four-channel copy took
/// 0.17 ms in parallel against 0.07 serially, keys 0.15 against 0.18 (before the inline
/// rounding halved the serial figure), labels 0.08 against 0.11 (screen set, pool already
/// running). In the command-line tool the palette is also the first parallel call of the
/// process, which spawns the global pool (0.37 ms median); serial here, that cost moves to
/// the next parallel stage rather than disappearing, so it is not counted as a gain.
///
/// Inspired by: rayon's `IndexedParallelIterator::with_min_len`
/// (<https://docs.rs/rayon/latest/rayon/iter/trait.IndexedParallelIterator.html>), which sets
/// the smallest job; a plain branch is used instead because asking rayon for even one job
/// starts its thread pool.
const PARALLEL_MIN_PIXELS: usize = 1 << 16;

/// The inks, and one label (an ink index) per pixel. `alpha`, when given, is the source's
/// opacity per pixel and `rgb` the image over white: inks then carry an opacity, and the
/// clear ground is an ink of its own.
///
/// Steps (see the module documentation for the layout): bin every pixel and count the bins
/// ([`keys_and_histogram`]); found inks from flat bins ([`found_inks`]) and add stroke inks
/// with no flat pixel ([`thin_inks`]); snap each ink's opacity ([`snap_alpha`]); give every
/// occupied bin its nearest ink once, marking it "is that ink" when within
/// `merge_distance` (OKLab); then label each pixel from that table ([`label_rows`],
/// [`Blends::blend_label`]). With no flat bin anywhere (pure noise, or a tiny image) the
/// palette is one ink, the mean colour of the image.
///
/// Outputs: the [`Palette`] with `rgb` (sRGB 0..1), `colors` (OKLab of `rgb`), `alpha`
/// (0..1) and `weight` (each ink's share of the labels, summing to 1), and `w · h` labels,
/// every one a valid ink index. `max_colors` caps the palette (flat inks and thin inks
/// together); [`found_inks`] clamps it to `1..=65535` so a label fits a `u16`. An empty
/// image gives one ink (the mean of nothing, zeros) and no labels.
///
/// From [`PARALLEL_MIN_PIXELS`] on, the keys, the histogram and the labels run in
/// parallel row bands. None of it depends on the thread count: keys and labels are pure
/// functions of the pixels and immutable tables, counts are integers, and the histogram's
/// f64 sums are merged across bands only when they are exact (see [`keys_and_histogram`]).
/// Time Θ(n) with small constants plus O(B · K) for the table; memory: the keys and labels
/// (2 bytes per pixel each) and O(B) per band.
pub(crate) fn palette_and_labels(
    rgb: &[[f32; 3]],
    alpha: Option<&[f32]>,
    w: usize,
    h: usize,
    merge_distance: f32,
    max_colors: usize,
) -> (Palette, Vec<u16>) {
    use rayon::prelude::*;
    let n = w * h;
    let grid = match alpha {
        Some(_) => Grid {
            bits: 4,
            alpha_bits: 4,
        },
        None => Grid {
            bits: 5,
            alpha_bits: 0,
        },
    };
    // With one worker (the single-threaded WebAssembly build runs rayon inline) the banded
    // histogram would only add its halo rows and merges, so it runs serially there too.
    let parallel = n >= PARALLEL_MIN_PIXELS && rayon::current_num_threads() > 1;
    let (keys, bins) = keys_and_histogram(rgb, alpha, w, h, grid, parallel);

    let found = found_inks(&bins, grid, merge_distance, max_colors);
    // Per bin id: taken by a flat ink.
    let mut used = vec![false; bins.len()];
    for i in &found {
        for &k in &i.bins {
            used[bins.id(k as u16)] = true;
        }
    }
    let mut inks: Vec<[f32; 4]> = found.iter().map(|i| mean(i.sum, i.flat)).collect();
    let thin = thin_inks(&bins, &used, &inks, n, merge_distance, max_colors);
    inks.extend(thin);
    if inks.is_empty() {
        // Nothing flat anywhere (noise, or a tiny image): one ink, the mean colour, summed
        // in raster order as before (rare, so not worth an exactness check).
        let mut s = [0.0f64; 4];
        for p in 0..n {
            for (t, v) in s.iter_mut().zip(pixel(rgb, alpha, p)) {
                *t += v as f64;
            }
        }
        inks.push(mean(s, n.max(1) as f64));
    }
    for c in inks.iter_mut() {
        c[3] = snap_alpha(c[3]);
    }
    let points: Vec<Ink2> = inks.iter().map(|&c| ink2(c)).collect();

    // Each occupied bin, once: its nearest ink, and whether the bin *is* that ink. O(B · K).
    //
    // Method from: S. W. Thomas, "Efficient Inverse Color Map Computation", Graphics Gems II,
    // pp. 116–125, 1991 -- the inverse colour map, "the colormap entry that is closest to the
    // (quantized) color" per grid cell. Adapted: only occupied cells are filled, each keyed by
    // its pixels' mean rather than the cell centre, with a flag for "is that ink". `lut` stays
    // indexed by key, since every pixel looks itself up by key, but only occupied entries are
    // written; the all-zero initial value is a zeroed allocation, and no pixel reads the rest.
    let mut lut = vec![(0u16, false); BINS];
    let mut bin_point = Vec::with_capacity(bins.len());
    for id in 0..bins.len() {
        let c = ink2(mean(bins.all_sum[id], bins.count[id] as f64));
        let (i, d) = nearest(&points, c);
        bin_point.push(c);
        lut[bins.key[id] as usize] = (i as u16, d < merge_distance);
    }

    let blends = Blends {
        rgb,
        alpha,
        keys: &keys,
        lut: &lut,
        slot: &bins.slot,
        bin_point: &bin_point,
        inks: &inks,
        points: &points,
        near: KEEP_OWN * merge_distance,
        w,
        h,
    };
    let mut labels = vec![0u16; n];
    // Row by row, in parallel on a large image: each label reads only `keys` and the
    // tables, so the result does not depend on the thread count. Each worker also counts
    // its labels; integer counts add up to the same totals in any order.
    let n_inks = inks.len();
    let count = if parallel {
        labels
            .par_chunks_mut(w.max(1))
            .enumerate()
            .fold(
                || vec![0usize; n_inks],
                |mut count, (y, row)| {
                    label_rows(&blends, y, row, &mut count);
                    count
                },
            )
            .reduce(
                || vec![0usize; n_inks],
                |mut a, b| {
                    for (s, v) in a.iter_mut().zip(b) {
                        *s += v;
                    }
                    a
                },
            )
    } else {
        let mut count = vec![0usize; n_inks];
        label_rows(&blends, 0, &mut labels, &mut count);
        count
    };

    let weight = ink_shares(&count, n);
    let rgb_inks: Vec<[f32; 3]> = inks.iter().map(|c| [c[0], c[1], c[2]]).collect();
    (
        Palette {
            colors: rgb_inks.iter().map(|&c| rgb_to_oklab(c)).collect(),
            rgb: rgb_inks,
            weight,
            alpha: inks.iter().map(|c| c[3]).collect(),
        },
        labels,
    )
}

/// Label the pixels of `rows` -- whole image rows, the first of them row `y0` -- and add each
/// ink's pixel count to `count` (indexed by ink).
///
/// Per pixel `p` with bin key `k_p`, the lookup table gives `(own, sure) = lut[k_p]`. A sure
/// pixel's label is `own` and is written here, inline; only the others go to
/// [`Blends::blend_label`]. That split is the whole change from the per-pixel call it
/// replaces: 99.7 % of pixels are sure at 2048 px and 95.6 % on the 128 px screen set
/// (medians), and for them the call was pure overhead -- 3.3 ms of the stage at 2048 px,
/// 1.0 ms inline (labels identical on 254 images).
///
/// *Why identical:* the old `label(x, y)` began `if sure { return own }`, and
/// `blend_label` is the rest of that function verbatim, so each pixel gets the same value
/// from the same expression.
///
/// Method from: M. J. Swain, D. H. Ballard, "Color Indexing", IJCV 7(1):11–32, 1991,
/// DOI 10.1007/BF00130487 -- histogram backprojection: a pixel is labelled by looking its
/// colour bin up in a table built from the histogram. Adapted: the table holds the bin's
/// nearest ink and whether the bin *is* that ink, and the pixels it cannot decide go to the
/// neighbourhood rule.
///
/// The counts are taken per run of equal labels, in a register, and added once when the
/// run ends. 99.6 % of neighbouring labels are equal on the 7-image 2048 px set, so the
/// per-pixel `count[l] += 1` made every iteration wait on the store of the one before it
/// (store-to-load forwarding on one address). Θ(pixels) reads, Θ(runs) count updates;
/// an empty `rows` changes nothing.
///
/// Inspired by: Y. Collet, FiniteStateEntropy `lib/hist.c`, `HIST_count_parallel_wksp`,
/// <https://github.com/Cyan4973/FiniteStateEntropy/blob/dev/lib/hist.c>, which breaks the
/// same chain with four sub-tables ("noticeably faster when some values are heavily
/// repeated"). Ours counts runs instead: labels are an image, so repeats come in runs, and
/// a run needs no second table.
fn label_rows(blends: &Blends, y0: usize, rows: &mut [u16], count: &mut [usize]) {
    let w = blends.w.max(1);
    let keys = &blends.keys[y0 * w..y0 * w + rows.len()];
    for (dy, (row, krow)) in rows.chunks_mut(w).zip(keys.chunks(w)).enumerate() {
        for (x, (out, &k)) in row.iter_mut().zip(krow).enumerate() {
            let (own, sure) = blends.lut[k as usize];
            *out = if sure {
                own
            } else {
                blends.blend_label(x, y0 + dy, own)
            };
        }
    }
    add_label_runs(rows, count);
}

/// Add to `count[l]` the number of entries of `labels` equal to `l`, one update per run of
/// equal entries (see [`label_rows`] for why). Every entry must be a valid index into
/// `count`. An empty slice adds nothing.
fn add_label_runs(labels: &[u16], count: &mut [usize]) {
    let mut it = labels.iter();
    let Some(&first) = it.next() else {
        return;
    };
    let (mut cur, mut run) = (first, 1usize);
    for &l in it {
        if l == cur {
            run += 1;
        } else {
            count[cur as usize] += run;
            (cur, run) = (l, 1);
        }
    }
    count[cur as usize] += run;
}

/// Each ink's share of the `n` pixels, `Palette::weight`: `min(count_i, 2²⁴) / max(n, 1)`,
/// computed in f32.
///
/// # Why this is the number the old code gave
///
/// The old code summed `1.0` into an f32 per pixel, then divided by `n`. Adding 1.0 to an
/// f32 holding an integer below 2²⁴ is exact (the result is an integer ≤ 2²⁴, which has a
/// 24-bit significand), so up to 2²⁴ pixels the running sum equals the integer count. At
/// 2²⁴ it stops: 2²⁴ + 1 lies halfway between 2²⁴ and 2²⁴ + 2, and round-half-to-even keeps
/// 2²⁴. So the old sum was exactly `min(count, 2²⁴)`, and `min(count, 2²⁴) as f32` is that
/// value converted exactly; the division is the same f32 operation on the same operands.
///
/// Nothing in the workspace reads `weight` (checked with `git grep` at 7a4e054), but
/// [`Palette`] is a public type, so the field keeps its meaning rather than being dropped.
/// It used to cost 23 % of the palette stage at 2048 px (9.8 ms), a serial chain of
/// dependent f32 additions; counted per run it is under a millisecond.
///
/// Not from the literature: an exactness argument about f32 integer sums, because nothing
/// published covers replacing a running float count with an integer one bit for bit.
/// See also: D. Goldberg, "What Every Computer Scientist Should Know About Floating-Point
/// Arithmetic", ACM Computing Surveys, March 1991,
/// <https://docs.oracle.com/cd/E19957-01/806-3568/ncg_goldberg.html> (IEEE 754 operations are
/// "computed exactly and then rounded").
fn ink_shares(counts: &[usize], n: usize) -> Vec<f32> {
    /// Where an f32 running count of ones stops growing.
    const F32_COUNT_LIMIT: usize = 1 << 24;
    counts
        .iter()
        .map(|&c| c.min(F32_COUNT_LIMIT) as f32 / n.max(1) as f32)
        .collect()
}

#[cfg(test)]
mod tests;
