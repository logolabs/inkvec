//! Undoing a nearest-neighbour upscale at intake: the exact lattice inverse.
//!
//! Someone who has a 96-px logo and wants a big SVG resizes the PNG first. Every viewer
//! does that with nearest neighbour on request, and plenty do it without being asked
//! (browser zoom, 125-350 % screenshots, "pixel-perfect" export settings), so what arrives
//! is the source drawn as a grid of constant cells. The tracer then describes exactly what
//! it is given: on a 96-px logo blown up to 768 the palette shatters from 3 inks to 13, and
//! the boundary comes back as 1568 straight lines walking round pixel corners -- the
//! staircase is not an artefact of the fit, it is in the file. [`pixel_grid`] finds that grid
//! and [`PixelGrid::reduce`] hands the tracer the source pixels back; the intake in `lib.rs`
//! runs it before any other resampling, and the SVG is still written at the size that
//! arrived.
//!
//! # The problem, stated exactly
//!
//! A nearest-neighbour resize of an `m`-pixel line to `n > m` pixels gives output pixel `x`
//! the value of source pixel `idx(x)`, where `idx` is non-decreasing and steps by one at the
//! *cell starts*. Libraries differ in where the steps fall -- Pillow, scikit-image and
//! OpenCV's `INTER_NEAREST_EXACT` sample near the pixel centre, `idx(x) ≈ ⌊(x + ½)·m/n⌋`
//! (OpenCV in 16-bit fixed point); OpenCV's `INTER_NEAREST` at the corner,
//! `idx(x) = ⌊x·m/n⌋` (`cvFloor(x*ifx)` in its `resize.cpp`); a float scale breaks ties at
//! exact half pixels either way -- but in every convention the `i`-th cell starts within
//! one pixel of `φ + i·p`, with pitch `p = n/m` and some phase `φ`. In
//! two dimensions the image is such a resize along each axis, with one scale `s` for both
//! (`n_x ≈ s·m_x`, `n_y ≈ s·m_y`, each side rounded).
//!
//! The inverse: find `m_x`, `m_y` and the cell starts, and keep one pixel per cell. It is
//! **exact** when every cell is constant, because then repeating each kept pixel over its
//! cell rebuilds the input bit for bit; nothing is averaged, guessed or lost. That is what
//! [`pixel_grid`] verifies before it answers (`PixelGrid::reproduces`), so the function can
//! only fire on a raster that *is* a nearest-neighbour expansion of a smaller one, and the
//! reduction it then makes is information-preserving by construction.
//!
//! # How, in passes
//!
//! 1. **Change positions** ([`line_changes`]): column `x ≥ 1` is a change when it differs
//!    from column `x − 1` in any pixel of any row, bit for bit; likewise rows. A cell start
//!    need not be a change (two neighbouring source pixels can be equal), but every change
//!    must be a cell start. Two adjacent changes mean a one-pixel cell, which no pitch of
//!    2 or more makes, so the scan stops at the first such pair: on a native render that is
//!    the first anti-aliased curve, a few rows into the content.
//! 2. **Admissible source sizes, per axis** ([`admissible_sources`]): `m` is admissible when
//!    every change `b` (and `0`, the first cell's start) lies in `[φ + i·p, φ + i·p + 1]`
//!    for one phase `φ`, which is the statement that the residues `b mod p` fit in an arc
//!    of length at most one pixel on the circle of circumference `p`. Multiplying through by
//!    `m` makes it integer arithmetic with no tolerance: the residues `q_b = b·m mod n` must
//!    fit in an arc of at most `m` on the circle of circumference `n` ([`residue_arc`]). For a
//!    fractional pitch the arc is closed, because a float scale breaks exact-half-pixel ties
//!    inconsistently: Pillow's 3.5x put alternate cell starts at either end of the window.
//!    For a whole pitch it is half-open (shorter than `m`): there every window would hold two
//!    integers, and a closed arc would admit any change set at pitch 2.
//!    Before any of this, the two axes together must show at least [`MIN_CHANGES`] changes,
//!    so that a lattice is a finding and not a coincidence (the bound is at the constant).
//! 3. **One scale** ([`coarsest_pair`]): `(m_x, m_y)` such that some `s` has
//!    `|n_x − s·m_x| < 1` and `|n_y − s·m_y| < 1`, and of those the fewest source pixels.
//!    The coarsest is the source itself whenever the source's own pixels differ often enough
//!    to pin its grid; where they do not (a flat run), a coarser lattice reproduces the input
//!    just as exactly, and the uniform mapping back is still within one pixel of the cell
//!    starts. (On the r2-inputs stress set, 55 of the 56 nearest upscales come back as their
//!    128 px source bit for bit; the 56th, a flat brown square, as a 127 px raster that
//!    re-expands to the same input.)
//! 4. **Cell starts** ([`cell_starts`]): the lattice point `⌈φ + i·p⌉` for each source pixel,
//!    moved onto the change in its window where there is one.
//! 5. **Verification** (`PixelGrid::reproduces`): every pixel equals its cell's first pixel,
//!    bit for bit. Only a grid that passes is returned.
//!
//! # Limits
//!
//! * Pitch at least 2 on both axes (`m ≤ n / 2`). Fractional factors between 1 and 2 (125 %
//!   and 150 % screenshots) are not undone: their cells are one or two pixels long, so a
//!   native render whose edges happen to sit on pixel boundaries also fits such a lattice --
//!   on the r2-inputs stress set a 24 px lucide icon did, and was reduced to 19 x 20.
//! * At least [`MIN_SOURCE_LONG`] source pixels on the long axis and [`MIN_SOURCE`] on the
//!   short one. A smaller source comes back as the coarsest finer lattice that keeps the
//!   floors (a 32 px sprite at 8x as 64 px), or not at all.
//! * Exact repetition only. A nearest upscale that was then JPEG-compressed has no constant
//!   cells and is not undone here (the r2-inputs report's tolerant unblock is the follow-up).
//!
//! Not from the literature: an exact inverse decided by an integer residue test and a
//! bit-for-bit re-expansion, because the published resampling detectors are statistical --
//! they estimate a periodic correlation of an interpolated signal and say "resampled, by
//! about this much" -- and cannot promise that the reduction loses nothing. See also:
//! A. C. Popescu, H. Farid, "Exposing Digital Forgeries by Detecting Traces of Resampling",
//! IEEE Trans. Signal Processing 53(2):758–767, 2005, DOI 10.1109/TSP.2004.839932. See also,
//! for the shape of the answer: the cell lengths along one axis are a mechanical word of
//! slope `p` (the differences of `⌈φ + i·p⌉`), the object of M. Lothaire, "Algebraic
//! Combinatorics on Words", Cambridge University Press, 2002, chapter 2 "Sturmian Words"
//! (pp. 45–110), DOI 10.1017/CBO9781107326019; the residue arc is our own test that a set of
//! change positions lies on one such sequence. The sampling conventions are OpenCV's
//! `InterpolationFlags` (`modules/imgproc/include/opencv2/imgproc.hpp`, read 2026-10-04):
//! `INTER_NEAREST`, "nearest neighbor interpolation", against `INTER_NEAREST_EXACT`, "Bit
//! exact nearest neighbor interpolation. This will produce same results as the nearest
//! neighbor method in PIL, scikit-image or Matlab"; and `resizeNN` / `resizeNN_bitexact` in
//! `modules/imgproc/src/resize.cpp`, <https://github.com/opencv/opencv>.
//!
//! Integer arithmetic here avoids the `%` operator: wazero's arm64 compiler miscompiled
//! `i32.rem_u` in a hot loop (the Go binding runs this crate as WebAssembly), so remainders
//! are written as `t − ⌊t / n⌋·n`.

