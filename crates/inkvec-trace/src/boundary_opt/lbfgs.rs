//! The descent on the band energy: limited-memory BFGS with a Moré–Thuente line search on
//! the projected path, run separately on each independent part of the boundary, and
//! stopped on the projected gradient and the relative decrease.
//!
//! # The problem
//!
//! Minimise the band energy `E(x)` (see `boundary_opt`) over the boundary points `x`,
//! subject to two simple constraints per point `v`: it stays in the disc of radius
//! `MAX_TOTAL` round its start `x⁰_v`, and a coordinate pinned to the image frame keeps
//! its value. The energy is continuous and piecewise smooth (its slope jumps where a
//! piece of boundary meets a gridline).
//!
//! # The method, step by step
//!
//! For each independent part (below), from the start:
//!
//! 1. **Stop test.** Stop when the projected gradient (the gradient with the components
//!    the constraints block removed, [`projected_gradient_norm`]) is below `PG_TOL` of the
//!    whole problem's largest gradient at the start, when the last step lowered the energy
//!    by less than `FUNC_TOL` of the whole problem's changeable energy at the start
//!    ([`Scales`]), after `MAX_ITERS` iterations, when the line search finds no step of
//!    sufficient decrease, or when the caller's time budget has run out.
//! 2. **Direction.** Limited-memory BFGS, the two-loop recursion over the last `MEMORY`
//!    steps with the initial scaling `γ = sᵀy / yᵀy`, with the component of a point's
//!    direction that would push it out of its disc removed where it already sits on the
//!    disc's edge ([`tangent_cone`]). A direction that is not downhill clears the memory
//!    and falls back to the projected steepest descent.
//! 3. **Step length.** Moré–Thuente (`linesearch`), from the unit quasi-Newton step (on
//!    the first iteration, a move of `MAX_STEP` for the furthest point), never beyond a
//!    move of `MAX_STEP`, along the projected path `a ↦ P(x + a·d)`. Each trial renders the
//!    exact coverage; the path's slope `φ'(a)` is the gradient there dotted with the
//!    path's derivative ([`path_slope`]), exact also where the projection is active.
//! 4. **Update.** Store the pair `(s, y)` when its curvature `yᵀs` is positive, and go to 1.
//!
//! # Sources
//!
//! * **Direction.** Method from: D. C. Liu, J. Nocedal (1989), *On the limited memory BFGS
//!   method for large scale optimization*, Math. Programming 45,
//!   <https://doi.org/10.1007/BF01589116>; as in J. Nocedal, S. J. Wright (2006),
//!   *Numerical Optimization*, Springer, Algorithm 7.4 and §7.2,
//!   <https://doi.org/10.1007/978-0-387-40065-5>. A pair with `yᵀs ≤ 10⁻¹²·‖y‖·‖s‖` is not
//!   stored, which keeps the implied Hessian positive definite where the curvature
//!   condition could not be met (a search ended at a kink).
//! * **Constraints and the stopping rule.** Inspired by: R. H. Byrd, P. Lu, J. Nocedal,
//!   C. Zhu (1995), *A limited memory algorithm for bound constrained optimization*, SIAM
//!   J. Sci. Comput. 16(5):1190–1208, <https://doi.org/10.1137/0916069> (L-BFGS-B), which
//!   stops on the norm of the projected gradient and on the relative reduction of the
//!   objective, never on the length of a step, and searches along a direction that keeps
//!   the iterate feasible. Adapted: our constraints are discs and pinned coordinates, not
//!   boxes, so instead of L-BFGS-B's generalised Cauchy point and subspace minimisation the
//!   direction is projected onto the tangent cone of the active discs and the trial points
//!   onto the discs (a projected-path search); both thresholds are relative to the whole
//!   problem at the start ([`Scales`]), because the energy's scale differs by orders of
//!   magnitude between a two-colour logo and a crowded emoji; and the relative reduction
//!   is measured on the part of the energy the geometry can change (the energy less the
//!   residual of the pixels no boundary touches), which the absolute energy would swamp.
//! * **Line search.** Method from: J. J. Moré, D. J. Thuente (1994), *Line search
//!   algorithms with guaranteed sufficient decrease*, ACM TOMS 20(3),
//!   <https://doi.org/10.1145/192115.192132>; see `linesearch` for the algorithm and the
//!   adaptations to this piecewise-smooth energy (A. S. Lewis, M. L. Overton (2013),
//!   *Nonsmooth optimization via quasi-Newton methods*, Math. Program. 141:135–163,
//!   <https://doi.org/10.1007/s10107-012-0514-2>).
//! * **Per component.** The unknowns split into groups that share no pixel and no prior
//!   term (a median of five per icon, hundreds on a page of text); the energy is the sum of
//!   the groups' energies, so each is minimised on its own and stops when it has converged,
//!   instead of every group paying for the slowest one: block-separable minimisation, as a
//!   sparse solver's independent residual blocks (Ceres Solver documentation,
//!   <https://github.com/ceres-solver/ceres-solver>, `docs/source/nnls_solving.rst`).
//!
//! # What it replaced, and why
//!
//! The first L-BFGS form of this stage (v0.2.4) halved the step from the unit step until
//! the Armijo condition held, and stopped when an accepted step moved no point more than
//! 0.005 px. Both are wrong for this energy (r2-fidelity research, 2026-10-02, section 2.5):
//! a backtracked step is short because the search halved it, not because the minimum is
//! near, so the step-length test fired while the energy could still fall a long way
//! (lucide `equal` stopped after two iterations with every bar 0.05 px thin, dE00 0.067
//! where the converged solve reaches 0.002), and elsewhere the 32-iteration cap was hit
//! while points still moved 0.03–0.13 px a step. Run to convergence with the old line
//! search (192 iterations and tighter tolerances) the stage was worth −7.6 % dE00 on the
//! 128 px screen set and −9.6 % on `held_a`.
//!
//! Before that, Fletcher–Reeves conjugate gradients took the first trial that lowered the
//! energy, from a fresh 0.35 px step every iteration: on the continuous band energy it moved
//! the furthest point 0.35 px on every one of its 48 iterations while the energy fell by a
//! hundredth of a percent -- it wandered along the energy's flat directions.
//! Levenberg–Marquardt with a Gauss–Newton Hessian did the same at a hundred times the
//! cost; preconditioning L-BFGS with the priors' exact banded Hessian made the steps far too
//! short, because the smoothed ℓ1 kink term is a hundred times stiffer at a straight run than
//! it is anywhere a real corner is.

