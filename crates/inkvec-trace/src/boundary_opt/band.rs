//! The data term over a fixed narrow band, with each pixel's coverage accumulated from
//! signed areas along its row.
//!
//! # Why
//!
//! The first form of this stage summed the rendering error only over the pixels a boundary
//! cut, decided pixel by pixel. That makes the energy jump whenever a boundary leaves or
//! enters a pixel (the pixel drops out of the sum, or joins it), and it skipped pixels
//! where a chain re-entered or two boundaries met. Measured on the 246-icon screen set, a
//! quarter of every accepted decrease came from pixels leaving the sum, not from fitting
//! them, and 170 of 246 solves stopped because a step of 0.0014 px raised the energy by a
//! whole pixel's residual: the line search was not converging, it was hitting the jumps.
//!
//! # What
//!
//! The error is summed over a set of pixels fixed for the whole solve, the band `B`, and
//! every pixel of the band is rendered exactly from the current geometry:
//!
//! ```text
//! E_data(x) = Σ_{p ∈ B} w_p · Σ_k ( Σ_f cov_f(p; x) · c_{f,k}(p) − t_{p,k} )²
//! ```
//!
//! with `cov_f(p)` the area of face `f` inside pixel `p` (a partition: they sum to 1) and
//! `w_p` a weight fixed at the start. Leaving a pixel now costs exactly what being a pure
//! pixel of the other face costs, so the energy is a continuous function of the points.
//!
//! * **Whole-domain region fidelity.** The residual is the Chan–Vese / Mumford–Shah region
//!   term with each face's fill model as its constant, integrated over the pixels rather
//!   than only over the pixels the curve touches: T. F. Chan, L. A. Vese (2001), *Active
//!   contours without edges*, IEEE TIP 10(2), <https://doi.org/10.1109/83.902291>; formula as
//!   in P. Getreuer (2012), *Chan–Vese Segmentation*, IPOL 2:214–224,
//!   <https://doi.org/10.5201/ipol.2012.g-cv>. Adapted: the faces' models are fixed during
//!   the solve and the domain is restricted to the band below.
//! * **Narrow band.** Only pixels near the boundary can change, so only they are summed:
//!   D. Adalsteinsson, J. A. Sethian (1995), *A fast level set method for propagating
//!   interfaces*, J. Comput. Phys. 118, <https://doi.org/10.1006/jcph.1995.1098>. Adapted:
//!   no point moves more than `MAX_TOTAL` = 1 px, so the band (every pixel within one
//!   pixel of a pixel the starting boundary crosses) never needs rebuilding, and the pixel
//!   set is truly fixed.
//! * **Exact box coverage by signed-area accumulation.** Each piece of boundary inside a
//!   pixel deposits the signed area between itself and the pixel's right side, and its
//!   height is carried to every pixel to its right; the coverage of a pixel is its own
//!   deposits plus the carry. This is the accumulation-buffer rasteriser of libart and
//!   R. Levien's font-rs (<https://github.com/raphlinus/font-rs>), an exact box filter as in
//!   J. Manson, S. Schaefer (2011), *Wavelet Rasterization*, Computer Graphics Forum 30(2),
//!   <https://doi.org/10.1111/j.1467-8659.2011.01887.x>. Adapted: one carry per face instead
//!   of one winding number, since the faces are a partition; and the derivative of the
//!   carry is a suffix sum along the row, so the gradient costs one more pass. No pixel
//!   decides "which side" is which: a piece lying on a pixel's border simply deposits zero
//!   area and a full carry, the case the per-pixel construction got wrong.
//!
//! # The band and its seeds
//!
//! `B` is every pixel within one pixel (Chebyshev) of a pixel holding a piece at the start.
//! A point moves at most 1 px, so a piece can only ever land in `B`. In each row `B` is a set
//! of runs; the pixel left of a run is never touched, so its whole area is one face, found
//! once at the start by carrying from the left edge of the image (where the outside is the
//! only face, and the frame edges on `x = −½` turn it into the first face). A run starting
//! at column 0 also takes the pieces left of the image (the frame, and anything pushed past
//! it) every time.

use super::{scatter, Piece, Problem, Prov};
use inkvec_core::Point;

/// The outside of the image, as a face id.
pub(super) const OUT: u16 = u16::MAX;

/// How far around the starting boundary the band reaches, in pixels (Chebyshev). A point
/// moves at most `MAX_TOTAL` = 1 px, so a piece stays within one pixel of a pixel holding a
/// piece at the start (see `nearest_in_band` for the one exception).
const REACH: i64 = 1;

/// One horizontal run of band pixels, and the faces that can appear in it.
#[derive(Clone, Copy, Debug)]
pub(super) struct Run {
    pub y: u32,
    pub x0: u32,
    /// Last pixel, inclusive.
    pub x1: u32,
    /// Index of the run's first pixel in the per-pixel arrays.
    pub first: u32,
    /// The run's faces are `faces[fstart .. fstart + nf]`.
    pub fstart: u32,
    pub nf: u32,
    /// Slot of the face filling everything left of the run.
    pub seed: u32,
    /// The run's colours are `colour[cstart ..]`, `nf` per pixel.
    pub cstart: u32,
    /// The run's prefix sums are `prefix[pstart ..]` (see `fill_prefix`).
    pub pstart: u32,
}

/// The fixed band: runs, their faces and those faces' colours at every pixel, and the
/// fixed weights.
pub(super) struct Band {
    pub runs: Vec<Run>,
    pub faces: Vec<u16>,
    /// Per run, per pixel, per face slot: the face's colour at the pixel centre (the
    /// target colour for the outside, which then carries no residual).
    pub colour: Vec<[f64; 3]>,
    /// Same layout: the face's opacity (the target's alpha for the outside); empty when
    /// alpha is not a channel.
    pub opacity: Vec<f64>,
    /// Data weight of each band pixel, fixed for the solve.
    pub weight: Vec<f64>,
    /// Whether alpha is a fourth channel at each band pixel, fixed for the solve.
    pub alpha: Vec<bool>,
    pub total_cells: usize,
    /// Whether each pixel of the image is in the band.
    pub inb: Vec<bool>,
    /// Per run, the starting residual of its pixels no boundary touched at the start.
    pub run_const: Vec<f64>,
    /// Per run, prefix sums over its pixels of `w·c_i·c_j` (`i ≤ j`), `w·c_i·t` and
    /// `w·t·t`, for summing a stretch without pieces at once (see `band_run`).
    pub prefix: Vec<f64>,
}

/// Scratch for one evaluation of the band term.
#[derive(Default)]
pub(super) struct BandScratch {
    /// Each run's energy.
    run_e: Vec<f64>,
    /// Each run's energy in pixels holding pieces (the rest is its stretches without).
    run_cut: Vec<f64>,
    /// Each piece's gradient at its two ends.
    piece_g: Vec<[f64; 4]>,
}

/// Scratch for one run (one per thread).
#[derive(Default)]
struct RunScratch {
    carry: Vec<f64>,
    cov: Vec<f64>,
    /// Per pixel of the run, per face slot: the derivative of the energy by that coverage.
    g: Vec<f64>,
    /// Suffix sums of `g` along the run.
    suf: Vec<f64>,
    /// The gradients of the run's pieces.
    out: Vec<(u32, [f64; 4])>,
    /// The run's energy in pixels holding pieces.
    cut: f64,
    /// The run's segments in order: `(first pixel, end pixel or u32::MAX for one pixel
    /// holding pieces, offset into kap or g)`.
    segs: Vec<(u32, u32, u32)>,
    /// The carry of each stretch.
    kap: Vec<f64>,
}

