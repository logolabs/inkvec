//! One face's measured boundary, with inward normals, and the width read by pairing its
//! two sides.
//!
//! **Data.** The face's rings (outline and holes) come from the planar map after the
//! boundary solve: sub-pixel points, each with its positional sigma (px). They are stored
//! concatenated in [`Boundary::pts`]; `next[i]` / `prev[i]` walk point `i`'s own ring, and
//! segment `i` runs from `pts[i]` to `pts[next[i]]`. Every segment and every point carries
//! a unit normal pointing *into* the face.
//!
//! **Orientation.** The planar map walks every ring with its face on one side, but which
//! side depends on the crack tracer's conventions, so it is not assumed: each ring's
//! segments are probed 0.6 px along their left normal against the face's pixel mask, and
//! the majority decides whether the left normal points in or out. A stroke is at least
//! [`super::MIN_WIDTH`] px wide, so 0.6 px inside is inside.
//!
//! **Pairing** ([`Boundary::pair`]). A stroke drawn as centreline `C` and width `w` has
//! two sides that are offset curves of `C` at `±w/2`. The normal line of an offset curve
//! at a point is the normal line of `C` there, and it meets the other side at the
//! corresponding offset point, perpendicularly. So walking from a boundary point along
//! its inward normal reaches the other side after exactly `w`, at a segment whose inward
//! normal is anti-parallel: that pair is a *sleeve* sample, and `t` is a width sample.
//! A cap, a junction or a blob gives no anti-parallel partner, or one at the wrong
//! distance. Measured on our own lucide outlines in research (r2-compact, 15 icons), the
//! median of the paired widths was within 0.013 px of the artist's width, and 81.5% of the
//! outline paired.
//!
//! Inspired by: the contour-matching family of line-drawing vectorisers, which read a
//! line's centre and width from its two opposite contours rather than from a skeleton:
//! Hilaire, Tombre (2006), Robust and accurate vectorization of line drawings, IEEE TPAMI
//! 28(6) 890-904, doi:10.1109/TPAMI.2006.127; and the survey of Tombre, Ah-Soon, Dosch,
//! Masini, Tabbone (2000), Stable and robust vectorization: how to make the right choices,
//! Graphics Recognition: Recent Advances, Springer LNCS, pp. 3-18,
//! doi:10.1007/3-540-40953-X_1. Ours pairs points of an outline that the boundary solve
//! has already placed to a few hundredths of a pixel, so the pair distance *is* the width
//! to that precision, with no raster distance transform in between (a skeleton distance
//! transform read lucide widths with an 0.09 px spread in the same research, twice the
//! error budget).

use inkvec_core::{Point, Polyline, Vec2};

use super::grid::SegGrid;

/// Cosine of the largest angle between one side's inward normal and the reversed inward
/// normal of the other side for the two to count as a stroke's opposite sides (25°).
///
/// The normals of a polyline are noisy at the sub-pixel level, and a curved sleeve tilts
/// the far side's segment by up to half a pixel over a segment, so the test cannot be
/// tight; 25° still refuses a cap (whose normals turn through 180°) beyond its first
/// eighth and every concave junction corner.
pub(crate) const COS_PAIR: f64 = 0.906;

/// A face's boundary: every ring's points, sigmas and inward normals, and a segment grid.
pub(crate) struct Boundary {
    /// Measured points of all rings, concatenated, px.
    pub(crate) pts: Vec<Point>,
    /// Each point's positional sigma along the normal, px.
    pub(crate) sigma: Vec<f64>,
    /// Index of the next point of the same ring (wrapping at the ring's end).
    pub(crate) next: Vec<usize>,
    /// Index of the previous point of the same ring.
    pub(crate) prev: Vec<usize>,
    /// Inward unit normal of segment `i` (`pts[i] -> pts[next[i]]`).
    pub(crate) seg_n: Vec<Vec2>,
    /// Inward unit normal at point `i`: the mean of its two segments' normals, normalised.
    pub(crate) pt_n: Vec<Vec2>,
    /// Segment `i` of the grid is segment `i` above.
    pub(crate) grid: SegGrid,
    /// Point `i` lies on the canvas frame ([`Boundary::mark_frame`]).
    pub(crate) frame: Vec<bool>,
}

/// Sigma of a frame point's one-sided residual, px. The frame is where the canvas cuts
/// the face, known exactly, unlike the 0.5 px the planar map gives these unmeasured points.
pub(crate) const SIGMA_FRAME: f64 = 0.05;