use super::linesearch::{MoreThuente, Next};
use super::{Problem, Report, Vars, K_ANCHOR, K_KINK, MAX_STEP, MAX_TOTAL};
use inkvec_core::clock::Instant;
use inkvec_core::Point;

/// Steps remembered by the L-BFGS direction. Three did as well as seven, fifteen or thirty
/// on the standard inputs with the Armijo search; with the Wolfe search seven read
/// −4.26 % dE00 against −4.37 % for three on the 512 px screen set (2026-10-03, at 128
/// iterations): the energy's kinks, not the curvature model, set the pace.
const MEMORY: usize = 3;
/// Iteration ceiling per independent part, set by measurement (2026-10-03, the 246-icon
/// screen set against v0.2.5, family-macro dE00, with the local fold guard and every band
/// pixel kept): 64 iterations read −4.15 % at 512 px and −6.35 % at 128 px, 128 read
/// −4.37 % and −8.40 %; the whole trace at 512 px took 1.13× v0.2.5's time at 64 and
/// 1.28–1.30× at 128 (30 icons, two interleaved passes, default threads, under load). The
/// 512 px difference is a fifth of a percent, under what the set resolves, and 128 would
/// leave no room under the 1.3× time budget for anything else.
const MAX_ITERS: usize = 64;
/// Stop a part once an iteration lowers the energy by less than this fraction of the whole
/// problem's changeable energy at the start (see [`Scales`]). Measured on 21 icons of the
/// 512 px screen set: a part-relative 10⁻⁶ left 72 of 107 parts still running at 128
/// iterations, the whole-problem 10⁻⁷ only 25 (median 68 iterations), and read −4.35 %
/// dE00 against −4.13 % on the full 512 px set.
const FUNC_TOL: f64 = 1e-7;
/// Stop a part once its projected gradient's largest point ([`projected_gradient_norm`]) is
/// below this fraction of the whole problem's largest gradient at the start. On this
/// piecewise-smooth energy the gradient does not vanish at a minimum that sits on a kink,
/// so the test rarely fires: none of 13,285 steps on 41 icons of the 128 px screen set
/// stopped on it. It is there for the smooth case, where it is the right test.
const PG_TOL: f64 = 1e-6;

