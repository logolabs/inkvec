//! The partition of an edge's partial pixels into column and row windows, and the
//! measurement each window makes (`docs/theory/chain-boundary.md`, B2.2).
//!
//! Every pixel within reach of the starting map is claimed by its nearest edge. A pixel
//! claimed within reach by two edges, or within the local radius of a junction node, belongs
//! to that junction's local term instead of a run. Of an edge's own pixels, those whose
//! unmixed weight is strictly between its two faces' (the partial pixels) are classed by the
//! edge's direction at their nearest point: column where it runs within 45° of horizontal,
//! row elsewhere. A window is a maximal run of one class's partial pixels along one column
//! (row), extended by the pure pixel at each end when that pixel is free. So every partial
//! pixel is in exactly one window, whatever the direction, including where the edge turns
//! through 45° (where a column's run ends a row's begins, and neither takes the other's
//! pixels); `windows_partition_every_partial_pixel` in the tests checks it.

use inkvec_core::likelihood::{Axis, RunObs, Window};
use inkvec_core::Point;

use crate::planar::PlanarMap;

/// What the nearest (and second nearest) edges say about one pixel.
#[derive(Debug, Clone, Copy)]
pub(super) struct Claim {
    /// Nearest edge, or `u32::MAX`.
    pub(super) edge: u32,
    /// Its distance from the pixel centre, px.
    pub(super) dist: f64,
    /// Arclength of the nearest point along the edge, px.
    pub(super) s: f64,
    /// Unit tangent of the edge there.
    pub(super) t: (f64, f64),
    /// Second nearest edge (another edge), or `u32::MAX`.
    pub(super) edge2: u32,
    /// Its distance, px.
    pub(super) dist2: f64,
}

impl Default for Claim {
    fn default() -> Self {
        Claim {
            edge: u32::MAX,
            dist: f64::INFINITY,
            s: 0.0,
            t: (1.0, 0.0),
            edge2: u32::MAX,
            dist2: f64::INFINITY,
        }
    }
}

/// Per pixel (row-major), the nearest and second nearest edges within `reach` px of its
/// centre, by distance to each edge's polyline. `O(points × reach²)`.
pub(super) fn claims(map: &PlanarMap, reach: f64) -> Vec<Claim> {
    let (w, h) = (map.width as i64, map.height as i64);
    let mut out = vec![Claim::default(); (w * h) as usize];
    for (k, e) in map.edges.iter().enumerate() {
        let k = k as u32;
        let pts = &e.points;
        let n = pts.len();
        let segs = if e.closed { n } else { n.saturating_sub(1) };
        let mut s_acc = 0.0;
        for i in 0..segs {
            let (a, b) = (pts[i], pts[(i + 1) % n]);
            let (dx, dy) = (b.x - a.x, b.y - a.y);
            let len = dx.hypot(dy);
            let t = if len > 1e-12 {
                (dx / len, dy / len)
            } else {
                (1.0, 0.0)
            };
            let x0 = (a.x.min(b.x) - reach).floor().max(0.0) as i64;
            let x1 = (a.x.max(b.x) + reach).ceil().min((w - 1) as f64) as i64;
            let y0 = (a.y.min(b.y) - reach).floor().max(0.0) as i64;
            let y1 = (a.y.max(b.y) + reach).ceil().min((h - 1) as f64) as i64;
            for y in y0..=y1 {
                for x in x0..=x1 {
                    let (px, py) = (x as f64, y as f64);
                    let u = if len > 1e-12 {
                        (((px - a.x) * dx + (py - a.y) * dy) / (len * len)).clamp(0.0, 1.0)
                    } else {
                        0.0
                    };
                    let (qx, qy) = (a.x + u * dx, a.y + u * dy);
                    let d = (px - qx).hypot(py - qy);
                    if d > reach {
                        continue;
                    }
                    let c = &mut out[(y * w + x) as usize];
                    if c.edge == k {
                        if d < c.dist {
                            c.dist = d;
                            c.s = s_acc + u * len;
                            c.t = t;
                        }
                    } else if d < c.dist {
                        if c.edge != u32::MAX {
                            c.edge2 = c.edge;
                            c.dist2 = c.dist;
                        }
                        c.edge = k;
                        c.dist = d;
                        c.s = s_acc + u * len;
                        c.t = t;
                    } else if c.edge2 == k {
                        c.dist2 = c.dist2.min(d);
                    } else if d < c.dist2 {
                        c.edge2 = k;
                        c.dist2 = d;
                    }
                }
            }
            s_acc += len;
        }
    }
    out
}