/// `v / |v|`, or `None` when `|v| <= 1e-12`.
pub(crate) fn unit(v: Vec2) -> Option<Vec2> {
    let l = v.norm();
    (l > 1e-12).then(|| Vec2 {
        x: v.x / l,
        y: v.y / l,
    })
}

/// The left normal of direction `e` in the image's own axes, `(-e.y, e.x) / |e|`, or
/// `None` for a zero vector.
fn left_normal(e: Vec2) -> Option<Vec2> {
    unit(Vec2 { x: -e.y, y: e.x })
}

/// `p + s·v`.
pub(crate) fn along(p: Point, v: Vec2, s: f64) -> Point {
    Point::new(p.x + s * v.x, p.y + s * v.y)
}

impl Boundary {
    /// Build the boundary from closed `rings` (points with per-point sigma, px), with
    /// `inside(p)` saying whether image point `p` is in the face. `cell` is the segment
    /// grid's cell side, px.
    ///
    /// Consecutive points closer than 1e-6 px are merged (the planar map repeats each
    /// junction node at the end of one edge and the start of the next), and a ring that
    /// closes on its first point drops the repeat. Rings with fewer than three distinct
    /// points are skipped. `None` when nothing is left.
    pub(crate) fn new(
        rings: &[Polyline],
        inside: &dyn Fn(Point) -> bool,
        cell: f64,
    ) -> Option<Boundary> {
        let mut pts = Vec::new();
        let mut sigma = Vec::new();
        let mut next = Vec::new();
        let mut prev = Vec::new();
        let mut seg_n = Vec::new();
        for ring in rings {
            let mut rp: Vec<Point> = Vec::with_capacity(ring.points.len());
            let mut rs: Vec<f64> = Vec::with_capacity(ring.points.len());
            for (k, &p) in ring.points.iter().enumerate() {
                if rp.last().is_some_and(|q: &Point| q.dist(p) < 1e-6) {
                    continue;
                }
                rp.push(p);
                rs.push(ring.sigma.get(k).copied().unwrap_or(0.1));
            }
            while rp.len() > 1 && rp[0].dist(rp[rp.len() - 1]) < 1e-6 {
                rp.pop();
                rs.pop();
            }
            let n = rp.len();
            if n < 3 {
                continue;
            }
            let base = pts.len();
            // Ring successor and predecessor by comparison, not `%`: wazero's arm64
            // compiler once miscompiled `i32.rem_u` in a hot loop (the Go binding).
            let succ = |i: usize| if i + 1 == n { 0 } else { i + 1 };
            let pred = |i: usize| if i == 0 { n - 1 } else { i - 1 };
            // Left normals, then the orientation vote against the mask.
            let left: Vec<Vec2> = (0..n)
                .map(|i| left_normal(rp[succ(i)] - rp[i]).unwrap_or(Vec2 { x: 0.0, y: 0.0 }))
                .collect();
            let mut votes_in = 0usize;
            for i in 0..n {
                let (a, b) = (rp[i], rp[succ(i)]);
                if inside(along(
                    Point::new(0.5 * (a.x + b.x), 0.5 * (a.y + b.y)),
                    left[i],
                    0.6,
                )) {
                    votes_in += 1;
                }
            }
            let sign = if 2 * votes_in >= n { 1.0 } else { -1.0 };
            for i in 0..n {
                next.push(base + succ(i));
                prev.push(base + pred(i));
                seg_n.push(Vec2 {
                    x: sign * left[i].x,
                    y: sign * left[i].y,
                });
            }
            pts.extend(rp);
            sigma.extend(rs);
        }
        if pts.is_empty() {
            return None;
        }
        let pt_n: Vec<Vec2> = (0..pts.len())
            .map(|i| {
                let (a, b) = (seg_n[prev[i]], seg_n[i]);
                unit(Vec2 {
                    x: a.x + b.x,
                    y: a.y + b.y,
                })
                .unwrap_or(b)
            })
            .collect();
        let segs: Vec<(Point, Point)> = (0..pts.len()).map(|i| (pts[i], pts[next[i]])).collect();
        Some(Boundary {
            grid: SegGrid::new(&segs, cell),
            frame: vec![false; pts.len()],
            pts,
            sigma,
            next,
            prev,
            seg_n,
            pt_n,
        })
    }

    /// Number of measured points (and of segments).
    pub(crate) fn len(&self) -> usize {
        self.pts.len()
    }