/// One independent part of the problem.
#[derive(Clone, Debug, Default)]
pub(super) struct Active {
    pub edges: Vec<u32>,
    pub runs: Vec<u32>,
    pub vars: Vec<u32>,
}

/// The denominators of the two relative stopping tests, taken from the *whole* problem at
/// the start rather than from the part being solved.
///
/// The parts are solved one after another only because they are independent; the problem
/// is one. Measured against its own energy, a part with almost nothing to gain (a frame
/// edge, a speck) keeps iterating for gains of a ten-thousandth of nothing, and a large
/// part looks converged early; measured against the whole, each part stops when its next
/// step would not move the whole problem's energy, which is the test the un-split solve
/// would apply (Byrd et al. 1995 test the reduction relative to the objective). The cost of
/// the denominators is one evaluation of the whole energy and gradient at the start.
#[derive(Clone, Copy, Debug)]
struct Scales {
    /// The whole problem's changeable energy at the start: its energy less the residual of
    /// the band pixels no boundary touches (`band_norm.1`).
    f: f64,
    /// The largest point of the whole problem's gradient at the start (no point sits on
    /// its disc's edge there, so this is its projected gradient too).
    pg: f64,
}

fn dot(a: &[Point], b: &[Point]) -> f64 {
    a.iter().zip(b).map(|(p, q)| p.x * q.x + p.y * q.y).sum()
}

/// Minimise `prob`'s energy from the start (the band term must be set up), one
/// independent part at a time. Returns the report and the solved positions, or `None`
/// when the energy did not fall.
/// `deadline` is the caller's wall-clock budget (its start and length in ms); with `None`
/// the result depends only on the input.
pub(super) fn descend(
    prob: &mut Problem,
    vars: &Vars,
    deadline: Option<(Instant, u128)>,
    dbg: bool,
) -> Option<(Report, Vec<Point>)> {
    let n = vars.start.len();
    let (data0, _) = prob.band_norm;
    let kink0 = prob.priors(&vars.start, None);
    if data0 <= 0.0 || kink0 <= 0.0 {
        return None;
    }
    prob.w_kink = K_KINK * data0 / kink0;
    prob.w_anchor = K_ANCHOR * data0 / n as f64;
    let comps = super::band::components(prob);
    let mut pos = vars.start.clone();
    let mut gfull = vec![Point::new(0.0, 0.0); n];
    // The whole problem at the start, for the stopping tests' denominators (`Scales`).
    let e_all = prob.energy(&vars.start, Some(&mut gfull));
    let scales = Scales {
        f: (e_all - prob.band_norm.1).max(1e-12),
        pg: gfull.iter().map(|g| g.x.hypot(g.y)).fold(0.0f64, f64::max),
    };
    let (mut before, mut after, mut iters) = (0.0, 0.0, 0usize);
    for comp in comps {
        prob.active = Some(comp);
        let (e0, e1, it) = solve(prob, vars, &mut pos, &mut gfull, deadline, scales, dbg);
        prob.active = None;
        before += e0;
        after += e1;
        iters = iters.max(it);
    }
    if after >= before || iters == 0 {
        return None;
    }
    Some((
        Report {
            before,
            after,
            iters,
            moved: 0,
            scale: 1.0,
        },
        pos,
    ))
}

