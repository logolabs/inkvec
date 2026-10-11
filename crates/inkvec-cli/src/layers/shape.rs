//! Geometry to regions: fitted paths, primitives and stroke bands, flattened finely enough
//! that the region of [`super::region`] is the shape to within [`FLAT_TOL`].

use inkvec_core::Point;
use inkvec_fit::curves::Segment;
use inkvec_fit::primitives::PrimitiveKind;
use inkvec_fit::FittedPath;

use super::region::{line_y, Region, DY};

/// Largest distance, px, between a curve and the polyline that stands for it.
pub(crate) const FLAT_TOL: f64 = 0.02;

/// How many pieces a curve of the given control-polygon length and turn needs so that the
/// chord error stays under [`FLAT_TOL`]: for an arc of radius `r` cut into `n` pieces the
/// sagitta is `r(1 − cos(θ/2n)) ≈ rθ²/8n²`, and a control polygon bounds `rθ` from above.
fn pieces(len: f64, turn: f64) -> usize {
    let s = (len.max(0.0) * turn.abs().max(0.05) / (8.0 * FLAT_TOL)).sqrt();
    (s.ceil() as usize).clamp(2, 512)
}

/// Appends the points of `seg` after `from` (excluding `from` itself).
pub(crate) fn flatten_segment(from: Point, seg: &Segment, out: &mut Vec<Point>) {
    match *seg {
        Segment::Line(p) => out.push(p),
        Segment::Cubic(c1, c2, p) => {
            let len = from.dist(c1) + c1.dist(c2) + c2.dist(p);
            let turn = angle_between(sub(c1, from), sub(c2, c1)).abs()
                + angle_between(sub(c2, c1), sub(p, c2)).abs()
                + 0.5;
            let n = pieces(len, turn);
            for i in 1..=n {
                let t = i as f64 / n as f64;
                out.push(inkvec_fit::curves::eval_cubic([from, c1, c2, p], t));
            }
        }
        Segment::Arc {
            rx,
            ry,
            phi,
            large_arc,
            sweep,
            end,
        } => {
            let f = inkvec_fit::curves::arc_ellipse_center(from, rx, ry, phi, large_arc, sweep, end);
            let n = pieces(rx.max(ry) * f.delta.abs(), f.delta);
            for i in 1..=n {
                out.push(f.at(f.theta1 + f.delta * i as f64 / n as f64));
            }
            if let Some(last) = out.last_mut() {
                *last = end;
            }
        }
    }
}

/// A path's points: its start and every segment flattened.
pub(crate) fn flatten_path(path: &FittedPath) -> Vec<Point> {
    let mut out = vec![path.start];
    let mut at = path.start;
    for s in &path.segments {
        flatten_segment(at, s, &mut out);
        at = s.end();
    }
    out
}

/// A primitive's outline as a closed polygon.
pub(crate) fn flatten_primitive(kind: &PrimitiveKind) -> Vec<Point> {
    match *kind {
        PrimitiveKind::Circle { c, r } => ellipse_points(c, r, r, 0.0),
        PrimitiveKind::Ellipse { c, rx, ry, angle } => ellipse_points(c, rx, ry, angle),
        PrimitiveKind::RoundRect { x, y, w, h, rx } => {
            let rx = rx.clamp(0.0, 0.5 * w.min(h));
            if rx <= 1e-9 {
                return vec![
                    Point::new(x, y),
                    Point::new(x + w, y),
                    Point::new(x + w, y + h),
                    Point::new(x, y + h),
                ];
            }
            let n = pieces(rx * std::f64::consts::FRAC_PI_2, std::f64::consts::FRAC_PI_2);
            let mut out = Vec::new();
            let corners = [
                (x + w - rx, y + rx, -std::f64::consts::FRAC_PI_2),
                (x + w - rx, y + h - rx, 0.0),
                (x + rx, y + h - rx, std::f64::consts::FRAC_PI_2),
                (x + rx, y + rx, std::f64::consts::PI),
            ];
            for (cx, cy, a0) in corners {
                for i in 0..=n {
                    let a = a0 + std::f64::consts::FRAC_PI_2 * i as f64 / n as f64;
                    out.push(Point::new(cx + rx * a.cos(), cy + rx * a.sin()));
                }
            }
            out
        }
    }
}

