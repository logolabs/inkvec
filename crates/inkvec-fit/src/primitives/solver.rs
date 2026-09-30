//! Small dense linear algebra and Levenberg-Marquardt optimizer.
//!
//! The primitive fitters' numerical kit: at most 5x5 systems (a circle has three
//! parameters, an ellipse or rounded rectangle five), so plain `Vec<Vec<f64>>` matrices
//! and textbook algorithms are the right size. Used by `super::fit_circle`,
//! `super::ellipse` and `super::round_rect`.

/// Solve `a x = b` by Gaussian elimination with partial pivoting. `None` if singular.
///
/// Singular means a pivot below 1e-300 in magnitude, or a non-finite solution; the test
/// is absolute, so callers pass reasonably scaled systems.
pub(crate) fn solve(mut a: Vec<Vec<f64>>, mut b: Vec<f64>) -> Option<Vec<f64>> {
    let n = b.len();
    for col in 0..n {
        let piv = (col..n).max_by(|&i, &j| a[i][col].abs().total_cmp(&a[j][col].abs()))?;
        if a[piv][col].abs() < 1e-300 {
            return None;
        }
        a.swap(col, piv);
        b.swap(col, piv);
        for row in col + 1..n {
            let f = a[row][col] / a[col][col];
            if f == 0.0 {
                continue;
            }
            let (upper, lower) = a.split_at_mut(row);
            let pivot_row = &upper[col];
            for (x, y) in lower[0][col..].iter_mut().zip(&pivot_row[col..]) {
                *x -= f * y;
            }
            b[row] -= f * b[col];
        }
    }
    let mut x = vec![0.0; n];
    for row in (0..n).rev() {
        let mut s = b[row];
        for k in row + 1..n {
            s -= a[row][k] * x[k];
        }
        x[row] = s / a[row][row];
        if !x[row].is_finite() {
            return None;
        }
    }
    Some(x)
}

/// A small dense square matrix on the stack, row-major.
pub(crate) type Mat<const N: usize> = [[f64; N]; N];

/// Lower Cholesky factor of a symmetric positive-definite matrix: `L` with `L·Lᵀ = a`.
/// `None` when a diagonal pivot is not positive, i.e. `a` is not positive definite.
///
/// On fixed-size arrays rather than `Vec<Vec<f64>>`: the elliptical-arc candidate of the
/// dynamic program calls this through [`gen_eigen_5`] tens of thousands of times per
/// trace, and the heap allocations were most of its cost. The arithmetic, and its order,
/// is unchanged.
pub(crate) fn cholesky<const N: usize>(a: &Mat<N>) -> Option<Mat<N>> {
    let mut l = [[0.0; N]; N];
    for i in 0..N {
        for j in 0..=i {
            let mut s = a[i][j];
            s -= l[i][..j]
                .iter()
                .zip(&l[j][..j])
                .map(|(x, y)| x * y)
                .sum::<f64>();
            if i == j {
                if s <= 0.0 {
                    return None;
                }
                l[i][j] = s.sqrt();
            } else {
                l[i][j] = s / l[j][j];
            }
        }
    }
    Some(l)
}

/// `x = L⁻¹ b`, by forward substitution on the lower-triangular `l`.
pub(crate) fn forward_sub<const N: usize>(l: &Mat<N>, b: &[f64; N]) -> [f64; N] {
    let mut x = [0.0; N];
    for i in 0..N {
        let mut s = b[i];
        for k in 0..i {
            s -= l[i][k] * x[k];
        }
        x[i] = s / l[i][i];
    }
    x
}

/// `x = L⁻ᵀ b`, by back substitution on the transpose of the lower-triangular `l`.
pub(crate) fn back_sub_t<const N: usize>(l: &Mat<N>, b: &[f64; N]) -> [f64; N] {
    let mut x = [0.0; N];
    for i in (0..N).rev() {
        let mut s = b[i];
        for k in i + 1..N {
            s -= l[k][i] * x[k];
        }
        x[i] = s / l[i][i];
    }
    x
}

