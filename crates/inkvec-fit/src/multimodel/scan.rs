//! The multimodel program's table, filled endpoint by endpoint with provably useless work
//! skipped, in parallel, without changing a bit of it.
//!
//! [`solve_open`] is a shortest path over the spans `i..j` of one boundary. Every span
//! costs `base(i) + terms(i, j)`, where only `base(i)`, the cost of reaching `i`, depends
//! on the program's progress, and the search cut-off for a start reads only the terms.
//!
//! # The recurrence
//!
//! For points `0..n` of the opened polyline, with `best[0] = 0`,
//!
//! ```text
//!     base(i) = best[i] + (i > 0 ? vertex_cost(i) : 0)
//!     best[j] = min over admissible i < j of  base(i) + min_model cost_model(i, j)
//! ```
//!
//! where `cost_model` is the line, G1 cubic, free cubic, circular arc or elliptical arc
//! price of `crate::candidates` (nats), ties going to the smallest `i`, and a span is
//! admissible when [`Limits::allows`] it: at most `max_span` points and no forced vertex
//! strictly inside (both unconstrained in normal fitting; the crossing repair sets them).
//! A forced vertex `f` therefore splits the table: every entry after `f` is reached through
//! `f`, and nothing before `f` can see past it (for the first forced vertex, `best[f]` is the
//! unconstrained optimum of `0..=f`). `from[j]`,
//! `kind[j]` and the fitted parameters record the winner. The scan from a start `i` stops
//! after [`PRUNE_PATIENCE`] consecutive spans whose line and cubic fidelity terms both
//! exceed `PRUNE_SLACK·λ·PARAMS_LINE·(j − i)`. A curved model is fitted only when the line
//! (for the ellipse, the best candidate so far) already costs more than the curved model's
//! parameter floor `λ·P`, which is a proof, not a heuristic, that it could not otherwise
//! win. The one exception is the circle, also fitted wherever the line's residual exceeds
//! one per point, because it is the evidence for the line's bow penalty.
//!
//! # Bounds: work the table can never use
//!
//! Almost every candidate loses. Measured on the 246-icon gate set, 65% of the G1 cubics
//! the start-by-start scan fitted could not beat the table's value at their end *at the
//! moment they were fitted* (84-95% on the flat-art inputs), and 99.8% could not beat its
//! final value. The program only ever reads a candidate through the minimum at `j`, the
//! ellipse's price gate and the cut-off's "over" test, so a candidate whose price floor
//! already reaches the best at `j` needs no residual at all, and one whose residual is
//! being summed can stop the moment the partial sum settles all three. This is branch and
//! bound inside a dynamic program, one candidate at a time:
//!
//! - Morin, T. L. & Marsten, R. E. (1976), "Branch-and-bound strategies for dynamic
//!   programming", *Operations Research* 24(4):611–627, doi:10.1287/opre.24.4.611;
//! - Killick, R., Fearnhead, P. & Eckley, I. A. (2012), "Optimal detection of changepoints
//!   with a linear computational cost", *JASA* 107:1590–1598,
//!   doi:10.1080/01621459.2012.737745 — the per-candidate half of PELT: `F(τ) + C(τ..t)
//!   ≥ F(t)` means `τ` is not the minimiser at `t`. PELT's *permanent* pruning is not
//!   used: it needs their condition (4), which these costs break (a sub-span of a span
//!   with fixed G1 tangents often has no admissible cubic);
//! - Rakthanmanon, T. et al. (2012), "Searching and mining trillions of time series
//!   subsequences under dynamic time warping", *KDD '12* 262–270,
//!   doi:10.1145/2339530.2339576, §4.1.3: early abandoning of a sum of non-negative terms
//!   against the best so far (here: [`crate::candidates::best_cubic_bounded`]).
//!
//! Every bound is a floor on an IEEE expression the program itself evaluates:
//! `fl(fl(base + x) + floor)` is monotone in `base` and `x`, and every residual and
//! penalty is non-negative, so `fl(base + λ·P) ≥ best` proves the model's cost is at least
//! `best` and it cannot win. [`SpanScorer::fill`] scores each endpoint's candidates
//! ("pull"), the likeliest winner first, so the rest meet a bound near the final `F(j)`
//! rather than whatever the table held when their start came round ("push").
//!
//! The cut-off is the one reader these papers do not have. *Not from the literature:*
//! a span whose cubic is dead but whose line is over leaves its "over" answer unknown
//! ([`Over::Unknown`]), and [`CutOff`] asks for it only when the last [`PRUNE_PATIENCE`]
//! spans hold no known "not over", latest first — the same stop, at the same span, as
//! counting every answer, because the stop is the first span ending such a window of
//! "over"s. Simulated on the gate set, the answers it has to look up cost 2% (push order)
//! to 8% (pull order) of the residual work the bounds save.

use super::*;
use crate::candidates::{
    best_cubic_bounded, free_cubic_enabled, Abandon, ArcSpan, EllipseSpan, FreeFit, G1Fit,
};

/// Polylines shorter than this are solved sequentially: their scans are too short to pay
/// for a fork.
pub(super) const DP_PAR_MIN_POINTS: usize = 128;
/// Endpoints scored at once when the candidates are shared between threads (see
/// [`SpanScorer::fill`]): enough work per start to pay for a task, few enough that the
/// first start's spans still bound the block's endpoints well.
const DP_BLOCK_ENDPOINTS: usize = 32;
/// Fewest candidates at one endpoint worth sharing between threads.
pub(super) const DP_PAR_MIN_LIVE: usize = 64;

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
    /// Count `k` more threads as busy until the returned guard is dropped.
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
    /// Cheapest cost (nats) of describing points `0..=j`; infinite where unreached.
    pub(super) best: Vec<f64>,
    /// Start of the last segment on that cheapest path; `usize::MAX` for point 0.
    pub(super) from: Vec<usize>,
    /// That segment's model.
    pub(super) kind: Vec<SegKind>,
    /// Its arm lengths, fractions of the chord, when it is a cubic.
    pub(super) arms: Vec<Option<(f64, f64)>>,
    /// Its own end directions, when it is a free cubic or an arc.
    pub(super) tans: Vec<Option<(Vec2, Vec2)>>,
    /// `(rx, ry, phi, large_arc, sweep)` when it is an arc.
    #[allow(clippy::type_complexity)]
    pub(super) arcs: Vec<Option<(f64, f64, f64, bool, bool)>>,
}

