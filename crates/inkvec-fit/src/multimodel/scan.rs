//! The multimodel program's table, filled in parallel and with provably useless work
//! skipped, without changing a bit of it.
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
//!
//! # The recurrence
//!
//! For points `0..n` of the opened polyline, with `best[0] = 0`,
//!
//! ```text
//!     base(i) = best[i] + (i > 0 ? vertex_cost(i) : 0)
//!     best[j] = min over i < j, j − i ≤ max_span, of  base(i) + min_model cost_model(i, j)
//! ```
//!
//! where `cost_model` is the line, G1 cubic, free cubic, circular arc or elliptical arc
//! price of `crate::candidates` (nats). `from[j]`, `kind[j]` and the fitted parameters
//! record the winner. The scan from a start `i` stops after [`PRUNE_PATIENCE`]
//! consecutive spans whose line and cubic fidelity terms both exceed
//! `PRUNE_SLACK·λ·PARAMS_LINE·(j − i)`. A curved model is fitted only when the line (for
//! the ellipse, the best candidate so far) already costs more than the curved model's
//! parameter floor `λ·P`, which is a proof, not a heuristic, that it could not otherwise
//! win. The one exception is the circle, also fitted wherever the line's residual exceeds
//! one per point, because it is the evidence for the line's bow penalty.
//!
//! # Bounds: work the table can never use
//!
//! Almost every candidate loses. Measured on the 246-icon gate set, 65% of the G1 cubics
//! the scan fitted could not beat the table's value at their end *at the moment they were
//! fitted* (84-95% on the flat-art inputs), and 99.8% could not beat its final value. The
//! program only ever reads a candidate through `offer`'s `c < best[j]`, the ellipse's
//! price gate and the cut-off's "over" test, so a candidate whose price floor already
//! reaches `best[j]` needs no residual at all, and one whose residual is being summed can
//! stop the moment the partial sum settles all three. This is branch and bound inside a
//! dynamic program, one candidate at a time:
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
//! `fl(fl(base + x) + floor)` is monotone in `base` and `x`, every residual and penalty is
//! non-negative, so `fl(base + λ·P) ≥ best[j]` proves the model's cost is at least
//! `best[j]` and `offer` would refuse it. The sequential fill bounds with the exact `base`
//! and the live `best[j]`; the parallel scan-ahead with a certified lower bound on `base`
//! ([`SpanScorer::block_bases`]) and the block's starting `best[j]`, which only falls
//! while the block is replayed.
//!
//! The cut-off is the one reader these papers do not have. *Not from the literature:*
//! a span whose cubic is dead but whose line is over leaves its "over" answer unknown
//! ([`Over::Unknown`]), and [`CutOff`] asks for it only when the last [`PRUNE_PATIENCE`]
//! spans hold no known "not over", latest first — the same stop, at the same span, as
//! counting every answer, because the stop is the first span ending such a window of
//! "over"s. On the gate set that costs under 4% of the residual work it saves.

