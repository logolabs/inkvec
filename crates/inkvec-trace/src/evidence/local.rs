//! The per-pixel term of a junction: its pixels' colours against the mixture the candidate
//! curves draw there (`docs/theory/chain-boundary.md`, B1.3, B3.2). A junction pixel holds
//! three or more inks, so it is scored in colour, not unmixed: each face's area in the pixel
//! comes from the sector its two arms cut out of a disc around the vertex.

use inkvec_core::likelihood::{Chi2, Piece, RenderModel};
use inkvec_core::Point;

/// One pixel of a local term.
#[derive(Debug, Clone, Copy)]
pub(super) struct LocalPixel {
    pub(super) x: i32,
    pub(super) y: i32,
    /// Its colour, sRGB.
    pub(super) rgb: [f64; 3],
    /// Variance per channel.
    pub(super) var: f64,
}

/// A junction's local term: its arms (edge, whether the edge starts here), and its pixels.
#[derive(Debug, Clone)]
pub(super) struct Local {
    pub(super) node: u32,
    pub(super) at: Point,
    pub(super) arms: Vec<(usize, bool)>,
    pub(super) pixels: Vec<LocalPixel>,
}

/// Points along a curve, flattened finely (the local term is a few pixels; `1e-3` px).
fn flatten(rm: &RenderModel, pieces: &[Piece]) -> Vec<Point> {
    // Arcs the model draws exactly are followed through fine cubics.
    let drawn: Vec<Piece> = rm
        .as_rendered(pieces)
        .into_iter()
        .flat_map(|p| match p {
            Piece::Arc(a) => a.to_cubics(1e-4),
            other => vec![other],
        })
        .collect();
    let mut out: Vec<Point> = Vec::new();
    for p in &drawn {
        let (pts, n): (Vec<Point>, usize) = match p {
            Piece::Line([a, b]) => (vec![*a, *b], 1),
            Piece::Quad(q) => (q.to_vec(), 24),
            Piece::Cubic(q) => (q.to_vec(), 32),
            Piece::Arc(_) => continue,
        };
        if out.is_empty() {
            out.push(pts[0]);
        }
        for k in 1..=n {
            let t = k as f64 / n as f64;
            let q = match pts.len() {
                2 => Point::new(
                    pts[0].x + t * (pts[1].x - pts[0].x),
                    pts[0].y + t * (pts[1].y - pts[0].y),
                ),
                3 => {
                    let m = 1.0 - t;
                    Point::new(
                        m * m * pts[0].x + 2.0 * m * t * pts[1].x + t * t * pts[2].x,
                        m * m * pts[0].y + 2.0 * m * t * pts[1].y + t * t * pts[2].y,
                    )
                }
                _ => {
                    let m = 1.0 - t;
                    let (a, b, c, d) = (m * m * m, 3.0 * m * m * t, 3.0 * m * t * t, t * t * t);
                    Point::new(
                        a * pts[0].x + b * pts[1].x + c * pts[2].x + d * pts[3].x,
                        a * pts[0].y + b * pts[1].y + c * pts[2].y + d * pts[3].y,
                    )
                }
            };
            out.push(q);
        }
    }
    out
}