fn ellipse_points(c: Point, rx: f64, ry: f64, angle: f64) -> Vec<Point> {
    let tau = std::f64::consts::TAU;
    let n = pieces(rx.max(ry) * tau, tau).max(16);
    let (s, co) = angle.sin_cos();
    (0..n)
        .map(|i| {
            let t = tau * i as f64 / n as f64;
            let (ex, ey) = (rx * t.cos(), ry * t.sin());
            Point::new(c.x + ex * co - ey * s, c.y + ex * s + ey * co)
        })
        .collect()
}

/// How a stroke's open ends are drawn.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum EndCap {
    Butt,
    Round,
    Square,
}

/// How a stroke's pieces meet. Only `Round` is drawn as such; a miter or bevel join is
/// taken as no join at all, which can only make the band smaller than the renderer's.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum JoinKind {
    Round,
    Other,
}

/// The band a stroke of width `width` paints along the centrelines `lines`. Smaller than or
/// equal to what a renderer paints wherever the two differ (miter and bevel joins are left
/// out), which is the safe side for a region used as cover: a face may reach under it only
/// where it certainly is.
pub(crate) fn stroke_band(lines: &[FittedPath], width: f64, cap: EndCap, join: JoinKind) -> Region {
    let r = 0.5 * width;
    if r <= 0.0 {
        return Region::empty();
    }
    let mut spans: Vec<(i32, f64, f64)> = Vec::new();
    for line in lines {
        let mut pts = flatten_path(line);
        if line.closed && pts.len() > 1 && pts.first() != pts.last() {
            pts.push(pts[0]);
        }
        let n = pts.len();
        if n == 0 {
            continue;
        }
        if n == 1 {
            if cap == EndCap::Round {
                disc_spans(pts[0], r, &mut spans);
            }
            continue;
        }
        for i in 0..n - 1 {
            let (mut a, mut b) = (pts[i], pts[i + 1]);
            if !line.closed && cap == EndCap::Square {
                let d = sub(b, a);
                let l = d.x.hypot(d.y);
                if l > 0.0 {
                    let u = Point::new(d.x / l, d.y / l);
                    if i == 0 {
                        a = Point::new(a.x - u.x * r, a.y - u.y * r);
                    }
                    if i == n - 2 {
                        b = Point::new(b.x + u.x * r, b.y + u.y * r);
                    }
                }
            }
            rect_spans(a, b, r, &mut spans);
        }
        // Discs: at interior vertices (a flattened curve is a run of round joins, which is
        // what a curve's band is), at written joins when they are round, at open ends with
        // round caps. A written join and a flattening vertex cannot be told apart here, so
        // a non-round join is only honoured at the path's own segment ends.
        let mut ends: Vec<Point> = Vec::new();
        let mut at = line.start;
        for s in &line.segments {
            at = s.end();
            ends.push(at);
        }
        let _ = at;
        for (i, &p) in pts.iter().enumerate() {
            let open_end = !line.closed && (i == 0 || i == n - 1);
            let written_join = ends.iter().any(|e| e.dist(p) < 1e-9) && !open_end;
            let disc = if open_end {
                cap == EndCap::Round
            } else if written_join {
                join == JoinKind::Round
            } else {
                true
            };
            if disc {
                disc_spans(p, r, &mut spans);
            }
        }
    }
    Region::from_line_spans(spans)
}