    /// Mark the points on the frame of a `w` x `h` canvas (within 1e-3 px of
    /// `x = -0.5`, `x = w - 0.5`, `y = -0.5` or `y = h - 0.5`).
    ///
    /// A face that runs off the canvas has boundary points along the frame that are where
    /// the canvas cut it, not where it ends, and a stroke may paint past them (the canvas
    /// clips it). Their residual is one-sided ([`Boundary::residual`]).
    pub(crate) fn mark_frame(&mut self, w: usize, h: usize) {
        let (x1, y1) = (w as f64 - 0.5, h as f64 - 0.5);
        for (i, p) in self.pts.iter().enumerate() {
            self.frame[i] = (p.x + 0.5).abs() < 1e-3
                || (p.x - x1).abs() < 1e-3
                || (p.y + 0.5).abs() < 1e-3
                || (p.y - y1).abs() < 1e-3;
        }
    }

    /// Point `i`'s weighted residual against strokes of half-width `h` whose distance
    /// term to it is `d`: `(r, σ, active)` with `r = (d - h)/σ`.
    ///
    /// For a measured point `σ` is its own sigma (floored at 1e-3 px, as the curve
    /// fitter's chi-squared floors it). For a frame point the residual is one-sided --
    /// zero, and inactive (no gradient), once the point is painted (`d <= h`), and
    /// `(d - h)/`[`SIGMA_FRAME`] while it is not -- because the face reaches the frame and
    /// beyond it nothing is drawn. Found on simple-icons `wxt`: a stroke stopping short of
    /// the canvas edge left a grey line along it, dE00 0.037 -> 0.062, with the two-sided
    /// residual reading the frame points at their unmeasured 0.5 px.
    pub(crate) fn residual(&self, i: usize, d: f64, h: f64) -> (f64, f64, bool) {
        if self.frame[i] {
            return if d <= h {
                (0.0, SIGMA_FRAME, false)
            } else {
                ((d - h) / SIGMA_FRAME, SIGMA_FRAME, true)
            };
        }
        let s = self.sigma[i].max(1e-3);
        ((d - h) / s, s, true)
    }

    /// The sigma at position `u` along segment `seg`, interpolated linearly between its
    /// endpoints' sigmas, px.
    pub(crate) fn sigma_at(&self, seg: usize, u: f64) -> f64 {
        (1.0 - u) * self.sigma[seg] + u * self.sigma[self.next[seg]]
    }

    /// From boundary point `p` on segment `seg` (or at a vertex shared by `seg` and its
    /// predecessor), walk along inward normal `n` to the other side of the face: the
    /// first crossing `t` in `(0.05, t_max]` px, skipping `seg` and its two neighbours.
    ///
    /// Returns `(t, hit segment, u along it)` only when the hit segment's inward normal is
    /// anti-parallel to `n` within [`COS_PAIR`] -- the far side of a stroke, not a cap, a
    /// junction corner or a neighbouring stroke seen at an angle.
    pub(crate) fn across(
        &self,
        p: Point,
        n: Vec2,
        seg: usize,
        t_max: f64,
    ) -> Option<(f64, usize, f64)> {
        let (a, b) = (self.prev[seg], self.next[seg]);
        let skip = |k: usize| k == seg || k == a || k == b;
        let (t, k, u) = self.grid.ray(p, n, 0.05, t_max, &skip)?;
        let m = self.seg_n[k];
        (n.dot(m) <= -COS_PAIR).then_some((t, k, u))
    }

    /// The width read at every measured point by [`Boundary::across`] along its own
    /// inward normal, `None` where no anti-parallel side lies within `t_max` px.
    pub(crate) fn pair(&self, t_max: f64) -> Vec<Option<f64>> {
        (0..self.len())
            .map(|i| {
                self.across(self.pts[i], self.pt_n[i], i, t_max)
                    .map(|h| h.0)
            })
            .collect()
    }
}

