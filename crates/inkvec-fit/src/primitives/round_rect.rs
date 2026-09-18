//! Rounded-rectangle geometry and orthogonal-distance fitting.

use inkvec_core::Point;

use super::solver::levenberg_marquardt;
use super::weights;

/// A fitted rounded rectangle with its weighted orthogonal-distance chi².
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RoundRectFit {
    /// Left edge.
    pub x: f64,
    /// Top edge.
    pub y: f64,
    /// Width.
    pub w: f64,
    /// Height.
    pub h: f64,
    /// Corner radius, equal on both axes.
    pub rx: f64,
    /// Weighted sum of squared orthogonal distances from the measured points.
    pub chi2: f64,
}

/// Signed orthogonal distance to an axis-aligned rounded rectangle.
///
/// The rounded rectangle is the Minkowski sum of the inner rectangle (half-extents
/// `hw − rx`, `hh − rx`) with a disc of radius `rx`, so its distance field is the inner
/// rectangle's distance field minus `rx`. That is exact, not an approximation, and it
/// gives the true orthogonal distance whether the contact point is on a side or a
/// corner arc — which is what makes "do the corners fit equal-radius quarter arcs and
/// the sides straight lines" a single least-squares problem instead of a case analysis.
pub fn round_rect_distance(p: Point, cx: f64, cy: f64, hw: f64, hh: f64, rx: f64) -> f64 {
    let qx = (p.x - cx).abs() - (hw - rx);
    let qy = (p.y - cy).abs() - (hh - rx);
    let outside = qx.max(0.0).hypot(qy.max(0.0));
    let inside = qx.max(qy).min(0.0);
    outside + inside - rx
}

pub(crate) fn round_rect_chi2(pts: &[Point], w: &[f64], p: &[f64]) -> f64 {
    pts.iter()
        .zip(w)
        .map(|(pt, wk)| {
            let d = round_rect_distance(*pt, p[0], p[1], p[2], p[3], p[4]);
            wk * d * d
        })
        .sum()
}

/// Orthogonal-distance rounded-rectangle fit. `fixed_rx` pins the corner radius (to
fn corner_radius_guesses(
    pts: &[Point],
    sigma: &[f64],
    bounds: [f64; 4],
    rmax: f64,
    fixed_rx: Option<f64>,
) -> Vec<f64> {
    if let Some(r) = fixed_rx {
        return vec![r.clamp(0.0, rmax)];
    }
    let [x0, x1, y0, y1] = bounds;
    let tol = 3.0 * sigma.iter().copied().fold(0.0f64, f64::max).max(0.05);
    let mut guesses = vec![0.0, 0.1 * rmax, 0.25 * rmax, 0.5 * rmax, 0.8 * rmax, rmax];
    let mut side_est = Vec::new();
    let mut extent =
        |sel: &dyn Fn(&Point) -> bool, coord: &dyn Fn(&Point) -> f64, lo: f64, hi: f64| {
            let vals: Vec<f64> = pts.iter().filter(|p| sel(p)).map(coord).collect();
            if vals.len() >= 2 {
                let a = vals.iter().copied().fold(f64::MAX, f64::min);
                let b = vals.iter().copied().fold(f64::MIN, f64::max);
                side_est.push(a - lo);
                side_est.push(hi - b);
            }
        };
    extent(&|p| (p.y - y0).abs() <= tol, &|p| p.x, x0, x1);
    extent(&|p| (p.y - y1).abs() <= tol, &|p| p.x, x0, x1);
    extent(&|p| (p.x - x0).abs() <= tol, &|p| p.y, y0, y1);
    extent(&|p| (p.x - x1).abs() <= tol, &|p| p.y, y0, y1);
    if !side_est.is_empty() {
        side_est.sort_by(f64::total_cmp);
        guesses.push(side_est[side_est.len() / 2].clamp(0.0, rmax));
    }
    guesses
}