/// Below this many band pixels the runs are evaluated on one thread (the result is the
/// same either way).
const PARALLEL_CELLS: usize = 16384;

/// The slot of face `f` among a run's faces. A face with no slot cannot occur (the run's
/// faces are every face within reach of it at the start); it maps to slot 0 rather than
/// failing.
#[inline]
fn slot(faces: &[u16], f: u16) -> usize {
    faces.iter().position(|&g| g == f).unwrap_or(0)
}

impl Problem<'_> {
    /// Cut every segment of every edge at its gridline crossings and file each piece
    /// under the pixel holding its midpoint. Edges on the outside of the image are kept,
    /// pieces left of the image are filed under their row (`vhead`), and a crossing is put
    /// exactly on its gridline, so that the pieces of a chain crossing a row add up to that
    /// row's height exactly.
    pub(super) fn bucket_band(&mut self, pos: &[Point]) {
        self.pieces.clear();
        for &cell in &self.touched {
            self.head[cell] = -1;
        }
        self.touched.clear();
        for v in self.vhead.iter_mut() {
            *v = -1;
        }
        let (w, h) = (self.w, self.h);
        let mut cr = std::mem::take(&mut self.scratch.cross);
        let inb = self.band.as_ref().map(|b| &b.inb[..]);
        let edges = self.map.edges.len();
        let active = self.active.as_ref().map(|a| &a.edges[..]);
        let count = active.map_or(edges, |a| a.len());
        for idx in 0..count {
            let k = active.map_or(idx, |a| a[idx] as usize);
            let e = &self.map.edges[k];
            let ids = &self.vars.var[k];
            let n = ids.len();
            if n < 2 {
                continue;
            }
            let last = if e.closed { n } else { n - 1 };
            for i in 0..last {
                // The next vertex without `%`: under wazero's arm64 compiler (v1.12.0, the Go
                // package on Apple silicon) this loop's `i32.rem_u` divided a clobbered
                // register and indexed `ids` at 1 - 168 * 6, "len is 6 but the index is
                // 4294966289". The same module ran correctly under wazero on amd64.
                let (va, vb) = (ids[i], if i + 1 == n { ids[0] } else { ids[i + 1] });
                let pieces = &mut self.pieces;
                let (head, vhead, touched) = (&mut self.head, &mut self.vhead, &mut self.touched);
                cut_segment(
                    pos,
                    va,
                    vb,
                    &mut cr,
                    |from, to, from_prov, to_prov, px, py| {
                        if py < 0.0 || py >= h as f64 || px >= w as f64 {
                            return;
                        }
                        let y = py as usize;
                        let slot = if px < 0.0 {
                            &mut vhead[y]
                        } else {
                            let mut x = px as usize;
                            if let Some(inb) = inb {
                                x = nearest_in_band(inb, w, y, x);
                            }
                            let cell = y * w + x;
                            if head[cell] < 0 {
                                touched.push(cell);
                            }
                            &mut head[cell]
                        };
                        pieces.push(Piece {
                            edge: k as u32,
                            l: e.left,
                            r: e.right,
                            from,
                            to,
                            from_prov,
                            to_prov,
                            next: *slot,
                        });
                        *slot = pieces.len() as i32 - 1;
                    },
                );
            }
        }
        self.scratch.cross = cr;
        self.order_pieces();
    }

    /// Lay the pieces out in pixel order (rows, then columns; each pixel's pieces
    /// together), so that the passes over the band read them front to back instead of
    /// hopping through memory in the order the edges were walked. The lists keep their
    /// meaning; only the order of the pieces within one pixel changes.
    fn order_pieces(&mut self) {
        self.touched.sort_unstable();
        let mut out: Vec<Piece> = std::mem::take(&mut self.spare);
        out.clear();
        out.reserve(self.pieces.len());
        let relink = |head: &mut i32, pieces: &[Piece], out: &mut Vec<Piece>| {
            let mut id = *head;
            if id < 0 {
                return;
            }
            *head = out.len() as i32;
            while id >= 0 {
                let mut pc = pieces[id as usize];
                id = pc.next;
                pc.next = if id >= 0 { out.len() as i32 + 1 } else { -1 };
                out.push(pc);
            }
        };
        for y in 0..self.h {
            relink(&mut self.vhead[y], &self.pieces, &mut out);
        }
        for &cell in &self.touched {
            relink(&mut self.head[cell], &self.pieces, &mut out);
        }
        self.spare = std::mem::replace(&mut self.pieces, out);
    }

    /// The faces either side of piece `i`: (left, right).
    #[inline]
    fn sides(&self, i: usize) -> (u16, u16) {
        let pc = &self.pieces[i];
        (pc.l, pc.r)
    }

    /// The band term at `pos` (after [`Problem::bucket_band`] on the same `pos`), with its
    /// gradient added into `grad` when asked for, over the active part's runs (or all).
    ///
    /// The runs are independent, so large bands are evaluated in parallel; each run's
    /// energy is kept apart and summed in run order, and each piece's gradient is kept
    /// apart and pushed onto the unknowns in piece order, so the result does not depend on
    /// how the runs were shared between threads.
    pub(super) fn band_data(
        &self,
        band: &Band,
        pos: &[Point],
        bs: &mut BandScratch,
        grad: Option<&mut [Point]>,
    ) -> f64 {
        use rayon::prelude::*;
        let with_grad = grad.is_some();
        let all_runs: Vec<u32>;
        let run_ids: &[u32] = match &self.active {
            Some(a) => &a.runs,
            None => {
                all_runs = (0..band.runs.len() as u32).collect();
                &all_runs
            }
        };
        let runs = run_ids.len();
        let cells: usize = run_ids
            .iter()
            .map(|&r| (band.runs[r as usize].x1 - band.runs[r as usize].x0 + 1) as usize)
            .sum();
        bs.run_e.clear();
        bs.run_e.resize(runs, 0.0);
        bs.run_cut.clear();
        bs.run_cut.resize(runs, 0.0);
        if with_grad {
            bs.piece_g.clear();
            bs.piece_g.resize(self.pieces.len(), [0.0; 4]);
        }
        if cells < PARALLEL_CELLS {
            let mut rs = RunScratch::default();
            for (k, &r) in run_ids.iter().enumerate() {
                let (e, pg) = self.band_run(band, &band.runs[r as usize], &mut rs, with_grad);
                bs.run_e[k] = e;
                for &(id, v) in pg {
                    bs.piece_g[id as usize] = v;
                }
                bs.run_cut[k] = rs.cut;
            }
        } else {
            let chunk = runs.div_ceil(64).max(1);
            type Part = (usize, Vec<(f64, f64)>, Vec<(u32, [f64; 4])>);
            let parts: Vec<Part> = run_ids
                .par_chunks(chunk)
                .enumerate()
                .map_init(RunScratch::default, |rs, (c, part)| {
                    let mut es = Vec::with_capacity(part.len());
                    let mut pgs = Vec::new();
                    for &r in part {
                        let (e, pg) = self.band_run(band, &band.runs[r as usize], rs, with_grad);
                        pgs.extend_from_slice(pg);
                        es.push((e, rs.cut));
                    }
                    (c * chunk, es, pgs)
                })
                .collect();
            for (start, es, pgs) in parts {
                for (k, (e, c)) in es.into_iter().enumerate() {
                    bs.run_e[start + k] = e;
                    bs.run_cut[start + k] = c;
                }
                for (id, v) in pgs {
                    bs.piece_g[id as usize] = v;
                }
            }
        }
        if let Some(g) = grad {
            for (pc, v) in self.pieces.iter().zip(&bs.piece_g) {
                if v[0] != 0.0 || v[1] != 0.0 {
                    scatter(pc.from_prov, v[0], v[1], pos, g);
                }
                if v[2] != 0.0 || v[3] != 0.0 {
                    scatter(pc.to_prov, v[2], v[3], pos, g);
                }
            }
        }
        bs.run_e.iter().sum()
    }

    /// [`Problem::band_data`] pixel by pixel, the reference `band_run` is tested against.
    #[cfg(test)]
    pub(super) fn band_data_cells(
        &self,
        band: &Band,
        pos: &[Point],
        grad: Option<&mut [Point]>,
    ) -> f64 {
        let with_grad = grad.is_some();
        let mut rs = RunScratch::default();
        let mut total = 0.0;
        let mut piece_g = vec![[0.0; 4]; self.pieces.len()];
        for run in &band.runs {
            let (e, pg) = self.band_run_cells(band, run, &mut rs, with_grad);
            total += e;
            for &(id, v) in pg {
                piece_g[id as usize] = v;
            }
        }
        if let Some(g) = grad {
            for (pc, v) in self.pieces.iter().zip(&piece_g) {
                scatter(pc.from_prov, v[0], v[1], pos, g);
                scatter(pc.to_prov, v[2], v[3], pos, g);
            }
        }
        total
    }

    /// Start a run: the carry is the seed face, plus the heights of the pieces left of the
    /// image when the run starts at column 0. Returns the first of those pieces (-1 for
    /// none).
    fn start_run(&self, run: &Run, faces: &[u16], bs: &mut RunScratch) -> i32 {
        let nf = faces.len();
        bs.carry.clear();
        bs.carry.resize(nf, 0.0);
        bs.carry[run.seed as usize] = 1.0;
        bs.cov.resize(nf, 0.0);
        bs.out.clear();
        bs.segs.clear();
        bs.g.clear();
        bs.kap.clear();
        bs.cut = 0.0;
        let vstart = if run.x0 == 0 {
            self.vhead[run.y as usize]
        } else {
            -1
        };
        let mut id = vstart;
        while id >= 0 {
            let pc = &self.pieces[id as usize];
            let s = pc.to.y - pc.from.y;
            bs.carry[slot(faces, pc.l)] += s;
            bs.carry[slot(faces, pc.r)] -= s;
            id = pc.next;
        }
        vstart
    }

    /// The coverage of pixel `cell` (right side at `xr`): the carry plus the area each of its
    /// pieces deposits, `A = s·(x_r − (a.x + b.x)/2)` to its left face and `−A` to its right,
    /// with `s = b.y − a.y`; then each piece's height `±s` joins the carry.
    fn deposit(&self, faces: &[u16], cell: usize, xr: f64, bs: &mut RunScratch) {
        let nf = faces.len();
        bs.cov[..nf].copy_from_slice(&bs.carry[..nf]);
        let mut id = self.head[cell];
        while id >= 0 {
            let pc = &self.pieces[id as usize];
            let s = pc.to.y - pc.from.y;
            let a = s * (xr - 0.5 * (pc.from.x + pc.to.x));
            let (sl, sr) = (slot(faces, pc.l), slot(faces, pc.r));
            bs.cov[sl] += a;
            bs.cov[sr] -= a;
            bs.carry[sl] += s;
            bs.carry[sr] -= s;
            id = pc.next;
        }
    }

    /// The weighted residual `w·‖Σ_f cov_f·c_f − t‖²` of pixel `i` of `run` at the coverage
    /// in `cov`, and, into `g` when given, `∂E/∂cov_f = 2·w·r·c_f` per face slot.
    fn pixel_energy(
        &self,
        band: &Band,
        run: &Run,
        i: usize,
        cov: &[f64],
        g: Option<&mut [f64]>,
    ) -> f64 {
        let nf = cov.len();
        let bi = run.first as usize + i;
        let wgt = band.weight[bi];
        if wgt == 0.0 {
            return 0.0;
        }
        let cell = run.y as usize * self.w + run.x0 as usize + i;
        let t = self.rgb[cell];
        let col = &band.colour[run.cstart as usize + i * nf..][..nf];
        let mut r = [-(t[0] as f64), -(t[1] as f64), -(t[2] as f64), 0.0];
        for (cv, c) in cov.iter().zip(col) {
            r[0] += cv * c[0];
            r[1] += cv * c[1];
            r[2] += cv * c[2];
        }
        let mut e = r[0] * r[0] + r[1] * r[1] + r[2] * r[2];
        let alpha_img = self.alpha.map(|(a, _)| a).filter(|_| band.alpha[bi]);
        let op: &[f64] = match alpha_img {
            Some(_) => &band.opacity[run.cstart as usize + i * nf..][..nf],
            None => &[],
        };
        if let Some(img_a) = alpha_img {
            let mut c = -(img_a[cell] as f64);
            for (cv, o) in cov.iter().zip(op) {
                c += cv * o;
            }
            r[3] = c;
            e += c * c;
        }
        if let Some(g) = g {
            for (j, (gj, c)) in g.iter_mut().zip(col).enumerate() {
                let mut d = r[0] * c[0] + r[1] * c[1] + r[2] * c[2];
                if !op.is_empty() {
                    d += r[3] * op[j];
                }
                *gj = 2.0 * wgt * d;
            }
        }
        wgt * e
    }

    /// The gradient at both ends of each piece of pixel `cell`: its area moves the energy by
    /// `da = g_L − g_R` here and its height by `ds`, the same difference summed over the
    /// pixels after it (`suf`). `A = s·off` gives `∂A/∂x = −s/2` at either end and
    /// `∂A/∂y = ∓off`; `s = y_to − y_from`.
    fn piece_grads(
        &self,
        faces: &[u16],
        cell: usize,
        xr: f64,
        g: &[f64],
        suf: &[f64],
        out: &mut Vec<(u32, [f64; 4])>,
    ) {
        let mut id = self.head[cell];
        while id >= 0 {
            let pc = &self.pieces[id as usize];
            let (sl, sr) = (slot(faces, pc.l), slot(faces, pc.r));
            let da = g[sl] - g[sr];
            let ds = suf[sl] - suf[sr];
            let s = pc.to.y - pc.from.y;
            let off = xr - 0.5 * (pc.from.x + pc.to.x);
            let gx = -0.5 * s * da;
            out.push((id as u32, [gx, -off * da - ds, gx, off * da + ds]));
            id = pc.next;
        }
    }

    /// The gradient of the pieces left of the image: their height reaches every pixel of
    /// the run (`suf` is the sum over all of them).
    fn virtual_grads(
        &self,
        faces: &[u16],
        vstart: i32,
        suf: &[f64],
        out: &mut Vec<(u32, [f64; 4])>,
    ) {
        let mut id = vstart;
        while id >= 0 {
            let pc = &self.pieces[id as usize];
            let ds = suf[slot(faces, pc.l)] - suf[slot(faces, pc.r)];
            out.push((id as u32, [0.0, -ds, 0.0, ds]));
            id = pc.next;
        }
    }

    /// One run of the band term, pixel by pixel: the reference [`Problem::band_run`] is
    /// tested against. Returns the energy and the gradient at both ends of each piece, as
    /// `(piece, [dE/dfrom.x, dE/dfrom.y, dE/dto.x, dE/dto.y])`.
    #[cfg(test)]
    fn band_run_cells<'s>(
        &self,
        band: &Band,
        run: &Run,
        bs: &'s mut RunScratch,
        with_grad: bool,
    ) -> (f64, &'s [(u32, [f64; 4])]) {
        let len = (run.x1 - run.x0 + 1) as usize;
        let nf = run.nf as usize;
        let faces = &band.faces[run.fstart as usize..run.fstart as usize + nf];
        let vstart = self.start_run(run, faces, bs);
        bs.g.resize(len * nf, 0.0);
        let row = run.y as usize * self.w + run.x0 as usize;
        let mut total = 0.0;
        for i in 0..len {
            self.deposit(faces, row + i, (run.x0 as usize + i) as f64 + 0.5, bs);
            let g = if with_grad {
                Some(&mut bs.g[i * nf..(i + 1) * nf])
            } else {
                None
            };
            total += self.pixel_energy(band, run, i, &bs.cov[..nf], g);
        }
        if !with_grad {
            return (total, &bs.out);
        }
        // Suffix sums: `suf[i]` is the sum of `g` over the run's pixels from `i` on.
        bs.suf.clear();
        bs.suf.resize((len + 1) * nf, 0.0);
        for i in (0..len).rev() {
            for j in 0..nf {
                bs.suf[i * nf + j] = bs.suf[(i + 1) * nf + j] + bs.g[i * nf + j];
            }
        }
        let mut out = std::mem::take(&mut bs.out);
        self.virtual_grads(faces, vstart, &bs.suf[..nf], &mut out);
        for i in 0..len {
            let (g, suf) = (
                &bs.g[i * nf..(i + 1) * nf],
                &bs.suf[(i + 1) * nf..(i + 2) * nf],
            );
            self.piece_grads(
                faces,
                row + i,
                (run.x0 as usize + i) as f64 + 0.5,
                g,
                suf,
                &mut out,
            );
        }
        bs.out = out;
        (total, &bs.out)
    }

    /// One run of the band term: its energy, and (with `with_grad`) the gradient at both
    /// ends of each of its pieces.
    ///
    /// The pixels holding pieces are rendered one by one (`deposit`, `pixel_energy`).
    /// Between two of them the carry `κ` is constant, so over such a stretch the energy is
    /// the quadratic form `κᵀ(ΣA)κ − 2κᵀ(Σb) + Σc`, with `A_ij = w·c_i·c_j`, `b_i = w·c_i·t`
    /// and `c = w·t·t` per pixel (and the opacities as a fourth channel where alpha is one),
    /// and the stretch's share of each face's suffix sum is `2((ΣA)κ − Σb)`. The sums over
    /// the stretch are differences of prefix sums fixed at the start (`fill_prefix`), so a
    /// run costs its pixels holding pieces plus one quadratic form per stretch instead of
    /// every pixel: the same energy and gradient as `band_run_cells`, summed in another
    /// order.
    fn band_run<'s>(
        &self,
        band: &Band,
        run: &Run,
        bs: &'s mut RunScratch,
        with_grad: bool,
    ) -> (f64, &'s [(u32, [f64; 4])]) {
        let len = (run.x1 - run.x0 + 1) as usize;
        let nf = run.nf as usize;
        let kk = nf * (nf + 1) / 2 + nf + 1;
        let pre = &band.prefix[run.pstart as usize..run.pstart as usize + (len + 1) * kk];
        let faces = &band.faces[run.fstart as usize..run.fstart as usize + nf];
        let vstart = self.start_run(run, faces, bs);
        let row = run.y as usize * self.w + run.x0 as usize;
        let mut total = 0.0;
        let mut i = 0usize;
        while i < len {
            if self.head[row + i] < 0 {
                let mut j = i + 1;
                while j < len && self.head[row + j] < 0 {
                    j += 1;
                }
                total += stretch_energy(
                    &pre[i * kk..(i + 1) * kk],
                    &pre[j * kk..(j + 1) * kk],
                    &bs.carry[..nf],
                );
                if with_grad {
                    bs.segs.push((i as u32, j as u32, bs.kap.len() as u32));
                    bs.kap.extend_from_slice(&bs.carry[..nf]);
                }
                i = j;
                continue;
            }
            self.deposit(faces, row + i, (run.x0 as usize + i) as f64 + 0.5, bs);
            let g0 = bs.g.len();
            if with_grad {
                bs.segs.push((i as u32, u32::MAX, g0 as u32));
                bs.g.resize(g0 + nf, 0.0);
            }
            let g = if with_grad {
                Some(&mut bs.g[g0..g0 + nf])
            } else {
                None
            };
            let e = self.pixel_energy(band, run, i, &bs.cov[..nf], g);
            total += e;
            bs.cut += e;
            i += 1;
        }
        if !with_grad {
            return (total, &bs.out);
        }
        // Backwards: `suf` is the sum of each face's `g` over everything after the current
        // segment; a stretch adds `2((ΣA)κ − Σb)`, a pixel its own `g`.
        bs.suf.clear();
        bs.suf.resize(nf, 0.0);
        let mut out = std::mem::take(&mut bs.out);
        for k in (0..bs.segs.len()).rev() {
            let (i, j, at) = bs.segs[k];
            let (i, at) = (i as usize, at as usize);
            if j != u32::MAX {
                let (p0, p1) = (
                    &pre[i * kk..(i + 1) * kk],
                    &pre[j as usize * kk..(j as usize + 1) * kk],
                );
                stretch_grad(p0, p1, &bs.kap[at..at + nf], &mut bs.suf);
                continue;
            }
            let g = &bs.g[at..at + nf];
            self.piece_grads(
                faces,
                row + i,
                (run.x0 as usize + i) as f64 + 0.5,
                g,
                &bs.suf,
                &mut out,
            );
            for a in 0..nf {
                bs.suf[a] += bs.g[at + a];
            }
        }
        self.virtual_grads(faces, vstart, &bs.suf, &mut out);
        bs.out = out;
        (total, &bs.out)
    }
}