/// L-BFGS on the active component, updating its unknowns in `pos`. Returns its energy
/// before and after, and the iterations taken (steps accepted).
///
/// The loop is the module docs' steps 1-4. Each iteration renders the band once per
/// line-search trial (usually one); nothing is allocated per trial except when a trial
/// becomes the search's best, whose points and gradient are copied.
fn solve(
    prob: &mut Problem,
    vars: &Vars,
    pos: &mut [Point],
    gfull: &mut [Point],
    deadline: Option<(Instant, u128)>,
    scales: Scales,
    dbg: bool,
) -> (f64, f64, usize) {
    let mut part = Part::new(prob, vars, pos);
    let mut e = Part::eval(&part.ids, prob, pos, gfull, &part.x, &mut part.g);
    let e0 = e;
    let mut mem: Vec<(Vec<Point>, Vec<Point>, f64)> = Vec::new();
    let mut done = 0usize;
    for it in 0..MAX_ITERS {
        if deadline.is_some_and(|(t0, ms)| t0.elapsed().as_millis() > ms) {
            break;
        }
        // Step 1: the projected gradient is (relatively) zero: a stationary point of the
        // constrained problem.
        let pg = projected_gradient_norm(&part.start, &part.x, &part.g);
        if pg <= PG_TOL * scales.pg {
            if dbg {
                eprintln!(
                    "  [bopt] it {it} projected gradient {pg:.3e} <= {PG_TOL:.0e} of {:.3e}",
                    scales.pg
                );
            }
            break;
        }
        // Step 2: the direction, kept inside the tangent cone of the active discs.
        let Some((gd, dmax)) =
            descent_direction(&part.g, &mut mem, &part.start, &part.x, &mut part.dir)
        else {
            break;
        };
        // Step 3: the unit quasi-Newton step, or on the first iteration a move of
        // `MAX_STEP`; never more than `MAX_STEP` for the furthest point.
        let amax = MAX_STEP / dmax;
        let a0 = if it == 0 { amax } else { amax.min(1.0) };
        let (accepted, trials) = part.search(prob, pos, gfull, (e, gd, a0, amax));
        let Some((et, a)) = accepted else {
            break;
        };
        let rel = (e - et) / scales.f;
        if dbg {
            let moved = (part.x.iter().zip(&part.trial))
                .map(|(p, q)| p.dist(*q))
                .fold(0.0f64, f64::max);
            eprintln!(
                "  [bopt] it {it} step {a:.4} trials {trials} E {et:.4} rel {rel:.2e} moved {moved:.4} pg {pg:.3e}"
            );
        }
        // Step 4: the curvature pair, then the step is taken.
        let pair = curvature_pair(&part.x, &part.trial, &part.g, &part.tg);
        std::mem::swap(&mut part.x, &mut part.trial);
        std::mem::swap(&mut part.g, &mut part.tg);
        e = et;
        done = it + 1;
        // Step 1's second test, on the step just taken: the relative decrease.
        if rel < FUNC_TOL {
            break;
        }
        if let Some(pair) = pair {
            if mem.len() == MEMORY {
                mem.remove(0);
            }
            mem.push(pair);
        }
    }
    for (&v, p) in part.ids.iter().zip(&part.x) {
        pos[v as usize] = *p;
    }
    (e0, e, done)
}

/// The unknowns of the part being solved, gathered out of the whole problem's, and the
/// buffers its iterations reuse: the current point `x` and its gradient `g`, the
/// direction, the trial point and its gradient, and the line search's best trial.
struct Part {
    /// The part's unknowns, as indices into the whole problem's.
    ids: Vec<u32>,
    /// Their starting positions.
    start: Vec<Point>,
    /// Their frame pins (see `band::pin_frame`).
    pin: Vec<u8>,
    x: Vec<Point>,
    g: Vec<Point>,
    dir: Vec<Point>,
    trial: Vec<Point>,
    tg: Vec<Point>,
    best_x: Vec<Point>,
    best_g: Vec<Point>,
}