impl Table {
    /// An empty table for `n` points: only point 0 is reached, at cost 0.
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

/// The bound one span is scored against: nothing whose cost provably reaches `best` is
/// worth computing (see the module overview).
#[derive(Debug, Clone, Copy)]
struct Bound {
    /// At most the cost of reaching the span's start and leaving it, `base(i)`.
    base: f64,
    /// At least the table's value at the span's end when the span is offered.
    best: f64,
}

impl Bound {
    /// No bound: every model is live and every residual is summed in full.
    const NONE: Bound = Bound {
        base: 0.0,
        best: f64::INFINITY,
    };

    /// A model priced at least `floor` above `base` cannot be offered: `fl(base + floor)`
    /// is a floor on its cost, by the monotonicity of IEEE addition.
    #[inline]
    fn dead(&self, floor: f64) -> bool {
        self.base + floor >= self.best
    }
}

/// What the search cut-off knows about one span.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Over {
    /// Not over: the line fits within the floor, or no cubic was tried, or the cubic does.
    No,
    /// Both fidelity terms exceed the floor.
    Yes,
    /// The line is over and the cubic, which cannot be offered, was not scored: answered
    /// on demand by [`SpanScorer::cubic_over`].
    Unknown,
}

/// The scan's cut-off, fed one span at a time: the scan from a start stops at the first
/// span that ends [`PRUNE_PATIENCE`] consecutive "over"s, exactly as a counter of every
/// answer would stop it, but [`Over::Unknown`] answers are only looked up when the stop
/// depends on them. *Not from the literature* (see the module overview).
struct CutOff {
    /// The latest span end known not to be over; the start itself before any.
    last_no: usize,
    /// Span ends after `last_no` whose answer is not known yet, oldest first. At most
    /// [`PRUNE_PATIENCE`] of them: a full window is always resolved.
    pending: [usize; PRUNE_PATIENCE],
    /// How many entries of `pending` are real.
    len: usize,
}

impl CutOff {
    /// A cut-off for the scan from start `i`.
    fn new(i: usize) -> Self {
        CutOff {
            last_no: i,
            pending: [0; PRUNE_PATIENCE],
            len: 0,
        }
    }

    /// Record span `j`'s answer; true when the scan must stop after `j`. `resolve(k)`
    /// answers an unknown span `k`: whether it is over.
    ///
    /// Invariant: the spans in `(last_no, j]` are all "over" or unknown. When that window
    /// reaches [`PRUNE_PATIENCE`] spans, the unknowns in it are answered latest first; the
    /// first "not over" moves `last_no` (and the scan goes on), and if there is none the
    /// window is a run of "over"s and this is the counter's stop.
    fn push(&mut self, j: usize, over: Over, mut resolve: impl FnMut(usize) -> bool) -> bool {
        match over {
            Over::No => {
                self.last_no = j;
                self.len = 0;
            }
            Over::Yes => {}
            Over::Unknown => {
                self.pending[self.len] = j;
                self.len += 1;
            }
        }
        if j - self.last_no < PRUNE_PATIENCE {
            return false;
        }
        while self.len > 0 {
            self.len -= 1;
            let k = self.pending[self.len];
            if !resolve(k) {
                self.last_no = k;
                self.len = 0;
                return false;
            }
        }
        true
    }
}

/// The G1 cubic's share of a span's terms.
struct G1Terms {
    /// Residual over the subsampled interior points.
    chi2: f64,
    /// [`Cubic::wobble_penalty`], nats.
    wobble: f64,
    /// Arm lengths as fractions of the chord.
    arms: (f64, f64),
}

/// Everything one candidate span `i..j` offers that does not depend on the cost of
/// reaching `i`: the fitted models and their prices above it.
struct SpanTerms {
    /// The line's total-least-squares residual.
    chi2_l: f64,
    /// The line's cost, bow penalty included (without it when the line and the circle
    /// were both dead, which only lowers a price that cannot be offered anyway).
    line: f64,
    /// The G1 cubic, when one was fitted and the program can use it.
    g1: Option<G1Terms>,
    /// The free cubic (research builds only).
    free: Option<FreeFit>,
    /// The circular arc, when it was tried and accepted.
    circle: Option<ArcSpan>,
    /// The better of the two cubics' residuals, for the debug dump (exact when the dump is
    /// on, since the bounds are then off).
    chi2_c: f64,
    /// The cut-off's answer for this span.
    over: Over,
}

/// The cubics' share of a span's terms (see [`SpanScorer::cubic_terms`]).
struct CubicTerms {
    /// The G1 cubic, when one was fitted and the program can use it.
    g1: Option<G1Terms>,
    /// The free cubic (research builds only).
    free: Option<FreeFit>,
    /// The better residual, for the debug dump and the cut-off.
    chi2_c: f64,
    /// The cut-off's answer for the span.
    over: Over,
}

impl CubicTerms {
    /// No cubic tried: not over.
    const NONE: CubicTerms = CubicTerms {
        g1: None,
        free: None,
        chi2_c: f64::INFINITY,
        over: Over::No,
    };
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
/// what the debug dump reads.
struct SpanCandidate {
    /// `base + ` the winning model's cost, nats.
    cost: f64,
    /// The winning model.
    kind: SegKind,
    /// Its arms, when it is a cubic.
    arms: Option<(f64, f64)>,
    /// Its own end directions, when it is a free cubic or an arc.
    tans: Option<(Vec2, Vec2)>,
    /// Its `(rx, ry, phi, large_arc, sweep)`, when it is an arc.
    #[allow(clippy::type_complexity)]
    arc: Option<(f64, f64, f64, bool, bool)>,
    /// The line's residual, for the debug dump.
    chi2_l: f64,
    /// The better cubic's residual, for the debug dump.
    chi2_c: f64,
    /// The line's cost above `base`, for the debug dump.
    line: f64,
}

/// Prices the candidate spans of one [`solve_open`] run.
///
/// In two steps: [`SpanScorer::terms`] fits the models a span offers, skipping what its
/// [`Bound`] proves useless, and [`SpanScorer::resolve`] adds the cost of reaching the
/// start with the program's own expressions, evaluated in the same order.
pub(super) struct SpanScorer<'a> {
    /// The opened polyline's points (px, centred) and sigmas (px).
    pts: &'a [Point],
    sigma: &'a [f64],
    /// Tangents estimated at every point.
    tan: &'a Tangents,
    /// Line, moment and arc-length prefix sums of the same points.
    pre: &'a Prefix,
    cfg: &'a FitConfig,
    /// Whether both ends of the polyline are joins (an opened loop).
    joins_at_ends: bool,
    /// Moment sums for the circle fit.
    circles: Option<CirclePrefix>,
    /// The parameter price of each curved model, `λ·P`, nats: the least it can cost.
    cubic_floor: f64,
    arc_floor: f64,
    ellipse_floor: f64,
    /// `INKVEC_DPDBG`: dump every span's numbers to stderr.
    debug: bool,
    /// Skip what the table can never use (the module overview). Off for the debug dump,
    /// which prints every residual, and for the research free cubic, whose residual the
    /// cut-off also reads; the table is the same either way.
    bounds: bool,
}