/// Area of a (possibly non-convex) polygon inside the pixel square centred at `(x, y)`:
/// Sutherland–Hodgman against the square's four sides, then the shoelace formula.
pub(super) fn area_in_pixel(poly: &[Point], x: i32, y: i32) -> f64 {
    let (x0, x1, y0, y1) = (
        x as f64 - 0.5,
        x as f64 + 0.5,
        y as f64 - 0.5,
        y as f64 + 0.5,
    );
    let mut cur: Vec<Point> = poly.to_vec();
    let sides: [&dyn Fn(Point) -> f64; 4] = [
        &|p: Point| p.x - x0,
        &|p: Point| x1 - p.x,
        &|p: Point| p.y - y0,
        &|p: Point| y1 - p.y,
    ];
    for f in sides {
        if cur.is_empty() {
            return 0.0;
        }
        let mut next = Vec::with_capacity(cur.len() + 4);
        for i in 0..cur.len() {
            let (a, b) = (cur[i], cur[(i + 1) % cur.len()]);
            let (fa, fb) = (f(a), f(b));
            if fa >= 0.0 {
                next.push(a);
            }
            if (fa >= 0.0) != (fb >= 0.0) {
                let t = fa / (fa - fb);
                next.push(Point::new(a.x + t * (b.x - a.x), a.y + t * (b.y - a.y)));
            }
        }
        cur = next;
    }
    let n = cur.len();
    let mut s = 0.0;
    for i in 0..n {
        let (a, b) = (cur[i], cur[(i + 1) % n]);
        s += a.x * b.y - b.x * a.y;
    }
    (0.5 * s).abs()
}

/// One pixel of a corner term: its unmixed weight of the edge's left face.
#[derive(Debug, Clone, Copy)]
pub(super) struct CornerPixel {
    pub(super) x: i32,
    pub(super) y: i32,
    pub(super) a: f64,
    pub(super) var: f64,
}

/// The part of segment `a → b` inside the box `[x0, x1] × [y0, y1]` (Liang–Barsky), as
/// parameters `(t0, t1)`, or `None`.
fn clip_segment(a: Point, b: Point, x0: f64, x1: f64, y0: f64, y1: f64) -> Option<(f64, f64)> {
    let (dx, dy) = (b.x - a.x, b.y - a.y);
    let (mut t0, mut t1) = (0.0f64, 1.0f64);
    for (p, q) in [
        (-dx, a.x - x0),
        (dx, x1 - a.x),
        (-dy, a.y - y0),
        (dy, y1 - a.y),
    ] {
        if p == 0.0 {
            if q < 0.0 {
                return None;
            }
            continue;
        }
        let r = q / p;
        if p < 0.0 {
            t0 = t0.max(r);
        } else {
            t1 = t1.min(r);
        }
        if t0 > t1 {
            return None;
        }
    }
    Some((t0, t1))
}

/// Position of a point on the box's boundary as a perimeter coordinate, walking
/// `(x0, y0) → (x1, y0) → (x1, y1) → (x0, y1)`.
fn perimeter(p: Point, x0: f64, x1: f64, y0: f64, y1: f64) -> f64 {
    let (w, h) = (x1 - x0, y1 - y0);
    let e = 1e-9 * (w + h);
    if (p.y - y0).abs() <= e {
        p.x - x0
    } else if (p.x - x1).abs() <= e {
        w + (p.y - y0)
    } else if (p.y - y1).abs() <= e {
        w + h + (x1 - p.x)
    } else {
        2.0 * w + h + (y1 - p.y)
    }
}

fn point_in(poly: &[Point], q: Point) -> bool {
    let mut inside = false;
    let n = poly.len();
    for i in 0..n {
        let (a, b) = (poly[i], poly[(i + 1) % n]);
        if (a.y > q.y) != (b.y > q.y) && q.x < a.x + (q.y - a.y) / (b.y - a.y) * (b.x - a.x) {
            inside = !inside;
        }
    }
    inside
}