impl Part {
    /// The active part of `prob`, its points read from `pos`.
    fn new(prob: &Problem, vars: &Vars, pos: &[Point]) -> Self {
        let ids: Vec<u32> = prob.active.as_ref().map_or(Vec::new(), |a| a.vars.clone());
        let gather =
            |src: &[Point]| -> Vec<Point> { ids.iter().map(|&v| src[v as usize]).collect() };
        let zero = vec![Point::new(0.0, 0.0); ids.len()];
        let x = gather(pos);
        Part {
            start: gather(&vars.start),
            pin: ids.iter().map(|&v| vars.pin[v as usize]).collect(),
            trial: x.clone(),
            best_x: x.clone(),
            x,
            g: zero.clone(),
            dir: zero.clone(),
            tg: zero.clone(),
            best_g: zero,
            ids,
        }
    }

    /// The energy at the part's points `at` (written into `pos`, the rest of `pos`
    /// unchanged), with the part's share of the gradient gathered into `g`.
    fn eval(
        ids: &[u32],
        prob: &mut Problem,
        pos: &mut [Point],
        gfull: &mut [Point],
        at: &[Point],
        g: &mut [Point],
    ) -> f64 {
        for (&v, p) in ids.iter().zip(at) {
            pos[v as usize] = *p;
        }
        let e = prob.energy(pos, Some(&mut *gfull));
        for (gi, &v) in g.iter_mut().zip(ids) {
            *gi = gfull[v as usize];
        }
        e
    }

    /// Step 3: the Moré–Thuente search along the projected path from `x` along `dir`,
    /// from `φ(0) = e` with slope `gd`, first trying `a0`, never beyond `amax`. On success
    /// `trial` and `tg` hold the accepted point and its gradient, and the energy there and
    /// the step are returned; with the number of trials either way.
    fn search(
        &mut self,
        prob: &mut Problem,
        pos: &mut [Point],
        gfull: &mut [Point],
        (e, gd, a0, amax): (f64, f64, f64, f64),
    ) -> (Option<(f64, f64)>, usize) {
        let mut ls = MoreThuente::new(e, gd, a0, amax);
        let mut best_a = f64::NAN;
        let (outcome, last_e, last_a) = loop {
            inkvec_core::progress::checkpoint();
            let a = ls.stp();
            project(
                &self.start,
                &self.pin,
                &self.x,
                &self.dir,
                a,
                &mut self.trial,
            );
            let et = Self::eval(&self.ids, prob, pos, gfull, &self.trial, &mut self.tg);
            let slope = path_slope(&self.start, &self.x, &self.dir, a, &self.tg);
            let next = ls.update(et, slope);
            if ls.best().is_some_and(|(ab, _)| ab == a) && best_a != a {
                // This trial is the search's best so far: keep it, so a search that stops
                // on a later, worse trial can return it without rendering it again.
                best_a = a;
                self.best_x.copy_from_slice(&self.trial);
                self.best_g.copy_from_slice(&self.tg);
            }
            match next {
                Next::Eval(_) => continue,
                other => break (other, et, a),
            }
        };
        let accepted = match outcome {
            // The strong Wolfe step is the trial just evaluated.
            Next::Converged => Some((last_e, last_a)),
            // Otherwise the best sufficient-decrease trial, when there was one.
            _ => ls.best().map(|(a, et)| {
                if a != last_a {
                    self.trial.copy_from_slice(&self.best_x);
                    self.tg.copy_from_slice(&self.best_g);
                }
                (et, a)
            }),
        };
        (accepted, ls.evals())
    }
}

/// Step 2: the L-BFGS direction from the remembered pairs, projected onto the tangent cone
/// of the active discs; when that is not downhill, the memory is cleared and the projected
/// steepest descent taken instead. Returns `gᵀd` and the largest point of `d`, or `None`
/// when no downhill direction is left (the part is stationary).
fn descent_direction(
    g: &[Point],
    mem: &mut Vec<(Vec<Point>, Vec<Point>, f64)>,
    start: &[Point],
    x: &[Point],
    dir: &mut [Point],
) -> Option<(f64, f64)> {
    lbfgs_direction(g, mem, dir);
    tangent_cone(start, x, dir);
    let mut gd = dot(g, dir);
    if gd >= 0.0 {
        mem.clear();
        lbfgs_direction(g, mem, dir);
        tangent_cone(start, x, dir);
        gd = dot(g, dir);
    }
    let dmax = dir.iter().map(|d| d.x.hypot(d.y)).fold(0.0f64, f64::max);
    (dmax >= 1e-12 && gd < 0.0).then_some((gd, dmax))
}