/// The energy of a stretch of pixels with the constant coverage `kap`, from the prefix sums
/// at its two ends: `κᵀ(ΣA)κ − 2κᵀ(Σb) + Σc` (see `Problem::band_run`).
fn stretch_energy(p0: &[f64], p1: &[f64], kap: &[f64]) -> f64 {
    let nf = kap.len();
    let kk = p0.len();
    let d = |k: usize| p1[k] - p0[k];
    let mut e = d(kk - 1);
    let mut t = 0;
    for a in 0..nf {
        for b in a..nf {
            let f = if a == b { 1.0 } else { 2.0 };
            e += f * kap[a] * kap[b] * d(t);
            t += 1;
        }
    }
    for a in 0..nf {
        e -= 2.0 * kap[a] * d(t + a);
    }
    e
}

/// Add a stretch's share of each face's suffix sum, `2((ΣA)κ − Σb)`, to `suf`.
fn stretch_grad(p0: &[f64], p1: &[f64], kap: &[f64], suf: &mut [f64]) {
    let nf = kap.len();
    let mut t = 0;
    for a in 0..nf {
        for b in a..nf {
            let dab = p1[t] - p0[t];
            suf[a] += 2.0 * dab * kap[b];
            if a != b {
                suf[b] += 2.0 * dab * kap[a];
            }
            t += 1;
        }
    }
    for a in 0..nf {
        suf[a] -= 2.0 * (p1[t + a] - p0[t + a]);
    }
}