/// The area the polyline `curve` gives its left face in pixel `(x, y)`: the pass of the
/// curve through a box of half-size `reach` around the pixel that comes nearest its centre,
/// closed along the box's boundary on the curve's left, clipped to the pixel. Exact for a
/// polyline that passes through the box once; `None` when the curve ends inside the box.
pub(super) fn left_area_pixel(curve: &[Point], x: i32, y: i32, reach: f64) -> Option<f64> {
    let (x0, x1, y0, y1) = (
        x as f64 - reach,
        x as f64 + reach,
        y as f64 - reach,
        y as f64 + reach,
    );
    let c = Point::new(x as f64, y as f64);
    // Passes: maximal runs of consecutive clipped segments.
    let mut passes: Vec<Vec<Point>> = Vec::new();
    let mut open = false;
    for s in curve.windows(2) {
        let Some((t0, t1)) = clip_segment(s[0], s[1], x0, x1, y0, y1) else {
            open = false;
            continue;
        };
        let at = |t: f64| {
            Point::new(
                s[0].x + t * (s[1].x - s[0].x),
                s[0].y + t * (s[1].y - s[0].y),
            )
        };
        if open && t0 == 0.0 {
            passes.last_mut().expect("open pass").push(at(t1));
        } else {
            passes.push(vec![at(t0), at(t1)]);
        }
        open = t1 == 1.0;
    }
    let dist = |p: &Vec<Point>| p.iter().map(|q| q.dist(c)).fold(f64::INFINITY, f64::min);
    let pass = passes
        .into_iter()
        .min_by(|a, b| dist(a).total_cmp(&dist(b)))?;
    let (s, e) = (pass[0], *pass.last().expect("two points"));
    let on_box = |p: Point| {
        let tol = 1e-9 * reach;
        (p.x - x0).abs() <= tol
            || (p.x - x1).abs() <= tol
            || (p.y - y0).abs() <= tol
            || (p.y - y1).abs() <= tol
    };
    if !on_box(s) || !on_box(e) {
        return None;
    }
    // The two ways round the box from the exit back to the entry.
    let per = 4.0 * 2.0 * reach;
    let (pe, ps) = (perimeter(e, x0, x1, y0, y1), perimeter(s, x0, x1, y0, y1));
    let corners = [
        (0.0, Point::new(x0, y0)),
        (2.0 * reach, Point::new(x1, y0)),
        (4.0 * reach, Point::new(x1, y1)),
        (6.0 * reach, Point::new(x0, y1)),
    ];
    let walk = |forward: bool| -> Vec<Point> {
        let mut out = Vec::new();
        let span = if forward {
            (ps - pe).rem_euclid(per)
        } else {
            (pe - ps).rem_euclid(per)
        };
        let mut cs: Vec<(f64, Point)> = corners
            .iter()
            .map(|&(p, q)| {
                (
                    if forward {
                        (p - pe).rem_euclid(per)
                    } else {
                        (pe - p).rem_euclid(per)
                    },
                    q,
                )
            })
            .filter(|(d, _)| *d > 0.0 && *d < span)
            .collect();
        cs.sort_by(|a, b| a.0.total_cmp(&b.0));
        out.extend(cs.into_iter().map(|(_, q)| q));
        out
    };
    // A point just left of the pass's middle: left of direction d is (d.y, -d.x).
    let k = pass.len() / 2;
    let (a, b) = (pass[k.saturating_sub(1)], pass[k.min(pass.len() - 1)]);
    let (dx, dy) = (b.x - a.x, b.y - a.y);
    let l = dx.hypot(dy).max(1e-12);
    let probe = Point::new(
        0.5 * (a.x + b.x) + 1e-6 * dy / l,
        0.5 * (a.y + b.y) - 1e-6 * dx / l,
    );
    for forward in [true, false] {
        let mut poly = pass.clone();
        poly.extend(walk(forward));
        if point_in(&poly, probe) {
            return Some(area_in_pixel(&poly, x, y));
        }
    }
    None
}

/// `χ²` of a corner term's pixels against a candidate curve (pieces, oriented as the edge).
pub(super) fn chi2_corner(pixels: &[CornerPixel], rm: &RenderModel, curve: &[Piece]) -> Chi2 {
    let pts = flatten(rm, curve);
    let mut out = Chi2::default();
    for p in pixels {
        let Some(a) = left_area_pixel(&pts, p.x, p.y, 4.0) else {
            continue;
        };
        let r = p.a - a;
        out.chi2 += r * r / p.var.max(1e-300);
        out.m += 1;
    }
    out.chi2_floor = out.chi2;
    out.dof = out.m as f64;
    out.cost = out.chi2;
    out
}

