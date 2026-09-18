//! Junction refinement for the planar map (DESIGN.md S3).
//!
//! Refines junction positions where 3 or more boundaries meet, using weighted
//! least-squares intersection of boundary tangents, falling back to taper fits
//! where boundaries meet tangentially.

use super::{node_point, Edge, PlanarMap};
use inkvec_core::{Point, Vec2};
use std::collections::HashMap;

/// Number of refined interior points, nearest the junction, used to estimate each
/// incident edge's end tangent.
fn junction_fit_points() -> usize {
    6
}

/// Interior points this close to the junction (in index terms) are skipped: the pixel
/// containing the junction is a three-way mixture, and the two-colour unmixing that
/// placed those points does not know it.
fn junction_skip() -> usize {
    1
}

fn junction_curvature_points() -> usize {
    16
}

/// Farthest a junction may move from its grid node before the solution is distrusted.
const JUNCTION_MAX_MOVE: f64 = 1.5;

/// Minimum ratio of the eigenvalues of the (unweighted) normal matrix. Two lines at
/// angle `theta` give `tan^2(theta / 2)`, so this rejects intersections of lines closer
/// than about 16 degrees, where the crossing point is not localized along them.
const JUNCTION_MIN_CONDITION: f64 = 0.02;

/// One incident edge's end tangent, as a line `n . p = c` with unit normal `n`, and the
/// variance of its perpendicular position at the junction.
#[derive(Clone, Copy)]
struct TangentLine {
    n: (f64, f64),
    c: f64,
    var: f64,
}

/// Grid position of a node id.
///
/// A corner the map split (see `build`) has two ids one whole grid apart, and both stand
/// for that same corner, so the id is folded back into the grid before it is read.
pub fn node_position(id: u32, w: usize, h: usize) -> Point {
    let id = (id as usize) % ((w + 1) * (h + 1));
    node_point(id % (w + 1), id / (w + 1))
}

/// Weighted least-squares line through the interior points of `e` nearest the end at
/// `at_start`, extrapolated to the junction at `origin`.
fn end_tangent(e: &Edge, at_start: bool, origin: Point) -> Option<TangentLine> {
    let n = e.points.len();
    if n < 2 {
        return None;
    }
    if e.left == u16::MAX || e.right == u16::MAX {
        // The image border: never refined, and exact by construction. Its line is the
        // segment adjacent to the junction, known with essentially no uncertainty.
        let (p, q) = if at_start {
            (e.points[0], e.points[1])
        } else {
            (e.points[n - 1], e.points[n - 2])
        };
        let d = q - p;
        let l = d.norm();
        if l < 1e-9 {
            return None;
        }
        let nrm = (-d.y / l, d.x / l);
        return Some(TangentLine {
            n: nrm,
            c: nrm.0 * p.x + nrm.1 * p.y,
            var: 1e-6,
        });
    }

    // Interior indices nearest the junction, excluding both junction points.
    let idx: Vec<usize> = if at_start {
        (1 + junction_skip()..n - 1)
            .take(junction_curvature_points())
            .collect()
    } else {
        (1..n.saturating_sub(1 + junction_skip()))
            .rev()
            .take(junction_curvature_points())
            .collect()
    };
    if idx.len() < 2 {
        return None;
    }
    let wts: Vec<f64> = idx
        .iter()
        .map(|&k| 1.0 / e.sigma[k].max(1e-3).powi(2))
        .collect();
    let wsum: f64 = wts.iter().sum();

    // Principal direction of the weighted scatter.
    let (mut mx, mut my) = (0.0, 0.0);
    for (&k, &wk) in idx.iter().zip(&wts) {
        mx += wk * e.points[k].x;
        my += wk * e.points[k].y;
    }
    mx /= wsum;
    my /= wsum;
    let (mut sxx, mut syy, mut sxy) = (0.0, 0.0, 0.0);
    for (&k, &wk) in idx.iter().zip(&wts) {
        let (dx, dy) = (e.points[k].x - mx, e.points[k].y - my);
        sxx += wk * dx * dx;
        syy += wk * dy * dy;
        sxy += wk * dx * dy;
    }
    let theta = 0.5 * (2.0 * sxy).atan2(sxx - syy);
    let u = (theta.cos(), theta.sin());
    let v = (-u.1, u.0);

    // Local coordinates: distance along `u` from the junction, and offset along `v`.
    let (ts, rs): (Vec<f64>, Vec<f64>) = idx
        .iter()
        .map(|&k| {
            let (dx, dy) = (e.points[k].x - origin.x, e.points[k].y - origin.y);
            (dx * u.0 + dy * u.1, dx * v.0 + dy * v.1)
        })
        .unzip();
    let (a, b, var_a) = fit_end_polynomial(&ts, &rs, &wts, junction_fit_points())?;

    // The fitted line: through `origin + a v` with direction `u + b v`.
    let d = (u.0 + b * v.0, u.1 + b * v.1);
    let dl = d.0.hypot(d.1);
    let nrm = (-d.1 / dl, d.0 / dl);
    let p0 = Point::new(origin.x + a * v.0, origin.y + a * v.1);
    // The variance of `a` is along `v`; project onto the line's actual normal.
    let cosang = (nrm.0 * v.0 + nrm.1 * v.1).abs().max(1e-3);
    Some(TangentLine {
        n: nrm,
        c: nrm.0 * p0.x + nrm.1 * p0.y,
        var: (var_a * cosang * cosang).max(1e-6),
    })
}

