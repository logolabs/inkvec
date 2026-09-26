//! Two post-fit passes that trade a little residual for fewer numbers: an axis-aligned line
//! (`INKVEC_AXIS`) and a G1-smooth join (`INKVEC_G1`). Research only: neither runs in a
//! release build.

use inkvec_core::{Point, Polyline};

use crate::curves::Segment;
use crate::{FitConfig, FittedPath, PARAMS_LINE};

/// A line the drawing constrains to an axis costs one number, not two: the artist writes
/// `h16`, not `L 16,0`.
pub const PARAMS_AXIS_LINE: f64 = 1.0;

/// How many sigma the single worst-placed sample may sit from the axis candidate. See
/// `snap_axis_aligned`'s use of it for why the aggregate chi2 budget is not enough on its
/// own.
const MAX_AXIS_DEV_SIGMA: f64 = 3.0;

/// Put a line the measurement cannot distinguish from axis-aligned onto the axis.
///
/// Artists constrain lines to the axes and the corpus says so plainly: 68% of lucide's
/// straight segments and 46% of simple-icons' are *exactly* horizontal or vertical, and
/// widening the tolerance does not find more (68.3% -> 68.4% out to a quarter pixel).
/// That is the signature of a constraint rather than a coincidence. Our own output is
/// smeared instead: 12% exactly axis-aligned, but a third of all lines within 0.05 px of
/// it -- inside the accuracy the boundary itself is measured to.
///
/// They come out that way because the objective cannot see the difference. An axis-aligned
/// line has one degree of freedom and a general one has two, but the fitter charges
/// `PARAMS_LINE` either way, so nothing is ever gained by snapping and the free parameter
/// is spent on measurement noise. This adds the missing member of the alphabet and settles
/// it the way every other choice here is settled: the constrained line is taken when it is
/// cheaper under `0.5 * chi2 + lambda * params`, which is
///
/// ```text
///     0.5 * (chi2_axis - chi2_free)  <  lambda * (PARAMS_LINE - PARAMS_AXIS_LINE)
/// ```
///
/// -- one parameter's worth of evidence, not a tolerance anyone chose. A line off the axis
/// by more than its own noise pays more in chi2 than it saves and stays where it is.
///
/// Snapping moves the vertex this segment shares with the next, so the arithmetic is
/// greedy: the neighbour's own residual is not re-examined. It stays honest because the
/// test only ever passes for moves inside the measurement's own uncertainty, which is
/// where the neighbour cannot notice either. Measured over 32 icons at the tolerance this
/// implies, snapping a third of all lines moved mean dE00 by -0.001.
pub fn snap_axis_aligned(
    path: &mut FittedPath,
    poly: &Polyline,
    vertices: &[usize],
    cfg: &FitConfig,
) -> usize {
    if path.segments.is_empty() || vertices.len() != path.segments.len() + 1 {
        return 0;
    }
    let budget = 2.0 * cfg.lambda * (PARAMS_LINE - PARAMS_AXIS_LINE);
    let mut snapped = 0usize;
    let mut cur = path.start;
    for k in 0..path.segments.len() {
        let Segment::Line(end) = path.segments[k] else {
            cur = path.segments[k].end();
            continue;
        };
        let (dx, dy) = (end.x - cur.x, end.y - cur.y);
        if dx == 0.0 && dy == 0.0 {
            cur = end;
            continue;
        }
        // The nearer axis, and the endpoint it would take.
        let want = if dy.abs() <= dx.abs() {
            Point::new(end.x, cur.y)
        } else {
            Point::new(cur.x, end.y)
        };
        if want.dist(end) < 1e-12 {
            cur = end; // already on the axis
            continue;
        }
        // The point about to move is also where the *next* segment starts, and that
        // segment's control points -- if it has any -- are fixed in absolute
        // coordinates. Moving the shared vertex without them silently distorts whatever
        // comes next: a cubic's own shape changes at its very anchor, with no check on
        // it at all. Only a Line has no such baggage, so only a line-line join is safe to
        // move; a closed ring's last segment is left alone outright, since its endpoint
        // is `path.start` by construction and this pass does not track that separately.
        // Caught on `simple-icons/atlassian`: a 0.21 px move (inside its own sigma
        // budget) shifted a cubic's start away from its fixed control points and clipped
        // a whole pixel row from full coverage to partial at the shape's edge, for +0.023
        // dE00 on one icon -- far more damage than the parameter this move was saving.
        // This function fits one EDGE of the planar map at a time -- `run_color` calls
        // it per edge, and the emitter concatenates several edges' fitted segments into
        // one ring afterwards (`emit_color` / `ring_points`). So a segment with no
        // successor *inside this path* does not mean there is no successor at all: it
        // means the successor is the first segment of a different edge's fit, fit
        // independently, and unknown here. That case must refuse exactly like a known
        // Cubic does -- it very nearly did once already, and only luck (the true next
        // segment happening to be a Line too) kept the first version of this bound from
        // shipping a bug: treating "no next segment in this Vec" as "safe" is not the
        // same fact as "no next segment at all", and only the second one is required.
        let next_is_line =
            k + 1 < path.segments.len() && matches!(path.segments[k + 1], Segment::Line(_));
        if !next_is_line {
            cur = end;
            continue;
        }

        let (a, b) = (vertices[k], vertices[k + 1]);
        if b <= a || b >= poly.points.len() {
            cur = end;
            continue;
        }
        // The vertex this moves is shared with the next segment, and that segment's own
        // residual is not in the test below. Bound the damage instead of pricing it: a
        // move no larger than the uncertainty of the point being moved cannot be
        // something the neighbour could have resolved either. Without this the pass
        // helps drawings built on a grid and hurts drawn ones -- icons gained (lucide
        // dE00 0.090 -> 0.085) while emoji lost (noto 0.389 -> 0.401), which is a
        // neighbouring cubic being dragged off its own evidence.
        let room = poly.sigma.get(b).copied().unwrap_or(0.5).max(1e-6);
        if want.dist(end) > room {
            cur = end;
            continue;
        }
        // The chi2 budget alone is not enough on a long line: it sums over every sample
        // between a and b, so a real, consistent, tiny slope -- not noise, a trend -- can
        // hide under the budget once spread across enough points, the same way a small
        // systematic error survives averaging where an equally large random one would
        // not. Flattening it then does not cost fitting quality in aggregate, but it
        // shifts render coverage the *same direction* over the segment's whole length --
        // one long line, one pixel row, fully in or fully out end to end, rather than
        // scattered sub-pixel noise that mostly cancels. Caught on `lucide/bath`: two
        // points 91 units apart differed by 0.02 px, well inside the chi2 budget summed
        // over ~90 samples, and flattening it turned a 90 px stretch of a correctly grey
        // (partially covered) row solid black. So a second, independent test: no single
        // measured point may sit more than a few sigma from the axis candidate, which
        // catches a trend the sum cannot.
        if max_dev_sigma(poly, a, b, cur, want) > MAX_AXIS_DEV_SIGMA {
            cur = end;
            continue;
        }
        let free = chi2_about(poly, a, b, cur, end);
        let axis = chi2_about(poly, a, b, cur, want);
        if axis - free < budget {
            path.segments[k] = Segment::Line(want);
            snapped += 1;
            cur = want;
        } else {
            cur = end;
        }
    }
    snapped
}

