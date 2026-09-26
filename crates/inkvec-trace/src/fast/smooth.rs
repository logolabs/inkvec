//! From polygon to curve: least-squares vertex placement, then corner-aware smoothing into
//! Bézier pieces (Selinger 2003, sections 2.3.1 and 2.3.2, restated for sub-pixel input).
//!
//! Each side of the polygon gets the least-squares line through the points it spans, and
//! each vertex moves to the point nearest both of its sides' lines, within a small box
//! around where the polygon put it. The curve then runs from a join on one side to a join on
//! the next, tangent to both sides there. Potrace puts the joins at the sides' midpoints and
//! the control points a fraction `alpha` of the way to the vertex, with `alpha` read off
//! the lattice ("the curve should touch the unit square around the vertex"). These points
//! are already sub-pixel, so both are taken from the data instead: each join moves off its
//! side's line to where the points are, and the two arm lengths are the least-squares fit
//! to the points the piece replaces, capped at the vertex (Potrace's `alpha <= 1`). A
//! vertex is a corner when that curve misses the points by more than `corner_tol` and the
//! two sides through the vertex miss them by much less -- Potrace's `alphamax` test, stated
//! as a distance -- and the piece then becomes two lines.

use inkvec_core::{Point, Vec2};

/// One piece of the smoothed boundary: a cubic, possibly degenerate (a line), and whether
/// it meets the previous piece smoothly.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Piece {
    /// Start, two control points, end.
    pub p: [Point; 4],
    /// The piece meets its predecessor with a common tangent. False at corners and at the
    /// start of an open run.
    pub smooth_in: bool,
}

impl Piece {
    fn line(a: Point, b: Point, smooth_in: bool) -> Self {
        Self {
            p: [a, lerp(a, b, 1.0 / 3.0), lerp(a, b, 2.0 / 3.0), b],
            smooth_in,
        }
    }
}

pub(crate) fn lerp(a: Point, b: Point, t: f64) -> Point {
    Point::new(a.x + (b.x - a.x) * t, a.y + (b.y - a.y) * t)
}

/// The measured points with the staircase taken out: a `[1 2 1] / 4` average along the
/// boundary, which removes the alternation the pixel lattice leaves in refined points
/// (about 0.1 px) and moves a curve of radius `r` by only `1 / 4r` px. The ends of an open
/// boundary are junctions and stay put; a closed ring is averaged all the way round.
pub(crate) fn denoise(pts: &[Point], closed: bool) -> Vec<Point> {
    let n = pts.len();
    if n < 4 {
        return pts.to_vec();
    }
    let src = pts;
    let avg = |a: Point, b: Point, c: Point| {
        Point::new(
            0.25 * (a.x + c.x) + 0.5 * b.x,
            0.25 * (a.y + c.y) + 0.5 * b.y,
        )
    };
    (0..n)
        .map(|k| {
            if closed {
                avg(src[(k + n - 1) % n], src[k], src[(k + 1) % n])
            } else if k == 0 || k == n - 1 {
                src[k]
            } else {
                avg(src[k - 1], src[k], src[k + 1])
            }
        })
        .collect()
}

/// The least-squares line through `pts` as a quadratic form in (x, y, 1): the squared
/// distance from a point to the line is `[x y 1] Q [x y 1]^T`.
fn line_form(pts: &[Point]) -> [[f64; 3]; 3] {
    let n = pts.len().max(1) as f64;
    let (mut cx, mut cy) = (0.0, 0.0);
    for p in pts {
        cx += p.x;
        cy += p.y;
    }
    cx /= n;
    cy /= n;
    let (mut sxx, mut sxy, mut syy) = (0.0, 0.0, 0.0);
    for p in pts {
        let (dx, dy) = (p.x - cx, p.y - cy);
        sxx += dx * dx;
        sxy += dx * dy;
        syy += dy * dy;
    }
    // The line's direction is the principal axis; its normal the other one.
    let theta = 0.5 * (2.0 * sxy).atan2(sxx - syy);
    let (a, b) = (-theta.sin(), theta.cos());
    let c = -(a * cx + b * cy);
    let v = [a, b, c];
    let mut q = [[0.0; 3]; 3];
    for (i, row) in q.iter_mut().enumerate() {
        for (j, e) in row.iter_mut().enumerate() {
            *e = v[i] * v[j];
        }
    }
    q
}