/// Weighted least-squares polynomial `r(t)` through local samples, returning its tangent
/// line at `t = 0` — intercept `a`, slope `b` — and the variance of `a`.
fn fit_end_polynomial(ts: &[f64], rs: &[f64], ws: &[f64], n_fit: usize) -> Option<(f64, f64, f64)> {
    const MIN_QUADRATIC_POINTS: usize = 5;
    const CURVATURE_SIGNIFICANCE: f64 = 3.0;

    let fit = |degree: usize, n: usize| -> Option<([f64; 3], [f64; 3])> {
        let m = degree + 1;
        let mut mat = [[0.0; 3]; 3];
        let mut rhs = [0.0; 3];
        for ((&t, &r), &w) in ts.iter().zip(rs).zip(ws).take(n) {
            let tp = [1.0, t, t * t, t * t * t, t * t * t * t];
            for i in 0..m {
                for j in 0..m {
                    mat[i][j] += w * tp[i + j];
                }
                rhs[i] += w * tp[i] * r;
            }
        }
        let inv = invert_small(&mat, m)?;
        let mut x = [0.0; 3];
        for i in 0..m {
            for j in 0..m {
                x[i] += inv[i][j] * rhs[j];
            }
        }
        let mut chi2 = 0.0;
        for ((&t, &r), &w) in ts.iter().zip(rs).zip(ws).take(n) {
            let res = r - (x[0] + x[1] * t + x[2] * t * t);
            chi2 += w * res * res;
        }
        let dof = n.saturating_sub(m);
        let scale = if dof > 0 {
            (chi2 / dof as f64).max(1.0)
        } else {
            1.0
        };
        Some((x, [inv[0][0] * scale, inv[1][1] * scale, inv[2][2] * scale]))
    };

    let n_all = ts.len();
    let n_fit = n_fit.min(n_all);
    let (xl, vl) = fit(1, n_fit)?;
    if std::env::var("JDBG").is_ok() {
        let q = if n_all >= MIN_QUADRATIC_POINTS {
            fit(2, n_all)
        } else {
            None
        };
        eprintln!(
            "JFIT n={n_all}/{n_fit} ts={:?} rs={:?} ws={:?} lin={:?}/{:?} quad={:?}",
            ts.iter()
                .map(|t| (t * 100.0).round() / 100.0)
                .collect::<Vec<_>>(),
            rs.iter()
                .map(|t| (t * 1000.0).round() / 1000.0)
                .collect::<Vec<_>>(),
            ws.iter().map(|t| t.round()).collect::<Vec<_>>(),
            xl,
            vl,
            q
        );
    }
    // Is the edge curved? Decided on the whole window, where the curvature term is far
    // better determined than on the points used for extrapolation.
    if n_all >= MIN_QUADRATIC_POINTS && n_fit >= MIN_QUADRATIC_POINTS {
        if let Some((xq, vq)) = fit(2, n_all) {
            if xq[2].abs() > CURVATURE_SIGNIFICANCE * vq[2].max(0.0).sqrt() {
                let (xq, vq) = if n_fit >= n_all {
                    (xq, vq)
                } else {
                    fit(2, n_fit)?
                };
                return Some((xq[0], xq[1], vq[0]));
            }
        }
    }
    Some((xl[0], xl[1], vl[0]))
}

