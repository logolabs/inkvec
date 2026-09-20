//! The fitter: source runs in, cheapest descriptions out, everything guarded
//! by the tolerance it must keep.

use crate::geom::{cubic_at, dist_to_segment, max_deviation, nearest_t_cubic, point_segment_dist};
use crate::path::{corners, sample_mapped, sub, Src, Subpath};
use inkvec_core::{Point, Polyline};
use inkvec_fit::curves::Segment;
use inkvec_fit::multimodel::optimal_multimodel;
use inkvec_fit::primitives::{fit_primitive_or_arcs, PrimitiveKind};
use inkvec_fit::structural::eval_segment;
use inkvec_fit::{FitConfig, FittedPath};

/// The cubic through `p0` and `p3` closest to `pts` in least squares: the two control
/// points solved linearly at the parameters `t`, then each parameter moved to its point's
/// nearest place on the new curve and the solve repeated until it settles (Schneider,
/// 1990, with an exact projection in place of his Newton step). The dynamic program's
/// cubic is a tangent-constrained approximation good enough to *choose* a cubic; this is
/// what makes the chosen cubic *exact* where the source was one.
///
/// `t` is where each point sits along the source, which the caller knows and chord length
/// only guesses. It matters: a piece of a cubic, walked at its own rate, *is* a cubic and
/// the solve lands on it in one round, where from chord length the alternation settles
/// into a local minimum twenty-five tolerances away -- measured, on one cubic cut at 0.79.
pub(crate) fn ls_cubic_from(
    pts: &[Point],
    p0: Point,
    p3: Point,
    mut t: Vec<f64>,
) -> Option<(Point, Point)> {
    if pts.len() < 3 || t.len() != pts.len() {
        return None;
    }
    let (mut c1, mut c2);
    let mut last_err = f64::INFINITY;
    let mut best: Option<(Point, Point)> = None;
    let mut stalled = 0;
    for _ in 0..40 {
        let (mut s11, mut s12, mut s22) = (0.0, 0.0, 0.0);
        let (mut r1, mut r2) = (Point::new(0.0, 0.0), Point::new(0.0, 0.0));
        for (&p, &tt) in pts.iter().zip(&t) {
            let u = 1.0 - tt;
            let (b1, b2) = (3.0 * u * u * tt, 3.0 * u * tt * tt);
            let base = Point::new(
                u * u * u * p0.x + tt * tt * tt * p3.x,
                u * u * u * p0.y + tt * tt * tt * p3.y,
            );
            let r = sub(p, base);
            s11 += b1 * b1;
            s12 += b1 * b2;
            s22 += b2 * b2;
            r1 = Point::new(r1.x + b1 * r.x, r1.y + b1 * r.y);
            r2 = Point::new(r2.x + b2 * r.x, r2.y + b2 * r.y);
        }
        let det = s11 * s22 - s12 * s12;
        if det.abs() < 1e-18 {
            return None;
        }
        c1 = Point::new(
            (s22 * r1.x - s12 * r2.x) / det,
            (s22 * r1.y - s12 * r2.y) / det,
        );
        c2 = Point::new(
            (s11 * r2.x - s12 * r1.x) / det,
            (s11 * r2.y - s12 * r1.y) / det,
        );
        // Move every parameter to its point's nearest place on this cubic, and stop once
        // that no longer brings the points closer. An exact search each round: two Newton
        // steps from the previous parameter were tried and settled a tenth of a unit off on
        // a deep curve, where this reaches 1e-5. The refit is 3% of the run time.
        let mut err: f64 = 0.0;
        for (&p, tt) in pts.iter().zip(t.iter_mut()) {
            *tt = nearest_t_cubic(p, p0, c1, c2, p3);
            err = err.max(p.dist(cubic_at(p0, c1, c2, p3, *tt)));
        }
        // Keep the best round, not the last one: reprojection is not monotone, and a round
        // that overshoots would otherwise be the answer. And one bad round is not the end
        // of the search -- stopping at the first of them left a curve that is exactly a
        // cubic fitted 25 tolerances away from itself.
        if err < last_err {
            last_err = err;
            stalled = 0;
            if c1.x.is_finite() && c1.y.is_finite() && c2.x.is_finite() && c2.y.is_finite() {
                best = Some((c1, c2));
            }
        } else {
            stalled += 1;
            if stalled == 3 {
                break;
            }
        }
    }
    best
}
/// Refit one cubic to the samples it stands for, keeping the refit only where it brings
/// them closer. Anything that is not a cubic, and any cubic the least squares cannot
/// improve, comes back exactly as it went in. `along` is where each sample sits on the
/// source, normalised across the span.
fn refit_cubic(seg: &Segment, start: Point, span: &[Point], along: Vec<f64>) -> Segment {
    let Segment::Cubic(c1, c2, end) = *seg else {
        return seg.clone();
    };
    if span.len() < 3 {
        return seg.clone();
    }
    let Some((n1, n2)) = ls_cubic_from(span, start, end, along) else {
        return seg.clone();
    };
    let dev = |a: Point, b: Point| {
        span.iter()
            .map(|&p| dist_to_segment(p, &Segment::Cubic(a, b, end), start))
            .fold(0.0, f64::max)
    };
    if dev(n1, n2) < dev(c1, c2) {
        Segment::Cubic(n1, n2, end)
    } else {
        seg.clone()
    }
}
/// A tuning knob read from the environment, for measurement; the default is the shipped
/// value.
pub(crate) fn knob(k: &str, d: f64) -> f64 {
    std::env::var(k)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(d)
}
/// Points the dynamic program is shown on the first pass; the tracer's own cap
/// (`inkvec_fit::multimodel::DP_MAX_POINTS`) on the second.
pub(crate) const COARSE_CAP: usize = 128;
pub(crate) const FINE_CAP: usize = 768;

