//! Preserve sharp corners when reducing the dynamic program's sampling grid.

use crate::FitConfig;
use inkvec_core::Polyline;

/// Keep a bounded sampling grid, moving each sample within its own cell to a bend
/// that the chord would otherwise miss by more than the measurement tolerance.
pub(crate) fn indices(poly: &Polyline, stride: usize, cfg: &FitConfig) -> Vec<usize> {
    let n = poly.len();
    let mut grid: Vec<usize> = (0..n).step_by(stride).collect();
    if !poly.closed && *grid.last().unwrap() != n - 1 {
        grid.push(n - 1);
    }
    let mut keep = grid.clone();
    for k in 0..grid.len() {
        if !poly.closed && (k == 0 || k + 1 == grid.len()) {
            continue;
        }
        let i = grid[k] as isize;
        let prev = if k == 0 {
            grid[grid.len() - 1] as isize - n as isize
        } else {
            grid[k - 1] as isize
        };
        let next = if k + 1 == grid.len() {
            n as isize
        } else {
            grid[k + 1] as isize
        };
        let index = |v: isize| v.rem_euclid(n as isize) as usize;
        let a = poly.points[index(prev)];
        let chord = poly.points[index(next)] - a;
        let length2 = chord.dot(chord);
        if length2 < 1e-12 {
            continue;
        }
        let mut best = cfg.tau * cfg.tau;
        // Disjoint cells: ties at the midpoint belong to the following sample.
        let lo = (prev + i + 1).div_euclid(2);
        let hi = (i + next + 1).div_euclid(2);
        for candidate in lo..hi {
            let j = index(candidate);
            let v = poly.points[j] - a;
            let outgoing = poly.points[index(next)] - poly.points[j];
            // A smooth arc or an uncertain straight run must keep its uniform
            // samples. Moving those to their largest residual selects noise peaks.
            // Restrict relocation to the same sharp-turn class used by vertex repair.
            let norm = v.norm() * outgoing.norm();
            if norm < 1e-12
                || (v.dot(outgoing) / norm).clamp(-1.0, 1.0).acos()
                    < crate::multimodel::g1_break_radians()
            {
                continue;
            }
            let cross = v.x * chord.y - v.y * chord.x;
            let residual = cross * cross / (length2 * poly.sigma[j].powi(2).max(1e-12));
            if residual > best {
                best = residual;
                keep[k] = j;
            }
        }
    }
    keep.sort_unstable();
    keep
}

#[cfg(test)]
mod tests {
    use super::*;
    use inkvec_core::Point;

    #[test]
    fn smooth_circle_keeps_the_uniform_grid() {
        let n = 2052;
        let points = (0..n)
            .map(|i| {
                let a = std::f64::consts::TAU * i as f64 / n as f64;
                Point::new(300.0 * a.cos(), 300.0 * a.sin())
            })
            .collect();
        let poly = Polyline::new(points, vec![0.05; n], true);
        assert_eq!(
            indices(&poly, 3, &FitConfig::default()),
            (0..n).step_by(3).collect::<Vec<_>>()
        );
    }

    #[test]
    fn open_grid_keeps_endpoints_and_its_budget() {
        let points = (0..1603)
            .map(|i| {
                if i <= 801 {
                    Point::new(i as f64, 0.0)
                } else {
                    Point::new(801.0, (i - 801) as f64)
                }
            })
            .collect();
        let poly = Polyline::new(points, vec![0.05; 1603], false);
        let kept = indices(&poly, 4, &FitConfig::default());
        assert_eq!(kept.first(), Some(&0));
        assert_eq!(kept.last(), Some(&1602));
        assert_eq!(kept.len(), (0..1603).step_by(4).count() + 1);
        assert!(kept.contains(&801), "exact corner must survive");
        assert!(kept.windows(2).all(|pair| pair[0] < pair[1]));
    }
}