/// Inverse of the leading `m x m` block of a symmetric positive matrix, by Gauss-Jordan
/// elimination with partial pivoting. `None` when singular.
fn invert_small(mat: &[[f64; 3]; 3], m: usize) -> Option<[[f64; 3]; 3]> {
    let mut a = *mat;
    let mut inv = [[0.0; 3]; 3];
    for (i, row) in inv.iter_mut().enumerate().take(m) {
        row[i] = 1.0;
    }
    let scale = (0..m).map(|i| a[i][i].abs()).fold(0.0, f64::max);
    for col in 0..m {
        let piv = (col..m)
            .max_by(|&p, &q| a[p][col].abs().total_cmp(&a[q][col].abs()))
            .unwrap();
        if a[piv][col].abs() <= 1e-12 * scale {
            return None;
        }
        a.swap(col, piv);
        inv.swap(col, piv);
        let d = a[col][col];
        for j in 0..m {
            a[col][j] /= d;
            inv[col][j] /= d;
        }
        for r in 0..m {
            if r == col {
                continue;
            }
            let f = a[r][col];
            if f == 0.0 {
                continue;
            }
            for j in 0..m {
                a[r][j] -= f * a[col][j];
                inv[r][j] -= f * inv[col][j];
            }
        }
    }
    Some(inv)
}

/// Move every junction node to the sub-pixel point where its incident boundaries meet.
pub fn refine_junctions(map: &mut PlanarMap) {
    let (w, h) = (map.width, map.height);

    let mut inc: HashMap<u32, Vec<(usize, bool)>> = HashMap::new();
    for (k, e) in map.edges.iter().enumerate() {
        if e.closed || e.points.len() < 2 {
            continue;
        }
        inc.entry(e.start_node).or_default().push((k, true));
        inc.entry(e.end_node).or_default().push((k, false));
    }
    let mut nodes: Vec<u32> = inc.keys().copied().collect();
    nodes.sort_unstable();

    for node in nodes {
        let list = &inc[&node];
        if list.len() < 2 {
            continue;
        }
        let origin = node_position(node, w, h);

        let lines: Vec<TangentLine> = list
            .iter()
            .filter_map(|&(k, at_start)| end_tangent(&map.edges[k], at_start, origin))
            .collect();

        let solved = solve_junction(&lines, origin);
        let taper = if solved.is_none() {
            taper_junction(map, list, origin)
        } else {
            None
        };
        let slid = taper.is_some();
        let (p, sigma) = taper.or(solved).unwrap_or((origin, 0.5));
        if std::env::var("JDBG").is_ok() {
            eprintln!(
                "JDBG node {node} deg {} lines {} origin ({:.2},{:.2}) -> ({:.3},{:.3}) move {:.3} sigma {:.3} vars {:?}",
                list.len(),
                lines.len(),
                origin.x,
                origin.y,
                p.x,
                p.y,
                p.dist(origin),
                sigma,
                lines.iter().map(|l| l.var).collect::<Vec<_>>()
            );
        }

        if slid {
            trim_passed_over(map, list, origin, p);
        }
        for &(k, at_start) in list {
            let e = &mut map.edges[k];
            let i = if at_start { 0 } else { e.points.len() - 1 };
            e.points[i] = p;
            e.sigma[i] = sigma;
        }
    }
}

/// Fewest points an edge must keep. Below this it has no shape left to fit.
const TRIM_MIN_POINTS: usize = 4;

/// How far along an edge to look when asking which way it leaves the junction, in points.
const TRIM_LOOK: usize = 3;