use inkvec_trace::Rgba;

/// Least source size kept on the *short* axis, in px.
///
/// The old test kept at least 64 px on each side, and that refused to finish the job on
/// wordmarks: a 128 x 50 logo blown up 4x was undone only 2x (its short side would have
/// dropped under 64) and traced as a still-blocky 256 x 100 raster. On five such r2-inputs
/// wordmarks, undoing the whole 4x took dE00 from 1.971 to 1.400 and the parameters from
/// 928 to 327. The 64 px floor stays on the long side ([`MIN_SOURCE_LONG`]); this one only
/// stops a degenerate strip.
pub(crate) const MIN_SOURCE: usize = 16;

/// Least source size kept on the *long* axis, in px: the old floor, for the old reason. "A
/// 2x undo of a small icon leaves too few pixels for the boundary solve to work with": a
/// 32 px sprite blown up 8x comes back at 64 px (a finer lattice of the same image, still
/// exact), not at 32, where one sprite pixel is one traced pixel and the fit rounds its
/// steps. Between the two floors, [`coarsest_pair`] keeps the coarsest lattice that leaves
/// the long side this many pixels.
pub(crate) const MIN_SOURCE_LONG: usize = 64;

/// Least number of change positions, both axes together, for a lattice to be believed.
///
/// A native drawing whose edges happen to fall on pixel boundaries (no anti-aliasing, so no
/// adjacent changes) can fit some lattice by accident. The windows of one pixel at pitch
/// `p ≥ 2` hold at most 3 of every 5 integers (the closed windows of `p = 5/2`; at a pitch
/// `(2d+1)/d` they hold `(d+1)/(2d+1)`, at a whole pitch `1/p`), so `r` changes placed
/// independently of the lattice fit one with probability at most `0.6^r`. Counting the
/// hypotheses -- under `n/2` source sizes and `p + 1` phases on the first axis, then about
/// three sizes and `p + 1` phases on the second, which one scale ties to the first -- the
/// chance of an accidental fit is below `3·n·(p+1)²·0.6^r`, about `2·10⁻⁶` at `n = 2048`,
/// `p = 2.5` and `r = 48`, and it falls by 0.6 per further change. (A heuristic bound: real
/// edges are not independent, but nothing here depends on it being tight.) A real nearest
/// upscale of anti-aliased art shows a change at nearly every source pixel boundary,
/// hundreds at 128 px; an upscale of art with fewer than 48 edges in all is left as it
/// arrived, which costs little, because its cells are large flat rectangles the fitter
/// draws exactly anyway.
pub(crate) const MIN_CHANGES: usize = 48;