/// Cut the segment from unknown `va` to `vb` at its gridline crossings and call `f` with
/// each piece: its ends, their provenance, and the pixel holding its midpoint (rounded,
/// possibly outside the image). A crossing is put exactly on its gridline, so that the
/// pieces of a chain crossing a row add up to that row's height exactly.
fn cut_segment(
    pos: &[Point],
    va: u32,
    vb: u32,
    cr: &mut Vec<(f64, Prov)>,
    mut f: impl FnMut(Point, Point, Prov, Prov, f64, f64),
) {
    let (a, b) = (pos[va as usize], pos[vb as usize]);
    super::crossings(a, b, va, vb, cr);
    let mut t0 = 0.0;
    let mut from = a;
    let mut from_prov = Prov::Vertex(va);
    for idx in 0..=cr.len() {
        let (t1, to, to_prov) = if idx == cr.len() {
            (1.0, b, Prov::Vertex(vb))
        } else {
            let (t, prov) = cr[idx];
            let p = match prov {
                Prov::CrossV { line, .. } => Point::new(line, a.y + (b.y - a.y) * t),
                Prov::CrossH { line, .. } => Point::new(a.x + (b.x - a.x) * t, line),
                _ => Point::new(a.x + (b.x - a.x) * t, a.y + (b.y - a.y) * t),
            };
            (t, p, prov)
        };
        if t1 - t0 > 1e-9 {
            let tm = 0.5 * (t0 + t1);
            let px = (a.x + (b.x - a.x) * tm).round();
            let py = (a.y + (b.y - a.y) * tm).round();
            f(from, to, from_prov, to_prov, px, py);
        }
        t0 = t1;
        from = to;
        from_prov = to_prov;
    }
}