impl<'a> SpanScorer<'a> {
    /// A scorer for one opened polyline; builds the circle moment sums, O(n).
    pub(super) fn new(
        pts: &'a [Point],
        sigma: &'a [f64],
        tan: &'a Tangents,
        pre: &'a Prefix,
        cfg: &'a FitConfig,
        joins_at_ends: bool,
    ) -> Self {
        let debug = dp_debug();
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
            debug,
            bounds: !debug && !free_cubic_enabled(),
        }
    }

    /// The same scorer with every bound off: every candidate fitted and summed in full,
    /// as the program did before the bounds. The table must not change; the tests hold
    /// the two to that.
    #[cfg(test)]
    fn unbounded(mut self) -> Self {
        self.bounds = false;
        self
    }

    /// The bound for a span from a start whose `base` is (at least) as given, ending where
    /// the table holds `best`; [`Bound::NONE`] when bounds are off.
    #[inline]
    fn bound(&self, base: f64, best: f64) -> Bound {
        if self.bounds {
            Bound { base, best }
        } else {
            Bound::NONE
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

    /// The elliptical-arc candidate for `i..j` (see [`try_ellipse`]).
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

    /// The cut-off's per-span floor, `PRUNE_SLACK·λ·PARAMS_LINE·(j − i)` nats: covering
    /// `i..j` with the finest segmentation costs at least `2λ(j−i)`.
    #[inline]
    fn cutoff_floor(&self, i: usize, j: usize) -> f64 {
        PRUNE_SLACK * self.cfg.lambda * PARAMS_LINE * (j - i) as f64
    }

    /// Everything the span `i..j` offers that does not depend on the cost of reaching
    /// `i`, scored against `bd` (see the module overview).
    ///
    /// Inlined, with [`Self::resolve`], so the two fuse and the terms are never
    /// materialised: left to the compiler it measured several per cent slower than the
    /// loop this replaced.
    #[inline(always)]
    fn terms(&self, i: usize, j: usize, bd: Bound) -> SpanTerms {
        let (pts, tan, pre, cfg) = (self.pts, self.tan, self.pre, self.cfg);
        let cubic_floor = self.cubic_floor;
        let chi2_l = pre.chi2_line(i, j);
        let line_plain = line_cost_terms(pts, tan, i, j, chi2_l, cfg, self.joins_at_ends);
        // The circle is asked for first, because what it finds is evidence about the
        // line: see `bow_penalty`. It is O(1) from the moment sums, so asking costs
        // nothing but the guards.
        // The price floor is a proof, not a heuristic: an arc costs at least its own
        // 5 lambda, so a span the line already covers for less can never take one.
        // Its two uses are the arc itself and the line's bow penalty; when the arc's floor
        // and the line without its penalty both reach `best[j]` neither can be offered,
        // and the ellipse, dearer still, cannot either.
        let circle_used = !(bd.dead(self.arc_floor) && bd.dead(line_plain));
        let circle = if circle_used
            && j >= i + 2
            && (line_plain > self.arc_floor || chi2_l > (j - i) as f64)
        {
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

        // Search cut-off derived from the objective (see `optimal_polygon`): covering
        // `i..j` with the finest segmentation costs at least `2λ(j−i)`, so once both
        // models' fidelity terms alone exceed that by the slack, no longer span from
        // `i` can win. Both models must be over the bound — a line blows up at the
        // first bend while the cubic is still fine.
        let floor = self.cutoff_floor(i, j);
        let line_over = 0.5 * chi2_l > floor;

        // A cubic needs an interior point to be worth anything, and costs `6λ` before
        // any residual: if the line already costs less, the residual is never
        // evaluated. This is the O(1) pre-check that keeps straight runs cheap.
        let CubicTerms {
            g1,
            free,
            chi2_c,
            over,
        } = if j >= i + 2 && line > cubic_floor {
            self.cubic_terms(i, j, bd, line_over, floor)
        } else {
            CubicTerms::NONE
        };

        SpanTerms {
            chi2_l,
            line,
            g1,
            free,
            circle,
            chi2_c,
            over,
        }
    }

    /// The two cubics of a span `i..j` the pre-check lets through, scored against `bd`;
    /// `line_over` and `floor` are the cut-off's line test and floor for the span.
    #[inline(always)]
    fn cubic_terms(
        &self,
        i: usize,
        j: usize,
        bd: Bound,
        line_over: bool,
        floor: f64,
    ) -> CubicTerms {
        let (pts, sigma, tan, pre, cfg) = (self.pts, self.sigma, self.tan, self.pre, self.cfg);
        let cubic_floor = self.cubic_floor;
        let mut out = CubicTerms::NONE;
        if bd.dead(cubic_floor) {
            // Its price alone reaches `best[j]`: never offered. Its residual is read
            // only by the cut-off, and only if the line is over too.
            if line_over {
                out.over = Over::Unknown;
            }
            return out;
        }
        // Where an ellipse may still be offered, a dropped cubic must be clear of
        // its gate: `½χ² ≥ (7λ − 6λ) + δ` puts `cost − base` above the ellipse's
        // floor with room for every rounding (δ is 1e-9 relative, rounding 1e-16).
        let gate = if bd.dead(self.ellipse_floor) {
            0.0
        } else {
            2.0 * ((self.ellipse_floor - cubic_floor) + 1e-9 * (bd.best.abs() + cfg.lambda))
        };
        let ab = if bd.best == f64::INFINITY {
            Abandon::NONE
        } else {
            let over_floor = if line_over { floor } else { f64::NEG_INFINITY };
            Abandon::new(bd.base, bd.best, cubic_floor, gate, over_floor)
        };
        let (t0, tj) = (tan.outgoing[i], tan.incoming[j]);
        let raw = pre.raw_moments(i, j);
        match best_cubic_bounded(pts, sigma, &pre.s, i, j, t0, tj, raw, &ab) {
            G1Fit::Untried => {}
            G1Fit::Exact(chi2, d0, d1) => {
                out.chi2_c = chi2;
                let chord = (pts[j] - pts[i]).norm();
                let cb = Cubic::from_arms(pts[i], pts[j], t0, tj, chord, d0, d1);
                out.g1 = Some(G1Terms {
                    chi2,
                    wobble: cb.wobble_penalty(cfg.lambda),
                    arms: (d0, d1),
                });
            }
            // Admissible arms, none of which the table can take: only the cut-off's
            // answer is kept.
            G1Fit::Dead { over } => {
                if line_over && over {
                    out.over = Over::Yes;
                }
            }
        }

        // The same span with the tangent directions fitted rather than inherited. It
        // gives up G1 with its neighbours, so it pays for the two breaks it makes, and
        // only wins if the residual it saves is worth more than the smoothness it costs.
        out.free = try_free_cubic(pts, sigma, &pre.s, i, j, t0, tj, cfg.lambda, true);
        if let Some(f) = &out.free {
            if f.chi2 < out.chi2_c {
                out.chi2_c = f.chi2;
            }
        }
        // "No admissible arms" is not evidence of hopelessness, so an untried G1 cubic
        // never makes a span over, whatever the free cubic found.
        if out.g1.is_some() && line_over && 0.5 * out.chi2_c > floor {
            out.over = Over::Yes;
        }
        out
    }

    /// The cut-off's answer for a span `i..j` left [`Over::Unknown`]: its line is over and
    /// its cubic cannot be offered, so only "is the best root's residual over the floor"
    /// is asked, and each root is summed only until that is settled
    /// ([`Abandon::over_only`]).
    fn cubic_over(&self, i: usize, j: usize) -> bool {
        let floor = self.cutoff_floor(i, j);
        match best_cubic_bounded(
            self.pts,
            self.sigma,
            &self.pre.s,
            i,
            j,
            self.tan.outgoing[i],
            self.tan.incoming[j],
            self.pre.raw_moments(i, j),
            &Abandon::over_only(floor),
        ) {
            G1Fit::Untried => false,
            G1Fit::Exact(chi2, _, _) => 0.5 * chi2 > floor,
            G1Fit::Dead { over } => over,
        }
    }

    /// The span's cheapest model once `base`, the cost of reaching `i` and leaving it, is
    /// known; `best_j` is the table's value at `j` now.
    #[inline(always)]
    fn resolve(&self, i: usize, j: usize, base: f64, best_j: f64, t: SpanTerms) -> SpanCandidate {
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
        // the ellipse was 635 ms of the fitter's 1439. Nor is one fitted whose price
        // alone reaches the table's value at `j`: `offer` would refuse it.
        if Self::ellipse_asked(i, j, t.circle.as_ref())
            && c - base > self.ellipse_floor
            && !self.bound(base, best_j).dead(self.ellipse_floor)
        {
            if let Some(e) = self.ellipse(i, j) {
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
            chi2_l: t.chi2_l,
            chi2_c: t.chi2_c,
            line: t.line,
        }
    }

    /// Offers one span's candidate to the table, and prints it under `INKVEC_DPDBG`.
    fn step(&self, tab: &mut Table, i: usize, j: usize, s: SpanCandidate) {
        tab.offer(i, j, &s);

        // Why does a line win where the boundary curves? Dumps the two models' own
        // numbers for every span considered, so the answer comes from the program
        // rather than from a story about it.
        if self.debug && j >= i + 2 {
            eprintln!(
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
    }
}

/// One start the pull-order fill is still extending: its cost is final, its scan has not
/// hit the cut-off, and its span cap has not run out.
struct Start {
    /// The start's index.
    i: usize,
    /// The cost of reaching it and leaving it, `base(i)`, nats.
    base: f64,
    /// Its scan's cut-off.
    cut: CutOff,
    /// The cut-off fired at the endpoint just scored: this start offers no further span.
    stopped: bool,
}

/// The winning candidate at one endpoint so far: its start and what it offers.
struct Winner {
    i: usize,
    s: SpanCandidate,
}

impl Winner {
    /// Whether `s` from start `i` replaces the winner: cheaper, or as cheap from an earlier
    /// start. That is `offer`'s strict `<` over starts taken in increasing order, the
    /// order the push fill offers them in.
    fn beaten_by(&self, i: usize, s: &SpanCandidate) -> bool {
        s.cost < self.s.cost || (s.cost == self.s.cost && i < self.i)
    }

    /// The bound a span from start `i` is scored against: only a cost below the winner's
    /// (or equal to it, from an earlier start) can replace it. `next_up` turns "at least
    /// the winner's cost" into "above it" for the earlier starts, which win ties.
    fn bound_for(&self, i: usize) -> f64 {
        if i < self.i {
            self.s.cost.next_up()
        } else {
            self.s.cost
        }
    }
}

impl SpanScorer<'_> {
    /// The program's table for the spans `lim` allows: at most `lim.max_span` points, and
    /// none passing over a forced vertex ([`Limits::allows`]).
    ///
    /// Filled endpoint by endpoint ("pull"): every `F(i)` a span from `i` to `j` needs is
    /// final before `j` is scored, and the candidates for `j` are scored against the best
    /// one found so far rather than against whatever the table held when the start came
    /// round ("push"). The span most likely to win — the one from the start that won
    /// `j − 1` — is scored first, so the others meet a bound near `F(j)` itself: measured on
    /// the gate set, 99.8% of the G1 cubics the push order fitted could not beat the final
    /// `F(j)`. The bound is the per-candidate test of Killick, Fearnhead & Eckley (2012,
    /// doi:10.1080/01621459.2012.737745) with the best candidate evaluated first, the
    /// ordering Morin & Marsten (1976, doi:10.1287/opre.24.4.611) recommend for fathoming
    /// (see the module overview).
    ///
    /// The table is the push fill's ([`Self::fill_push`]) bit for bit: each span's price is
    /// the same expression of the same final `base(i)`, a winner is replaced only by a
    /// cheaper span or an equal one from an earlier start, and a span is skipped only when
    /// its bound proves it cannot replace the winner. Each start's cut-off sees its spans
    /// in order, one per endpoint, so the scans stop where they did.
    ///
    /// `width(live)`, asked with the number of candidates, says how many threads to share
    /// them with. Shared, a block of [`DP_BLOCK_ENDPOINTS`] endpoints is scored at once
    /// ([`Self::block`]): one task per start runs through the block against the first
    /// start's spans, and the winners are reduced endpoint by endpoint, so the table does
    /// not depend on the widths either.
    ///
    /// A start refused by `lim` at `j` is refused at every later endpoint too (both of its
    /// conditions are monotone in `j`), so it is dropped from `live` for good, exactly as the
    /// push fill's scan from it ends there.
    pub(super) fn fill(&self, lim: &Limits, mut width: impl FnMut(usize) -> usize) -> Table {
        if !self.bounds {
            return self.fill_push(lim);
        }
        let n = self.pts.len();
        let mut tab = Table::new(n);
        let mut live: Vec<Start> = Vec::new();
        let mut j = 1;
        let mut checked = 0;
        while j < n {
            // A boundary of a few thousand points is seconds of this loop, and a trace
            // somebody has moved past stops here rather than at the end of it.
            if j >= checked + 64 {
                checked = j;
                inkvec_core::progress::checkpoint();
            }
            self.admit(&tab, &mut live, j);
            live.retain(|st| lim.allows(st.i, j));
            if live.is_empty() {
                j += 1;
                continue;
            }
            let w = if live.len() >= DP_PAR_MIN_LIVE {
                width(live.len()).clamp(1, live.len())
            } else {
                1
            };
            if w < 2 {
                self.endpoint(&mut tab, &mut live, j);
                live.retain(|st| !st.stopped);
                j += 1;
            } else {
                let b = DP_BLOCK_ENDPOINTS.min(n - j);
                self.block(&mut tab, &mut live, j, b, w, lim);
                j += b;
            }
        }
        tab
    }

    /// Add start `j − 1` to the candidates once its cost is final (and finite): leaving it
    /// makes it a vertex, which pays its turn.
    fn admit(&self, tab: &Table, live: &mut Vec<Start>, j: usize) {
        let s = j - 1;
        if tab.best[s].is_finite() {
            let base = tab.best[s]
                + if s > 0 {
                    vertex_cost(self.tan, s, self.cfg)
                } else {
                    0.0
                };
            live.push(Start {
                i: s,
                base,
                cut: CutOff::new(s),
                stopped: false,
            });
        }
    }

    /// The start among `live` that won `j − 1`, which most often wins `j` too; the latest
    /// start when it is gone.
    fn guess(tab: &Table, live: &[Start], j: usize) -> usize {
        live.iter()
            .position(|st| st.i == tab.from[j - 1])
            .unwrap_or(live.len() - 1)
    }

    /// Endpoint `j` on this thread: the guess first, unbounded, then every other start
    /// against the winner so far; the winner goes into the table.
    fn endpoint(&self, tab: &mut Table, live: &mut [Start], j: usize) {
        let guess = Self::guess(tab, live, j);
        let mut win = {
            let st = &mut live[guess];
            let s = self.score(st, j, f64::INFINITY);
            Winner { i: st.i, s }
        };
        for (k, st) in live.iter_mut().enumerate() {
            if k == guess {
                continue;
            }
            let s = self.score(st, j, win.bound_for(st.i));
            if win.beaten_by(st.i, &s) {
                win = Winner { i: st.i, s };
            }
        }
        self.step(tab, win.i, j, win.s);
    }

    /// Endpoints `j0..j0 + b` on `w` threads.
    ///
    /// 1. The guess (the start that won `j0 − 1`) runs through the block unbounded: its
    ///    span at each endpoint is a real candidate, so its cost bounds `F` there.
    /// 2. Every other start of `live` (all before `j0`, so all final) runs through the
    ///    block as one task, each span scored against the guess's at that endpoint, and
    ///    keeps only the spans that beat it.
    /// 3. Endpoint by endpoint, on this thread: the winner among those, and among the
    ///    starts inside the block, which become final one by one and are scored here
    ///    against the running winner.
    ///
    /// Every start's cut-off sees its spans in order, as in [`Self::endpoint`].
    fn block(
        &self,
        tab: &mut Table,
        live: &mut Vec<Start>,
        j0: usize,
        b: usize,
        w: usize,
        lim: &Limits,
    ) {
        let reaches = |st: &Start, j: usize| !st.stopped && lim.allows(st.i, j);
        let guess = Self::guess(tab, live, j0);
        let mut first: Vec<Option<Winner>> = (0..b).map(|_| None).collect();
        {
            let st = &mut live[guess];
            for (k, slot) in first.iter_mut().enumerate() {
                if !reaches(st, j0 + k) {
                    break;
                }
                let s = self.score(st, j0 + k, f64::INFINITY);
                *slot = Some(Winner { i: st.i, s });
            }
        }
        let found: Vec<Vec<(usize, usize, SpanCandidate)>> =
            inkvec_core::progress::detached(|| {
                without_dp_cap_override(|| {
                    use rayon::prelude::*;
                    let _helpers = BusyThreads::claim(w - 1);
                    live.par_iter_mut()
                        .enumerate()
                        .filter(|(k, _)| *k != guess)
                        .map(|(_, st)| {
                            let mut out = Vec::new();
                            for (k, first) in first.iter().enumerate() {
                                if !reaches(st, j0 + k) {
                                    break;
                                }
                                let bound =
                                    first.as_ref().map_or(f64::INFINITY, |f| f.bound_for(st.i));
                                let s = self.score(st, j0 + k, bound);
                                if first.as_ref().is_none_or(|f| f.beaten_by(st.i, &s)) {
                                    out.push((k, st.i, s));
                                }
                            }
                            out
                        })
                        .collect()
                })
            });
        let mut by_end: Vec<Vec<(usize, SpanCandidate)>> = (0..b).map(|_| Vec::new()).collect();
        for (k, i, s) in found.into_iter().flatten() {
            by_end[k].push((i, s));
        }

        let mut inner: Vec<Start> = Vec::new();
        for (k, (first, ends)) in first.into_iter().zip(by_end).enumerate() {
            let j = j0 + k;
            if k > 0 {
                self.admit(tab, &mut inner, j);
            }
            let mut win = first;
            for (i, s) in ends {
                if win.as_ref().is_none_or(|w| w.beaten_by(i, &s)) {
                    win = Some(Winner { i, s });
                }
            }
            for st in inner.iter_mut() {
                if !reaches(st, j) {
                    continue;
                }
                let bound = win.as_ref().map_or(f64::INFINITY, |w| w.bound_for(st.i));
                let s = self.score(st, j, bound);
                if win.as_ref().is_none_or(|w| w.beaten_by(st.i, &s)) {
                    win = Some(Winner { i: st.i, s });
                }
            }
            if let Some(win) = win {
                self.step(tab, win.i, j, win.s);
            }
        }
        live.retain(|st| !st.stopped);
        inner.retain(|st| !st.stopped);
        live.extend(inner);
    }

    /// Score the span `st.i..j` against `best`, feed the start's cut-off, and return what
    /// it offers.
    #[inline(always)]
    fn score(&self, st: &mut Start, j: usize, best: f64) -> SpanCandidate {
        let t = self.terms(st.i, j, self.bound(st.base, best));
        let over = t.over;
        let s = self.resolve(st.i, j, st.base, best, t);
        let i = st.i;
        st.stopped = st.cut.push(j, over, |k| self.cubic_over(i, k));
        s
    }

    /// The program's table filled start by start, every span scored in full: the program
    /// as it was before the bounds, kept for the debug dump (which prints every span in
    /// this order), for the research free cubic, and as the tests' reference. The scan from
    /// `i` ends at the span cap or at the first forced vertex after `i`, whichever is first.
    fn fill_push(&self, lim: &Limits) -> Table {
        let n = self.pts.len();
        let mut tab = Table::new(n);
        for i in 0..n - 1 {
            inkvec_core::progress::checkpoint();
            if !tab.best[i].is_finite() {
                continue;
            }
            let base = tab.best[i]
                + if i > 0 {
                    vertex_cost(self.tan, i, self.cfg)
                } else {
                    0.0
                };
            let mut cut = CutOff::new(i);
            let end = (i.saturating_add(lim.max_span).saturating_add(1))
                .min(lim.wall_after(i).saturating_add(1))
                .min(n);
            for j in i + 1..end {
                let t = self.terms(i, j, Bound::NONE);
                let over = t.over;
                let s = self.resolve(i, j, base, f64::INFINITY, t);
                self.step(&mut tab, i, j, s);
                if cut.push(j, over, |k| self.cubic_over(i, k)) {
                    break;
                }
            }
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

    /// A deterministic pseudo-random stream in `[0, 1)`.
    fn rng(seed: u64) -> impl FnMut() -> f64 {
        let mut st = seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1;
        move || {
            st = st
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            (st >> 11) as f64 / (1u64 << 53) as f64
        }
    }

    /// Boundaries like the tracer's: rasterised shapes (a rounded square, a lobed blob, a
    /// letter-like hook with a sharp corner), noisy to about a twentieth of a pixel,
    /// sampled about a pixel apart, with sigma near the tracer's floor and a few faint
    /// points; plus the degenerate ones (a straight noisy run, a zigzag, coincident
    /// points).
    fn tracer_like() -> Vec<Polyline> {
        let mut out = Vec::new();
        for seed in 1..=4u64 {
            let mut r = rng(seed);
            let mut pts = Vec::new();
            let lobes = 2.0 + seed as f64;
            let n = 150 + 40 * seed as usize;
            for k in 0..n {
                let t = k as f64 / n as f64 * std::f64::consts::TAU;
                let rad = 30.0 + 6.0 * (lobes * t).sin() + 2.0 * (3.0 * lobes * t).cos();
                pts.push(Point::new(
                    rad * t.cos() + 0.05 * (r() - 0.5),
                    rad * t.sin() + 0.05 * (r() - 0.5),
                ));
            }
            let sigma = (0..n)
                .map(|k| if k % 17 == 0 { 0.5 } else { 0.05 + 0.01 * r() })
                .collect();
            out.push(Polyline::new(pts, sigma, true));
        }
        // A rounded square, corners of radius 4, and a hook ending in a sharp corner.
        let mut pts = Vec::new();
        for side in 0..4 {
            let (dx, dy) = [(1.0, 0.0), (0.0, 1.0), (-1.0, 0.0), (0.0, -1.0)][side];
            let (ox, oy) = [(4.0, 0.0), (40.0, 4.0), (36.0, 40.0), (0.0, 36.0)][side];
            for k in 0..32 {
                pts.push(Point::new(ox + dx * k as f64, oy + dy * k as f64));
            }
            let (cx, cy) = [(36.0, 4.0), (36.0, 36.0), (4.0, 36.0), (4.0, 4.0)][side];
            for k in 0..6 {
                let a = (side as f64 - 1.0 + k as f64 / 6.0) * std::f64::consts::FRAC_PI_2;
                pts.push(Point::new(cx + 4.0 * a.cos(), cy + 4.0 * a.sin()));
            }
        }
        let n = pts.len();
        out.push(Polyline::new(pts, vec![0.056; n], true));
        let mut r = rng(99);
        let mut pts = Vec::new();
        for k in 0..70 {
            let t = k as f64 / 70.0 * 2.5;
            pts.push(Point::new(20.0 * t.cos(), 20.0 * t.sin() + 0.04 * r()));
        }
        for k in 0..40 {
            pts.push(Point::new(-16.0 + k as f64, 12.0 + 0.04 * r()));
        }
        let n = pts.len();
        out.push(Polyline::new(pts, vec![0.06; n], false));
        // Degenerate: a straight noisy run, a sawtooth, repeated points.
        let mut r = rng(7);
        let pts: Vec<Point> = (0..200)
            .map(|k| Point::new(k as f64, 0.03 * (r() - 0.5)))
            .collect();
        out.push(Polyline::new(pts, vec![0.05; 200], false));
        let pts: Vec<Point> = (0..120)
            .map(|k| Point::new(k as f64, if k % 2 == 0 { 0.0 } else { 0.8 }))
            .collect();
        out.push(Polyline::new(pts, vec![0.05; 120], false));
        let pts: Vec<Point> = (0..90)
            .map(|k| Point::new((k / 3) as f64, ((k / 3) as f64 * 0.3).sin() * 4.0))
            .collect();
        out.push(Polyline::new(pts, vec![0.1; 90], false));
        out
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

    /// The parallel endpoints fill the sequential table bit for bit, and both the push
    /// reference's: the widths decide only where candidates are scored.
    #[test]
    fn parallel_endpoints_fill_the_sequential_table_bit_for_bit() {
        let poly = boundary();
        let cfg = FitConfig::default();
        let tan = estimate_tangents(&poly, &cfg);
        let pre = Prefix::new(&poly.points, &poly.sigma);
        let mut forked = false;
        for joins_at_ends in [false, true] {
            let sc = SpanScorer::new(&poly.points, &poly.sigma, &tan, &pre, &cfg, joins_at_ends);
            for lim in limits(poly.len()) {
                let push = bits(&sc.fill_push(&lim));
                let seq = bits(&sc.fill(&lim, |_| 1));
                assert!(seq.iter().all(|r| f64::from_bits(r.0).is_finite()));
                assert!(seq == push, "sequential pull, {lim:?}");
                for w in [2, 16] {
                    let par = bits(&sc.fill(&lim, |live| {
                        forked |= live >= DP_PAR_MIN_LIVE;
                        w
                    }));
                    assert!(par == seq, "width {w}, {lim:?}, joins {joins_at_ends}");
                }
                // Widths that change from endpoint to endpoint, as a busy pool's do.
                let mut k = 0usize;
                let mixed = bits(&sc.fill(&lim, |_| {
                    k += 1;
                    [1, 5, 2, 1, 9][k % 5]
                }));
                assert!(mixed == seq, "mixed widths, {lim:?}");
            }
        }
        assert!(forked, "no endpoint had enough candidates to share");
    }

    /// The constraints the tables are compared under, for a polyline of `n` points: none,
    /// two span caps, and forced vertices with and without a cap — one early, two adjacent
    /// (an empty run between them), one in the middle, one just before the end.
    fn limits(n: usize) -> Vec<Limits> {
        let forced = vec![n / 7, n / 3, n / 3 + 1, n / 2, n - 2];
        vec![
            Limits::capped(usize::MAX),
            Limits::capped(45),
            Limits::capped(7),
            Limits {
                max_span: usize::MAX,
                forced: forced.clone(),
            },
            Limits {
                max_span: 7,
                forced,
            },
        ]
    }

    /// Every solution of a program with forced vertices has them all as vertices, and is
    /// the optimum over such segmentations: its cost equals the sum of the unconstrained
    /// programs' optima on the pieces between consecutive forced vertices, where each piece
    /// is solved as a run of the same table (same tangents, same vertex costs).
    #[test]
    fn forced_vertices_are_vertices_and_split_the_program() {
        let mut polys = vec![boundary()];
        polys.extend(tracer_like());
        for poly in &polys {
            let cfg = FitConfig {
                tau: 2.0,
                lambda: 2.5,
            };
            let tan = estimate_tangents(poly, &cfg);
            let pre = Prefix::new(&poly.points, &poly.sigma);
            let n = poly.len();
            let lim = Limits {
                max_span: usize::MAX,
                forced: vec![n / 4, n / 2, (3 * n) / 4],
            };
            for joins_at_ends in [false, true] {
                let sc =
                    SpanScorer::new(&poly.points, &poly.sigma, &tan, &pre, &cfg, joins_at_ends);
                let tab = sc.fill(&lim, |_| 1);
                // Walk the chosen segmentation back from the end: it passes every forced
                // vertex, and no segment straddles one.
                let mut verts = vec![n - 1];
                let mut cur = n - 1;
                while cur != 0 {
                    cur = tab.from[cur];
                    verts.push(cur);
                }
                for &f in &lim.forced {
                    assert!(verts.contains(&f), "forced {f} missing from {verts:?}");
                }
                // Up to the first forced vertex the table is the unconstrained one: nothing
                // ahead of a forced vertex can see past it.
                let free = sc.fill(&Limits::capped(usize::MAX), |_| 1);
                for j in 0..=lim.forced[0] {
                    assert_eq!(tab.best[j].to_bits(), free.best[j].to_bits(), "at {j}");
                }
            }
        }
    }

    /// The bounds skip work, never a decision: with them on, at any width, the table is
    /// the unbounded push program's bit for bit — on the test boundary and on
    /// tracer-like and degenerate ones, at three prices, open and opened-loop, capped and
    /// not.
    #[test]
    fn bounds_leave_the_table_bit_for_bit() {
        let mut polys = vec![boundary()];
        polys.extend(tracer_like());
        let mut compared = 0usize;
        for poly in &polys {
            for lambda in [0.7, 2.5, 8.54] {
                let cfg = FitConfig { tau: 2.0, lambda };
                let tan = estimate_tangents(poly, &cfg);
                let pre = Prefix::new(&poly.points, &poly.sigma);
                for joins_at_ends in [false, true] {
                    let reference =
                        SpanScorer::new(&poly.points, &poly.sigma, &tan, &pre, &cfg, joins_at_ends)
                            .unbounded();
                    let sc =
                        SpanScorer::new(&poly.points, &poly.sigma, &tan, &pre, &cfg, joins_at_ends);
                    assert!(sc.bounds, "the bounds are on by default");
                    for lim in limits(poly.len()) {
                        let seq = bits(&reference.fill(&lim, |_| 1));
                        for w in [1, 2, 16] {
                            let got = bits(&sc.fill(&lim, |_| w));
                            assert!(
                                got == seq,
                                "n {}, lambda {lambda}, joins {joins_at_ends}, {lim:?}, width {w}",
                                poly.len()
                            );
                            compared += 1;
                        }
                        let mut k = 0usize;
                        let mixed = bits(&sc.fill(&lim, |_| {
                            k += 1;
                            [1, 5, 2, 1, 9][k % 5]
                        }));
                        assert!(mixed == seq, "mixed widths, lambda {lambda}");
                    }
                }
            }
        }
        assert!(compared > 100);
    }

    /// And they do skip work: on the test boundary the bounded fill projects far fewer
    /// points than the unbounded one.
    #[test]
    fn bounds_skip_most_projections() {
        let poly = boundary();
        let cfg = FitConfig::default();
        let tan = estimate_tangents(&poly, &cfg);
        let pre = Prefix::new(&poly.points, &poly.sigma);
        let count = |sc: &SpanScorer<'_>| {
            let before = crate::candidates::PROJECTIONS.with(|c| c.get());
            sc.fill(&Limits::capped(usize::MAX), |_| 1);
            crate::candidates::PROJECTIONS.with(|c| c.get()) - before
        };
        let plain = SpanScorer::new(&poly.points, &poly.sigma, &tan, &pre, &cfg, true).unbounded();
        let bounded = SpanScorer::new(&poly.points, &poly.sigma, &tan, &pre, &cfg, true);
        let (a, b) = (count(&plain), count(&bounded));
        assert!(b * 2 < a, "bounded {b} vs unbounded {a} projections");
    }

    /// Where a start's scan stops: the first endpoint whose span is the cut-off's, or the
    /// end. Each span is scored against `bound(j)`.
    fn stop(sc: &SpanScorer<'_>, i: usize, n: usize, bound: impl Fn(usize) -> Bound) -> usize {
        let mut cut = CutOff::new(i);
        for j in i + 1..n {
            let t = sc.terms(i, j, bound(j));
            if cut.push(j, t.over, |k| sc.cubic_over(i, k)) {
                return j;
            }
        }
        n
    }

    /// The lazily answered cut-off stops each scan where the plain counter does, whatever
    /// the bounds leave unknown.
    #[test]
    fn lazy_cutoff_stops_where_the_counter_does() {
        let mut polys = vec![boundary()];
        polys.extend(tracer_like());
        let mut stopped_early = 0usize;
        for poly in &polys {
            let cfg = FitConfig {
                tau: 2.0,
                lambda: 2.5,
            };
            let tan = estimate_tangents(poly, &cfg);
            let pre = Prefix::new(&poly.points, &poly.sigma);
            let reference =
                SpanScorer::new(&poly.points, &poly.sigma, &tan, &pre, &cfg, true).unbounded();
            let sc = SpanScorer::new(&poly.points, &poly.sigma, &tan, &pre, &cfg, true);
            let n = poly.len();
            let table = reference.fill(&Limits::capped(usize::MAX), |_| 1);
            for i in 0..n - 1 {
                if !table.best[i].is_finite() {
                    continue;
                }
                let want = stop(&reference, i, n, |_| Bound::NONE);
                // A high base makes most cubics dead, so most "over"s are left unknown.
                let high = table.best[i] + 50.0;
                let got = stop(&sc, i, n, |j| sc.bound(high, table.best[j]));
                assert_eq!(got, want, "start {i} of {n}");
                if want < n {
                    stopped_early += 1;
                }
            }
        }
        assert!(stopped_early > 0, "the cut-off never fired");
    }

    /// Ties at an endpoint go to the earlier start, as `offer`'s strict `<` over starts in
    /// increasing order gives them, and the bound lets an earlier start through on a tie.
    #[test]
    fn winner_ties_go_to_the_earlier_start() {
        let cand = |cost: f64| SpanCandidate {
            cost,
            kind: SegKind::Line,
            arms: None,
            tans: None,
            arc: None,
            chi2_l: 0.0,
            chi2_c: 0.0,
            line: 0.0,
        };
        let win = Winner {
            i: 10,
            s: cand(5.0),
        };
        assert!(win.beaten_by(3, &cand(5.0)));
        assert!(!win.beaten_by(12, &cand(5.0)));
        assert!(win.beaten_by(12, &cand(4.999)));
        assert!(!win.beaten_by(3, &cand(5.0_f64.next_up())));
        // An earlier start is dead only above the winner's cost, a later one at it: a floor
        // that rounds away (5 + 1e-300 is 5) cannot kill an earlier start's tie.
        let dead = |i: usize, floor: f64| {
            Bound {
                base: 5.0,
                best: win.bound_for(i),
            }
            .dead(floor)
        };
        assert!(dead(3, 1e-9));
        assert!(!dead(3, 1e-300));
        assert!(!dead(3, 0.0));
        assert!(dead(12, 0.0));
    }

    /// The cut-off helper against the plain counter, on random answer streams in which
    /// some answers are hidden until asked.
    #[test]
    fn cutoff_matches_the_counter_on_random_streams() {
        let mut r = rng(5);
        for case in 0..2000 {
            let len = 1 + (r() * 40.0) as usize;
            let p_over = [0.5, 0.8, 0.95][case % 3];
            let truth: Vec<bool> = (0..len).map(|_| r() < p_over).collect();
            let hidden: Vec<bool> = (0..len).map(|_| r() < 0.6).collect();
            let i = 3usize;
            // The counter.
            let mut run = 0usize;
            let mut want = len;
            for (k, &o) in truth.iter().enumerate() {
                run = if o { run + 1 } else { 0 };
                if run >= PRUNE_PATIENCE {
                    want = k + 1;
                    break;
                }
            }
            // The lazy cut-off.
            let mut cut = CutOff::new(i);
            let mut got = len;
            for k in 0..len {
                let j = i + 1 + k;
                let answer = if hidden[k] && truth[k] {
                    Over::Unknown
                } else if truth[k] {
                    Over::Yes
                } else if hidden[k] {
                    Over::Unknown
                } else {
                    Over::No
                };
                if cut.push(j, answer, |q| truth[q - i - 1]) {
                    got = k + 1;
                    break;
                }
            }
            assert_eq!(got, want, "case {case}");
        }
    }
}