/// The nearest-neighbour grid of an image: where each source pixel's cell starts, per axis.
///
/// `xs[i]` is the first column of source column `i`'s cell and `ys[j]` the first row of
/// source row `j`'s; both start at 0 and ascend strictly, and the last cell runs to the image
/// edge. Built only by [`pixel_grid`], which has verified that every cell is constant.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PixelGrid {
    xs: Vec<usize>,
    ys: Vec<usize>,
}

impl PixelGrid {
    /// The source raster's size, `(m_x, m_y)`.
    pub(crate) fn source_size(&self) -> (usize, usize) {
        (self.xs.len(), self.ys.len())
    }

    /// One pixel per cell: the source raster, bit for bit (up to the conventions below).
    ///
    /// The value kept is the cell's first pixel, which equals every pixel of the cell, and
    /// therefore also the cell's area average. It is written with the area average's
    /// conventions (`coverage::downsample_to`, which the integer-factor unblock used before):
    /// alpha clamped to `[0, 1]` and a non-finite alpha read as 0; colour clamped to `[0, 1]`,
    /// a non-finite channel read as 0, and the colour of a cell with no positive alpha
    /// written as 0. So for an integer factor the result is `downsample_to(img, w/k, h/k)`
    /// bit for bit on any decoded input: there every cell has `k²` equal pixels, the
    /// premultiplied sum `Σ c·a` of `k² ≤ 2¹⁰` equal terms is within `2⁻⁴³` relative of
    /// `k²·c·a` in f64, and its quotient by the exact `Σ a` rounds back to the f32 `c`.
    /// (The area average zeroes colour below `Σ a = 10⁻⁹`; a decoded alpha is 0 or at least
    /// `1/65535`, so the two thresholds agree.) O(source pixels).
    pub(crate) fn reduce(&self, img: &Rgba) -> Rgba {
        let (mw, mh) = self.source_size();
        let mut data = Vec::with_capacity(mw * mh * 4);
        for &sy in &self.ys {
            for &sx in &self.xs {
                let p = img.pixel(sx, sy);
                let a = if p[3].is_finite() {
                    p[3].clamp(0.0, 1.0)
                } else {
                    0.0
                };
                let painted = p[3] > 0.0;
                for &c in &p[..3] {
                    data.push(if painted && c.is_finite() {
                        c.clamp(0.0, 1.0)
                    } else {
                        0.0
                    });
                }
                data.push(a);
            }
        }
        Rgba {
            width: mw,
            height: mh,
            data,
        }
    }

    /// The pitch along each axis, `(n_x / m_x, n_y / m_y)`, for an image `w × h`: how many
    /// output pixels one source pixel became.
    pub(crate) fn pitch(&self, w: usize, h: usize) -> (f64, f64) {
        let (mw, mh) = self.source_size();
        (w as f64 / mw as f64, h as f64 / mh as f64)
    }

