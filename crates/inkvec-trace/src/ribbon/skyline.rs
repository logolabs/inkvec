//! A symmetric matrix in profile (skyline) storage and its Cholesky solve: the normal
//! equations of the stroke solve ([`super::refine`]) without their zeros.
//!
//! **Why the matrix is sparse.** The stroke solve's unknowns are read in path order --
//! a path's start point, then each segment's handles and end point -- and one boundary
//! point's residual depends only on the segment nearest to it: that segment's own
//! variables and its start point (the previous segment's end), plus the half-width `h`,
//! which is the last unknown. So in `JᵀJ` row `i` is nonzero only from a column a few
//! places to the left of `i` (one segment's worth, up to 9 for a cubic after an arc, 15
//! for a miter gauge reading two segments) and in the last row and column (`h`). A
//! closed path adds one long reach: its last segment ends on the path's start point. On
//! lucide at 512 px a face has up to a few hundred unknowns, and the dense factorisation
//! cost `n³/3` flops where the nonzeros need about `n·b²` (`b` the reach).
//!
//! **Storage.** Row `i` keeps the entries `(i, j)` for `first[i] <= j <= i`: its
//! *envelope*, from the first column that can be nonzero to the diagonal. The rows are
//! concatenated in [`Skyline::val`], row `i` starting at [`Skyline::start`]`[i]`.
//!
//! **Factorisation.** Cholesky `A = L·Lᵀ`, row by row:
//!
//! `L[i][j] = (A[i][j] - Σ_{k = max(first[i], first[j])}^{j-1} L[i][k]·L[j][k]) / L[j][j]`
//! for `first[i] <= j < i`, and `L[i][i] = sqrt(A[i][i] - Σ_{k = first[i]}^{i-1} L[i][k]²)`.
//!
//! `L` has the envelope of `A` -- no entry left of `first[i]` ever fills in, because every
//! term of its sum has a factor `L[i][k]` with `k < first[i]`, zero by induction -- so the
//! factor overwrites `A` in place. Then `L·z = y` forwards and `Lᵀ·x = z` backwards.
//!
//! **Why the result is the dense solve's, bit for bit.** The dense Cholesky this replaces
//! sums each `A[i][j] - Σ_k L[i][k]·L[j][k]` over every `k < j` in increasing `k`. Every
//! term this one skips has a factor that is exactly zero (left of an envelope), so it is
//! `±0`, and `s - (±0) = s` in IEEE arithmetic for every `s` the sums can hold (the
//! accumulated entries are never `-0`: they start at `+0` and `x + (-x) = +0`). The terms
//! that remain are the same products, subtracted in the same increasing order. The
//! substitutions skip the same zeros in the same way, and back substitution reads column
//! `i` of `L` in increasing row order exactly as the dense loop did. So for an envelope
//! that contains every nonzero of `A`, every quotient, square root and failed pivot is the
//! dense one's.
//!
//! Method from: Jennings (1966), A compact storage scheme for the solution of symmetric
//! linear simultaneous equations, The Computer Journal 9(3), 281-285,
//! doi:10.1093/comjnl/9.3.281 -- the variable-bandwidth ("skyline") store and the
//! envelope Cholesky, used here as published. See also: George, Liu (1981), Computer
//! Solution of Large Sparse Positive Definite Systems, Prentice-Hall, ch. 4 (envelope
//! methods); and Triggs, McLauchlan, Hartley, Fitzgibbon (2000), Bundle adjustment -- a
//! modern synthesis, Vision Algorithms: Theory and Practice, LNCS 1883, 298-372,
//! doi:10.1007/3-540-44480-7_21, on exploiting the sparsity of Gauss-Newton normal
//! equations; the single dense row of `h` is their "arrowhead" border, which the
//! envelope stores at the cost of one full row.

/// A symmetric `n x n` matrix stored as the envelope of its lower triangle.
#[derive(Clone, Debug)]
pub(crate) struct Skyline {
    /// First stored column of each row (`first[i] <= i`).
    first: Vec<usize>,
    /// Offset of row `i`'s entry `(i, first[i])` in `val`.
    start: Vec<usize>,
    /// The stored entries, row after row.
    val: Vec<f64>,
}

impl Skyline {
    /// A zero matrix whose row `i` stores columns `first[i]..=i`. Entries clamped to `i`
    /// when `first[i] > i`.
    pub(crate) fn zeros(first: Vec<usize>) -> Skyline {
        let mut first = first;
        let mut start = Vec::with_capacity(first.len());
        let mut len = 0usize;
        for (i, f) in first.iter_mut().enumerate() {
            *f = (*f).min(i);
            start.push(len);
            len += i - *f + 1;
        }
        Skyline {
            first,
            start,
            val: vec![0.0; len],
        }
    }