/// One pixel's unmixed measurement against an edge's two faces.
#[derive(Debug, Clone, Copy)]
pub(super) struct Unmixed {
    /// Weight of the left face, unclamped.
    pub(super) a: f64,
    /// Variance of `a` from quantisation (and measured noise above it).
    pub(super) var: f64,
    /// Whether the pixel is a mixture of the two faces (not pure, within half a level).
    pub(super) partial: bool,
    /// Whether the colour lies off the line through the two faces' colours (a third ink).
    pub(super) third: bool,
}

/// The two faces' colours an edge is unmixed against, and the per-pixel variance model.
#[derive(Debug, Clone, Copy)]
pub(super) struct Axis2 {
    /// Right face's colour (weight 0).
    pub(super) cr: [f64; 3],
    /// `left − right`.
    pub(super) d: [f64; 3],
    /// `|d|²`.
    pub(super) dd: f64,
    /// Variance of the unmixed weight of a partial pixel.
    pub(super) var_partial: f64,
    /// Variance of a pure pixel's (measured noise beyond quantisation, usually 0).
    pub(super) var_pure: f64,
    /// Half a quantisation level, in weight units: a pixel this close to 0 or 1 is pure.
    pub(super) pure_tol: f64,
    /// Squared distance off the colour line beyond which a pixel holds a third ink.
    pub(super) third_tol2: f64,
}

/// 8-bit quantisation step.
const Q: f64 = 1.0 / 255.0;

impl Axis2 {
    /// The axis from `right` (weight 0) to `left` (weight 1), with pixel noise `sigma_noise`
    /// (sRGB units). Quantisation's variance per channel is `Q²/12`; channels whose two
    /// colours agree to within a level round together (a grey axis rounds all three alike),
    /// so their errors are summed before squaring: `var = Σ_groups (Σ_{c∈g} d_c)² Q²/12 / |d|⁴`
    /// (`chain-boundary.md` B1.2). Noise measured above half a level adds `σ²/|d|²`.
    pub(super) fn new(left: [f32; 3], right: [f32; 3], sigma_noise: f64) -> Self {
        let cl = [left[0] as f64, left[1] as f64, left[2] as f64];
        let cr = [right[0] as f64, right[1] as f64, right[2] as f64];
        let d = [cl[0] - cr[0], cl[1] - cr[1], cl[2] - cr[2]];
        let dd = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).max(1e-300);
        // Group channels whose (right, left) colours agree to within a level.
        let mut grouped = [false; 3];
        let mut qsum = 0.0;
        for i in 0..3 {
            if grouped[i] {
                continue;
            }
            let mut g = d[i];
            for j in i + 1..3 {
                if !grouped[j] && (cr[i] - cr[j]).abs() < Q && (cl[i] - cl[j]).abs() < Q {
                    grouped[j] = true;
                    g += d[j];
                }
            }
            qsum += g * g;
        }
        let var_q = qsum * Q * Q / 12.0 / (dd * dd);
        let extra = (sigma_noise * sigma_noise - 0.25 * Q * Q).max(0.0) / dd;
        Axis2 {
            cr,
            d,
            dd,
            var_partial: var_q + extra,
            var_pure: extra,
            pure_tol: 0.5 * Q / dd.sqrt(),
            third_tol2: (4.0 * sigma_noise.max(Q)).powi(2),
        }
    }

    pub(super) fn unmix(&self, p: [f32; 3]) -> Unmixed {
        let q = [
            p[0] as f64 - self.cr[0],
            p[1] as f64 - self.cr[1],
            p[2] as f64 - self.cr[2],
        ];
        let a = (q[0] * self.d[0] + q[1] * self.d[1] + q[2] * self.d[2]) / self.dd;
        let r2: f64 = (0..3).map(|c| (q[c] - a * self.d[c]).powi(2)).sum();
        let partial = a > self.pure_tol && a < 1.0 - self.pure_tol;
        Unmixed {
            a,
            var: if partial {
                self.var_partial
            } else {
                self.var_pure
            },
            partial,
            third: r2 > self.third_tol2,
        }
    }
}