/// The band pixel of row `y` nearest to column `x` (`x` itself when it is in the band).
///
/// A point moves at most 1 px, so a piece lands at most one pixel outside the pixels the
/// band was grown from except when its midpoint sits exactly on a pixel border one and a
/// half pixels away; such a piece is filed under the neighbouring band pixel, which keeps
/// its carried height, and so every coverage in the row, exact.
fn nearest_in_band(inb: &[bool], w: usize, y: usize, x: usize) -> usize {
    let row = &inb[y * w..(y + 1) * w];
    if row[x] {
        return x;
    }
    for d in 1..4usize {
        if x >= d && row[x - d] {
            return x - d;
        }
        if x + d < w && row[x + d] {
            return x + d;
        }
    }
    x
}

/// Adds `s` to face `f`'s entry of a small carry list.
fn bump(carry: &mut Vec<(u16, f64)>, f: u16, s: f64) {
    if let Some(c) = carry.iter_mut().find(|c| c.0 == f) {
        c.1 += s;
    } else {
        carry.push((f, s));
    }
}

/// The face on the far left of row `y` at the start: the outside when the frame crosses
/// the row, otherwise the face on the left of the leftmost boundary in it (a map without a
/// frame, as some tests build). `None` for a row no boundary crosses.
fn row_seed(prob: &Problem, y: usize) -> Option<u16> {
    let mut best: Option<(f64, u16)> = None;
    let consider = |id: i32, best: &mut Option<(f64, u16)>| {
        let pc = &prob.pieces[id as usize];
        let (l, r) = prob.sides(id as usize);
        let s = pc.to.y - pc.from.y;
        if s == 0.0 {
            return;
        }
        let xm = 0.5 * (pc.from.x + pc.to.x);
        // Walking down (s > 0) the left face is on +x, so the face towards −x is `r`.
        let west = if s > 0.0 { r } else { l };
        if best.is_none_or(|b| xm < b.0) {
            *best = Some((xm, west));
        }
    };
    let mut id = prob.vhead[y];
    while id >= 0 {
        let (l, r) = prob.sides(id as usize);
        if l == OUT || r == OUT {
            return Some(OUT);
        }
        consider(id, &mut best);
        id = prob.pieces[id as usize].next;
    }
    for x in 0..prob.w {
        let mut id = prob.head[y * prob.w + x];
        while id >= 0 {
            consider(id, &mut best);
            id = prob.pieces[id as usize].next;
        }
        if best.is_some() {
            // Pieces further right cannot be further left than x − 1.
            break;
        }
    }
    best.map(|b| b.1)
}

/// The floor of the band-table budget, in bytes (see [`table_budget`]).
pub(super) const TABLE_BUDGET_FLOOR: u64 = 256 << 20;

/// Bytes per image pixel the band-table budget grows by above its floor (see
/// [`table_budget`]).
pub(super) const TABLE_BUDGET_PER_PIXEL: u64 = 32;

/// Most bytes the band's per-run tables may take for a `w x h` image:
/// `max(TABLE_BUDGET_FLOOR, TABLE_BUDGET_PER_PIXEL · w · h)`, i.e. 256 MiB up to about
/// 2900 x 2900 px and 32 bytes a pixel beyond. The tables are the colours (and opacities)
/// of every face of a run at every pixel of it, and the run's prefix sums (see
/// [`table_bytes`]).
///
/// # Why a budget
///
/// The tables grow as `Σ_runs len · nf²` for a run of `len` pixels holding pieces of `nf`
/// faces, and `nf` is bounded by nothing but the image. On art whose boundaries are dense
/// enough that the band covers whole rows, a run is a whole row and `nf` is every face the
/// row crosses: 1-px vertical stripes at 512 px made `nf = 513` and asked for a 265 GB
/// prefix table (the process aborted on a 34.7 GB allocation after 19.7 GB of working set),
/// a 512 px pixel checkerboard took 398 MB (`nf = 259`), a baked grey-and-white
/// "transparency" checkerboard behind an icon 116–711 MB, a 3.5x nearest-neighbour
/// upscale of a wordmark 497 MB (all measured 2026-10-02 by the r2-inputs research,
/// `bandstats.py`). Halftone screens, hatched or engraved logos and dense labyrinth
/// textures build bands of the same shape.
///
/// What the solve buys on such an image is small (on the stripes it reports no gain at
/// all: every pixel is a mixture of two faces whatever the boundary does), so when the
/// tables would exceed the budget the solve is skipped and the map keeps the boundary
/// `planar::refine_subpixel` measured, exactly as when the solve finds nothing to gain.
///
/// # Why these numbers
///
/// It never binds on the art the tracer is judged on. Measured 2026-10-02 (`INKVEC_DIAG`,
/// `band ... table_bytes`, the largest solve per image): 2.8 MB on the 246-icon screen set
/// at 128 px, 5.7 MB on the same icons at 512 px, 2.0 MB on `held_a`, 5.9 MB on the
/// 51-image 512 px set, 22.2 MB on the seven opaque 2048 px inputs (the masthead, 14 bytes a
/// pixel) and 16.3 MB on the three transparent ones. On the 772 non-pathological images of
/// the r2-inputs stress set (clean, compressed, resampled, palette, glow, shadow, 4k and
/// 8k, tiny) the largest was 106 MB, a 4x bicubic upscale of a dense wordmark at 512 x 313
/// px, 2.4 times under the floor. The pathological class above starts at 116 MB and runs to
/// hundreds of gigabytes. The floor is 256 MiB, 11 times the gate's largest table, about
/// what a 2048 px trace already holds; the per-pixel term lets an uncapped 8192 px trace
/// (`--max-dim 0`) keep twice the masthead's 14 bytes a pixel.
///
/// Not from the literature: a resource bound on our own data structure. See also: the
/// narrow-band level set of D. Adalsteinsson, J. A. Sethian (1995), *A fast level set method
/// for propagating interfaces*, J. Comput. Phys. 118,
/// <https://doi.org/10.1006/jcph.1995.1098>, whose band is what grows here; the paper bounds
/// the band's width, not the number of regions meeting inside it.
pub(super) fn table_budget(w: usize, h: usize) -> u64 {
    TABLE_BUDGET_PER_PIXEL
        .saturating_mul((w as u64).saturating_mul(h as u64))
        .max(TABLE_BUDGET_FLOOR)
}