fn quad_eval(q: &[[f64; 3]; 3], x: f64, y: f64) -> f64 {
    let v = [x, y, 1.0];
    let mut s = 0.0;
    for i in 0..3 {
        for j in 0..3 {
            s += v[i] * q[i][j] * v[j];
        }
    }
    s
}

/// The minimum of the quadratic form over the box `c +- h`, Potrace's constrained vertex.
fn constrained_min(q: &[[f64; 3]; 3], c: Point, h: f64) -> Point {
    // A small pull toward the polygon's own vertex keeps parallel sides well posed.
    const RIDGE: f64 = 1e-3;
    let (a, b, d) = (q[0][0] + RIDGE, q[0][1], q[1][1] + RIDGE);
    let (gx, gy) = (q[0][2] - RIDGE * c.x, q[1][2] - RIDGE * c.y);
    let det = a * d - b * b;
    let eval =
        |x: f64, y: f64| quad_eval(q, x, y) + RIDGE * ((x - c.x).powi(2) + (y - c.y).powi(2));
    if det.abs() > 1e-12 {
        let x = (-gx * d + gy * b) / det;
        let y = (gx * b - gy * a) / det;
        if (x - c.x).abs() <= h && (y - c.y).abs() <= h {
            return Point::new(x, y);
        }
    }
    // On the box: each side is a one-dimensional quadratic.
    let mut best = (c, eval(c.x, c.y));
    let mut consider = |x: f64, y: f64| {
        let e = eval(x, y);
        if e < best.1 {
            best = (Point::new(x, y), e);
        }
    };
    for &x in &[c.x - h, c.x + h] {
        let y = if d > 1e-12 {
            (-(gy + b * x) / d).clamp(c.y - h, c.y + h)
        } else {
            c.y
        };
        consider(x, y);
    }
    for &y in &[c.y - h, c.y + h] {
        let x = if a > 1e-12 {
            (-(gx + b * y) / a).clamp(c.x - h, c.x + h)
        } else {
            c.x
        };
        consider(x, y);
    }
    best.0
}

/// Vertex positions for polygon `vtx` (indices into `pts`). The ends of an open run stay
/// exactly where they are: they are junctions, shared with other boundaries.
pub(crate) fn adjust_vertices(pts: &[Point], vtx: &[usize], closed: bool, h: f64) -> Vec<Point> {
    let m = vtx.len();
    let n = pts.len();
    if m < 2 {
        return vtx.iter().map(|&k| pts[k]).collect();
    }
    let sides = if closed { m } else { m - 1 };
    let forms: Vec<[[f64; 3]; 3]> = (0..sides)
        .map(|s| {
            let (a, b) = (vtx[s], vtx[(s + 1) % m]);
            if b >= a {
                line_form(&pts[a..=b])
            } else {
                let run: Vec<Point> = (a..n).chain(0..=b).map(|k| pts[k]).collect();
                line_form(&run)
            }
        })
        .collect();
    (0..m)
        .map(|i| {
            let here = pts[vtx[i]];
            if !closed && (i == 0 || i == m - 1) {
                return here;
            }
            let prev = forms[(i + sides - 1) % sides];
            let next = forms[i % sides];
            let mut q = [[0.0; 3]; 3];
            for r in 0..3 {
                for c in 0..3 {
                    q[r][c] = prev[r][c] + next[r][c];
                }
            }
            constrained_min(&q, here, h)
        })
        .collect()
}

/// The points strictly between polygon indices `lo` and `hi` of a ring of `n` points.
fn between(pts: &[Point], lo: usize, hi: usize) -> Vec<Point> {
    let n = pts.len();
    let mut out = Vec::new();
    if n == 0 || lo % n == hi % n {
        return out;
    }
    let mut k = (lo + 1) % n;
    while k != hi % n {
        out.push(pts[k]);
        k = (k + 1) % n;
    }
    out
}

/// Midpoint index between two polygon indices, walking forwards round a ring of `n`.
fn mid_index(lo: usize, hi: usize, n: usize) -> usize {
    let span = (hi + n - lo) % n;
    (lo + span / 2) % n
}

/// Largest distance, in pixels, a join is moved off its side's line towards the data.
const JOIN_MAX: f64 = 0.5;