/// Eigen-decomposition of a small symmetric matrix by cyclic Jacobi rotations.
/// Returns `(eigenvalues, eigenvectors)` with eigenvectors as columns.
///
/// Each rotation zeroes one off-diagonal entry `a[p][q]`, with `t = tan θ` the smaller
/// root of `t² + 2·t·cot 2θ − 1 = 0` for stability. Sweeps repeat until the sum of
/// squared upper off-diagonal entries falls below 1e-30, or 100 sweeps. Eigenvalues are
/// unsorted; eigenvectors are orthonormal.
pub(crate) fn sym_eigen<const N: usize>(mut a: Mat<N>) -> ([f64; N], Mat<N>) {
    let mut v = [[0.0; N]; N];
    for (i, row) in v.iter_mut().enumerate() {
        row[i] = 1.0;
    }
    for _sweep in 0..100 {
        let off: f64 = a
            .iter()
            .enumerate()
            .map(|(i, row)| row[i + 1..].iter().map(|x| x * x).sum::<f64>())
            .sum();
        if off < 1e-30 {
            break;
        }
        for p in 0..N {
            for q in p + 1..N {
                if a[p][q].abs() < 1e-300 {
                    continue;
                }
                let theta = (a[q][q] - a[p][p]) / (2.0 * a[p][q]);
                let t = theta.signum() / (theta.abs() + (theta * theta + 1.0).sqrt());
                let t = if theta == 0.0 { 1.0 } else { t };
                let c = 1.0 / (t * t + 1.0).sqrt();
                let s = t * c;
                for row in a.iter_mut() {
                    let (akp, akq) = (row[p], row[q]);
                    row[p] = c * akp - s * akq;
                    row[q] = s * akp + c * akq;
                }
                let (head, tail) = a.split_at_mut(q);
                for (apk, aqk) in head[p].iter_mut().zip(tail[0].iter_mut()) {
                    let (x, y) = (*apk, *aqk);
                    *apk = c * x - s * y;
                    *aqk = s * x + c * y;
                }
                for row in v.iter_mut() {
                    let (vkp, vkq) = (row[p], row[q]);
                    row[p] = c * vkp - s * vkq;
                    row[q] = s * vkp + c * vkq;
                }
            }
        }
    }
    let mut evals = [0.0; N];
    for (i, e) in evals.iter_mut().enumerate() {
        *e = a[i][i];
    }
    (evals, v)
}

/// Solve the 5x5 generalized symmetric eigenvalue problem: finds the vector `theta`
/// minimizing `thetaᵀ cov theta` subject to `thetaᵀ nrm theta = 1`.
///
/// With `nrm = L·Lᵀ` (Cholesky) and `y = Lᵀ·theta`, the problem becomes the ordinary
/// symmetric one for `A = L⁻¹·cov·L⁻ᵀ` (symmetrised against rounding): the minimiser
/// is `A`'s eigenvector for its smallest eigenvalue, and `theta = L⁻ᵀ·y`. `theta` is
/// normalised in the `nrm` metric, not to unit length. `None` if `nrm` is not positive
/// definite.
///
/// Also returns the smallest generalised eigenvalue `μ = θᵀ·cov·θ / θᵀ·nrm·θ`, the
/// minimised ratio itself.
pub(crate) fn gen_eigen_5(cov: &Mat<5>, nrm: &Mat<5>) -> Option<([f64; 5], f64)> {
    let l = cholesky(nrm)?;
    let mut tmp = [[0.0; 5]; 5];
    for j in 0..5 {
        let col: [f64; 5] = std::array::from_fn(|i| cov[i][j]);
        let x = forward_sub(&l, &col);
        for i in 0..5 {
            tmp[i][j] = x[i];
        }
    }
    let mut a = [[0.0; 5]; 5];
    for i in 0..5 {
        a[i] = forward_sub(&l, &tmp[i]);
    }
    for i in 0..5 {
        let (head, tail) = a.split_at_mut(i + 1);
        for (j, row) in tail.iter_mut().enumerate() {
            let m = 0.5 * (head[i][i + 1 + j] + row[i]);
            head[i][i + 1 + j] = m;
            row[i] = m;
        }
    }
    let (evals, evecs) = sym_eigen(a);
    let best = (0..5).min_by(|&i, &j| evals[i].total_cmp(&evals[j]))?;
    let y: [f64; 5] = std::array::from_fn(|i| evecs[i][best]);
    Some((back_sub_t(&l, &y), evals[best]))
}