/// The stroke width a face's paired widths agree on, and the share of its boundary
/// points that agree with it.
///
/// `w0` is the median of the paired widths; the points within `max(0.1·w0, 0.3)` px of it
/// are the sleeve's, and the width returned is their mean (the mean of the mode's core, as
/// the research measurement used). `None` when fewer than a tenth of the points paired.
pub(crate) fn stroke_width(pairs: &[Option<f64>]) -> Option<(f64, f64)> {
    let mut ws: Vec<f64> = pairs.iter().flatten().copied().collect();
    if ws.is_empty() || ws.len() * 10 < pairs.len() {
        return None;
    }
    ws.sort_by(f64::total_cmp);
    let w0 = ws[ws.len() / 2];
    let tol = (0.1 * w0).max(0.3);
    let core: Vec<f64> = ws
        .iter()
        .copied()
        .filter(|w| (w - w0).abs() <= tol)
        .collect();
    let w = core.iter().sum::<f64>() / core.len() as f64;
    Some((w, core.len() as f64 / pairs.len() as f64))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A horizontal bar `x in [0, len]`, `y in [0, w]`, sampled every `step` px, as one
    /// closed ring (clockwise in image axes).
    pub(crate) fn bar(len: f64, w: f64, step: f64) -> Polyline {
        let mut pts = Vec::new();
        let n = (len / step).round() as usize;
        let m = (w / step).round() as usize;
        for i in 0..n {
            pts.push(Point::new(i as f64 * step, 0.0));
        }
        for j in 0..m {
            pts.push(Point::new(len, j as f64 * w / m as f64));
        }
        for i in 0..n {
            pts.push(Point::new(len - i as f64 * step, w));
        }
        for j in 0..m {
            pts.push(Point::new(0.0, w - j as f64 * w / m as f64));
        }
        Polyline::with_uniform_sigma(pts, 0.05, true)
    }

    #[test]
    fn normals_point_into_the_bar_and_pairing_reads_its_width() {
        let ring = bar(40.0, 6.0, 0.5);
        let inside = |p: Point| p.x > 0.0 && p.x < 40.0 && p.y > 0.0 && p.y < 6.0;
        let b = Boundary::new(std::slice::from_ref(&ring), &inside, 2.0).expect("a ring");
        for i in 0..b.len() {
            let q = along(b.pts[i], b.pt_n[i], 0.25);
            // Corners' averaged normals point diagonally inward; everything is inside.
            assert!(inside(q), "point {i} normal points out");
        }
        let pairs = b.pair(20.0);
        let (w, share) = stroke_width(&pairs).expect("the bar pairs");
        assert!((w - 6.0).abs() < 1e-9, "width {w}");
        assert!(share > 0.7, "share {share}");
        // The same ring walked the other way: the vote flips the normals back inward.
        let mut rev = ring.clone();
        rev.points.reverse();
        let b2 = Boundary::new(&[rev], &inside, 2.0).expect("a ring");
        let (w2, _) = stroke_width(&b2.pair(20.0)).expect("the bar pairs");
        assert!((w2 - 6.0).abs() < 1e-9, "width {w2}");
    }

    #[test]
    fn a_square_blob_mostly_fails_to_pair_at_one_width() {
        let ring = bar(20.0, 20.0, 0.5);
        let inside = |p: Point| p.x > 0.0 && p.x < 20.0 && p.y > 0.0 && p.y < 20.0;
        let b = Boundary::new(&[ring], &inside, 2.0).expect("a ring");
        let (w, _) = stroke_width(&b.pair(100.0)).expect("a square pairs across itself");
        // A square pairs, but at its own size: the caller's aspect tests refuse it.
        assert!((w - 20.0).abs() < 1e-9);
    }

    #[test]
    fn degenerate_rings_are_skipped() {
        let ring = Polyline::with_uniform_sigma(
            vec![Point::new(0.0, 0.0), Point::new(1.0, 0.0)],
            0.1,
            true,
        );
        assert!(Boundary::new(&[ring], &|_| true, 1.0).is_none());
        assert!(stroke_width(&[]).is_none());
        assert!(stroke_width(&[None, None, None]).is_none());
    }

    #[test]
    fn frame_points_are_one_sided() {
        // A bar touching the left frame of a 50 x 50 canvas at x = -0.5.
        let ring = bar(20.0, 6.0, 0.5);
        let shifted: Vec<Point> = ring
            .points
            .iter()
            .map(|p| Point::new(p.x - 0.5, p.y + 10.0))
            .collect();
        let ring = Polyline::with_uniform_sigma(shifted, 0.5, true);
        let mut b = Boundary::new(
            &[ring],
            &|p| p.x > -0.5 && p.x < 19.5 && p.y > 10.0 && p.y < 16.0,
            2.0,
        )
        .expect("a ring");
        b.mark_frame(50, 50);
        let i = (0..b.len()).find(|&i| b.frame[i]).expect("a frame point");
        let j = (0..b.len()).find(|&i| !b.frame[i]).expect("an inner point");
        // Painted frame point: no residual, no gradient; unpainted: tight sigma.
        assert_eq!(b.residual(i, 2.0, 3.0), (0.0, SIGMA_FRAME, false));
        let (r, s, on) = b.residual(i, 3.1, 3.0);
        assert!(on && s == SIGMA_FRAME && (r - 0.1 / SIGMA_FRAME).abs() < 1e-9);
        // Other points are two-sided at their own sigma.
        let (r, s, on) = b.residual(j, 2.9, 3.0);
        assert!(on && s == 0.5 && (r + 0.2).abs() < 1e-9);
    }
}