/// Runs of at most this many samples are *also* fitted whole, at the fine cap, and the
/// cheaper of the two descriptions kept. Past here the program costs seconds rather than
/// milliseconds -- the square of the points it is shown -- and only the coarse view with
/// its repairs is affordable.
pub(crate) const WHOLE_BELOW: usize = 256;

/// Points the whole-run primitive fitter is shown. It costs O(n²) as well, and a circle
/// needs no more witnesses than this to be told from a cubic.
pub(crate) const PRIMITIVE_CAP: usize = 192;

/// Samples taken of one source segment, at most. What a fit needs is enough witnesses to
/// tell one curve from another, which is a property of the drawing; the spacing alone is
/// a property of the tolerance, and at a tight one it asks for hundreds of them per
/// segment -- which the program then pays for squared. Measured on the 40-file batch,
/// coming down from 48 to 24 saved *more* description length (17.3% to 17.6%) in half the
/// time: past two dozen witnesses a cubic has nothing left to say about itself.
pub(crate) const PER_SEGMENT: usize = 24;
/// What a description costs *as this tool writes it*, which is what the minifier is
/// trying to make small. It is not the fitter's parameter count: [`fmt_subpath`] restates
/// a cubic that continues the previous one smoothly as an `S`, four numbers instead of
/// six, so a chain of cubics is cheaper than its segments suggest and an arc -- five,
/// always -- has more to beat than it looks.
fn params_of(segs: &[Segment]) -> f64 {
    let mut total = 0.0;
    let mut prev: Option<(Point, Point)> = None;
    for seg in segs {
        match *seg {
            Segment::Line(_) => {
                total += 2.0;
                prev = None;
            }
            Segment::Cubic(c1, c2, p) => {
                let smooth = prev.is_some_and(|(pc2, pp): (Point, Point)| {
                    Point::new(2.0 * pp.x - pc2.x, 2.0 * pp.y - pc2.y).dist(c1) < 5e-4
                });
                total += if smooth { 4.0 } else { 6.0 };
                prev = Some((c2, p));
            }
            Segment::Arc { .. } => {
                total += 5.0;
                prev = None;
            }
        }
    }
    total
}
/// One run being fitted: the source, the samples taken along it, and where on the source
/// each sample sits. Sample indices are *unwrapped* on a closed run -- `i` and `i + n`
/// are the same point one lap apart -- so a span never has to wrap.
struct RunFit<'a> {
    run: &'a [Src],
    pts: Vec<Point>,
    /// Source position of each sample: segment index plus parameter within it.
    at: Vec<f64>,
    eps: f64,
    cfg: &'a FitConfig,
}