/// The largest normalised perpendicular distance from any single point in `poly[a..=b]`
/// to the line through `p` and `q`. A trend the aggregate chi2 in [`chi2_about`] cannot
/// see shows up here as one number well past a few sigma.
fn max_dev_sigma(poly: &Polyline, a: usize, b: usize, p: Point, q: Point) -> f64 {
    let (dx, dy) = (q.x - p.x, q.y - p.y);
    let len = (dx * dx + dy * dy).sqrt();
    if len < 1e-12 {
        return f64::INFINITY;
    }
    let (nx, ny) = (-dy / len, dx / len);
    let mut worst = 0.0f64;
    for i in a..=b.min(poly.points.len() - 1) {
        let pt = poly.points[i];
        let d = (pt.x - p.x) * nx + (pt.y - p.y) * ny;
        let s = poly.sigma.get(i).copied().unwrap_or(0.5).max(1e-6);
        worst = worst.max((d / s).abs());
    }
    worst
}

/// Weighted sum of squared perpendicular distances from `poly[a..=b]` to the line through
/// `p` and `q`, each point scaled by its own measured sigma -- the same chi2 the program
/// minimises, for one candidate segment.
fn chi2_about(poly: &Polyline, a: usize, b: usize, p: Point, q: Point) -> f64 {
    let (dx, dy) = (q.x - p.x, q.y - p.y);
    let len = (dx * dx + dy * dy).sqrt();
    if len < 1e-12 {
        return f64::INFINITY;
    }
    let (nx, ny) = (-dy / len, dx / len);
    let mut acc = 0.0;
    for i in a..=b.min(poly.points.len() - 1) {
        let pt = poly.points[i];
        let d = (pt.x - p.x) * nx + (pt.y - p.y) * ny;
        let s = poly.sigma.get(i).copied().unwrap_or(0.5).max(1e-6);
        acc += (d / s) * (d / s);
    }
    acc
}