/// Step 4: the L-BFGS pair `(s, y, 1/yᵀs)` of the step from `x` to `trial`, with `s` the
/// step and `y` the change of gradient (`g` to `tg`), or `None` when its curvature `yᵀs` is
/// not above `10⁻¹²·‖y‖·‖s‖` (a pair that would make the implied Hessian indefinite).
fn curvature_pair(
    x: &[Point],
    trial: &[Point],
    g: &[Point],
    tg: &[Point],
) -> Option<(Vec<Point>, Vec<Point>, f64)> {
    let diff = |a: &[Point], b: &[Point]| -> Vec<Point> {
        a.iter()
            .zip(b)
            .map(|(p, q)| Point::new(p.x - q.x, p.y - q.y))
            .collect()
    };
    let (s, y) = (diff(trial, x), diff(tg, g));
    let ys = dot(&y, &s);
    (ys > 1e-12 * dot(&y, &y).sqrt() * dot(&s, &s).sqrt()).then(|| (s, y, 1.0 / ys))
}

/// Whether a point at `p` with start `s` sits on the edge of its disc (to rounding), and
/// the outward unit normal there.
#[inline]
fn on_disc_edge(s: Point, p: Point) -> Option<(f64, f64)> {
    let (dx, dy) = (p.x - s.x, p.y - s.y);
    let r = dx.hypot(dy);
    // MAX_TOTAL > 0, so a point on the edge is never at its start and r > 0 here.
    (r >= MAX_TOTAL * (1.0 - 1e-9)).then(|| (dx / r, dy / r))
}

/// Remove from each point's direction the outward normal component at a point that sits on
/// the edge of its disc: the projection of `dir` onto the tangent cone of the feasible set
/// (Nocedal & Wright §16.7; Byrd et al. 1995 keep the direction feasible the same way
/// for boxes, by freezing the variables at a bound). A step along the result moves such a
/// point along the circle to first order instead of into the projection, so the search
/// path is close to a straight line and `φ'(0) = gᵀd` holds.
///
/// Pinned coordinates need nothing here: the gradient there is zero, so the L-BFGS
/// direction is too (every stored `s` and `y` has a zero there), and a pinned point's
/// offset from its start has a zero there as well, so the normal does.
fn tangent_cone(start: &[Point], x: &[Point], dir: &mut [Point]) {
    for v in 0..x.len() {
        if let Some((ux, uy)) = on_disc_edge(start[v], x[v]) {
            let out = ux * dir[v].x + uy * dir[v].y;
            if out > 0.0 {
                dir[v].x -= out * ux;
                dir[v].y -= out * uy;
            }
        }
    }
}

/// The largest point of the projected gradient, `max_v |P_T(g_v)|`, with `P_T` removing the
/// outward normal component of `−g_v` at a point on the edge of its disc (the direction of
/// steepest descent the constraint blocks). This is the stationarity measure of L-BFGS-B
/// (Byrd et al. 1995, §6: `‖P(x − g) − x‖∞`), for discs: it is zero exactly at a
/// first-order stationary point of the constrained problem. Pinned coordinates have a zero
/// gradient already.
fn projected_gradient_norm(start: &[Point], x: &[Point], g: &[Point]) -> f64 {
    let mut worst = 0.0f64;
    for v in 0..x.len() {
        let (mut gx, mut gy) = (g[v].x, g[v].y);
        if let Some((ux, uy)) = on_disc_edge(start[v], x[v]) {
            // Steepest descent −g points outward when g·u < 0.
            let gu = gx * ux + gy * uy;
            if gu < 0.0 {
                gx -= gu * ux;
                gy -= gu * uy;
            }
        }
        worst = worst.max(gx.hypot(gy));
    }
    worst
}