/// A pixel taking part in an edge's runs.
#[derive(Debug, Clone, Copy)]
pub(super) struct RunPixel {
    pub(super) x: i32,
    pub(super) y: i32,
    pub(super) m: Unmixed,
    pub(super) s: f64,
    pub(super) t: (f64, f64),
}

/// Group an edge's partial pixels into windows; `free(x, y)` says whether a pixel may be taken
/// as a pure flank (and marks it taken). Returns the run observations in order of `s`, and for
/// each the pixels it holds.
pub(super) fn group(
    px: &[RunPixel],
    flank: &mut dyn FnMut(i32, i32) -> Option<Unmixed>,
    window_var: f64,
) -> Vec<(RunObs, Vec<(i32, i32)>)> {
    let mut out: Vec<(RunObs, Vec<(i32, i32)>)> = Vec::new();
    for axis in [Axis::Column, Axis::Row] {
        let mut mine: Vec<&RunPixel> = px
            .iter()
            .filter(|p| p.m.partial && ((p.t.0.abs() >= p.t.1.abs()) == (axis == Axis::Column)))
            .collect();
        // Sort by line, then position along it.
        mine.sort_by_key(|p| match axis {
            Axis::Column => (p.x, p.y),
            Axis::Row => (p.y, p.x),
        });
        let line_pos = |p: &RunPixel| match axis {
            Axis::Column => (p.x, p.y),
            Axis::Row => (p.y, p.x),
        };
        let mut i = 0;
        while i < mine.len() {
            let (line, start) = line_pos(mine[i]);
            let mut j = i;
            while j + 1 < mine.len() && line_pos(mine[j + 1]) == (line, line_pos(mine[j]).1 + 1) {
                j += 1;
            }
            let run = &mine[i..=j];
            let (mut lo, mut hi) = (start, start + (j - i) as i32);
            let mut sum: f64 = run.iter().map(|p| p.m.a).sum();
            let mut var: f64 = run.iter().map(|p| p.m.var).sum();
            let mut pix: Vec<(i32, i32)> = run.iter().map(|p| (p.x, p.y)).collect();
            let at = |k: i32| match axis {
                Axis::Column => (line, k),
                Axis::Row => (k, line),
            };
            if let Some(m) = flank(at(lo - 1).0, at(lo - 1).1) {
                lo -= 1;
                sum += m.a;
                var += m.var;
                pix.insert(0, at(lo));
            }
            if let Some(m) = flank(at(hi + 1).0, at(hi + 1).1) {
                hi += 1;
                sum += m.a;
                var += m.var;
                pix.push(at(hi));
            }
            let n = run.len() as f64;
            let s = run.iter().map(|p| p.s).sum::<f64>() / n;
            let (tx, ty) = run
                .iter()
                .fold((0.0, 0.0), |(a, b), p| (a + p.t.0, b + p.t.1));
            // The left face lies on (t.y, -t.x): above a column when walking +x; west of a
            // row when walking -y.
            let left_low = match axis {
                Axis::Column => tx > 0.0,
                Axis::Row => ty < 0.0,
            };
            out.push((
                RunObs {
                    window: Window { axis, line, lo, hi },
                    s,
                    sum,
                    var: var + window_var,
                    left_low,
                },
                pix,
            ));
            i = j + 1;
        }
    }
    out.sort_by(|a, b| a.0.s.total_cmp(&b.0.s));
    out
}

/// The point where window `o` meets the starting geometry: its line and its measured mean
/// position.
pub(super) fn crossing(o: &RunObs) -> Point {
    let m = o.mean_position();
    match o.window.axis {
        Axis::Column => Point::new(o.window.line as f64, m),
        Axis::Row => Point::new(m, o.window.line as f64),
    }
}