/// Orthogonal-distance rounded-rectangle fit. `fixed_rx` pins the corner radius (to
/// zero, for a plain rectangle) so that the alternative can be costed on its own terms.
pub fn fit_round_rect(pts: &[Point], sigma: &[f64], fixed_rx: Option<f64>) -> Option<RoundRectFit> {
    let n = pts.len();
    if n < 8 {
        return None;
    }
    let w = weights(sigma, n);
    let (mut x0, mut x1, mut y0, mut y1) = (f64::MAX, f64::MIN, f64::MAX, f64::MIN);
    for p in pts {
        x0 = x0.min(p.x);
        x1 = x1.max(p.x);
        y0 = y0.min(p.y);
        y1 = y1.max(p.y);
    }
    let (hw, hh) = (0.5 * (x1 - x0), 0.5 * (y1 - y0));
    if hw < 1e-6 || hh < 1e-6 {
        return None;
    }
    let (cx, cy) = (0.5 * (x0 + x1), 0.5 * (y0 + y1));
    let rmax = hw.min(hh);

    let guesses = corner_radius_guesses(pts, sigma, [x0, x1, y0, y1], rmax, fixed_rx);

    let k = if fixed_rx.is_some() { 4 } else { 5 };
    let scale = rmax.max(1.0);
    let eval = |p: &[f64]| -> Option<(f64, Vec<Vec<f64>>, Vec<f64>)> {
        let full = |q: &[f64]| -> Vec<f64> {
            let mut v = q.to_vec();
            if let Some(r) = fixed_rx {
                v.push(r);
            }
            v
        };
        let base = full(p);
        let mut jtj = vec![vec![0.0; k]; k];
        let mut jtr = vec![0.0; k];
        let mut chi2 = 0.0;
        let h = 1e-6 * scale;
        let mut plus = Vec::with_capacity(k);
        let mut minus = Vec::with_capacity(k);
        for a in 0..k {
            let mut pp = base.clone();
            pp[a] += h;
            let mut pm = base.clone();
            pm[a] -= h;
            plus.push(pp);
            minus.push(pm);
        }
        for (pt, wk) in pts.iter().zip(&w) {
            let d = round_rect_distance(*pt, base[0], base[1], base[2], base[3], base[4]);
            let mut j = [0.0; 5];
            for a in 0..k {
                let (pp, pm) = (&plus[a], &minus[a]);
                let dp = round_rect_distance(*pt, pp[0], pp[1], pp[2], pp[3], pp[4]);
                let dm = round_rect_distance(*pt, pm[0], pm[1], pm[2], pm[3], pm[4]);
                j[a] = (dp - dm) / (2.0 * h);
            }
            chi2 += wk * d * d;
            for a in 0..k {
                jtr[a] += wk * j[a] * d;
                for b in 0..k {
                    jtj[a][b] += wk * j[a] * j[b];
                }
            }
        }
        Some((chi2, jtj, jtr))
    };
    let project = |p: &mut [f64]| {
        p[2] = p[2].abs().max(1e-3);
        p[3] = p[3].abs().max(1e-3);
        if p.len() > 4 {
            p[4] = p[4].clamp(0.0, p[2].min(p[3]));
        }
    };

    let mut best: Option<(Vec<f64>, f64)> = None;
    for g in guesses {
        let mut p0 = vec![cx, cy, hw, hh];
        if fixed_rx.is_none() {
            p0.push(g);
        }
        if let Some((p, chi2)) = levenberg_marquardt(p0, 100, eval, project) {
            match best {
                Some((_, c)) if c <= chi2 => {}
                _ => best = Some((p, chi2)),
            }
        }
    }
    let (p, _) = best?;
    let rx = fixed_rx.unwrap_or_else(|| p[4]);
    let full = [p[0], p[1], p[2], p[3], rx];
    Some(RoundRectFit {
        x: p[0] - p[2],
        y: p[1] - p[3],
        w: 2.0 * p[2],
        h: 2.0 * p[3],
        rx,
        chi2: round_rect_chi2(pts, &w, &full),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_round_rect_distance_points() {
        // Round rect at (0, 0), half-width 10, half-height 5, rx 2
        // Center is (0, 0), hw = 10, hh = 5, rx = 2
        // On side x = 10, y = 0: distance should be 0.0
        let d1 = round_rect_distance(Point::new(10.0, 0.0), 0.0, 0.0, 10.0, 5.0, 2.0);
        assert!(d1.abs() < 1e-10);

        // Outside at x = 12, y = 0: distance should be 2.0
        let d2 = round_rect_distance(Point::new(12.0, 0.0), 0.0, 0.0, 10.0, 5.0, 2.0);
        assert!((d2 - 2.0).abs() < 1e-10);

        // Inside at (0, 0): distance should be -5.0
        let d3 = round_rect_distance(Point::new(0.0, 0.0), 0.0, 0.0, 10.0, 5.0, 2.0);
        assert!((d3 - (-5.0)).abs() < 1e-10);
    }
}