/// The bytes [`fill_colours`] and [`fill_prefix`] allocate for one run of `len` pixels and
/// `nf` faces: `24·nf·len` for the colours (three f64 per face per pixel), `8·nf·len` more for
/// the opacities when alpha is a channel (`alpha`), and `8·(len + 1)·kk` for the prefix sums,
/// with `kk = nf(nf + 1)/2 + nf + 1` sums per pixel (one per unordered face pair, one per
/// face against the target, one for the target against itself) and one leading row of
/// zeros. In u64, so the count itself cannot overflow on any image (`nf`, `len` < 2³²).
pub(super) fn table_bytes(len: u64, nf: u64, alpha: bool) -> u64 {
    let colour = (24 + if alpha { 8 } else { 0 }) * nf * len;
    let kk = nf * (nf + 1) / 2 + nf + 1;
    colour.saturating_add((len + 1).saturating_mul(kk).saturating_mul(8))
}

/// Build the band around the boundary at the start. `bucket_band` must have run on the
/// start. Finds the runs, each run's seed (by carrying along its row from the far left),
/// the faces that can appear in it (every face of a piece within reach of it), and those
/// faces' colours; weights start at 1 except for runs whose seed is not one clean face,
/// which are left out.
///
/// `None` when the band's tables would take more than `budget` bytes ([`table_bytes`],
/// summed over the runs as they are found, so building stops at the run that crosses the
/// budget and nothing of the size of the tables is ever allocated).
pub(super) fn build(prob: &Problem, budget: u64) -> Option<Band> {
    let (w, h) = (prob.w, prob.h);
    let mut band = Band {
        runs: Vec::new(),
        faces: Vec::new(),
        colour: Vec::new(),
        opacity: Vec::new(),
        weight: Vec::new(),
        alpha: Vec::new(),
        total_cells: 0,
        inb: band_pixels(prob),
        prefix: Vec::new(),
        run_const: Vec::new(),
    };
    let mut valid: Vec<bool> = Vec::new();
    let mut cstart = 0u32;
    let alpha = prob.alpha.is_some();
    // Running total of `table_bytes` over the runs found so far, and the largest run.
    let (mut bytes, mut max_nf) = (0u64, 0usize);
    for y in 0..h {
        let seed0 = row_seed(prob, y);
        let mut carry: Vec<(u16, f64)> = vec![(seed0.unwrap_or(OUT), 1.0)];
        carry_list(prob, prob.vhead[y], &mut carry);
        let mut x = 0usize;
        while x < w {
            if !band.inb[y * w + x] {
                x += 1;
                continue;
            }
            let x0 = x;
            while x < w && band.inb[y * w + x] {
                x += 1;
            }
            let x1 = x - 1;
            // The seed: everything left of a run at column 0 is the row's far-left face
            // (the run takes the pieces left of the image itself); left of any other run is
            // an untouched pixel, all one face.
            let (seed, ok) = if x0 == 0 {
                (seed0.unwrap_or(OUT), seed0.is_some())
            } else {
                let top = carry.iter().copied().max_by(|a, b| a.1.total_cmp(&b.1));
                let pure = top.is_some_and(|t| {
                    t.0 != OUT
                        && (t.1 - 1.0).abs() < 1e-6
                        && carry.iter().all(|c| c.0 == t.0 || c.1.abs() < 1e-6)
                });
                (top.map_or(OUT, |t| t.0), pure)
            };
            let fstart = band.faces.len();
            band.faces.push(seed);
            run_faces(prob, y, x0, x1, &mut band.faces, fstart);
            let nf = band.faces.len() - fstart;
            max_nf = max_nf.max(nf);
            bytes = bytes.saturating_add(table_bytes((x1 - x0 + 1) as u64, nf as u64, alpha));
            if bytes > budget {
                crate::diag::saturated(
                    "bopt",
                    "band_table_bytes",
                    bytes as f64,
                    budget as f64,
                    crate::diag::Stop::Cap,
                );
                crate::diag!(
                    "bopt",
                    "band over budget at row={y} of {h} max_nf={max_nf}: solve skipped"
                );
                return None;
            }
            // Carry across the run for the next seed.
            if x0 != 0 {
                carry = vec![(seed, 1.0)];
            }
            for xx in x0..=x1 {
                carry_list(prob, prob.head[y * w + xx], &mut carry);
            }
            band.runs.push(Run {
                y: y as u32,
                x0: x0 as u32,
                x1: x1 as u32,
                first: band.total_cells as u32,
                fstart: fstart as u32,
                nf: nf as u32,
                seed: 0,
                cstart,
                pstart: 0,
            });
            band.total_cells += x1 - x0 + 1;
            cstart += ((x1 - x0 + 1) * nf) as u32;
            valid.extend(std::iter::repeat_n(ok, x1 - x0 + 1));
        }
    }
    band.weight = valid.iter().map(|&v| if v { 1.0 } else { 0.0 }).collect();
    band.alpha = vec![false; band.total_cells];
    crate::diag!(
        "bopt",
        "band runs={} cells={} max_nf={max_nf} table_bytes={bytes}",
        band.runs.len(),
        band.total_cells
    );
    Some(band)
}

/// Every pixel within `REACH` of a pixel holding a piece at the start (a piece left of the
/// image counts as holding column −1 of its row).
fn band_pixels(prob: &Problem) -> Vec<bool> {
    let (w, h) = (prob.w, prob.h);
    let mut inb = vec![false; w * h];
    let mut mark = |cx: i64, cy: i64| {
        for y in (cy - REACH).max(0)..=(cy + REACH).min(h as i64 - 1) {
            for x in (cx - REACH).max(0)..=(cx + REACH).min(w as i64 - 1) {
                inb[y as usize * w + x as usize] = true;
            }
        }
    };
    for &cell in &prob.touched {
        mark((cell % w) as i64, (cell / w) as i64);
    }
    for y in 0..h {
        if prob.vhead[y] >= 0 {
            mark(-1, y as i64);
        }
    }
    inb
}

/// Add the heights of the pieces of one list (from `id` on) to a carry.
fn carry_list(prob: &Problem, mut id: i32, carry: &mut Vec<(u16, f64)>) {
    while id >= 0 {
        let pc = &prob.pieces[id as usize];
        let (l, r) = prob.sides(id as usize);
        let s = pc.to.y - pc.from.y;
        bump(carry, l, s);
        bump(carry, r, -s);
        id = pc.next;
    }
}