impl RunFit<'_> {
    fn point(&self, i: usize) -> Point {
        self.pts[i % self.pts.len()]
    }

    /// Source position of unwrapped sample `i`, laps included.
    fn source_at(&self, i: usize) -> f64 {
        let n = self.pts.len();
        self.at[i % n] + (i / n) as f64 * self.run.len() as f64
    }

    /// Samples `a..=b`, and where each one sits along the source between them, from 0 to
    /// 1. That is the parameterisation a fit of this span is looking for.
    fn span(&self, a: usize, b: usize) -> (Vec<Point>, Vec<f64>) {
        let (ua, ub) = (self.source_at(a), self.source_at(b));
        let width = if ub > ua { ub - ua } else { 1.0 };
        (
            (a..=b).map(|i| self.point(i)).collect(),
            (a..=b)
                .map(|i| ((self.source_at(i) - ua) / width).clamp(0.0, 1.0))
                .collect(),
        )
    }

    /// The exact source curve between two samples: whole segments where both ends lie
    /// outside, split where a span begins or ends inside one. This is what a span falls
    /// back to, so falling back costs fidelity nothing at all.
    fn source(&self, lo: usize, hi: usize) -> Vec<Segment> {
        let (ua, ub) = (self.source_at(lo), self.source_at(hi));
        let mut out = Vec::new();
        let mut k = ua.floor() as usize;
        while (k as f64) < ub - 1e-12 {
            let t0 = (ua - k as f64).max(0.0);
            let t1 = (ub - k as f64).min(1.0);
            if t1 - t0 > 1e-9 {
                out.push(self.run[k % self.run.len()].piece(t0, t1).segment());
            }
            k += 1;
        }
        out
    }

    /// Which sample each fitted segment ends on. The program places its vertices on the
    /// points it was given, so the nearest sample to each end is that vertex; the walk is
    /// forward-only, and the last end is the span's end by construction. `None` when the
    /// ends do not march through the span -- then the caller does not trust the fit.
    fn locate(&self, curve: &FittedPath, lo: usize, hi: usize, looped: bool) -> Option<Vec<usize>> {
        let nearest = |from: usize, to: usize, p: Point| {
            (from..=to).min_by(|&x, &y| self.point(x).dist(p).total_cmp(&self.point(y).dist(p)))
        };
        let first = if looped {
            nearest(lo, hi - 1, curve.start)?
        } else {
            lo
        };
        let last = first + (hi - lo);
        let mut ends = Vec::with_capacity(curve.segments.len() + 1);
        ends.push(first);
        let mut i = first;
        for (k, seg) in curve.segments.iter().enumerate() {
            let j = if k + 1 == curve.segments.len() {
                last
            } else {
                nearest(i + 1, last, seg.end())?
            };
            if j <= i || j > last {
                return None;
            }
            ends.push(j);
            i = j;
        }
        (i == last).then_some(ends)
    }

    /// Samples `lo..=hi` described within the tolerance: the start point and the segments.
    ///
    /// The program fits the span at `caps[0]` points. Each fitted segment is then judged
    /// against the samples it stands for; one that strays is fitted again on its own at
    /// the next cap -- a short span, where the square in the cost is cheap -- and past the
    /// last cap it is replaced by the exact source it covers.
    ///
    /// Judging the run as a whole instead is what the first version did, and on the
    /// corpus's heaviest file it threw away 62 runs entirely for one bad segment each.
    fn fit(&self, lo: usize, hi: usize, looped: bool, caps: &[usize]) -> (Point, Vec<Segment>) {
        let count = if looped { hi - lo } else { hi - lo + 1 };
        let exact = || (self.point(lo), self.source(lo, hi));
        let Some((&cap, rest)) = caps.split_first() else {
            return exact();
        };
        if count < 4 {
            return exact();
        }
        let points: Vec<Point> = (lo..lo + count).map(|i| self.point(i)).collect();
        let poly = Polyline::new(points, vec![self.eps; count], looped);
        let curve =
            inkvec_fit::multimodel::with_dp_max_points(cap, || optimal_multimodel(&poly, self.cfg));
        // Shown every point it has, the next cap would see exactly the same curve.
        let exhausted = count <= cap;
        let Some(ends) = self.locate(&curve, lo, hi, looped) else {
            return if exhausted {
                exact()
            } else {
                self.fit(lo, hi, looped, rest)
            };
        };
        let mut out = Vec::with_capacity(curve.segments.len());
        let mut start = curve.start;
        for (k, seg) in curve.segments.iter().enumerate() {
            let (a, b) = (ends[k], ends[k + 1]);
            let (span, along) = self.span(a, b);
            let seg = refit_cubic(seg, start, &span, along);
            let dev = span
                .iter()
                .map(|&p| dist_to_segment(p, &seg, start))
                .fold(0.0, f64::max);
            // The tolerance is a promise to the reader, not a suggestion to the fitter.
            if dev <= 3.0 * self.eps {
                out.push(seg.clone());
            } else if exhausted && b - a >= count - 1 {
                out.extend(self.source(a, b));
            } else {
                out.extend(self.fit(a, b, false, rest).1);
            }
            start = seg.end();
        }
        (curve.start, out)
    }

    /// How far the fitted curve wanders from the samples it claims to describe.
    ///
    /// Measuring the other direction -- every sample close to the curve -- is not the same
    /// promise and does not imply this one: a cubic can bow right out of the drawing and
    /// come back, and every sample will still find some part of it nearby. Merging two
    /// segments into one is exactly where that happens, and unchecked it cost 0.68 dE00 on
    /// a corpus the rest of this tool keeps under 0.01.
    fn wanders(&self, seg: &Segment, from: Point, a: usize, b: usize) -> f64 {
        let m = 16;
        (0..=m)
            .map(|i| {
                let p = eval_segment(seg, from, f64::from(i) / f64::from(m));
                (a..b)
                    .map(|j| point_segment_dist(p, self.point(j), self.point(j + 1)))
                    .fold(f64::INFINITY, f64::min)
            })
            .fold(0.0, f64::max)
    }

    /// Merge neighbouring segments into one cubic while it pays, and while the merge stays
    /// inside the tolerance.
    ///
    /// The program calls its own answer optimal, and it is -- for its alphabet, where an
    /// arc costs three parameters and every cubic six. SVG is not that alphabet: an arc is
    /// five numbers there, and a cubic continuing the one before it smoothly is four. So a
    /// description that is optimal to *fit* can still be cheaper to *write*, and this says
    /// so in the only terms that matter here. One cubic exactly subdivided in two comes
    /// back as two arcs and a cubic, sixteen numbers for a curve that needs six.
    fn merge_pairs(&self, path: FittedPath, lo: usize, hi: usize) -> FittedPath {
        if path.segments.len() < 2 {
            return path;
        }
        let debug = std::env::var_os("INKVEC_SVGMIN_DEBUG").is_some();
        let Some(mut ends) = self.locate(&path, lo, hi, path.closed) else {
            if debug {
                eprintln!("    merge: cannot locate {} segments", path.segments.len());
            }
            return path;
        };
        let FittedPath {
            start,
            mut segments,
            closed,
        } = path;
        loop {
            let mut heads = Vec::with_capacity(segments.len());
            let mut cur = start;
            for s in &segments {
                heads.push(cur);
                cur = s.end();
            }
            let mut candidates: Vec<(usize, Segment, f64)> = Vec::new();
            let here = params_of(&segments);
            for k in 0..segments.len() - 1 {
                let (a, b) = (ends[k], ends[k + 2]);
                let (p0, p1) = (heads[k], segments[k + 1].end());
                let (span, along) = self.span(a, b);
                if span.len() < 4 {
                    continue; // too few witnesses to hold a cubic to anything
                }
                let Some((c1, c2)) = ls_cubic_from(&span, p0, p1, along) else {
                    continue;
                };
                let merged = Segment::Cubic(c1, c2, p1);
                let dev = span
                    .iter()
                    .map(|&p| dist_to_segment(p, &merged, p0))
                    .fold(0.0, f64::max);
                if dev > 3.0 * self.eps {
                    continue;
                }
                let mut trial = segments.clone();
                trial.splice(k..=k + 1, [merged.clone()]);
                let saved = here - params_of(&trial);
                if saved > 0.0 {
                    candidates.push((k, merged, saved));
                }
            }
            // The expensive half of the promise is checked on the best candidate first,
            // and only until one keeps it: sweeping the whole curve for every pair, every
            // round, cost more than the fit it was checking.
            candidates.sort_by(|a, b| b.2.total_cmp(&a.2));
            let taken = candidates.into_iter().find(|(k, merged, _)| {
                let (a, b) = (ends[*k], ends[*k + 2]);
                let wander = self.wanders(merged, heads[*k], a, b);
                if debug {
                    eprintln!(
                        "    merge {k}+{}: samples {a}..{b}, wander {wander:.5}, eps {:.5}",
                        k + 1,
                        self.eps
                    );
                }
                wander <= 3.0 * self.eps
            });
            let Some((k, merged, _)) = taken else {
                break;
            };
            segments.splice(k..=k + 1, [merged]);
            ends.remove(k + 1);
        }
        FittedPath {
            start,
            segments,
            closed,
        }
    }
}
/// The cheapest description of one run within `eps`, or `None` when the source is already
/// as cheap (the caller then keeps the source exactly as it was written).
fn fit_run(
    run: &[Src],
    eps: f64,
    closed: bool,
    cfg: &FitConfig,
) -> Option<(FittedPath, Option<PrimitiveKind>)> {
    // The program decides structure; the exact refit and the per-segment guard decide
    // fidelity. Both see the curve sampled two tolerances apart.
    let (pts, at) = sample_mapped(run, 2.0 * eps, closed);
    let n = pts.len();
    if n < 4 {
        return None;
    }
    let source: Vec<Segment> = run.iter().map(Src::segment).collect();
    let before = params_of(&source);
    let fit = RunFit {
        run,
        pts,
        at,
        eps,
        cfg,
    };
    let (lo, hi) = if closed { (0, n) } else { (0, n - 1) };

    // The program costs O(points x span) and its early cut-off never fires on smooth,
    // exact input, so how many points it is shown decides what the run costs to fit. A
    // run a document would actually contain is shown all of them. Only the pathological
    // ones -- a 1,200-segment emoji outline -- are shown at most `COARSE_CAP`, decimated
    // by bends so their corners survive, and repaired span by span afterwards.
    //
    // Decimating is not free: it places the segment *boundaries* by a coarse view of the
    // curve, and repairing a boundary is not something the repair can do. Measured on the
    // 40-file batch, fitting whole where it is affordable is worth 1.6% of the whole
    // description.
    let coarse_cap = knob("INKVEC_SVGMIN_COARSE_CAP", COARSE_CAP as f64) as usize;
    let whole_below = knob("INKVEC_SVGMIN_WHOLE_BELOW", WHOLE_BELOW as f64) as usize;
    let (mut start, mut segments) = fit.fit(lo, hi, closed, &[coarse_cap, FINE_CAP]);
    if n > coarse_cap && n <= whole_below {
        // Neither view dominates: the coarse one can place a boundary where the whole
        // view would not, and the other way about. Both are within the tolerance, so the
        // cheaper one is simply the answer.
        let (s, whole) = fit.fit(lo, hi, closed, &[FINE_CAP]);
        if params_of(&whole) < params_of(&segments) {
            start = s;
            segments = whole;
        }
    }
    if knob("INKVEC_SVGMIN_MERGE", 1.0) > 0.0 {
        let path = fit.merge_pairs(
            FittedPath {
                start,
                segments,
                closed,
            },
            lo,
            hi,
        );
        start = path.start;
        segments = path.segments;
    }

    // The program was offered an arc per span; this offers one description of the whole
    // run, which is how a circle is found at all. It is shown a thinned copy of the
    // points, costing O(n²) as well, and judged the way everything here is judged: by what
    // it costs to write, within the same guard.
    //
    // Not by the fitter's own objective, which was the first draft's mistake: once the
    // cubics are repaired span by span they fit so well that `0.5·chi² + λ·params` prefers
    // them to four arcs that cost a third as much and are inside the tolerance.
    let mut primitive = None;
    if !segments.is_empty() && (closed || knob("INKVEC_SVGMIN_PRIM_OPEN", 1.0) > 0.0) {
        let stride = n.div_ceil(knob("INKVEC_SVGMIN_PRIM_CAP", PRIMITIVE_CAP as f64) as usize);
        let thin: Vec<Point> = fit.pts.iter().copied().step_by(stride.max(1)).collect();
        let sigma = vec![eps; thin.len()];
        if let Some((segs, prim, _)) = fit_primitive_or_arcs(&thin, &sigma, closed, cfg) {
            let candidate = FittedPath {
                start: fit.pts[0],
                segments: segs,
                closed,
            };
            // A whole primitive is written as its own element, cheaper than the path form
            // counted here, so it wins a tie.
            let cost = params_of(&candidate.segments);
            let win = if prim.is_some() {
                cost <= params_of(&segments)
            } else {
                cost < params_of(&segments)
            };
            if win && max_deviation(&fit.pts, &candidate) <= 3.0 * eps {
                primitive = prim.map(|p| p.kind);
                start = candidate.start;
                segments = candidate.segments;
            }
        }
    }

    let after = params_of(&segments);
    if std::env::var_os("INKVEC_SVGMIN_DEBUG").is_some() {
        let kinds: String = segments
            .iter()
            .map(|s| match s {
                Segment::Line(_) => 'L',
                Segment::Cubic(..) => 'C',
                Segment::Arc { .. } => 'A',
            })
            .collect();
        eprintln!(
            "  run: {} src segs, {n} samples, eps {eps:.5}, closed {closed} -> {} segs {kinds} params {before:.0}->{after:.0}",
            run.len(),
            segments.len(),
        );
    }
    if segments.is_empty() || after >= before {
        return None;
    }
    Some((
        FittedPath {
            start,
            segments,
            closed,
        },
        primitive,
    ))
}
/// A closed subpath of four axis-aligned lines is a `<rect>`, whatever the fitter would
/// make of it: its corners are exact, so nothing is fitted -- it is read off the source.
fn sharp_rect(sp: &Subpath, eps: f64) -> Option<PrimitiveKind> {
    if !sp.closed || sp.segs.len() != 4 {
        return None;
    }
    let mut xs = Vec::with_capacity(4);
    let mut ys = Vec::with_capacity(4);
    for s in &sp.segs {
        let Src::Line(a, b) = *s else {
            return None;
        };
        if (a.x - b.x).abs() > eps && (a.y - b.y).abs() > eps {
            return None; // not axis-aligned
        }
        xs.push(a.x);
        ys.push(a.y);
    }
    let (x0, x1) = (
        xs.iter().cloned().fold(f64::INFINITY, f64::min),
        xs.iter().cloned().fold(f64::NEG_INFINITY, f64::max),
    );
    let (y0, y1) = (
        ys.iter().cloned().fold(f64::INFINITY, f64::min),
        ys.iter().cloned().fold(f64::NEG_INFINITY, f64::max),
    );
    // Every vertex on the box's corners, and both widths used: a real rectangle, not a
    // degenerate zigzag.
    let on_corner = xs.iter().zip(&ys).all(|(&x, &y)| {
        ((x - x0).abs() <= eps || (x - x1).abs() <= eps)
            && ((y - y0).abs() <= eps || (y - y1).abs() <= eps)
    });
    (on_corner && x1 - x0 > eps && y1 - y0 > eps).then_some(PrimitiveKind::RoundRect {
        x: x0,
        y: y0,
        w: x1 - x0,
        h: y1 - y0,
        rx: 0.0,
    })
}
/// One subpath rewritten: `(start, segments, guarded_runs, primitive)`, the primitive
/// being `Some` when one element describes the whole subpath; `None` if nothing in it
/// got cheaper.
pub(crate) fn minify_subpath(
    sp: &Subpath,
    eps: f64,
    cfg: &FitConfig,
    corner_degrees: f64,
) -> Option<(Point, Vec<Segment>, usize, Option<PrimitiveKind>)> {
    let n = sp.segs.len();
    if n == 0 {
        return None;
    }
    if let Some(rect) = sharp_rect(sp, eps) {
        let segs: Vec<Segment> = sp.segs.iter().map(Src::segment).collect();
        return Some((sp.segs[0].start(), segs, 0, Some(rect)));
    }
    let is_corner = corners(sp, corner_degrees);
    let n_corners = is_corner.iter().filter(|&&c| c).count();

    // A closed loop with no corner is fitted whole: the dynamic program places its own
    // vertices and a circle or rounded rectangle can describe all of it.
    if sp.closed && n_corners == 0 {
        return fit_run(&sp.segs, eps, true, cfg).map(|(f, prim)| (f.start, f.segments, 0, prim));
    }

    // Otherwise the corners cut the subpath into runs, each refitted on its own with both
    // ends pinned. A closed subpath is rotated to begin at a corner so no run wraps.
    let first = is_corner.iter().position(|&c| c).unwrap_or(0);
    let order: Vec<usize> = (0..n).map(|i| (first + i) % n).collect();
    let mut runs: Vec<Vec<Src>> = Vec::new();
    for (i, &k) in order.iter().enumerate() {
        if i == 0 || is_corner[k] {
            runs.push(Vec::new());
        }
        runs.last_mut().expect("a run was opened").push(sp.segs[k]);
    }

    let mut improved = false;
    let mut guarded = 0;
    let mut segments = Vec::new();
    for run in &runs {
        match fit_run(run, eps, false, cfg) {
            Some((f, _)) => {
                improved = true;
                segments.extend(f.segments);
            }
            None => {
                if run.iter().map(Src::params).sum::<f64>() > 0.0 && run.len() > 1 {
                    guarded += 1;
                }
                segments.extend(run.iter().map(Src::segment));
            }
        }
    }
    improved.then(|| (runs[0][0].start(), segments, guarded, None))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geom::cubic_at;
    use crate::path::sample;

    #[test]
    fn least_squares_recovers_a_piece_of_an_exact_cubic() {
        // The same subdivided cubic the minifier sees, sampled as it samples it, with a
        // span that ends part-way through the second half.
        let (p0, c1, c2, p3) = ((10.0, 100.0), (30.0, 10.0), (90.0, 10.0), (110.0, 100.0));
        let mid = |a: (f64, f64), b: (f64, f64)| ((a.0 + b.0) / 2.0, (a.1 + b.1) / 2.0);
        let (q1, q2, q3) = (mid(p0, c1), mid(c1, c2), mid(c2, p3));
        let (r1, r2) = (mid(q1, q2), mid(q2, q3));
        let s = mid(r1, r2);
        let pt = |a: (f64, f64)| Point::new(a.0, a.1);
        let run = [
            Src::Cubic(pt(p0), pt(q1), pt(r1), pt(s)),
            Src::Cubic(pt(s), pt(r2), pt(q3), pt(p3)),
        ];
        let pts = sample(&run, 0.025, false);
        let span = &pts[..=38];
        let (a, b) = (span[0], span[span.len() - 1]);
        let along: Vec<f64> = (0..span.len()).map(|i| i as f64 / 38.0).collect();
        let (n1, n2) = ls_cubic_from(span, a, b, along).expect("fits");
        let seg = Segment::Cubic(n1, n2, b);
        let dev = span
            .iter()
            .map(|&p| dist_to_segment(p, &seg, a))
            .fold(0.0, f64::max);
        assert!(dev < 1e-3, "a piece of a cubic is a cubic; missed by {dev}");
    }

    #[test]
    fn least_squares_recovers_an_exact_cubic() {
        let (p0, c1, c2, p3) = (
            Point::new(10.0, 100.0),
            Point::new(30.0, 10.0),
            Point::new(90.0, 10.0),
            Point::new(110.0, 100.0),
        );
        let pts: Vec<Point> = (0..=96)
            .map(|i| cubic_at(p0, c1, c2, p3, i as f64 / 96.0))
            .collect();
        let along: Vec<f64> = (0..=96).map(|i| f64::from(i) / 96.0).collect();
        let (n1, n2) = ls_cubic_from(&pts, p0, p3, along).expect("solvable");
        let worst = pts
            .iter()
            .map(|&p| dist_to_segment(p, &Segment::Cubic(n1, n2, p3), p0))
            .fold(0.0, f64::max);
        assert!(worst < 1e-3, "c1 {n1:?} c2 {n2:?} worst {worst}");
    }
}