fn unit(v: Vec2) -> Vec2 {
    let n = v.norm();
    if n > 1e-12 {
        Vec2 {
            x: v.x / n,
            y: v.y / n,
        }
    } else {
        Vec2 { x: 1.0, y: 0.0 }
    }
}

/// Largest distance from `pts` to the polyline `a`-`b`-`c`.
fn corner_error(a: Point, b: Point, c: Point, pts: &[Point]) -> f64 {
    let seg = |p: Point, s: Point, e: Point| {
        let d = e - s;
        let l2 = d.dot(d);
        let t = if l2 > 1e-18 {
            ((p - s).dot(d) / l2).clamp(0.0, 1.0)
        } else {
            0.0
        };
        p.dist(lerp(s, e, t))
    };
    pts.iter()
        .map(|&p| seg(p, a, b).min(seg(p, b, c)))
        .fold(0.0, f64::max)
}

/// Smooth the adjusted polygon `v` (vertex `i` placed for point `vtx[i]`) into pieces.
///
/// The pieces meet at one join per side, with the side's direction as their common
/// tangent. A join starts at the side's midpoint, which lies on the side's least-squares
/// line, and moves along the side's normal to the data there: on a curve the line runs a
/// third of a sagitta inside the points at mid-side, and a curve drawn through the
/// unmoved midpoints would shrink every round shape by that much.
pub(crate) fn pieces(
    pts: &[Point],
    vtx: &[usize],
    v: &[Point],
    closed: bool,
    corner_tol: f64,
) -> Vec<Piece> {
    let m = v.len();
    let n = pts.len();
    if m < 2 {
        return Vec::new();
    }
    if !closed && m == 2 {
        return vec![Piece::line(v[0], v[1], false)];
    }
    let sides = if closed { m } else { m - 1 };
    let side: Vec<(Vec2, Point)> = (0..sides)
        .map(|s| {
            let (p, q) = (v[s], v[(s + 1) % m]);
            let d = unit(q - p);
            let mid = lerp(p, q, 0.5);
            let k = mid_index(vtx[s], vtx[(s + 1) % m], n);
            let at = |j: isize| {
                let j = if closed {
                    j.rem_euclid(n as isize)
                } else {
                    j.clamp(0, n as isize - 1)
                };
                pts[j as usize]
            };
            let k = k as isize;
            let (a, b, c) = (at(k - 1), at(k), at(k + 1));
            let data = Point::new((a.x + b.x + c.x) / 3.0, (a.y + b.y + c.y) / 3.0);
            let nrm = Vec2 { x: -d.y, y: d.x };
            let off = (data - mid).dot(nrm).clamp(-JOIN_MAX, JOIN_MAX);
            (d, Point::new(mid.x + off * nrm.x, mid.y + off * nrm.y))
        })
        .collect();
    let mut out = Vec::with_capacity(2 * m);
    // The vertex at polygon index `i`, between side i-1 and side i.
    let vertex = |out: &mut Vec<Piece>, i: usize, smooth_in: bool| {
        let (da, a) = side[(i + sides - 1) % sides];
        let (dc, c) = side[i % sides];
        let b = v[i];
        let lo = mid_index(vtx[(i + m - 1) % m], vtx[i], n);
        let hi = mid_index(vtx[i], vtx[(i + 1) % m], n);
        let data = between(pts, lo, hi);
        let back = Vec2 { x: -dc.x, y: -dc.y };
        let smooth = if data.is_empty() {
            // Nothing measured between the joins: Potrace's own controls, two thirds of
            // the way to the vertex along each side.
            let (la, lc) = (a.dist(b) * 2.0 / 3.0, c.dist(b) * 2.0 / 3.0);
            Some((
                [
                    a,
                    Point::new(a.x + la * da.x, a.y + la * da.y),
                    Point::new(c.x - lc * dc.x, c.y - lc * dc.y),
                    c,
                ],
                0.0,
            ))
        } else {
            // Potrace's `alpha <= 1`: a control point never passes the vertex, so a
            // corner cannot be passed off as a curve that overshoots it.
            super::curve::fit(a, da, c, back, &data).map(|(mut p, e)| {
                let cap_a = (b - a).dot(da).max(0.0);
                let cap_c = (b - c).dot(back).max(0.0);
                let (la, lc) = ((p[1] - a).norm(), (p[2] - c).norm());
                if la <= cap_a && lc <= cap_c {
                    return (p, e);
                }
                let (la, lc) = (la.min(cap_a), lc.min(cap_c));
                p[1] = Point::new(a.x + la * da.x, a.y + la * da.y);
                p[2] = Point::new(c.x + lc * back.x, c.y + lc * back.y);
                (p, super::curve::max_error(&p, &data))
            })
        };
        let e_corner = corner_error(a, b, c, &data);
        match smooth {
            Some((p, e_smooth)) if !(e_smooth > corner_tol && e_smooth > 2.0 * e_corner) => {
                out.push(Piece { p, smooth_in });
            }
            _ => {
                out.push(Piece::line(a, b, smooth_in));
                out.push(Piece::line(b, c, false));
            }
        }
    };
    if closed {
        for i in 0..m {
            vertex(&mut out, i, true);
        }
    } else {
        out.push(Piece::line(v[0], side[0].1, false));
        for i in 1..m - 1 {
            vertex(&mut out, i, true);
        }
        out.push(Piece::line(side[sides - 1].1, v[m - 1], true));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn denoising_keeps_open_ends_and_flattens_a_zigzag() {
        let pts: Vec<Point> = (0..10)
            .map(|k| Point::new(k as f64, if k % 2 == 0 { 0.1 } else { -0.1 }))
            .collect();
        let d = denoise(&pts, false);
        assert_eq!(d[0], pts[0]);
        assert_eq!(d[9], pts[9]);
        assert!(d[1..9].iter().all(|p| p.y.abs() < 0.051));
    }

    #[test]
    fn a_right_angle_vertex_is_a_corner() {
        let mut pts: Vec<Point> = (0..=10).map(|k| Point::new(k as f64, 0.0)).collect();
        pts.extend((1..=10).map(|k| Point::new(10.0, k as f64)));
        let vtx = [0, 10, 20];
        let v = adjust_vertices(&pts, &vtx, false, 0.5);
        assert!(v[1].dist(Point::new(10.0, 0.0)) < 1e-6);
        let p = pieces(&pts, &vtx, &v, false, 0.4);
        // Line to the first midpoint, the corner's two lines, line from the last midpoint.
        assert_eq!(p.len(), 4);
        assert!(!p[2].smooth_in);
    }

    #[test]
    fn an_arc_vertex_is_smooth_and_fits_the_points() {
        let pts: Vec<Point> = (0..=40)
            .map(|k| {
                let t = k as f64 / 40.0 * std::f64::consts::FRAC_PI_2;
                Point::new(20.0 * t.cos(), 20.0 * t.sin())
            })
            .collect();
        let vtx = super::super::polygon::open(&pts, 0.5);
        let v = adjust_vertices(&pts, &vtx, false, 0.5);
        let p = pieces(&pts, &vtx, &v, false, 0.4);
        assert_eq!(p.len(), vtx.len());
        // Every curve piece is smooth, and its midpoint lies on the circle.
        for pc in &p[1..p.len() - 1] {
            assert!(pc.smooth_in);
            let q = pc.p;
            let w = [0.125, 0.375, 0.375, 0.125];
            let x = w[0] * q[0].x + w[1] * q[1].x + w[2] * q[2].x + w[3] * q[3].x;
            let y = w[0] * q[0].y + w[1] * q[1].y + w[2] * q[2].y + w[3] * q[3].y;
            assert!((x.hypot(y) - 20.0).abs() < 0.25, "{}", x.hypot(y));
        }
    }

    #[test]
    fn a_vertex_moves_to_where_its_sides_meet_within_the_box() {
        // Two sides whose lines meet at (10, 0), with the polygon vertex a little off.
        let mut pts: Vec<Point> = (0..=10).map(|k| Point::new(k as f64, 0.0)).collect();
        pts[10] = Point::new(9.8, 0.2);
        pts.extend((1..=10).map(|k| Point::new(10.0, k as f64)));
        let v = adjust_vertices(&pts, &[0, 10, 20], false, 0.5);
        assert!(v[1].dist(Point::new(10.0, 0.0)) < 0.15, "{:?}", v[1]);
    }
}