/// `φ'(a)`, the slope of the energy along the projected path `a ↦ P(x + a·d)` at `a`, from
/// the gradient `g` at that point: `Σ_v g_v · dP_v/da`.
///
/// For a point whose `z = x_v + a·d_v` lies inside its disc, `P` is the identity and
/// `dP_v/da = d_v`. Outside, `P(z) = s + R·u` with `u = (z − s)/|z − s|` and `R` =
/// `MAX_TOTAL`, whose derivative is `dP_v/da = (R/|z − s|)·(d_v − (u·d_v)·u)`: the
/// tangential part of the motion, shrunk by the ratio of the radii. The pinned coordinate of
/// a point has `d` and `g` zero, and contributes nothing either way. This is the exact
/// one-sided derivative of the path `project` evaluates, so the line search's slopes and
/// values agree.
fn path_slope(start: &[Point], x: &[Point], dir: &[Point], a: f64, g: &[Point]) -> f64 {
    let mut slope = 0.0;
    for v in 0..x.len() {
        let (zx, zy) = (x[v].x + dir[v].x * a, x[v].y + dir[v].y * a);
        let (dx, dy) = (zx - start[v].x, zy - start[v].y);
        let r = dx.hypot(dy);
        let (mut px, mut py) = (dir[v].x, dir[v].y);
        if r > MAX_TOTAL {
            let (ux, uy) = (dx / r, dy / r);
            let ud = ux * px + uy * py;
            let k = MAX_TOTAL / r;
            px = k * (px - ud * ux);
            py = k * (py - ud * uy);
        }
        slope += g[v].x * px + g[v].y * py;
    }
    slope
}

/// `x + a·dir`, each point kept within `MAX_TOTAL` of its start and on the frame where it
/// is pinned.
fn project(start: &[Point], pin: &[u8], x: &[Point], dir: &[Point], a: f64, out: &mut [Point]) {
    for v in 0..x.len() {
        let mut q = Point::new(x[v].x + dir[v].x * a, x[v].y + dir[v].y * a);
        let (dx, dy) = (q.x - start[v].x, q.y - start[v].y);
        let d = dx.hypot(dy);
        if d > MAX_TOTAL {
            let s = MAX_TOTAL / d;
            q = Point::new(start[v].x + dx * s, start[v].y + dy * s);
        }
        if pin[v] & 1 != 0 {
            q.x = start[v].x;
        }
        if pin[v] & 2 != 0 {
            q.y = start[v].y;
        }
        out[v] = q;
    }
}

/// The L-BFGS two-loop recursion: `dir = −H·g` with `H` the limited-memory inverse Hessian
/// (the plain negative gradient with no pairs stored).
fn lbfgs_direction(g: &[Point], mem: &[(Vec<Point>, Vec<Point>, f64)], dir: &mut [Point]) {
    let mut q: Vec<Point> = g.to_vec();
    let mut alphas = vec![0.0; mem.len()];
    for (i, (s, y, rho)) in mem.iter().enumerate().rev() {
        let a = rho * dot(s, &q);
        alphas[i] = a;
        for (qq, yy) in q.iter_mut().zip(y) {
            qq.x -= a * yy.x;
            qq.y -= a * yy.y;
        }
    }
    let gamma = mem
        .last()
        .map_or(1.0, |(s, y, _)| dot(s, y) / dot(y, y).max(1e-300));
    for qq in q.iter_mut() {
        qq.x *= gamma;
        qq.y *= gamma;
    }
    for (i, (s, y, rho)) in mem.iter().enumerate() {
        let b = rho * dot(y, &q);
        for (qq, ss) in q.iter_mut().zip(s) {
            qq.x += (alphas[i] - b) * ss.x;
            qq.y += (alphas[i] - b) * ss.y;
        }
    }
    for (d, qq) in dir.iter_mut().zip(&q) {
        *d = Point::new(-qq.x, -qq.y);
    }
}