    /// Order of the matrix.
    pub(crate) fn n(&self) -> usize {
        self.first.len()
    }

    /// The position of `(i, j)` in `val`, for `first[i] <= j <= i`.
    fn at(&self, i: usize, j: usize) -> usize {
        self.start[i] + (j - self.first[i])
    }

    /// `A[i][j] += x` for `j <= i` inside row `i`'s envelope (the lower triangle; the
    /// matrix is symmetric). Panics in debug builds outside it.
    pub(crate) fn add(&mut self, i: usize, j: usize, x: f64) {
        debug_assert!(
            j <= i && j >= self.first[i],
            "({i}, {j}) outside the envelope"
        );
        let k = self.at(i, j);
        self.val[k] += x;
    }

    /// `A[i][i]`.
    pub(crate) fn diag(&self, i: usize) -> f64 {
        self.val[self.at(i, i)]
    }

    /// `A[i][i] += x`.
    pub(crate) fn add_diag(&mut self, i: usize, x: f64) {
        let k = self.at(i, i);
        self.val[k] += x;
    }

    /// `A[i][j]` for any `i`, `j` (zero outside the envelope).
    #[cfg(test)]
    pub(crate) fn get(&self, i: usize, j: usize) -> f64 {
        let (i, j) = if j > i { (j, i) } else { (i, j) };
        if j < self.first[i] {
            0.0
        } else {
            self.val[self.at(i, j)]
        }
    }