/// `χ²` of a junction's pixels against candidate curves for its arms. `curves[k]` is the
/// candidate for arm `k` (in the term's arm order), oriented as its edge; `faces(e, left)` is
/// the colour of edge `e`'s left (or right) face at a pixel. The arms are flattened, followed
/// outwards from the junction, extended radially to a disc well beyond the term's pixels, and
/// sorted by direction; the face between two consecutive arms is the right face of the first
/// walked outwards, and its sector (vertex, first arm, the disc's rim, second arm) clipped to
/// each pixel is its area there.
pub(super) fn chi2(
    term: &Local,
    rm: &RenderModel,
    curves: &[&[Piece]],
    colour: &dyn Fn(usize, bool, i32, i32) -> [f64; 3],
) -> Option<Chi2> {
    if curves.len() != term.arms.len() || curves.len() < 2 {
        return None;
    }
    let rim = term
        .pixels
        .iter()
        .map(|p| (p.x as f64 - term.at.x).hypot(p.y as f64 - term.at.y))
        .fold(0.0f64, f64::max)
        + 3.0;
    // Outward polylines.
    let mut arms: Vec<(Vec<Point>, usize, bool)> = Vec::new();
    for (k, &(e, at_start)) in term.arms.iter().enumerate() {
        let mut pts = flatten(rm, curves[k]);
        if pts.len() < 2 {
            return None;
        }
        if !at_start {
            pts.reverse();
        }
        arms.push((pts, e, at_start));
    }
    let vertex = {
        let n = arms.len() as f64;
        arms.iter().fold(Point::new(0.0, 0.0), |a, (p, _, _)| {
            Point::new(a.x + p[0].x / n, a.y + p[0].y / n)
        })
    };
    let mut rays: Vec<(f64, Vec<Point>, usize, bool)> = Vec::new();
    for (pts, e, at_start) in arms {
        let mut q: Vec<Point> = vec![vertex];
        for p in pts.iter().skip(1) {
            if p.dist(vertex) >= rim {
                break;
            }
            q.push(*p);
        }
        let last = *q.last().expect("vertex");
        let prev = if q.len() >= 2 { q[q.len() - 2] } else { vertex };
        let (mut dx, mut dy) = (last.x - prev.x, last.y - prev.y);
        if dx.hypot(dy) < 1e-9 {
            let p1 = pts[1];
            dx = p1.x - vertex.x;
            dy = p1.y - vertex.y;
        }
        let l = dx.hypot(dy).max(1e-12);
        q.push(Point::new(
            last.x + dx / l * 2.0 * rim,
            last.y + dy / l * 2.0 * rim,
        ));
        let tip = *q.last().expect("tip");
        rays.push(((tip.y - vertex.y).atan2(tip.x - vertex.x), q, e, at_start));
    }
    rays.sort_by(|a, b| a.0.total_cmp(&b.0));
    let n = rays.len();
    let mut model: Vec<[f64; 3]> = vec![[0.0; 3]; term.pixels.len()];
    for k in 0..n {
        let (a0, ref r0, e0, s0) = rays[k];
        let (mut a1, ref r1, _, _) = rays[(k + 1) % n];
        if a1 <= a0 {
            a1 += 2.0 * std::f64::consts::PI;
        }
        let mut poly: Vec<Point> = r0.clone();
        let steps = (((a1 - a0) / 0.05).ceil() as usize).max(1);
        for i in 1..steps {
            let a = a0 + (a1 - a0) * i as f64 / steps as f64;
            poly.push(Point::new(
                vertex.x + 3.0 * rim * a.cos(),
                vertex.y + 3.0 * rim * a.sin(),
            ));
        }
        poly.extend(r1.iter().rev());
        // Walking out along arm k (direction θ), its left face lies at θ − 90° and its right
        // face at θ + 90°, inside this sector. Walked outwards an edge that starts here keeps
        // its faces; one that ends here has them swapped: the sector's face is the edge's
        // right face, or its left face when the edge ends at the junction.
        let edge_left = !s0;
        for (pi, px) in term.pixels.iter().enumerate() {
            let a = area_in_pixel(&poly, px.x, px.y);
            if a <= 0.0 {
                continue;
            }
            let c = colour(e0, edge_left, px.x, px.y);
            for ch in 0..3 {
                model[pi][ch] += a * c[ch];
            }
        }
    }
    Some(colour_score(
        term.pixels
            .iter()
            .zip(&model)
            .map(|(px, m)| (px.rgb, *m, px.var)),
    ))
}