/// Append to `faces[fstart..]` every face of a piece within `REACH` of the run
/// `x0..=x1` of row `y` (with the pieces left of the image for a run at column 0).
fn run_faces(prob: &Problem, y: usize, x0: usize, x1: usize, faces: &mut Vec<u16>, fstart: usize) {
    let (w, h) = (prob.w, prob.h);
    let rows = (y as i64 - REACH).max(0) as usize..=(y as i64 + REACH).min(h as i64 - 1) as usize;
    for yy in rows {
        let mut ids: Vec<i32> = Vec::new();
        if x0 == 0 {
            ids.push(prob.vhead[yy]);
        }
        let xa = x0.saturating_sub(REACH as usize);
        let xb = (x1 + REACH as usize).min(w - 1);
        ids.extend((xa..=xb).map(|xx| prob.head[yy * w + xx]));
        for mut id in ids {
            while id >= 0 {
                let (l, r) = prob.sides(id as usize);
                for f in [l, r] {
                    if !faces[fstart..].contains(&f) {
                        faces.push(f);
                    }
                }
                id = prob.pieces[id as usize].next;
            }
        }
    }
}

/// Fill the band's colour (and opacity) tables: every face of every run at every pixel of
/// it, evaluated once for the whole solve.
fn fill_colours(prob: &Problem, band: &mut Band) {
    let evals: Vec<_> = prob.face.iter().map(|m| m.eval()).collect();
    let w = prob.w;
    band.colour = Vec::with_capacity(
        band.runs
            .iter()
            .map(|r| r.nf as usize * (r.x1 - r.x0 + 1) as usize)
            .sum(),
    );
    let alpha = prob.alpha;
    for run in &band.runs {
        let faces = &band.faces[run.fstart as usize..(run.fstart + run.nf) as usize];
        for x in run.x0..=run.x1 {
            let cell = run.y as usize * w + x as usize;
            for &f in faces {
                let c = if f == OUT {
                    prob.rgb[cell]
                } else {
                    evals[f as usize].color_at(x as f64, run.y as f64)
                };
                band.colour.push([c[0] as f64, c[1] as f64, c[2] as f64]);
                if let Some((img_a, fa)) = alpha {
                    band.opacity.push(if f == OUT {
                        img_a[cell] as f64
                    } else {
                        fa.get(f as usize).copied().unwrap_or(1.0) as f64
                    });
                }
            }
        }
    }
}

/// Fill each run's prefix sums (see [`Band::prefix`]), once the weights and the alpha
/// channels are fixed.
fn fill_prefix(prob: &Problem, band: &mut Band) {
    let w = prob.w;
    let alpha_img = prob.alpha.map(|(a, _)| a);
    let mut out: Vec<f64> = Vec::new();
    let mut runs = std::mem::take(&mut band.runs);
    for run in runs.iter_mut() {
        let nf = run.nf as usize;
        let kk = nf * (nf + 1) / 2 + nf + 1;
        run.pstart = out.len() as u32;
        let len = (run.x1 - run.x0 + 1) as usize;
        let mut acc = vec![0.0; kk];
        out.extend_from_slice(&acc);
        for i in 0..len {
            let bi = run.first as usize + i;
            let cell = run.y as usize * w + run.x0 as usize + i;
            let wgt = band.weight[bi];
            let col = &band.colour[run.cstart as usize + i * nf..][..nf];
            let t = prob.rgb[cell];
            let t = [t[0] as f64, t[1] as f64, t[2] as f64];
            let alpha = band.alpha[bi] && alpha_img.is_some();
            let op: &[f64] = if alpha {
                &band.opacity[run.cstart as usize + i * nf..][..nf]
            } else {
                &[]
            };
            let ta = alpha_img.map_or(0.0, |a| a[cell] as f64);
            let mut k = 0;
            for a in 0..nf {
                for b in a..nf {
                    let mut v =
                        col[a][0] * col[b][0] + col[a][1] * col[b][1] + col[a][2] * col[b][2];
                    if alpha {
                        v += op[a] * op[b];
                    }
                    acc[k] += wgt * v;
                    k += 1;
                }
            }
            for a in 0..nf {
                let mut v = col[a][0] * t[0] + col[a][1] * t[1] + col[a][2] * t[2];
                if alpha {
                    v += op[a] * ta;
                }
                acc[k + a] += wgt * v;
            }
            let mut v = t[0] * t[0] + t[1] * t[1] + t[2] * t[2];
            if alpha {
                v += ta * ta;
            }
            acc[kk - 1] += wgt * v;
            out.extend_from_slice(&acc);
        }
    }
    band.runs = runs;
    band.prefix = out;
}

/// Hold the image frame where it is.
///
/// The frame edges (the ones with the outside on one side) lie on the lines `x = −½`,
/// `x = w − ½`, `y = −½`, `y = h − ½`. Their points, and the junction nodes they share with
/// interior boundaries, keep the coordinate that puts them on that line (snapped exactly
/// onto it, from within 1e-3 px) and move only along it: the frame is the edge of the
/// image, not something the image measures. Not from the literature: a boundary
/// condition, needed here because the accumulation starts from the outside at `x = −½`.
pub(super) fn pin_frame(map: &super::PlanarMap, vars: &mut super::Vars) {
    let (w, h) = (map.width as f64, map.height as f64);
    let near = |v: f64, line: f64| (v - line).abs() < 1e-3;
    for (k, e) in map.edges.iter().enumerate() {
        if e.left != OUT && e.right != OUT {
            continue;
        }
        for &v in &vars.var[k] {
            let p = &mut vars.start[v as usize];
            for line in [-0.5, w - 0.5] {
                if near(p.x, line) {
                    p.x = line;
                    vars.pin[v as usize] |= 1;
                }
            }
            for line in [-0.5, h - 0.5] {
                if near(p.y, line) {
                    p.y = line;
                    vars.pin[v as usize] |= 2;
                }
            }
        }
    }
}

/// Whether both faces of `e` have a fill model (the frame's outside has none).
fn modelled(prob: &Problem, e: &crate::planar::Edge) -> bool {
    (e.left as usize) < prob.face.len() && (e.right as usize) < prob.face.len()
}

/// Leave out of the data term (weight zero, for the whole solve) every band pixel that
/// holds pieces of two or more boundaries between modelled faces at the start: a junction,
/// or both sides of a stroke too thin to have an interior. Their colour is a three-way
/// mixture the fills are least reliable at, and on a thin stroke the two sides compete for
/// one pixel's evidence (the sawtooth of the module docs). Fixing the set at the start keeps
/// the energy continuous. Measured on the screen set, with the solver of the time:
/// objective 0.3713 leaving them out against 0.3908 with them in; leaving out only the
/// pixels round the junction nodes read 0.3908 as well.
fn exclude_junctions(prob: &Problem, band: &mut Band, index: &[u32]) {
    for &cell in &prob.touched {
        let mut first: Option<u32> = None;
        let mut multi = false;
        let mut id = prob.head[cell];
        while id >= 0 {
            let pc = &prob.pieces[id as usize];
            if modelled(prob, &prob.map.edges[pc.edge as usize]) {
                match first {
                    None => first = Some(pc.edge),
                    Some(f) if f != pc.edge => multi = true,
                    _ => {}
                }
            }
            id = pc.next;
        }
        if multi && index[cell] != u32::MAX {
            band.weight[index[cell] as usize] = 0.0;
        }
    }
}