    /// Solve `A x = y` by the envelope Cholesky of the module documentation, overwriting
    /// `self` with the factor. `None` when a pivot is not positive (`<= 1e-300`), exactly
    /// where the dense factorisation fails.
    ///
    /// Cost `O(Σ_i (i - first[i]) · b_i)` for the factorisation, `b_i` the overlap of the
    /// envelopes it reads, plus `O(n²)` comparisons in back substitution (it scans each
    /// column's rows in increasing order, as the dense loop did).
    pub(crate) fn cholesky_solve(&mut self, y: &[f64]) -> Option<Vec<f64>> {
        let n = self.n();
        for i in 0..n {
            let fi = self.first[i];
            for j in fi..i {
                let fj = self.first[j];
                let k0 = fi.max(fj);
                let mut s = self.val[self.at(i, j)];
                let (ri, rj) = (self.at(i, k0), self.at(j, k0));
                for k in 0..j - k0 {
                    s -= self.val[ri + k] * self.val[rj + k];
                }
                let d = self.val[self.at(j, j)];
                let ij = self.at(i, j);
                self.val[ij] = s / d;
            }
            let ii = self.at(i, i);
            let mut d = self.val[ii];
            let r0 = self.at(i, fi);
            for k in 0..i - fi {
                d -= self.val[r0 + k] * self.val[r0 + k];
            }
            if d <= 1e-300 {
                return None;
            }
            self.val[ii] = d.sqrt();
        }
        // Forward: L z = y.
        let mut z = y.to_vec();
        for i in 0..n {
            let fi = self.first[i];
            let r0 = self.at(i, fi);
            let mut s = z[i];
            for k in 0..i - fi {
                s -= self.val[r0 + k] * z[fi + k];
            }
            z[i] = s / self.val[self.at(i, i)];
        }
        // Backward: Lᵀ x = z, column i of L read in increasing row order.
        for i in (0..n).rev() {
            let mut s = z[i];
            for k in i + 1..n {
                if self.first[k] <= i {
                    s -= self.val[self.at(k, i)] * z[k];
                }
            }
            z[i] = s / self.val[self.at(i, i)];
        }
        Some(z)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The dense Cholesky solve the skyline replaces (row-major `n x n`, overwritten),
    /// kept as the reference the skyline must equal bit for bit.
    fn dense_cholesky_solve(a: &mut [f64], y: &[f64], n: usize) -> Option<Vec<f64>> {
        for j in 0..n {
            let mut d = a[j * n + j];
            for k in 0..j {
                d -= a[j * n + k] * a[j * n + k];
            }
            if d <= 1e-300 {
                return None;
            }
            let d = d.sqrt();
            a[j * n + j] = d;
            for i in j + 1..n {
                let mut s = a[i * n + j];
                for k in 0..j {
                    s -= a[i * n + k] * a[j * n + k];
                }
                a[i * n + j] = s / d;
            }
        }
        let mut z = y.to_vec();
        for i in 0..n {
            let mut s = z[i];
            for k in 0..i {
                s -= a[i * n + k] * z[k];
            }
            z[i] = s / a[i * n + i];
        }
        for i in (0..n).rev() {
            let mut s = z[i];
            for k in i + 1..n {
                s -= a[k * n + i] * z[k];
            }
            z[i] = s / a[i * n + i];
        }
        Some(z)
    }

    /// A small deterministic generator (xorshift64*), so the test needs no crate.
    struct Rng(u64);
    impl Rng {
        fn next(&mut self) -> f64 {
            self.0 ^= self.0 >> 12;
            self.0 ^= self.0 << 25;
            self.0 ^= self.0 >> 27;
            (self.0.wrapping_mul(0x2545_f491_4f6c_dd1d) >> 11) as f64 / (1u64 << 53) as f64 - 0.5
        }
    }

    /// A random normal-equations-like system with the stroke solve's pattern: `rows`
    /// residuals, each touching a contiguous block of at most `b` unknowns (a "segment"),
    /// a closed-path wrap on some, and the last unknown touched by all. Returns the
    /// skyline, the dense copy and the right-hand side.
    fn system(n: usize, rows: usize, b: usize, rng: &mut Rng) -> (Skyline, Vec<f64>, Vec<f64>) {
        let h = n - 1;
        let mut sets: Vec<Vec<usize>> = Vec::new();
        for r in 0..rows {
            let lo = ((rng.next() + 0.5) * h.saturating_sub(1) as f64) as usize;
            let w = 1 + ((rng.next() + 0.5) * b as f64) as usize;
            let mut v: Vec<usize> = (lo..(lo + w).min(h)).collect();
            if r % 7 == 0 {
                v.push(0); // the wrap of a closed path onto its start
            }
            v.push(h);
            v.sort_unstable();
            v.dedup();
            sets.push(v);
        }
        let mut first: Vec<usize> = (0..n).collect();
        for v in &sets {
            for &u in v {
                first[u] = first[u].min(v[0]);
            }
        }
        let mut sky = Skyline::zeros(first);
        let mut dense = vec![0.0; n * n];
        let mut y = vec![0.0; n];
        for v in &sets {
            let jac: Vec<(usize, f64)> = v.iter().map(|&u| (u, rng.next())).collect();
            let r = rng.next();
            for &(u, ju) in &jac {
                y[u] += ju * r;
                for &(w, jw) in &jac {
                    dense[u * n + w] += ju * jw;
                    if w <= u {
                        sky.add(u, w, ju * jw);
                    }
                }
            }
        }
        for i in 0..n {
            dense[i * n + i] += 1.0;
            sky.add_diag(i, 1.0);
        }
        (sky, dense, y)
    }

    #[test]
    fn the_skyline_solve_is_the_dense_solve_bit_for_bit() {
        let mut rng = Rng(0x9e37_79b9_7f4a_7c15);
        for &(n, rows, b) in &[
            (1, 3, 1),
            (2, 5, 2),
            (12, 40, 4),
            (60, 400, 9),
            (200, 2000, 15),
        ] {
            let (mut sky, mut dense, y) = system(n, rows, b, &mut rng);
            for i in 0..n {
                for j in 0..n {
                    assert_eq!(sky.get(i, j).to_bits(), dense[i * n + j].to_bits());
                }
            }
            let a = sky.cholesky_solve(&y).expect("positive definite");
            let b = dense_cholesky_solve(&mut dense, &y, n).expect("positive definite");
            for (x, z) in a.iter().zip(&b) {
                assert_eq!(x.to_bits(), z.to_bits(), "n {n}: {x} vs {z}");
            }
        }
    }

    #[test]
    fn a_singular_matrix_fails_where_the_dense_one_does() {
        // Row 1 is all zero: the second pivot is 0.
        let mut sky = Skyline::zeros(vec![0, 0, 0]);
        sky.add_diag(0, 4.0);
        sky.add_diag(2, 1.0);
        sky.add(2, 0, 1.0);
        assert!(sky.cholesky_solve(&[1.0, 1.0, 1.0]).is_none());
        let mut dense = vec![4.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 1.0];
        assert!(dense_cholesky_solve(&mut dense, &[1.0, 1.0, 1.0], 3).is_none());
        // An empty system solves to nothing.
        assert_eq!(
            Skyline::zeros(Vec::new()).cholesky_solve(&[]),
            Some(Vec::new())
        );
    }

    #[test]
    fn a_banded_system_solves() {
        // Tridiagonal 2 -1 with first[i] = i - 1: A x = A·1 must give 1.
        let n = 6;
        let mut sky = Skyline::zeros((0..n).map(|i: usize| i.saturating_sub(1)).collect());
        for i in 0..n {
            sky.add_diag(i, 2.0);
            if i > 0 {
                sky.add(i, i - 1, -1.0);
            }
        }
        let y: Vec<f64> = (0..n)
            .map(|i| if i == 0 || i == n - 1 { 1.0 } else { 0.0 })
            .collect();
        let x = sky.cholesky_solve(&y).expect("positive definite");
        for v in x {
            assert!((v - 1.0).abs() < 1e-12, "{v}");
        }
    }
}