/// A cubic whose first control point is the reflection of the one before it costs four
/// numbers, not six: SVG writes it `S`, and the reflection is implied.
pub const PARAMS_SMOOTH_CUBIC: f64 = 4.0;

/// Make a nearly-smooth join between two cubics exactly smooth, where the picture allows.
///
/// Artists constrain these, and the corpus is unambiguous about it: of the joins between
/// consecutive cubics that are smooth to within a thousandth of a degree, 60% have equal
/// handle lengths either side -- the ratio's median is exactly 1.00 and its whole
/// interquartile range is 1.00. That is the `S` command: give it the reflection and the
/// first control point need not be written at all.
///
/// The fitter charges `PARAMS_CUBIC` either way, so it has never had a reason to prefer
/// the constrained form, and produces it only by accident. This offers the constrained
/// form as a candidate and settles it the way everything else here is settled:
///
/// ```text
///     0.5 * (chi2_reflected - chi2_free)  <  lambda * (PARAMS_CUBIC - PARAMS_SMOOTH_CUBIC)
/// ```
///
/// Note what makes this safe where `snap_axis_aligned` needed two guards to become so:
/// moving a control point moves no *shared vertex*. The join point, the previous segment,
/// and everything before it are untouched, so there is no neighbour to drag off its own
/// evidence and no successor in another edge's fit to worry about. Only the second
/// cubic's own shape changes, and its own residual is exactly what the test measures.
pub fn snap_smooth_joins(
    path: &mut FittedPath,
    poly: &Polyline,
    vertices: &[usize],
    cfg: &FitConfig,
) -> usize {
    if path.segments.len() < 2 || vertices.len() != path.segments.len() + 1 {
        return 0;
    }
    let budget = 2.0 * cfg.lambda * (crate::multimodel::params_cubic() - PARAMS_SMOOTH_CUBIC);
    let s = arc_lengths_of(&poly.points);
    let dbg = inkvec_core::env::flag("INKVEC_G1DBG");
    let mut snapped = 0usize;
    let mut starts: Vec<Point> = Vec::with_capacity(path.segments.len());
    let mut cur = path.start;
    for seg in &path.segments {
        starts.push(cur);
        cur = seg.end();
    }

    for k in 1..path.segments.len() {
        let (a0, b0) = (vertices[k - 1], vertices[k]);
        let (a1, b1) = (vertices[k], vertices[k + 1]);
        if b0 <= a0 || b1 <= a1 || b1 >= poly.points.len() {
            continue;
        }
        let (q0, q1) = (starts[k - 1], starts[k]);

        match (path.segments[k - 1].clone(), path.segments[k].clone()) {
            (Segment::Cubic(pc1, pc2, pp3), Segment::Cubic(c1, c2, p3)) => {
                let v_in = (pp3.x - pc2.x, pp3.y - pc2.y);
                let v_out = (c1.x - q1.x, c1.y - q1.y);
                let (n_in, n_out) = (v_in.0.hypot(v_in.1), v_out.0.hypot(v_out.1));
                if n_in < 1e-9 || n_out < 1e-9 {
                    continue;
                }
                let ang = (v_in.1.atan2(v_in.0) - v_out.1.atan2(v_out.0) + std::f64::consts::PI)
                    .rem_euclid(2.0 * std::f64::consts::PI)
                    - std::f64::consts::PI;
                if ang.abs() > 20.0_f64.to_radians() {
                    continue;
                }

                let chi2_of = |pc2: Point, c1: Point, c2: Point| -> f64 {
                    let prev = crate::multimodel::Cubic {
                        p0: q0,
                        p1: pc1,
                        p2: pc2,
                        p3: pp3,
                    };
                    let next = crate::multimodel::Cubic {
                        p0: q1,
                        p1: c1,
                        p2: c2,
                        p3,
                    };
                    crate::multimodel::chi2_cubic(
                        &poly.points,
                        &poly.sigma,
                        &s,
                        a0,
                        b0,
                        &prev,
                        true,
                    ) + crate::multimodel::chi2_cubic(
                        &poly.points,
                        &poly.sigma,
                        &s,
                        a1,
                        b1,
                        &next,
                        true,
                    )
                };
                let free = chi2_of(pc2, c1, c2);

                let reflect = |pc2: Point| Point::new(2.0 * q1.x - pc2.x, 2.0 * q1.y - pc2.y);
                let cost = |v: &[f64; 4]| -> f64 {
                    let p = Point::new(v[0], v[1]);
                    chi2_of(p, reflect(p), Point::new(v[2], v[3]))
                };
                let mut v = [pc2.x, pc2.y, c2.x, c2.y];
                let mut best = cost(&v);
                let mut step = 0.25_f64.max(q1.dist(p3) * 0.05);
                for _ in 0..12 {
                    let mut moved = false;
                    for i in 0..4 {
                        for dir in [-1.0, 1.0] {
                            let mut t = v;
                            t[i] += dir * step;
                            let c = cost(&t);
                            if c < best - 1e-9 {
                                best = c;
                                v = t;
                                moved = true;
                            }
                        }
                    }
                    if !moved {
                        step *= 0.5;
                        if step < 1e-4 {
                            break;
                        }
                    }
                }

                if dbg {
                    eprintln!(
                        "  [g1] pair {k}: free chi2 {free:.2} -> constrained {best:.2} \
        (d {:.2}) vs budget {budget:.2} -> {}",
                        best - free,
                        if best - free < budget { "SNAP" } else { "keep" }
                    );
                }
                if best - free < budget {
                    let new_pc2 = Point::new(v[0], v[1]);
                    path.segments[k - 1] = Segment::Cubic(pc1, new_pc2, pp3);
                    path.segments[k] = Segment::Cubic(reflect(new_pc2), Point::new(v[2], v[3]), p3);
                    snapped += 1;
                }
            }
            (Segment::Line(_), Segment::Cubic(c1, c2, p3)) => {
                let v_in = (q1.x - q0.x, q1.y - q0.y);
                let v_out = (c1.x - q1.x, c1.y - q1.y);
                let (n_in, n_out) = (v_in.0.hypot(v_in.1), v_out.0.hypot(v_out.1));
                if n_in < 1e-9 || n_out < 1e-9 {
                    continue;
                }
                let ang = (v_in.1.atan2(v_in.0) - v_out.1.atan2(v_out.0) + std::f64::consts::PI)
                    .rem_euclid(2.0 * std::f64::consts::PI)
                    - std::f64::consts::PI;
                if ang.abs() > 20.0_f64.to_radians() {
                    continue;
                }
                let u_in = (v_in.0 / n_in, v_in.1 / n_in);
                let chi2_of = |arm: f64, c2: Point| -> f64 {
                    let next = crate::multimodel::Cubic {
                        p0: q1,
                        p1: Point::new(q1.x + u_in.0 * arm, q1.y + u_in.1 * arm),
                        p2: c2,
                        p3,
                    };
                    crate::multimodel::chi2_cubic(
                        &poly.points,
                        &poly.sigma,
                        &s,
                        a1,
                        b1,
                        &next,
                        true,
                    )
                };
                let free = crate::multimodel::chi2_cubic(
                    &poly.points,
                    &poly.sigma,
                    &s,
                    a1,
                    b1,
                    &crate::multimodel::Cubic {
                        p0: q1,
                        p1: c1,
                        p2: c2,
                        p3,
                    },
                    true,
                );
                let mut v = [n_out, c2.x, c2.y];
                let mut best = chi2_of(v[0], Point::new(v[1], v[2]));
                let mut step = 0.25_f64.max(q1.dist(p3) * 0.05);
                for _ in 0..12 {
                    let mut moved = false;
                    for i in 0..3 {
                        for dir in [-1.0, 1.0] {
                            let mut t = v;
                            t[i] += dir * step;
                            if i == 0 && t[0] < 1e-4 {
                                continue;
                            }
                            let c = chi2_of(t[0], Point::new(t[1], t[2]));
                            if c < best - 1e-9 {
                                best = c;
                                v = t;
                                moved = true;
                            }
                        }
                    }
                    if !moved {
                        step *= 0.5;
                        if step < 1e-4 {
                            break;
                        }
                    }
                }
                let lc_budget = cfg.lambda;
                if dbg {
                    eprintln!(
                        "  [g1-lc] pair {k}: free chi2 {free:.2} -> constrained {best:.2} \
        (d {:.2}) vs budget {lc_budget:.2} -> {}",
                        best - free,
                        if best - free < lc_budget {
                            "SNAP"
                        } else {
                            "keep"
                        }
                    );
                }
                if best - free < lc_budget {
                    let new_c1 = Point::new(q1.x + u_in.0 * v[0], q1.y + u_in.1 * v[0]);
                    path.segments[k] = Segment::Cubic(new_c1, Point::new(v[1], v[2]), p3);
                    snapped += 1;
                }
            }
            (Segment::Cubic(pc1, pc2, pp3), Segment::Line(p3)) => {
                let v_in = (pp3.x - pc2.x, pp3.y - pc2.y);
                let v_out = (p3.x - q1.x, p3.y - q1.y);
                let (n_in, n_out) = (v_in.0.hypot(v_in.1), v_out.0.hypot(v_out.1));
                if n_in < 1e-9 || n_out < 1e-9 {
                    continue;
                }
                let ang = (v_in.1.atan2(v_in.0) - v_out.1.atan2(v_out.0) + std::f64::consts::PI)
                    .rem_euclid(2.0 * std::f64::consts::PI)
                    - std::f64::consts::PI;
                if ang.abs() > 20.0_f64.to_radians() {
                    continue;
                }
                let u_out = (v_out.0 / n_out, v_out.1 / n_out);
                let chi2_of = |pc1: Point, arm: f64| -> f64 {
                    let prev = crate::multimodel::Cubic {
                        p0: q0,
                        p1: pc1,
                        p2: Point::new(q1.x - u_out.0 * arm, q1.y - u_out.1 * arm),
                        p3: q1,
                    };
                    crate::multimodel::chi2_cubic(
                        &poly.points,
                        &poly.sigma,
                        &s,
                        a0,
                        b0,
                        &prev,
                        true,
                    )
                };
                let free = crate::multimodel::chi2_cubic(
                    &poly.points,
                    &poly.sigma,
                    &s,
                    a0,
                    b0,
                    &crate::multimodel::Cubic {
                        p0: q0,
                        p1: pc1,
                        p2: pc2,
                        p3: pp3,
                    },
                    true,
                );
                let mut v = [pc1.x, pc1.y, n_in];
                let mut best = chi2_of(Point::new(v[0], v[1]), v[2]);
                let mut step = 0.25_f64.max(q0.dist(q1) * 0.05);
                for _ in 0..12 {
                    let mut moved = false;
                    for i in 0..3 {
                        for dir in [-1.0, 1.0] {
                            let mut t = v;
                            t[i] += dir * step;
                            if i == 2 && t[2] < 1e-4 {
                                continue;
                            }
                            let c = chi2_of(Point::new(t[0], t[1]), t[2]);
                            if c < best - 1e-9 {
                                best = c;
                                v = t;
                                moved = true;
                            }
                        }
                    }
                    if !moved {
                        step *= 0.5;
                        if step < 1e-4 {
                            break;
                        }
                    }
                }
                let cl_budget = cfg.lambda;
                if dbg {
                    eprintln!(
                        "  [g1-cl] pair {k}: free chi2 {free:.2} -> constrained {best:.2} \
        (d {:.2}) vs budget {cl_budget:.2} -> {}",
                        best - free,
                        if best - free < cl_budget {
                            "SNAP"
                        } else {
                            "keep"
                        }
                    );
                }
                if best - free < cl_budget {
                    let new_pc2 = Point::new(q1.x - u_out.0 * v[2], q1.y - u_out.1 * v[2]);
                    path.segments[k - 1] = Segment::Cubic(Point::new(v[0], v[1]), new_pc2, pp3);
                    snapped += 1;
                }
            }
            _ => {}
        }
    }
    snapped
}

