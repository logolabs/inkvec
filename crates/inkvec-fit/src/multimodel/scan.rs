//! The multimodel program's table, filled in parallel without changing a bit of it.
//!
//! [`solve_open`] is a shortest path over the spans `i..j` of one boundary, and its wall
//! time was that of one core scanning them in turn: a single-outline logo or the largest
//! ring of an emoji waited on one thread while the rest of the pool sat idle. Every span
//! costs `base(i) + terms(i, j)`, where only `base(i)`, the cost of reaching `i`, depends
//! on the program's progress, and the search cut-off for a start reads only the terms. So
//! [`SpanScorer::fill`] scans blocks of starts at once, one start per task, keeps the
//! terms, and replays them in order with the sequential program's own expressions: the
//! table, and so the output, is the sequential program's bit for bit whatever the number
//! of threads or how the pool schedules them.
//!
//! Measured on sixteen Studio finals (2048 px, 16 threads, medians of five): the stage
//! `fit_dp` fell to 0.67 of its time over the set (0.34 on a six-ring logo whose largest
//! ring was the whole stage), and the self-intersection repair, which refits undecimated
//! boundaries with the same program, to 0.35.

use super::*;
use crate::candidates::{ArcSpan, EllipseSpan, FreeFit};

/// Polylines shorter than this are solved sequentially: their scans are too short to pay
/// for a fork.
pub(super) const DP_PAR_MIN_POINTS: usize = 128;
/// Starts scanned per thread in one parallel block (see [`SpanScorer::fill`]).
const DP_STARTS_PER_THREAD: usize = 4;
/// Most span candidates one parallel block holds at once, a few tens of megabytes; long
/// uncapped scans (the repair's refits of undecimated boundaries) get smaller blocks.
const DP_BLOCK_SPANS: usize = 1 << 16;

/// Threads the dynamic programs occupy right now: one per running program plus the
/// helpers its current block has claimed. A program forks only onto threads this says
/// are free. When every core already has a ring of its own a fork buys nothing and costs
/// its overhead, and it is the last, largest rings — the ones the stage waits on — that
/// find the pool idle. This decides only where spans are evaluated, never what is
/// chosen, so the output depends neither on it nor on the races in reading it.
static DP_THREADS_BUSY: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// Holds `.0` counts in [`DP_THREADS_BUSY`] for as long as it lives.
pub(super) struct BusyThreads(usize);

impl BusyThreads {
    pub(super) fn claim(k: usize) -> Self {
        DP_THREADS_BUSY.fetch_add(k, std::sync::atomic::Ordering::Relaxed);
        BusyThreads(k)
    }
}

impl Drop for BusyThreads {
    fn drop(&mut self) {
        DP_THREADS_BUSY.fetch_sub(self.0, std::sync::atomic::Ordering::Relaxed);
    }
}

/// How many threads (this one included) should share `units` pieces of work: this one
/// plus every idle thread of a `threads`-thread pool. One means work in turn.
pub(super) fn fork_width(units: usize, threads: usize) -> usize {
    let busy = DP_THREADS_BUSY.load(std::sync::atomic::Ordering::Relaxed);
    let idle = threads.saturating_sub(busy);
    (idle + 1).min(units).max(1)
}

/// The dynamic program's table: for each point, the cheapest cost of reaching it and the
/// last segment of the path that does.
pub(super) struct Table {
    pub(super) best: Vec<f64>,
    pub(super) from: Vec<usize>,
    pub(super) kind: Vec<SegKind>,
    pub(super) arms: Vec<Option<(f64, f64)>>,
    pub(super) tans: Vec<Option<(Vec2, Vec2)>>,
    #[allow(clippy::type_complexity)]
    pub(super) arcs: Vec<Option<(f64, f64, f64, bool, bool)>>,
}

impl Table {
    fn new(n: usize) -> Self {
        let mut best = vec![f64::INFINITY; n];
        best[0] = 0.0;
        Table {
            best,
            from: vec![usize::MAX; n],
            kind: vec![SegKind::Line; n],
            arms: vec![None; n],
            tans: vec![None; n],
            arcs: vec![None; n],
        }
    }