/// A pixel two edges both reach away from a junction (a thin feature), in colour.
#[derive(Debug, Clone, Copy)]
pub(super) struct StripPixel {
    pub(super) x: i32,
    pub(super) y: i32,
    pub(super) rgb: [f64; 3],
    /// Per-channel variance.
    pub(super) var: f64,
    /// The two edges, the lower index first.
    pub(super) pair: (u32, u32),
}

/// Sum per-pixel colour residuals into a score.
fn colour_score(pixels: impl Iterator<Item = ([f64; 3], [f64; 3], f64)>) -> Chi2 {
    let mut out = Chi2::default();
    for (obs, model, var) in pixels {
        for ch in 0..3 {
            let r = obs[ch] - model[ch];
            out.chi2 += r * r / var.max(1e-300);
        }
        out.m += 3;
    }
    out.chi2_floor = out.chi2;
    out.dof = out.m as f64;
    out.cost = out.chi2;
    out
}

/// A2: a stroke band against the strip pixels of its two edges: each pixel is the left
/// side's ink where it is left of the band's left side, the band's within the band, and the
/// right side's elsewhere.
pub(super) fn chi2_band(
    pixels: &[StripPixel],
    pair: (u32, u32),
    band: &inkvec_core::likelihood::StrokeBand,
    inks: [[f32; 3]; 3],
) -> Chi2 {
    let (left, right) = band.sides(0.05);
    if left.len() < 2 {
        return Chi2::default();
    }
    let mut poly = left.clone();
    poly.extend(right.iter().rev());
    let ink = |k: usize| [inks[k][0] as f64, inks[k][1] as f64, inks[k][2] as f64];
    let (il, ib, ir) = (ink(0), ink(1), ink(2));
    colour_score(pixels.iter().filter(|p| p.pair == pair).filter_map(|p| {
        let b = area_in_pixel(&poly, p.x, p.y).clamp(0.0, 1.0);
        let l = left_area_pixel(&left, p.x, p.y, 4.0)?.clamp(0.0, 1.0 - b);
        let r = (1.0 - b - l).max(0.0);
        let model: [f64; 3] = std::array::from_fn(|c| l * il[c] + b * ib[c] + r * ir[c]);
        Some((p.rgb, model, p.var))
    }))
}

/// A2: a junction's pixels under per-layer compositing: each layer's whole shape covers its
/// area of a pixel and is painted over what lies beneath, in order, over `ground`.
pub(super) fn chi2_layers(
    term: &Local,
    rm: &RenderModel,
    layers: &[(&[Piece], [f32; 3])],
    ground: [f32; 3],
) -> Chi2 {
    let shapes: Vec<(Vec<Point>, [f64; 3])> = layers
        .iter()
        .map(|(b, c)| (flatten(rm, b), [c[0] as f64, c[1] as f64, c[2] as f64]))
        .collect();
    let g = [ground[0] as f64, ground[1] as f64, ground[2] as f64];
    colour_score(term.pixels.iter().map(|p| {
        let mut c = g;
        for (poly, col) in &shapes {
            let a = if poly.len() >= 3 {
                area_in_pixel(poly, p.x, p.y).clamp(0.0, 1.0)
            } else {
                0.0
            };
            for ch in 0..3 {
                c[ch] = a * col[ch] + (1.0 - a) * c[ch];
            }
        }
        (p.rgb, c, p.var)
    }))
}