/// Levenberg–Marquardt on a residual vector whose normal equations `eval` supplies.
///
/// `eval(params) -> (chi², JᵀWJ, JᵀWr)` for the weighted residuals; `project` clamps a
/// trial step back into the feasible set (positive radii, and so on). Returns the best
/// parameters and their chi². Damping is multiplicative on the diagonal, so a badly
/// scaled problem still takes a sensible step, and the iteration stops when a step no
/// longer changes chi² by a relative 1e-10 — well past what the noise can resolve.
///
/// Each iteration solves `(JᵀWJ + μ·diag|JᵀWJ|)·δ = −JᵀWr` and tries `p + δ`, projected.
/// A step that does not raise χ² is taken and `μ` divided by 3 (floor 1e-15); a rejected
/// step multiplies `μ` by 4 and a singular system by 10; once `μ` exceeds 1e12 the
/// search gives up and returns the best point so far. At most `max_iter` iterations.
/// `None` only if `eval` fails at the (projected) start.
pub(crate) fn levenberg_marquardt(
    p0: Vec<f64>,
    max_iter: usize,
    eval: impl Fn(&[f64]) -> Option<(f64, Vec<Vec<f64>>, Vec<f64>)>,
    project: impl Fn(&mut [f64]),
) -> Option<(Vec<f64>, f64)> {
    let mut p = p0;
    project(&mut p);
    let (mut chi2, mut jtj, mut jtr) = eval(&p)?;
    let mut mu = 1e-3;
    let n = p.len();
    for _ in 0..max_iter {
        let mut a = jtj.clone();
        for (k, row) in a.iter_mut().enumerate() {
            row[k] += mu * row[k].abs().max(1e-12);
        }
        let rhs: Vec<f64> = jtr.iter().map(|v| -v).collect();
        let Some(delta) = solve(a, rhs) else {
            mu *= 10.0;
            if mu > 1e12 {
                break;
            }
            continue;
        };
        let mut q: Vec<f64> = (0..n).map(|k| p[k] + delta[k]).collect();
        project(&mut q);
        match eval(&q) {
            Some((c2, j2, r2)) if c2 <= chi2 && c2.is_finite() => {
                let improvement = chi2 - c2;
                p = q;
                chi2 = c2;
                jtj = j2;
                jtr = r2;
                mu = (mu / 3.0).max(1e-15);
                if improvement <= 1e-10 * chi2.max(1e-12) {
                    break;
                }
            }
            _ => {
                mu *= 4.0;
                if mu > 1e12 {
                    break;
                }
            }
        }
    }
    Some((p, chi2))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The dense solvers as they were on `Vec<Vec<f64>>`, kept to prove the array
    /// versions compute the same bits.
    mod reference {
        pub(super) fn cholesky(a: &[Vec<f64>]) -> Option<Vec<Vec<f64>>> {
            let n = a.len();
            let mut l = vec![vec![0.0; n]; n];
            for i in 0..n {
                for j in 0..=i {
                    let mut s = a[i][j];
                    s -= l[i][..j]
                        .iter()
                        .zip(&l[j][..j])
                        .map(|(x, y)| x * y)
                        .sum::<f64>();
                    if i == j {
                        if s <= 0.0 {
                            return None;
                        }
                        l[i][j] = s.sqrt();
                    } else {
                        l[i][j] = s / l[j][j];
                    }
                }
            }
            Some(l)
        }

        pub(super) fn forward_sub(l: &[Vec<f64>], b: &[f64]) -> Vec<f64> {
            let n = b.len();
            let mut x = vec![0.0; n];
            for i in 0..n {
                let mut s = b[i];
                for k in 0..i {
                    s -= l[i][k] * x[k];
                }
                x[i] = s / l[i][i];
            }
            x
        }

        pub(super) fn back_sub_t(l: &[Vec<f64>], b: &[f64]) -> Vec<f64> {
            let n = b.len();
            let mut x = vec![0.0; n];
            for i in (0..n).rev() {
                let mut s = b[i];
                for k in i + 1..n {
                    s -= l[k][i] * x[k];
                }
                x[i] = s / l[i][i];
            }
            x
        }

        pub(super) fn sym_eigen(mut a: Vec<Vec<f64>>) -> (Vec<f64>, Vec<Vec<f64>>) {
            let n = a.len();
            let mut v = vec![vec![0.0; n]; n];
            for (i, row) in v.iter_mut().enumerate() {
                row[i] = 1.0;
            }
            for _sweep in 0..100 {
                let off: f64 = a
                    .iter()
                    .enumerate()
                    .map(|(i, row)| row[i + 1..].iter().map(|x| x * x).sum::<f64>())
                    .sum();
                if off < 1e-30 {
                    break;
                }
                for p in 0..n {
                    for q in p + 1..n {
                        if a[p][q].abs() < 1e-300 {
                            continue;
                        }
                        let theta = (a[q][q] - a[p][p]) / (2.0 * a[p][q]);
                        let t = theta.signum() / (theta.abs() + (theta * theta + 1.0).sqrt());
                        let t = if theta == 0.0 { 1.0 } else { t };
                        let c = 1.0 / (t * t + 1.0).sqrt();
                        let s = t * c;
                        for row in a.iter_mut() {
                            let (akp, akq) = (row[p], row[q]);
                            row[p] = c * akp - s * akq;
                            row[q] = s * akp + c * akq;
                        }
                        let (head, tail) = a.split_at_mut(q);
                        for (apk, aqk) in head[p].iter_mut().zip(tail[0].iter_mut()) {
                            let (x, y) = (*apk, *aqk);
                            *apk = c * x - s * y;
                            *aqk = s * x + c * y;
                        }
                        for row in v.iter_mut() {
                            let (vkp, vkq) = (row[p], row[q]);
                            row[p] = c * vkp - s * vkq;
                            row[q] = s * vkp + c * vkq;
                        }
                    }
                }
            }
            let evals = (0..n).map(|i| a[i][i]).collect();
            (evals, v)
        }

        pub(super) fn gen_eigen_5(cov: &[Vec<f64>], nrm: &[Vec<f64>]) -> Option<Vec<f64>> {
            let l = cholesky(nrm)?;
            let mut tmp = vec![vec![0.0; 5]; 5];
            for j in 0..5 {
                let col: Vec<f64> = (0..5).map(|i| cov[i][j]).collect();
                let x = forward_sub(&l, &col);
                for i in 0..5 {
                    tmp[i][j] = x[i];
                }
            }
            let mut a = vec![vec![0.0; 5]; 5];
            for i in 0..5 {
                let x = forward_sub(&l, &tmp[i]);
                a[i][..5].copy_from_slice(&x[..5]);
            }
            for i in 0..5 {
                let (head, tail) = a.split_at_mut(i + 1);
                for (j, row) in tail.iter_mut().enumerate() {
                    let m = 0.5 * (head[i][i + 1 + j] + row[i]);
                    head[i][i + 1 + j] = m;
                    row[i] = m;
                }
            }
            let (evals, evecs) = sym_eigen(a);
            let best = (0..5).min_by(|&i, &j| evals[i].total_cmp(&evals[j]))?;
            let y: Vec<f64> = (0..5).map(|i| evecs[i][best]).collect();
            Some(back_sub_t(&l, &y))
        }
    }

    /// The stack-array eigen solve returns the very bits of the heap version it replaced,
    /// on random positive semi-definite `cov` and positive definite `nrm`, including
    /// near-singular and badly scaled ones, and fails exactly when it failed.
    #[test]
    fn gen_eigen_5_on_arrays_matches_the_vec_version_bit_for_bit() {
        let mut st = 5u64;
        let mut rnd = || {
            st = st
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            (st >> 11) as f64 / (1u64 << 53) as f64 - 0.5
        };
        for case in 0..2000 {
            let rows = 1 + case % 12;
            let scale = [1.0, 1e-6, 1e6, 1.0][case % 4];
            let mut cov = [[0.0f64; 5]; 5];
            let mut nrm = [[0.0f64; 5]; 5];
            for _ in 0..rows {
                let z: [f64; 5] = std::array::from_fn(|_| scale * rnd());
                let g: [f64; 5] = std::array::from_fn(|_| rnd());
                for a in 0..5 {
                    for b in 0..5 {
                        cov[a][b] += z[a] * z[b];
                        nrm[a][b] += g[a] * g[b];
                    }
                }
            }
            if case % 7 == 0 {
                for (a, row) in nrm.iter_mut().enumerate() {
                    row[a] += 1.0;
                }
            }
            let got = gen_eigen_5(&cov, &nrm).map(|(theta, _)| theta);
            let cv: Vec<Vec<f64>> = cov.iter().map(|r| r.to_vec()).collect();
            let nv: Vec<Vec<f64>> = nrm.iter().map(|r| r.to_vec()).collect();
            let want = reference::gen_eigen_5(&cv, &nv);
            assert_eq!(got.is_some(), want.is_some(), "case {case}");
            if let (Some(g), Some(w)) = (got, want) {
                for k in 0..5 {
                    assert_eq!(g[k].to_bits(), w[k].to_bits(), "case {case}, k {k}");
                }
            }
        }
    }

    #[test]
    fn test_solve_linear_system() {
        // 2x + y = 5
        // x + 3y = 5
        // -> x = 2, y = 1
        let a = vec![vec![2.0, 1.0], vec![1.0, 3.0]];
        let b = vec![5.0, 5.0];
        let x = solve(a, b).expect("solution exists");
        assert!((x[0] - 2.0).abs() < 1e-10);
        assert!((x[1] - 1.0).abs() < 1e-10);
    }

    #[test]
    fn test_cholesky_and_subs() {
        let a = [[4.0, 2.0], [2.0, 5.0]];
        let l = cholesky(&a).expect("pos-def matrix has cholesky");
        assert!((l[0][0] - 2.0).abs() < 1e-10);
        assert!((l[1][0] - 1.0).abs() < 1e-10);
        assert!((l[1][1] - 2.0).abs() < 1e-10);

        let b = [6.0, 8.0];
        let y = forward_sub(&l, &b);
        let x = back_sub_t(&l, &y);
        // a * x should equal b:
        assert!((4.0 * x[0] + 2.0 * x[1] - 6.0).abs() < 1e-10);
        assert!((2.0 * x[0] + 5.0 * x[1] - 8.0).abs() < 1e-10);
    }

    #[test]
    fn test_sym_eigen() {
        let a = [[2.0, 1.0], [1.0, 2.0]];
        let (evals, _) = sym_eigen(a);
        let mut sorted = evals;
        sorted.sort_by(|x, y| x.total_cmp(y));
        // eigenvalues of [[2,1],[1,2]] are 1 and 3
        assert!((sorted[0] - 1.0).abs() < 1e-10);
        assert!((sorted[1] - 3.0).abs() < 1e-10);
    }
}