/// Cumulative chord length, the parameterisation `chi2_cubic` expects.
fn arc_lengths_of(pts: &[Point]) -> Vec<f64> {
    let mut out = Vec::with_capacity(pts.len());
    let mut acc = 0.0;
    out.push(0.0);
    for w in pts.windows(2) {
        acc += w[0].dist(w[1]);
        out.push(acc);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An L-shaped boundary: a horizontal run at y = 5, then a vertical one at x = 10, with
    /// the corner fitted 0.02 px (or 0.5 px) off the axis.
    fn corner(off: f64) -> (FittedPath, Polyline, Vec<usize>) {
        let mut points: Vec<Point> = (0..=10).map(|x| Point::new(x as f64, 5.0)).collect();
        points.extend((6..=10).map(|y| Point::new(10.0, y as f64)));
        let sigma = vec![0.1; points.len()];
        let path = FittedPath {
            start: Point::new(0.0, 5.0),
            segments: vec![
                Segment::Line(Point::new(10.0, 5.0 + off)),
                Segment::Line(Point::new(10.0, 10.0)),
            ],
            closed: false,
        };
        let poly = Polyline {
            points,
            sigma,
            closed: false,
        };
        (path, poly, vec![0, 10, 15])
    }

    #[test]
    fn a_line_off_the_axis_by_less_than_its_noise_is_snapped() {
        let (mut path, poly, vertices) = corner(0.02);
        let n = snap_axis_aligned(&mut path, &poly, &vertices, &FitConfig::default());
        assert_eq!(n, 1);
        assert!(matches!(path.segments[0], Segment::Line(p) if p == Point::new(10.0, 5.0)));
    }

    #[test]
    fn a_line_off_the_axis_by_more_than_its_noise_stays() {
        let (mut path, poly, vertices) = corner(0.5);
        let n = snap_axis_aligned(&mut path, &poly, &vertices, &FitConfig::default());
        assert_eq!(n, 0);
        assert_eq!(path.segments[0].end(), Point::new(10.0, 5.5));
    }
}