    /// Take the span `i..j` as the way to reach `j` if it is cheaper than the best so far.
    fn offer(&mut self, i: usize, j: usize, s: &SpanCandidate) {
        if s.cost < self.best[j] {
            self.best[j] = s.cost;
            self.from[j] = i;
            self.kind[j] = s.kind;
            self.arms[j] = s.arms;
            self.tans[j] = s.tans;
            self.arcs[j] = s.arc;
        }
    }
}

/// The G1 cubic's share of a span's terms.
struct G1Terms {
    chi2: f64,
    wobble: f64,
    arms: (f64, f64),
}

/// Everything one candidate span `i..j` offers that does not depend on the cost of
/// reaching `i`: the fitted models and their prices above it.
struct SpanTerms {
    chi2_l: f64,
    /// The line's cost, bow penalty included.
    line: f64,
    g1: Option<G1Terms>,
    free: Option<FreeFit>,
    circle: Option<ArcSpan>,
    /// The better of the two cubics' residuals, for the cut-off and the debug dump.
    chi2_c: f64,
    /// Both models' fidelity terms exceed the cut-off bound.
    over: bool,
    /// The ellipse, when it was fitted ahead of its price gate.
    ellipse: Option<Option<EllipseSpan>>,
}

/// Run `f` with no [`with_dp_max_points`] override in force on this thread, then put the
/// caller's back (also on unwind).
///
/// For a thread that blocks on rayon work: while it waits it may steal an unrelated job
/// (another trace's ring fit, say), and that job must see the default cap, not the one
/// the waiting caller set for its own fit.
fn without_dp_cap_override<T>(f: impl FnOnce() -> T) -> T {
    struct Restore(Option<usize>);
    impl Drop for Restore {
        fn drop(&mut self) {
            DP_CAP_OVERRIDE.with(|c| c.set(self.0));
        }
    }
    let _restore = Restore(DP_CAP_OVERRIDE.with(|c| c.take()));
    f()
}

/// What one candidate span `i..j` offers the dynamic program: its cheapest model, and
/// what the search cut-off and the debug dump read.
struct SpanCandidate {
    cost: f64,
    kind: SegKind,
    arms: Option<(f64, f64)>,
    tans: Option<(Vec2, Vec2)>,
    #[allow(clippy::type_complexity)]
    arc: Option<(f64, f64, f64, bool, bool)>,
    /// Both models' fidelity terms exceed the cut-off bound.
    over: bool,
    chi2_l: f64,
    chi2_c: f64,
    line: f64,
}

/// Prices the candidate spans of one [`solve_open`] run.
///
/// Split in two so the program can run ahead of itself: [`SpanScorer::terms`] is
/// everything a span offers that does not depend on the cost of reaching its start, and
/// [`SpanScorer::resolve`] adds that cost with the sequential program's own expressions,
/// evaluated in the same order.
pub(super) struct SpanScorer<'a> {
    pts: &'a [Point],
    sigma: &'a [f64],
    tan: &'a Tangents,
    pre: &'a Prefix,
    cfg: &'a FitConfig,
    joins_at_ends: bool,
    circles: Option<CirclePrefix>,
    cubic_floor: f64,
    arc_floor: f64,
    ellipse_floor: f64,
    debug: bool,
}

impl<'a> SpanScorer<'a> {
    pub(super) fn new(
        pts: &'a [Point],
        sigma: &'a [f64],
        tan: &'a Tangents,
        pre: &'a Prefix,
        cfg: &'a FitConfig,
        joins_at_ends: bool,
    ) -> Self {
        SpanScorer {
            pts,
            sigma,
            tan,
            pre,
            cfg,
            joins_at_ends,
            circles: Some(CirclePrefix::new(pts, sigma)),
            cubic_floor: cfg.lambda * params_cubic(),
            arc_floor: cfg.lambda * crate::curves::PARAMS_ARC,
            ellipse_floor: cfg.lambda * crate::curves::PARAMS_ELLIPTICAL_ARC,
            debug: dp_debug(),
        }
    }

    /// An ellipse is asked about only where a circle has not already described the span
    /// — it is two parameters dearer, and a boundary a circle fits is not an ellipse's to
    /// claim. "Has not" includes the circle declining outright: a strongly elliptical run
    /// is exactly the shape whose angles about a *circle's* centre do not advance
    /// monotonically, so the circular candidate returns nothing and the ellipse has to be
    /// reached anyway. (The price gate that follows this is in [`Self::resolve`].)
    fn ellipse_asked(i: usize, j: usize, circle: Option<&ArcSpan>) -> bool {
        !circle.is_some_and(|f| f.chi2 <= 4.0 * (j - i) as f64) && j >= i + 2
    }