    /// Whether repeating each cell's first pixel over the cell rebuilds `img` bit for bit:
    /// every row equal to its cell's first row, and in each cell's first row every pixel
    /// equal to its cell's first pixel. Compared as bits, so `-0.0` differs from `0.0` and
    /// a NaN equals only the same NaN. O(pixels), stopping at the first difference.
    fn reproduces(&self, img: &Rgba) -> bool {
        let (w, h) = (img.width, img.height);
        let stride = w * 4;
        let row = |y: usize| &img.data[y * stride..(y + 1) * stride];
        // Every row against its cell's first row.
        let rows_match = cells(&self.ys, h).all(|(start, end)| {
            let first = row(start);
            (start + 1..end).all(|y| same_bits(first, row(y)))
        });
        // In each first row, every pixel against its cell's first pixel.
        rows_match
            && self.ys.iter().all(|&y| {
                let r = row(y);
                cells(&self.xs, w).all(|(start, end)| {
                    let first = &r[start * 4..start * 4 + 4];
                    (start + 1..end).all(|x| same_bits(first, &r[x * 4..x * 4 + 4]))
                })
            })
    }
}

/// The cells `[starts[i], starts[i + 1])`, the last one ending at `n`.
fn cells(starts: &[usize], n: usize) -> impl Iterator<Item = (usize, usize)> + '_ {
    starts
        .iter()
        .enumerate()
        .map(move |(i, &s)| (s, starts.get(i + 1).copied().unwrap_or(n)))
}

/// The nearest-neighbour grid this image was blown up on, if it is a nearest-neighbour
/// upscale by at least 2 on both axes of a raster at least [`MIN_SOURCE`] px on its short
/// side and [`MIN_SOURCE_LONG`] on its long side.
///
/// The passes, their invariants and the literature are in the module documentation. In
/// short: the change positions of each axis ([`line_changes`]), the source sizes whose
/// lattice contains every change ([`admissible_sources`]), the coarsest pair of them that
/// is one scale ([`coarsest_pair`]), the cell starts ([`cell_starts`]), and a bit-for-bit
/// check that repeating one pixel per cell rebuilds the image. `None` for anything else:
/// a zero side or a short buffer, an image too small to leave the floors at pitch 2,
/// adjacent changes, fewer than [`MIN_CHANGES`] changes (a flat image among them: there is
/// nothing to gain), no admissible pair, or a failed check.
///
/// Called by the intake in `lib.rs` (unless `--no-unblock`), before any other resampling.
/// On a native render it costs one pass over the rows down to the first anti-aliased curve.
pub(crate) fn pixel_grid(img: &Rgba) -> Option<PixelGrid> {
    let (w, h) = (img.width, img.height);
    // An image with a zero side has no cells. A GIF with a zero-wide logical screen once
    // reached the old test as 0 x 30000 and divided by zero (intake fuzz, 2026-10-02); the
    // decoder now refuses such files, and this keeps the function total for any caller that
    // builds an `Rgba` itself.
    if w.min(h) < 2 * MIN_SOURCE || w.max(h) < 2 * MIN_SOURCE_LONG || img.data.len() < w * h * 4 {
        return None;
    }
    let (cols, rows) = line_changes(img)?;
    if cols.len() + rows.len() < MIN_CHANGES {
        return None;
    }
    let (mx, my) = coarsest_pair(
        &admissible_sources(&cols, w),
        &admissible_sources(&rows, h),
        w,
        h,
    )?;
    let grid = PixelGrid {
        xs: cell_starts(&cols, w, mx)?,
        ys: cell_starts(&rows, h, my)?,
    };
    grid.reproduces(img).then_some(grid)
}