use super::*;
use crate::candidates::{
    best_cubic_bounded, free_cubic_enabled, Abandon, ArcSpan, EllipseSpan, FreeFit, G1Fit,
};

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
    /// The ellipse, when it was fitted ahead of its price gate.
    ellipse: Option<Option<EllipseSpan>>,
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
/// Split in two so the program can run ahead of itself: [`SpanScorer::terms`] is
/// everything a span offers that does not depend on the cost of reaching its start, and
/// [`SpanScorer::resolve`] adds that cost with the sequential program's own expressions,
/// evaluated in the same order.
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
    /// `i`, scored against `bd` (see the module overview). With `speculate`, the ellipse
    /// is also fitted wherever its price gate might pass, so a scan run ahead of the
    /// program has it ready.
    ///
    /// Inlined, with [`Self::resolve`], so the sequential scan (one thread, a busy pool,
    /// the single-threaded WebAssembly build) fuses the two and never materialises the
    /// terms: left to the compiler it measured several per cent slower than the loop this
    /// replaced.
    #[inline(always)]
    fn terms(&self, i: usize, j: usize, speculate: bool, bd: Bound) -> SpanTerms {
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

        // The gate compares the best candidate's cost above `base` with the ellipse's
        // floor. Ahead of the program `base` is unknown, and that difference is the
        // candidate's own price up to rounding in `base`'s last bits, so fit it wherever
        // the gate might pass with a margin far wider than the rounding. `resolve`
        // applies the exact gate, and fits the ellipse itself if the margin ever missed.
        let ellipse = if speculate
            && Self::ellipse_asked(i, j, circle.as_ref())
            && !bd.dead(self.ellipse_floor)
        {
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

    /// The terms of every span from `i` that the sequential program would evaluate, in
    /// order: up to `end`, or to the cut-off. `base` is at most the cost of reaching `i`,
    /// `best` at least the table's value at each end when the span is offered.
    fn scan(&self, i: usize, end: usize, base: f64, best: &[f64]) -> Vec<SpanTerms> {
        let mut scan = Vec::new();
        let mut cut = CutOff::new(i);
        for j in i + 1..end {
            let t = self.terms(i, j, true, self.bound(base, best[j]));
            let over = t.over;
            scan.push(t);
            if cut.push(j, over, |k| self.cubic_over(i, k)) {
                break;
            }
        }
        scan
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

    /// Lower bounds on `base(s)` for the starts `i..i + b` of a block, before any of them
    /// is replayed; infinite for a start nothing can reach.
    ///
    /// `best[i]` is final when the block starts. For a later start `s` the table still
    /// holds the value from before the block, and a start `s'` of the block can lower it
    /// only by an offer, whose cost is at least `fl(base(s') + 2λ)` (every model's price
    /// is at least a line's `λ·PARAMS_LINE`). So
    /// `lb(s) = min(best[s], min over s' in i..s of fl(lb(s') + 2λ)) (+ vertex cost)`,
    /// with the program's own expressions, is a floor on the `base(s)` the replay will
    /// compute, by the monotonicity of IEEE addition.
    fn block_bases(&self, tab: &Table, i: usize, b: usize) -> Vec<f64> {
        let step = self.cfg.lambda * PARAMS_LINE;
        let mut out = Vec::with_capacity(b);
        let mut reach = f64::INFINITY;
        for s in i..i + b {
            let mut best = tab.best[s];
            if reach < f64::INFINITY {
                best = best.min(reach + step);
            }
            let base = if !best.is_finite() {
                f64::INFINITY
            } else if s > 0 {
                best + vertex_cost(self.tan, s, self.cfg)
            } else {
                best
            };
            reach = reach.min(base);
            out.push(base);
        }
        out
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
    /// has reached them, against bounds that stay valid until the replay
    /// ([`Self::block_bases`]); the program then replays each start's spans in order on
    /// this thread, which makes the table identical to the sequential program's whatever
    /// the widths. No span is evaluated that the sequential program would not evaluate,
    /// except the ellipses fitted ahead of a gate that then fails.
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
            // A boundary of a few thousand points is seconds of this loop, and a trace
            // somebody has moved past stops here rather than at the end of it.
            inkvec_core::progress::checkpoint();
            let left = n - 1 - i;
            let w = width(left).clamp(1, left);
            if w < 2 {
                if tab.best[i].is_finite() {
                    let base = base_of(&tab, i);
                    let mut cut = CutOff::new(i);
                    for j in i + 1..jend(i) {
                        let best_j = tab.best[j];
                        let t = self.terms(i, j, false, self.bound(base, best_j));
                        let over = t.over;
                        let s = self.resolve(i, j, base, best_j, t);
                        self.step(&mut tab, i, j, s);
                        if cut.push(j, over, |k| self.cubic_over(i, k)) {
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
            let bases = self.block_bases(&tab, i, b);
            let best: &[f64] = &tab.best;
            let scans: Vec<Vec<SpanTerms>> = inkvec_core::progress::detached(|| {
                without_dp_cap_override(|| {
                    use rayon::prelude::*;
                    let _helpers = BusyThreads::claim(w - 1);
                    (i..i + b)
                        .into_par_iter()
                        .with_min_len(b.div_ceil(w))
                        .map(|s| {
                            // A start nothing reaches is skipped by the replay too.
                            let base = bases[s - i];
                            if base.is_finite() {
                                self.scan(s, jend(s), base, best)
                            } else {
                                Vec::new()
                            }
                        })
                        .collect()
                })
            });
            for (s, scan) in (i..i + b).zip(scans) {
                if !tab.best[s].is_finite() {
                    continue;
                }
                let base = base_of(&tab, s);
                for (j, t) in (s + 1..).zip(scan) {
                    let best_j = tab.best[j];
                    let c = self.resolve(s, j, base, best_j, t);
                    self.step(&mut tab, s, j, c);
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

    /// The bounds skip work, never a decision: with them on, at any width, the table is
    /// the unbounded sequential program's bit for bit — on the test boundary and on
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
                    for max_span in [usize::MAX, 45, 7] {
                        let seq = bits(&reference.fill(max_span, |_| 1));
                        for w in [1, 2, 16] {
                            let got = bits(&sc.fill(max_span, |_| w));
                            assert!(
                                got == seq,
                                "n {}, lambda {lambda}, joins {joins_at_ends}, max_span {max_span}, width {w}",
                                poly.len()
                            );
                            compared += 1;
                        }
                        let mut k = 0usize;
                        let mixed = bits(&sc.fill(max_span, |_| {
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

    /// And they do skip work: on the test boundary the bounded sequential fill projects
    /// far fewer points than the unbounded one.
    #[test]
    fn bounds_skip_most_projections() {
        let poly = boundary();
        let cfg = FitConfig::default();
        let tan = estimate_tangents(&poly, &cfg);
        let pre = Prefix::new(&poly.points, &poly.sigma);
        let count = |sc: &SpanScorer<'_>| {
            let before = crate::candidates::PROJECTIONS.with(|c| c.get());
            sc.fill(usize::MAX, |_| 1);
            crate::candidates::PROJECTIONS.with(|c| c.get()) - before
        };
        let plain = SpanScorer::new(&poly.points, &poly.sigma, &tan, &pre, &cfg, true).unbounded();
        let bounded = SpanScorer::new(&poly.points, &poly.sigma, &tan, &pre, &cfg, true);
        let (a, b) = (count(&plain), count(&bounded));
        assert!(b * 10 < a * 9, "bounded {b} vs unbounded {a} projections");
    }

    /// The lazily answered cut-off stops each scan where the plain counter does: the
    /// bounded scan of every start holds exactly as many spans as the unbounded one.
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
            let table = reference.fill(usize::MAX, |_| 1);
            for i in 0..n - 1 {
                if !table.best[i].is_finite() {
                    continue;
                }
                let want = reference.scan(i, n, 0.0, &table.best).len();
                // A high base makes most cubics dead, so most "over"s are left unknown.
                let got = sc.scan(i, n, table.best[i] + 50.0, &table.best).len();
                assert_eq!(got, want, "start {i} of {n}");
                if want < n - 1 - i {
                    stopped_early += 1;
                }
            }
        }
        assert!(stopped_early > 0, "the cut-off never fired");
    }

    /// A start's lower bounds hold for every start of a block: the replay's `base` is
    /// never below the one the scan-ahead assumed.
    #[test]
    fn block_bases_are_lower_bounds() {
        let poly = boundary();
        let cfg = FitConfig::default();
        let tan = estimate_tangents(&poly, &cfg);
        let pre = Prefix::new(&poly.points, &poly.sigma);
        let sc = SpanScorer::new(&poly.points, &poly.sigma, &tan, &pre, &cfg, false);
        let full = sc.fill(usize::MAX, |_| 1);
        let n = poly.len();
        let base = |j: usize| {
            full.best[j]
                + if j > 0 {
                    vertex_cost(&tan, j, &cfg)
                } else {
                    0.0
                }
        };
        // The table before a block: filled by the starts before it only.
        for block_start in [0usize, 1, 37, 180, 300] {
            let mut partial = Table::new(n);
            for i in 0..block_start {
                if !partial.best[i].is_finite() {
                    continue;
                }
                let b = partial.best[i]
                    + if i > 0 {
                        vertex_cost(&tan, i, &cfg)
                    } else {
                        0.0
                    };
                for (j, t) in (i + 1..).zip(sc.scan(i, n, b, &partial.best)) {
                    let bj = partial.best[j];
                    let c = sc.resolve(i, j, b, bj, t);
                    sc.step(&mut partial, i, j, c);
                }
            }
            let lbs = sc.block_bases(&partial, block_start, 40.min(n - 1 - block_start));
            for (k, lb) in lbs.iter().enumerate() {
                let s = block_start + k;
                if full.best[s].is_finite() {
                    assert!(*lb <= base(s), "start {s}: {lb} > {}", base(s));
                }
            }
            assert_eq!(lbs[0].to_bits(), base(block_start).to_bits());
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
            )
        };
        let mut ahead = 0;
        for i in (0..poly.len()).step_by(7) {
            for j in (i + 1..poly.len()).step_by(3) {
                if let Some(Some(_)) = sc.terms(i, j, true, Bound::NONE).ellipse {
                    ahead += 1;
                }
                // A base of zero, and bases whose last bits round the gate's difference.
                for base in [0.0, 1_234.567_890_1, 1.0e6 + 0.1, 3.0e9 + 0.7] {
                    let inf = f64::INFINITY;
                    let a = key(sc.resolve(i, j, base, inf, sc.terms(i, j, true, Bound::NONE)));
                    let b = key(sc.resolve(i, j, base, inf, sc.terms(i, j, false, Bound::NONE)));
                    assert!(a == b, "span {i}..{j}, base {base}");
                }
            }
        }
        assert!(ahead > 0, "the boundary never asked for an ellipse");
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