    fn ellipse(&self, i: usize, j: usize) -> Option<EllipseSpan> {
        try_ellipse(
            self.pts,
            self.sigma,
            self.tan,
            i,
            j,
            self.cfg,
            self.joins_at_ends,
        )
    }

    /// Everything the span `i..j` offers that does not depend on the cost of reaching
    /// `i`. With `speculate`, the ellipse is also fitted wherever its price gate might
    /// pass, so a scan run ahead of the program has it ready.
    ///
    /// Inlined, with [`Self::resolve`], so the sequential scan (one thread, a busy pool,
    /// the single-threaded WebAssembly build) fuses the two and never materialises the
    /// terms: left to the compiler it measured several per cent slower than the loop this
    /// replaced.
    #[inline(always)]
    fn terms(&self, i: usize, j: usize, speculate: bool) -> SpanTerms {
        let (pts, sigma, tan, pre, cfg) = (self.pts, self.sigma, self.tan, self.pre, self.cfg);
        let cubic_floor = self.cubic_floor;
        let t0 = tan.outgoing[i];
        let chi2_l = pre.chi2_line(i, j);
        let line_plain = line_cost_terms(pts, tan, i, j, chi2_l, cfg, self.joins_at_ends);
        // The circle is asked for first, because what it finds is evidence about the
        // line: see `bow_penalty`. It is O(1) from the moment sums, so asking costs
        // nothing but the guards.
        // The price floor is a proof, not a heuristic: an arc costs at least its own
        // 5 lambda, so a span the line already covers for less can never take one.
        let circle = if j >= i + 2 && (line_plain > self.arc_floor || chi2_l > (j - i) as f64) {
            self.circles
                .as_ref()
                .and_then(|c| try_arc(pts, tan, c, i, j, cfg, self.joins_at_ends))
        } else {
            None
        };
        let line = line_plain
            + circle
                .as_ref()
                .map_or(0.0, |f| bow_penalty(chi2_l, f.chi2, j - i));

        // A cubic needs an interior point to be worth anything, and costs `6λ` before
        // any residual: if the line already costs less, the residual is never
        // evaluated. This is the O(1) pre-check that keeps straight runs cheap.
        let mut g1 = None;
        let mut free = None;
        let mut chi2_c = f64::INFINITY;
        let mut cubic_tried = false;
        if j >= i + 2 && line > cubic_floor {
            cubic_tried = true;
            if let Some((chi2, d0, d1)) = best_cubic(
                pts,
                sigma,
                &pre.s,
                i,
                j,
                t0,
                tan.incoming[j],
                pre.raw_moments(i, j),
                true,
            ) {
                chi2_c = chi2;
                let chord = (pts[j] - pts[i]).norm();
                let cb = Cubic::from_arms(pts[i], pts[j], t0, tan.incoming[j], chord, d0, d1);
                g1 = Some(G1Terms {
                    chi2,
                    wobble: cb.wobble_penalty(cfg.lambda),
                    arms: (d0, d1),
                });
            } else {
                cubic_tried = false; // no admissible arms: not evidence of hopelessness
            }

            // The same span with the tangent directions fitted rather than
            // inherited. It gives up G1 with its neighbours, so it pays for the two
            // breaks it makes, and only wins if the residual it saves is worth more
            // than the smoothness it costs.
            free = try_free_cubic(
                pts,
                sigma,
                &pre.s,
                i,
                j,
                t0,
                tan.incoming[j],
                cfg.lambda,
                true,
            );
            if let Some(f) = &free {
                if f.chi2 < chi2_c {
                    chi2_c = f.chi2;
                }
            }
        }

        // Search cut-off derived from the objective (see `optimal_polygon`): covering
        // `i..j` with the finest segmentation costs at least `2λ(j−i)`, so once both
        // models' fidelity terms alone exceed that by the slack, no longer span from
        // `i` can win. Both models must be over the bound — a line blows up at the
        // first bend while the cubic is still fine.
        let floor = PRUNE_SLACK * cfg.lambda * PARAMS_LINE * (j - i) as f64;
        let over = 0.5 * chi2_l > floor && cubic_tried && 0.5 * chi2_c > floor;

        // The gate compares the best candidate's cost above `base` with the ellipse's
        // floor. Ahead of the program `base` is unknown, and that difference is the
        // candidate's own price up to rounding in `base`'s last bits, so fit it wherever
        // the gate might pass with a margin far wider than the rounding. `resolve`
        // applies the exact gate, and fits the ellipse itself if the margin ever missed.
        let ellipse = if speculate && Self::ellipse_asked(i, j, circle.as_ref()) {
            let mut m = line;
            if let Some(g) = &g1 {
                m = m.min(0.5 * g.chi2 + cubic_floor + g.wobble);
            }
            if let Some(f) = &free {
                m = m.min(0.5 * f.chi2 + cubic_floor + f.brk);
            }
            if let Some(f) = &circle {
                m = m.min(f.cost);
            }
            (m > self.ellipse_floor - 1e-6 * (1.0 + m.abs())).then(|| self.ellipse(i, j))
        } else {
            None
        };

        SpanTerms {
            chi2_l,
            line,
            g1,
            free,
            circle,
            chi2_c,
            over,
            ellipse,
        }
    }