/// Spans of the disc of radius `r` about `c`.
fn disc_spans(c: Point, r: f64, out: &mut Vec<(i32, f64, f64)>) {
    let ka = ((c.y - r + 0.5) / DY - 0.5).ceil() as i32;
    let kb = ((c.y + r + 0.5) / DY - 0.5).floor() as i32;
    for k in ka..=kb {
        let dy = line_y(k) - c.y;
        let h2 = r * r - dy * dy;
        if h2 > 0.0 {
            let h = h2.sqrt();
            out.push((k, c.x - h, c.x + h));
        }
    }
}

/// Spans of the rectangle `{a + t(b − a) + s·n : t ∈ [0, 1], |s| ≤ r}`.
fn rect_spans(a: Point, b: Point, r: f64, out: &mut Vec<(i32, f64, f64)>) {
    let d = sub(b, a);
    let l = d.x.hypot(d.y);
    if l <= 1e-12 {
        return;
    }
    let (ux, uy) = (d.x / l, d.y / l);
    let (nx, ny) = (-uy, ux);
    let corners = [
        Point::new(a.x + nx * r, a.y + ny * r),
        Point::new(b.x + nx * r, b.y + ny * r),
        Point::new(b.x - nx * r, b.y - ny * r),
        Point::new(a.x - nx * r, a.y - ny * r),
    ];
    let ymin = corners.iter().map(|p| p.y).fold(f64::INFINITY, f64::min);
    let ymax = corners.iter().map(|p| p.y).fold(f64::NEG_INFINITY, f64::max);
    let ka = ((ymin + 0.5) / DY - 0.5).ceil() as i32;
    let kb = ((ymax + 0.5) / DY - 0.5).floor() as i32;
    for k in ka..=kb {
        let y = line_y(k);
        let mut lo = f64::INFINITY;
        let mut hi = f64::NEG_INFINITY;
        for i in 0..4 {
            let (p, q) = (corners[i], corners[(i + 1) % 4]);
            if (p.y <= y && y <= q.y) || (q.y <= y && y <= p.y) {
                let x = if (q.y - p.y).abs() < 1e-15 {
                    lo = lo.min(p.x.min(q.x));
                    hi = hi.max(p.x.max(q.x));
                    continue;
                } else {
                    p.x + (y - p.y) * (q.x - p.x) / (q.y - p.y)
                };
                lo = lo.min(x);
                hi = hi.max(x);
            }
        }
        if hi > lo {
            out.push((k, lo, hi));
        }
    }
}

fn sub(a: Point, b: Point) -> Point {
    Point::new(a.x - b.x, a.y - b.y)
}

fn angle_between(a: Point, b: Point) -> f64 {
    (a.x * b.y - a.y * b.x).atan2(a.x * b.x + a.y * b.y)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layers::region::Rule;

    #[test]
    fn a_circle_flattens_to_its_area() {
        let poly = flatten_primitive(&PrimitiveKind::Circle {
            c: Point::new(20.0, 20.0),
            r: 10.0,
        });
        let r = Region::from_polygons(&[poly], Rule::EvenOdd);
        // An inscribed polygon within FLAT_TOL of the curve loses less than the perimeter
        // times that tolerance.
        let want = std::f64::consts::PI * 100.0;
        let slack = std::f64::consts::TAU * 10.0 * FLAT_TOL;
        assert!((r.area() - want).abs() < slack, "{} vs {want}", r.area());
    }

    #[test]
    fn a_round_capped_stroke_is_a_capsule() {
        let line = FittedPath {
            start: Point::new(0.0, 10.0),
            segments: vec![Segment::Line(Point::new(20.0, 10.0))],
            closed: false,
        };
        let band = stroke_band(&[line.clone()], 4.0, EndCap::Round, JoinKind::Round);
        let want = 20.0 * 4.0 + std::f64::consts::PI * 4.0;
        assert!((band.area() - want).abs() < 0.6, "{} vs {want}", band.area());
        let butt = stroke_band(&[line], 4.0, EndCap::Butt, JoinKind::Round);
        assert!((butt.area() - 80.0).abs() < 0.6, "{}", butt.area());
    }
}