/// Drop the measured points a moved junction has slid past.
fn trim_passed_over(map: &mut PlanarMap, list: &[(usize, bool)], origin: Point, p: Point) {
    let m = p - origin;
    let len = m.norm();
    if len < 1e-9 {
        return;
    }
    let step = Vec2 {
        x: m.x / len,
        y: m.y / len,
    };

    for &(k, at_start) in list {
        let e = &mut map.edges[k];
        let n = e.points.len();
        if n < TRIM_MIN_POINTS {
            continue;
        }
        let ahead = e.points[if at_start {
            TRIM_LOOK.min(n - 1)
        } else {
            n - 1 - TRIM_LOOK.min(n - 1)
        }];
        let end = e.points[if at_start { 0 } else { n - 1 }];
        if (ahead - end).dot(step) <= 0.0 {
            continue;
        }
        let mut drop = 0usize;
        while n - drop > TRIM_MIN_POINTS {
            let idx = if at_start { drop + 1 } else { n - 2 - drop };
            let q = e.points[idx];
            if (q - p).dot(step) < 0.0 {
                drop += 1;
            } else {
                break;
            }
        }
        if drop == 0 {
            continue;
        }
        if at_start {
            e.points.drain(1..=drop);
            e.sigma.drain(1..=drop);
        } else {
            let end = e.points.len() - 1;
            e.points.drain(end - drop..end);
            e.sigma.drain(end - drop..end);
        }
        debug_assert_eq!(e.points.len(), e.sigma.len());
    }
}

const TAPER_DEGREES: f64 = 55.0;
const TAPER_MAX_MOVE: f64 = 8.0;
const TAPER_MAX_CONSUMED: f64 = 0.35;
const TAPER_MAX_SIGMA: f64 = 1.0;

/// Place a junction where two boundaries meet tangentially, from the region that tapers.
fn taper_junction(map: &PlanarMap, list: &[(usize, bool)], origin: Point) -> Option<(Point, f64)> {
    static DISABLED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    if list.len() < 3
        || *DISABLED
            .get_or_init(|| std::env::var_os("INKVEC_NO_TAPER").is_some_and(|v| !v.is_empty()))
    {
        return None;
    }
    let dir = |&(k, at_start): &(usize, bool)| -> Option<Vec2> {
        let e = &map.edges[k];
        let n = e.points.len();
        if n < 2 {
            return None;
        }
        let step = 3.min(n - 1);
        let (a, b) = if at_start {
            (e.points[0], e.points[step])
        } else {
            (e.points[n - 1], e.points[n - 1 - step])
        };
        let v = b - a;
        (v.norm() > 1e-9).then(|| Vec2 {
            x: v.x / v.norm(),
            y: v.y / v.norm(),
        })
    };
    let dirs: Vec<(usize, bool, Vec2)> = list
        .iter()
        .filter_map(|e| dir(e).map(|d| (e.0, e.1, d)))
        .collect();
    if dirs.len() < 3 {
        return None;
    }

    let angle = |a: Vec2, b: Vec2| (a.x * b.y - a.y * b.x).atan2(a.x * b.x + a.y * b.y).abs();
    let mut best = (0.0f64, 0usize, 1usize);
    for i in 0..dirs.len() {
        for j in i + 1..dirs.len() {
            let t = angle(dirs[i].2, dirs[j].2);
            if t > best.0 {
                best = (t, i, j);
            }
        }
    }
    let through = dirs[best.1].2;

    let mut branch: Option<(usize, bool)> = None;
    for (k, (ek, at_start, d)) in dirs.iter().enumerate() {
        if k == best.1 || k == best.2 {
            continue;
        }
        let a = angle(through, *d).to_degrees();
        if a.min(180.0 - a) <= TAPER_DEGREES {
            branch = Some((*ek, *at_start));
        }
    }
    let (bk, _) = branch?;

    let normal = Vec2 {
        x: -through.y,
        y: through.x,
    };
    let samples: Vec<(f64, f64)> = map.edges[bk]
        .points
        .iter()
        .map(|p| {
            let v = *p - origin;
            (
                v.x * through.x + v.y * through.y,
                (v.x * normal.x + v.y * normal.y).abs(),
            )
        })
        .collect();

    let t = crate::taper::fit(&samples);
    if std::env::var("TAPERDBG").is_ok() {
        eprintln!(
            "  taper node at ({:.1},{:.1}): edge {bk}, {} pts -> {:?}",
            origin.x,
            origin.y,
            samples.len(),
            t.map(|x| (x.vanish, x.sigma, x.implied_radius, x.tangency_defect))
        );
    }
    let t = t?;
    if let Some(skip) = std::env::var("INKVEC_TAPER_SKIP")
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
    {
        static SEEN: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        if SEEN.fetch_add(1, std::sync::atomic::Ordering::SeqCst) == skip {
            return None;
        }
    }
    let shortest = list
        .iter()
        .map(|&(k, _)| {
            map.edges[k]
                .points
                .windows(2)
                .map(|w| w[0].dist(w[1]))
                .sum::<f64>()
        })
        .fold(f64::INFINITY, f64::min);
    if t.vanish.abs() > TAPER_MAX_MOVE
        || t.sigma > TAPER_MAX_SIGMA
        || t.vanish.abs() > TAPER_MAX_CONSUMED * shortest
    {
        return None;
    }
    Some((
        Point::new(
            origin.x + through.x * t.vanish,
            origin.y + through.y * t.vanish,
        ),
        t.sigma.clamp(0.05, 1.0),
    ))
}