    /// The terms of every span from `i` that the sequential program would evaluate, in
    /// order: up to `end`, or to the cut-off, which reads only the residuals.
    fn scan(&self, i: usize, end: usize) -> Vec<SpanTerms> {
        let mut scan = Vec::new();
        let mut over = 0usize;
        for j in i + 1..end {
            let t = self.terms(i, j, true);
            let o = t.over;
            scan.push(t);
            if o {
                over += 1;
                if over >= PRUNE_PATIENCE {
                    break;
                }
            } else {
                over = 0;
            }
        }
        scan
    }

    /// The span's cheapest model once `base`, the cost of reaching `i` and leaving it, is
    /// known.
    #[inline(always)]
    fn resolve(&self, i: usize, j: usize, base: f64, t: SpanTerms) -> SpanCandidate {
        let cubic_floor = self.cubic_floor;
        let mut c = base + t.line;
        let mut k = SegKind::Line;
        let mut a = None;
        let mut tv: Option<(Vec2, Vec2)> = None;
        let mut arc: Option<(f64, f64, f64, bool, bool)> = None;

        if let Some(g) = &t.g1 {
            let cc = base + 0.5 * g.chi2 + cubic_floor + g.wobble;
            if cc < c {
                c = cc;
                k = SegKind::Cubic;
                a = Some(g.arms);
            }
        }
        if let Some(f) = &t.free {
            let cc = base + 0.5 * f.chi2 + cubic_floor + f.brk;
            if cc < c {
                c = cc;
                k = SegKind::Cubic;
                a = Some(f.arms);
                tv = Some(f.tans);
            }
        }

        // The arc, tried on the same terms as the cubic: only once the line is
        // already paying more than the arc's price, so straight runs never fit a circle.
        if let Some(f) = &t.circle {
            let cc = base + f.cost;
            if cc < c {
                c = cc;
                k = SegKind::Arc;
                a = None;
                tv = Some(f.tans);
                arc = Some((f.radius, f.radius, 0.0, f.large_arc, f.sweep));
            }
        }

        // And the same proof, sharpened: what an ellipse has to beat is the best
        // candidate so far, not the line. A cubic that already covers the span for
        // less than an ellipse's seven lambda settles it without a conic being fitted
        // at all, which is most spans of a letterform — measured on a 768-px wordmark,
        // the ellipse was 635 ms of the fitter's 1439.
        if Self::ellipse_asked(i, j, t.circle.as_ref()) && c - base > self.ellipse_floor {
            let e = match t.ellipse {
                Some(e) => e,
                None => self.ellipse(i, j),
            };
            if let Some(e) = e {
                let cc = base + e.cost;
                if cc < c {
                    c = cc;
                    k = SegKind::Arc;
                    a = None;
                    tv = Some(e.tans);
                    arc = Some((e.rx, e.ry, e.phi, e.large_arc, e.sweep));
                }
            }
        }

        SpanCandidate {
            cost: c,
            kind: k,
            arms: a,
            tans: tv,
            arc,
            over: t.over,
            chi2_l: t.chi2_l,
            chi2_c: t.chi2_c,
            line: t.line,
        }
    }