/// The change positions of each axis, `(columns, rows)`, ascending: column `x ≥ 1` when some
/// row's pixel `x` differs from its pixel `x − 1`, row `y ≥ 1` when it differs from row
/// `y − 1`, bit for bit in any channel. `None` as soon as two changes are adjacent on either
/// axis, which no pitch of 2 or more can produce.
///
/// A row identical to the one above adds no column change (its left-neighbour comparisons
/// are the same as that row's), so only the first row and the rows that change are walked
/// pixel by pixel; on an upscale that is one row per source row. Rows are compared as whole
/// slices first ([`same_bits`], which vectorises), and so is a row with itself shifted by one
/// pixel, which settles a flat row (the blank page above a logo) without the pixel loop.
/// O(pixels read); on a native render the scan stops at the first anti-aliased curve, whose
/// coverage changes on consecutive columns or rows.
fn line_changes(img: &Rgba) -> Option<(Vec<usize>, Vec<usize>)> {
    let (w, h) = (img.width, img.height);
    let stride = w * 4;
    let mut col = vec![false; w];
    let mut rows: Vec<usize> = Vec::new();
    for y in 0..h {
        let row = &img.data[y * stride..(y + 1) * stride];
        if y > 0 {
            if same_bits(&img.data[(y - 1) * stride..y * stride], row) {
                continue;
            }
            if rows.last() == Some(&(y - 1)) {
                return None;
            }
            rows.push(y);
        }
        if same_bits(&row[4..], &row[..stride - 4]) {
            continue;
        }
        for x in 1..w {
            if !col[x] && !same_bits(&row[(x - 1) * 4..x * 4], &row[x * 4..x * 4 + 4]) {
                if col[x - 1] || (x + 1 < w && col[x + 1]) {
                    return None;
                }
                col[x] = true;
            }
        }
    }
    Some(((1..w).filter(|&x| col[x]).collect(), rows))
}

/// Whether two equally long float slices hold the same bits. Compared 64 floats at a time
/// with an OR of XORs, a loop without an early exit that the compiler vectorises; the early
/// exit is per block.
pub(super) fn same_bits(a: &[f32], b: &[f32]) -> bool {
    a.len() == b.len()
        && a.chunks(64).zip(b.chunks(64)).all(|(x, y)| {
            x.iter()
                .zip(y)
                .fold(0u32, |acc, (p, q)| acc | (p.to_bits() ^ q.to_bits()))
                == 0
        })
}

/// `t mod n` for `n > 0`, without the remainder operator (see the module documentation).
fn rem(t: u64, n: u64) -> u64 {
    t - (t / n) * n
}

/// `⌊a / b⌋` for `b > 0`, rounding toward −∞ for a negative `a`, without `%` or
/// `div_euclid` (which is written with `%`).
fn floor_div(a: i64, b: i64) -> i64 {
    let q = a / b;
    if q * b > a {
        q - 1
    } else {
        q
    }
}

/// The smallest arc of the circle of circumference `n` holding every residue
/// `q_b = b·m mod n` of the changes `b` and of 0: `(length, start)`, both in the residue
/// units (`1/m` px). The arc is the complement of the widest gap between circularly
/// consecutive residues; it starts at the residue after that gap (the first such gap, on a
/// tie). With no changes the arc is the single point 0. `q` is scratch space.
/// O(r log r) for `r` changes.
fn residue_arc(changes: &[usize], n: usize, m: usize, q: &mut Vec<u64>) -> (u64, u64) {
    let (n, m) = (n as u64, m as u64);
    q.clear();
    q.push(0);
    q.extend(changes.iter().map(|&b| rem(b as u64 * m, n)));
    q.sort_unstable();
    q.dedup();
    let last = q.len() - 1;
    // The gap that wraps from the last residue round to the first; the arc then starts at
    // the first.
    let (mut widest, mut start) = (q[0] + n - q[last], q[0]);
    for i in 1..q.len() {
        let gap = q[i] - q[i - 1];
        if gap > widest {
            (widest, start) = (gap, q[i]);
        }
    }
    (n - widest, start)
}

/// The source sizes `m` whose lattice of pitch `n/m` contains every change: `MIN_SOURCE ≤
/// m ≤ n/2` with the residue arc ([`residue_arc`]) no longer than `m` (one pixel), and
/// shorter than `m` when `m` divides `n`. Ascending. On an axis with no change every `m` in
/// range is admissible.
///
/// *Why `≤ m`.* `b` lies in the window `[φ + i·p, φ + i·p + 1]` of some cell `i` exactly when
/// `(b − φ) mod p ∈ [0, 1]`; scaled by `m`, `(q_b − m·φ) mod n ∈ [0, m]`. So a phase exists
/// for every change at once if and only if the residues fit in a closed arc of length `m`.
/// The closed end is there for the ties a float scale breaks either way at a fractional
/// pitch. *Why `< m` at a whole pitch.* A whole pitch has no ties (every implementation
/// computes the same cells), and its windows `[φ + i·k, φ + i·k + 1]` with an integer phase
/// hold two integers each: at `k = 2` they cover every integer, and the closed test would
/// admit any change set with no two adjacent. Half-open, a whole pitch is the old block test:
/// every change at the same position modulo `k`.
/// O((n/2) · r log r) for `r` changes; `r ≤ n/2` because changes are never adjacent.
fn admissible_sources(changes: &[usize], n: usize) -> Vec<usize> {
    let mut q = Vec::with_capacity(changes.len() + 1);
    (MIN_SOURCE..=n / 2)
        .filter(|&m| {
            let spread = residue_arc(changes, n, m, &mut q).0;
            let whole = (n / m) * m == n;
            spread < m as u64 || (!whole && spread == m as u64)
        })
        .collect()
}