/// Weighted least-squares intersection of `lines`, with the standard deviation of the
/// result, or `None` when the problem is not well posed.
fn solve_junction(lines: &[TangentLine], origin: Point) -> Option<(Point, f64)> {
    if lines.len() < 2 {
        return None;
    }

    let (mut gxx, mut gxy, mut gyy) = (0.0, 0.0, 0.0);
    for l in lines {
        gxx += l.n.0 * l.n.0;
        gxy += l.n.0 * l.n.1;
        gyy += l.n.1 * l.n.1;
    }
    let (lmin, lmax) = eigen2(gxx, gxy, gyy);
    if lmax <= 0.0 || lmin / lmax < JUNCTION_MIN_CONDITION {
        return None;
    }

    let (mut axx, mut axy, mut ayy, mut bx, mut by) = (0.0, 0.0, 0.0, 0.0, 0.0);
    for l in lines {
        let wt = 1.0 / l.var;
        axx += wt * l.n.0 * l.n.0;
        axy += wt * l.n.0 * l.n.1;
        ayy += wt * l.n.1 * l.n.1;
        bx += wt * l.n.0 * l.c;
        by += wt * l.n.1 * l.c;
    }
    let det = axx * ayy - axy * axy;
    if det <= 1e-12 * (axx + ayy) * (axx + ayy) {
        return None;
    }
    let p = Point::new((ayy * bx - axy * by) / det, (axx * by - axy * bx) / det);
    if !p.x.is_finite() || !p.y.is_finite() || p.dist(origin) > JUNCTION_MAX_MOVE {
        return None;
    }

    let (amin, _) = eigen2(axx, axy, ayy);
    let sigma = (1.0 / amin.max(1e-12)).sqrt().clamp(0.02, 2.0);
    Some((p, sigma))
}

/// Eigenvalues `(min, max)` of the symmetric matrix `[[a, b], [b, c]]`.
fn eigen2(a: f64, b: f64, c: f64) -> (f64, f64) {
    let m = 0.5 * (a + c);
    let d = (0.25 * (a - c) * (a - c) + b * b).sqrt();
    (m - d, m + d)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn poly_fit_recovers_line_and_parabola() {
        let ts: Vec<f64> = (1..=8).map(|k| k as f64).collect();
        let ws = vec![100.0; 8];
        let rs: Vec<f64> = ts.iter().map(|t| 0.3 + 0.2 * t).collect();
        let inv = invert_small(&[[2.0, 1.0, 0.0], [1.0, 2.0, 0.0], [0.0, 0.0, 0.0]], 2).unwrap();
        let _ = inv;
        let r = fit_end_polynomial(&ts, &rs, &ws, 8).unwrap();
        assert!((r.0 - 0.3).abs() < 1e-9 && (r.1 - 0.2).abs() < 1e-9);
        let rs: Vec<f64> = ts.iter().map(|t| 0.3 + 0.2 * t - 0.05 * t * t).collect();
        let r = fit_end_polynomial(&ts, &rs, &ws, 8).unwrap();
        assert!((r.0 - 0.3).abs() < 1e-6 && (r.1 - 0.2).abs() < 1e-6);
    }
}