    /// Offers one span's candidate to the table, in span order; false once the cut-off
    /// fires for this start.
    fn step(
        &self,
        tab: &mut Table,
        over: &mut usize,
        i: usize,
        j: usize,
        s: SpanCandidate,
    ) -> bool {
        tab.offer(i, j, &s);

        // Why does a line win where the boundary curves? Dumps the two models' own
        // numbers for every span considered, so the answer comes from the program
        // rather than from a story about it.
        if self.debug && j >= i + 2 {
            println!(
                "DP {i} {j} span {} line_chi2 {:.3} cubic_chi2 {:.3} line_cost {:.3} cubic_cost {:.3} chose {}",
                j - i,
                s.chi2_l,
                s.chi2_c,
                s.line,
                if s.chi2_c.is_finite() {
                    0.5 * s.chi2_c + self.cubic_floor
                } else {
                    f64::INFINITY
                },
                if matches!(s.kind, SegKind::Cubic) { "cubic" } else { "line" }
            );
        }

        if s.over {
            *over += 1;
            *over < PRUNE_PATIENCE
        } else {
            *over = 0;
            true
        }
    }
}

impl SpanScorer<'_> {
    /// The program's table for spans of at most `max_span` points.
    ///
    /// `width(left)`, asked before each block with `left` the starts still to go, says
    /// how many threads to share the next block of starts with; one means scan the next
    /// start alone, here.
    ///
    /// A block scans its starts at once, one start per task. What a scan computes
    /// ([`Self::terms`]) and where it stops (the cut-off reads only the residuals) do not
    /// depend on the cost of reaching its start, so the scans can run before the program
    /// has reached them; the program then replays each start's spans in order on this
    /// thread, which makes the table identical to the sequential program's whatever the
    /// widths. No span is evaluated that the sequential program would not evaluate, except
    /// the ellipses fitted ahead of a gate that then fails, and the scans of starts that
    /// turn out unreachable.
    pub(super) fn fill(&self, max_span: usize, mut width: impl FnMut(usize) -> usize) -> Table {
        let n = self.pts.len();
        let mut tab = Table::new(n);
        let jend = |i: usize| (i.saturating_add(max_span).saturating_add(1)).min(n);
        // Leaving `i` makes it a vertex; that is when its turn is paid.
        let base_of = |tab: &Table, i: usize| {
            tab.best[i]
                + if i > 0 {
                    vertex_cost(self.tan, i, self.cfg)
                } else {
                    0.0
                }
        };

        let mut i = 0;
        while i < n - 1 {
            let left = n - 1 - i;
            let w = width(left).clamp(1, left);
            if w < 2 {
                if tab.best[i].is_finite() {
                    let base = base_of(&tab, i);
                    let mut over = 0usize;
                    for j in i + 1..jend(i) {
                        let s = self.resolve(i, j, base, self.terms(i, j, false));
                        if !self.step(&mut tab, &mut over, i, j, s) {
                            break;
                        }
                    }
                }
                i += 1;
                continue;
            }

            // Enough starts to keep `w` threads busy, and no more spans held at once than
            // the budget allows.
            let spans = jend(i) - i;
            let b = (w * DP_STARTS_PER_THREAD)
                .min((DP_BLOCK_SPANS / spans.max(1)).max(w))
                .min(left);
            let scans: Vec<Vec<SpanTerms>> = without_dp_cap_override(|| {
                use rayon::prelude::*;
                let _helpers = BusyThreads::claim(w - 1);
                (i..i + b)
                    .into_par_iter()
                    .with_min_len(b.div_ceil(w))
                    .map(|s| self.scan(s, jend(s)))
                    .collect()
            });
            for (s, scan) in (i..i + b).zip(scans) {
                if !tab.best[s].is_finite() {
                    continue;
                }
                let base = base_of(&tab, s);
                let mut over = 0usize;
                for (j, t) in (s + 1..).zip(scan) {
                    let c = self.resolve(s, j, base, t);
                    if !self.step(&mut tab, &mut over, s, j, c) {
                        break;
                    }
                }
            }
            i += b;
        }
        tab
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A long open boundary with a bit of everything the alphabet has a model for: a
    /// straight run, a circular bend, an elliptical sweep, a wobble and a corner.
    fn boundary() -> Polyline {
        let mut pts = Vec::new();
        for k in 0..60 {
            pts.push(Point::new(k as f64, 0.0));
        }
        for k in 0..80 {
            let t = k as f64 / 80.0 * std::f64::consts::PI;
            pts.push(Point::new(60.0 + 25.0 * t.sin(), 25.0 - 25.0 * t.cos()));
        }
        for k in 0..120 {
            let t = k as f64 / 120.0 * std::f64::consts::PI;
            pts.push(Point::new(
                60.0 - 70.0 * t.sin(),
                50.0 + 18.0 * (1.0 - t.cos()),
            ));
        }
        for k in 0..90 {
            let x = 60.0 - k as f64 * 0.9;
            pts.push(Point::new(x, 86.0 + 3.0 * (k as f64 * 0.35).sin()));
        }
        for k in 0..40 {
            pts.push(Point::new(-21.0, 86.0 - k as f64 * 1.3));
        }
        let n = pts.len();
        Polyline::new(pts, vec![0.35; n], false)
    }

    fn bits(t: &Table) -> Vec<(u64, usize, SegKind, String)> {
        (0..t.best.len())
            .map(|j| {
                let payload = format!(
                    "{:?} {:?} {:?}",
                    t.arms[j].map(|(a, b)| (a.to_bits(), b.to_bits())),
                    t.tans[j].map(|(a, b)| [a.x, a.y, b.x, b.y].map(f64::to_bits)),
                    t.arcs[j].map(|(a, b, c, l, s)| (a.to_bits(), b.to_bits(), c.to_bits(), l, s)),
                );
                (t.best[j].to_bits(), t.from[j], t.kind[j], payload)
            })
            .collect()
    }

    #[test]
    fn blocks_of_starts_fill_the_sequential_table_bit_for_bit() {
        let poly = boundary();
        let cfg = FitConfig::default();
        let tan = estimate_tangents(&poly, &cfg);
        let pre = Prefix::new(&poly.points, &poly.sigma);
        for joins_at_ends in [false, true] {
            let sc = SpanScorer::new(&poly.points, &poly.sigma, &tan, &pre, &cfg, joins_at_ends);
            for max_span in [usize::MAX, 45] {
                let seq = bits(&sc.fill(max_span, |_| 1));
                assert!(seq.iter().all(|r| f64::from_bits(r.0).is_finite()));
                for w in [2, 16] {
                    let par = bits(&sc.fill(max_span, |_| w));
                    assert!(
                        par == seq,
                        "width {w}, max_span {max_span}, joins {joins_at_ends}"
                    );
                }
                // Widths that change from block to block, as a busy pool's do.
                let mut k = 0usize;
                let mixed = bits(&sc.fill(max_span, |_| {
                    k += 1;
                    [1, 5, 2, 1, 9][k % 5]
                }));
                assert!(mixed == seq, "mixed widths, max_span {max_span}");
            }
        }
    }

    #[test]
    fn an_ellipse_fitted_ahead_resolves_as_one_fitted_on_demand() {
        let poly = boundary();
        let cfg = FitConfig::default();
        let tan = estimate_tangents(&poly, &cfg);
        let pre = Prefix::new(&poly.points, &poly.sigma);
        let sc = SpanScorer::new(&poly.points, &poly.sigma, &tan, &pre, &cfg, false);
        let key = |s: SpanCandidate| {
            (
                s.cost.to_bits(),
                s.kind,
                format!("{:?} {:?} {:?}", s.arms, s.tans, s.arc),
                s.over,
            )
        };
        let mut ahead = 0;
        for i in (0..poly.len()).step_by(7) {
            for j in (i + 1..poly.len()).step_by(3) {
                if let Some(Some(_)) = sc.terms(i, j, true).ellipse {
                    ahead += 1;
                }
                // A base of zero, and bases whose last bits round the gate's difference.
                for base in [0.0, 1_234.567_890_1, 1.0e6 + 0.1, 3.0e9 + 0.7] {
                    let a = key(sc.resolve(i, j, base, sc.terms(i, j, true)));
                    let b = key(sc.resolve(i, j, base, sc.terms(i, j, false)));
                    assert!(a == b, "span {i}..{j}, base {base}");
                }
            }
        }
        assert!(ahead > 0, "the boundary never asked for an ellipse");
    }
}