/// Where alpha is a fourth channel: every boundary that can reach the pixel separates
/// faces the colour over white cannot tell apart but their opacities can (the rule of the
/// per-pixel term: white paint on the clear ground, the bands of one fade).
fn alpha_channels(prob: &Problem, band: &mut Band, index: &[u32]) {
    let Some((_, fa)) = prob.alpha else {
        return;
    };
    let (w, h) = (prob.w, prob.h);
    let op = |f: u16| fa.get(f as usize).copied().unwrap_or(1.0);
    let evals: Vec<_> = prob.face.iter().map(|m| m.eval()).collect();
    // Per band pixel: bit 0 an edge within reach separates colours, bit 1 one separates
    // only opacities. Each boundary is judged at the pixel holding its piece.
    let mut flags = vec![0u8; band.total_cells];
    for &cell in &prob.touched {
        let (cx, cy) = ((cell % w) as i64, (cell / w) as i64);
        let (mut high, mut low) = (false, false);
        let mut id = prob.head[cell];
        while id >= 0 {
            let pc = &prob.pieces[id as usize];
            let e = &prob.map.edges[pc.edge as usize];
            if modelled(prob, e) {
                let (px, py) = (cx as f64, cy as f64);
                let cl = evals[e.left as usize].color_at(px, py);
                let cr = evals[e.right as usize].color_at(px, py);
                let c = (0..3).map(|k| (cl[k] - cr[k]).abs()).fold(0.0f32, f32::max);
                if c >= super::MIN_CONTRAST {
                    high = true;
                } else if (op(e.left) - op(e.right)).abs() >= super::MIN_CONTRAST {
                    low = true;
                }
            }
            id = pc.next;
        }
        let bits = (high as u8) | ((low as u8) << 1);
        if bits == 0 {
            continue;
        }
        for y in (cy - REACH).max(0)..=(cy + REACH).min(h as i64 - 1) {
            for x in (cx - REACH).max(0)..=(cx + REACH).min(w as i64 - 1) {
                let i = index[y as usize * w + x as usize];
                if i != u32::MAX {
                    flags[i as usize] |= bits;
                }
            }
        }
    }
    for (a, f) in band.alpha.iter_mut().zip(&flags) {
        *a = *f == 2;
    }
}

/// Build the band at the start positions and fix everything about it for the solve: the
/// weights (junction pixels left out), where alpha is a channel, and the normalisation
/// (`band_norm`: the starting residual of the pixels the boundary cuts, and of the rest).
///
/// Returns false, with nothing set up, when the band's tables would exceed
/// [`table_budget`]; the caller then skips the solve.
pub(super) fn setup(prob: &mut Problem) -> bool {
    let budget = table_budget(prob.w, prob.h);
    setup_within(prob, budget)
}

/// [`setup`] with the table budget given, so the tests can make it bind on a small map.
pub(super) fn setup_within(prob: &mut Problem, budget: u64) -> bool {
    let start = prob.vars.start.clone();
    prob.bucket_band(&start);
    let Some(mut band) = build(prob, budget) else {
        return false;
    };
    fill_colours(prob, &mut band);
    let w = prob.w;
    let mut index = vec![u32::MAX; prob.w * prob.h];
    for run in &band.runs {
        for x in run.x0..=run.x1 {
            index[run.y as usize * w + x as usize] = run.first + (x - run.x0);
        }
    }
    exclude_junctions(prob, &mut band, &index);
    alpha_channels(prob, &mut band, &index);
    fill_prefix(prob, &mut band);
    // The starting residual, split into the pixels the boundary cuts (what the prior
    // weights are scaled to, as before) and the rest, per run.
    let mut bs = std::mem::take(&mut prob.bscratch);
    let _ = prob.band_data(&band, &start, &mut bs, None);
    let cut: f64 = bs.run_cut.iter().sum();
    band.run_const = bs
        .run_e
        .iter()
        .zip(&bs.run_cut)
        .map(|(e, c)| e - c)
        .collect();
    let rest: f64 = band.run_const.iter().sum();
    prob.bscratch = bs;
    prob.band_norm = (cut, rest);
    prob.band = Some(band);
    true
}

/// The problem's independent parts, at the start (`bucket_band` must have run there).
///
/// Two boundaries are in one part when they share an unknown (a junction) or when pieces of
/// both lie within reach of one run of the band (a piece can only ever land in a run it
/// was within a pixel of at the start, see `nearest_in_band`). The energy is then the sum of
/// the parts' energies exactly. Parts without a run (the frame's top, right and bottom,
/// which no pixel reads) have nothing to solve and are left out. Ordered by their first
/// boundary, so the result does not depend on hashing.
pub(super) fn components(prob: &Problem) -> Vec<super::lbfgs::Active> {
    let band = prob.band.as_ref().expect("band mode");
    let (w, h) = (prob.w, prob.h);
    let ne = prob.map.edges.len();
    let nr = band.runs.len();
    let mut parent: Vec<u32> = (0..(ne + nr) as u32).collect();
    fn find(p: &mut [u32], mut x: u32) -> u32 {
        while p[x as usize] != x {
            p[x as usize] = p[p[x as usize] as usize];
            x = p[x as usize];
        }
        x
    }
    let union = |p: &mut Vec<u32>, a: u32, b: u32| {
        let (ra, rb) = (find(p, a), find(p, b));
        if ra != rb {
            p[ra.max(rb) as usize] = ra.min(rb);
        }
    };
    // Junctions.
    let mut first_edge: Vec<u32> = vec![u32::MAX; prob.vars.start.len()];
    for (k, ids) in prob.vars.var.iter().enumerate() {
        for &v in ids {
            let f = &mut first_edge[v as usize];
            if *f == u32::MAX {
                *f = k as u32;
            } else {
                union(&mut parent, *f, k as u32);
            }
        }
    }
    // Runs and the boundaries within reach of them.
    for (r, run) in band.runs.iter().enumerate() {
        let node = (ne + r) as u32;
        let y = run.y as i64;
        for yy in (y - REACH).max(0)..=(y + REACH).min(h as i64 - 1) {
            let mut heads: Vec<i32> = Vec::new();
            if run.x0 == 0 {
                heads.push(prob.vhead[yy as usize]);
            }
            let xa = (run.x0 as i64 - REACH).max(0) as usize;
            let xb = (run.x1 as i64 + REACH).min(w as i64 - 1) as usize;
            heads.extend((xa..=xb).map(|x| prob.head[yy as usize * w + x]));
            for mut id in heads {
                while id >= 0 {
                    let e = prob.pieces[id as usize].edge;
                    union(&mut parent, node, e);
                    id = prob.pieces[id as usize].next;
                }
            }
        }
    }
    let mut group: Vec<u32> = vec![u32::MAX; ne + nr];
    let mut parts: Vec<super::lbfgs::Active> = Vec::new();
    for k in 0..ne {
        let root = find(&mut parent, k as u32) as usize;
        if group[root] == u32::MAX {
            group[root] = parts.len() as u32;
            parts.push(super::lbfgs::Active::default());
        }
        parts[group[root] as usize].edges.push(k as u32);
    }
    for r in 0..nr {
        let root = find(&mut parent, (ne + r) as u32) as usize;
        if group[root] == u32::MAX {
            continue;
        }
        let part = &mut parts[group[root] as usize];
        part.runs.push(r as u32);
        part.e_const += band.run_const[r];
    }
    let mut seen = vec![false; prob.vars.start.len()];
    for part in parts.iter_mut() {
        for &k in &part.edges {
            for &v in &prob.vars.var[k as usize] {
                if !seen[v as usize] {
                    seen[v as usize] = true;
                    part.vars.push(v);
                }
            }
        }
    }
    parts.retain(|p| !p.runs.is_empty());
    parts
}

#[cfg(test)]
#[path = "band_tests.rs"]
mod tests;