/// Of the admissible `(m_x, m_y)`, the pair that is one scale, keeps the long side at least
/// [`MIN_SOURCE_LONG`] px and has the fewest source pixels (ties to the smaller `m_x`), or
/// `None`.
///
/// *One scale* means some `s` with `|w − s·m_x| < 1` and `|h − s·m_y| < 1`: the open
/// intervals `((w−1)/m_x, (w+1)/m_x)` and `((h−1)/m_y, (h+1)/m_y)` overlap, which in integers
/// is `(w−1)·m_y < (h+1)·m_x` and `(h−1)·m_x < (w+1)·m_y`. A resize keeps the aspect up to
/// the rounding of each side, and this asks for no more than that; it is also what stops an
/// axis with no change from collapsing to [`MIN_SOURCE`] on its own. O(|ax| · |ay|).
fn coarsest_pair(ax: &[usize], ay: &[usize], w: usize, h: usize) -> Option<(usize, usize)> {
    let mut best: Option<(usize, usize)> = None;
    for &mx in ax {
        for &my in ay {
            let one_scale = (w - 1) * my < (h + 1) * mx && (h - 1) * mx < (w + 1) * my;
            let long_enough = mx.max(my) >= MIN_SOURCE_LONG;
            if one_scale && long_enough && best.is_none_or(|(bx, by)| mx * my < bx * by) {
                best = Some((mx, my));
            }
        }
    }
    best
}

/// The first pixel of each of the `m` cells along an axis of `n` pixels whose changes are
/// `changes`: the lattice point `⌈φ + i·p⌉`, with `p = n/m` and the phase `φ` at the start of
/// the residue arc, except where a change sits in the cell's window `[φ + i·p, φ + i·p + 1]`,
/// which then starts the cell. `None` when the starts do not ascend strictly from 0.
///
/// In residue units the lattice points are `L_i = (i·n + a)/m` for the arc start `a`. When
/// `a > 0` the cell holding pixel 0 is `i = −1` (`L_{−1} = (a − n)/m ∈ (−1, 0)`, and 0 is in
/// its window because 0's residue lies on the arc); otherwise it is `i = 0`. Since `L_{i+m} =
/// L_i + n`, exactly `m` cells cover `[0, n)`. A change `b` belongs to cell
/// `⌊(b·m − a)/n⌋`. Two changes in one window would leave one of them inside a cell; the
/// verification in [`pixel_grid`] rejects that. O(m + r).
fn cell_starts(changes: &[usize], n: usize, m: usize) -> Option<Vec<usize>> {
    let mut q = Vec::with_capacity(changes.len() + 1);
    let (_, a) = residue_arc(changes, n, m, &mut q);
    let (n, m, a) = (n as i64, m as i64, a as i64);
    let first = if a > 0 { -1 } else { 0 };
    // ⌈(i·n + a)/m⌉ = −⌊−(i·n + a)/m⌋; clamped at 0 for the partial first cell.
    let mut starts: Vec<usize> = (first..first + m)
        .map(|i| (-floor_div(-(i * n + a), m)).max(0) as usize)
        .collect();
    for &b in changes {
        let cell = floor_div(b as i64 * m - a, n) - first;
        if let Some(s) = usize::try_from(cell).ok().and_then(|c| starts.get_mut(c)) {
            *s = b;
        }
    }
    let ascending = starts.first() == Some(&0)
        && starts.windows(2).all(|p| p[0] < p[1])
        && starts.last().is_some_and(|&s| (s as i64) < n);
    ascending.then_some(starts)
}
