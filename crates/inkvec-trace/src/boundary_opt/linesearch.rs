//! The step length of one L-BFGS iteration: the Moré–Thuente line search, which finds a
//! step satisfying the strong Wolfe conditions by safeguarded cubic and quadratic
//! interpolation.
//!
//! # The problem
//!
//! Along the search path the energy is a function of one variable, `φ(a)`, the energy at
//! the point a step of length `a` reaches; `φ(0)` is the current energy and `φ'(0) < 0`
//! its slope along a descent direction. The step wanted satisfies the **strong Wolfe
//! conditions**
//!
//! ```text
//! φ(a) ≤ φ(0) + μ·a·φ'(0)          (sufficient decrease, μ = FTOL)
//! |φ'(a)| ≤ η·|φ'(0)|               (curvature, η = GTOL)
//! ```
//!
//! The first alone (the Armijo condition) is what a backtracking search enforces, and it
//! is satisfied by any step short enough: a search that halves from the unit step stops
//! at the first one that passes, which can be a small fraction of the step the energy
//! would reward. The boundary solve's old stopping rule then read that short step as
//! convergence (a lucide icon stopped after two iterations with the energy 0.1 % lower and
//! every boundary 0.05 px off; r2-fidelity report, section 2.5 (b)). The curvature
//! condition rules such steps out: a step at which the slope is still as steep as at the
//! start is too short, and the search extrapolates. It also guarantees `yᵀs > 0` for the
//! L-BFGS pair the step produces, so the curvature model is positive definite.
//!
//! # The method
//!
//! Method from: J. J. Moré, D. J. Thuente (1994), *Line search algorithms with guaranteed
//! sufficient decrease*, ACM Transactions on Mathematical Software 20(3):286–307,
//! <https://doi.org/10.1145/192115.192132>, Algorithm 1 with the safeguarded step of their
//! section 4, as in their MINPACK-2 routines `dcsrch` and `dcstep`; the textbook account
//! is J. Nocedal, S. J. Wright (2006), *Numerical Optimization*, 2nd ed., Springer,
//! §3.5, <https://doi.org/10.1007/978-0-387-40065-5>. In outline:
//!
//! 1. Keep an interval of uncertainty `[stx, sty]` (unordered) whose end `stx` is the best
//!    step so far, starting from `stx = sty = 0`.
//! 2. Evaluate `φ` and `φ'` at the trial step `stp`. Stop when it satisfies both
//!    conditions.
//! 3. Otherwise choose the next trial with [`step`]: a cubic fitted to the values and
//!    slopes at `stx` and `stp`, a quadratic, or a secant step, safeguarded so the
//!    interval shrinks; once the minimiser is bracketed the interval is required to shrink
//!    by a factor `0.66` at least every second trial (bisect otherwise). Before a bracket
//!    exists, the trial extrapolates to between `1.1` and `4` times the last step from
//!    `stx`, up to the largest step allowed.
//! 4. While no trial has both decreased the energy enough and turned the slope
//!    non-negative ("stage 1"), the steps are chosen on the auxiliary function
//!    `ψ(a) = φ(a) − φ(0) − μ·a·φ'(0)`, which is what guarantees that the step returned
//!    has sufficient decrease even when the curvature condition is never met.
//!
//! Adapted, and why:
//! * **A cap on trials** (`MAX_EVALS`). Each trial renders the band, the whole cost of an
//!   iteration. When the cap is reached the search returns its best step if that step
//!   decreased the energy sufficiently (step 4 makes `stx` such a step whenever one
//!   exists), and reports failure otherwise.
//! * **A largest step**, `stpmax`, set by the caller so that no boundary point moves more
//!   than `MAX_STEP` px in one step: the band is fixed for the whole solve and the energy
//!   is a refinement, not a search (see the module docs of `boundary_opt`).
//! * **A piecewise-smooth energy.** The band energy is continuous but its slope jumps
//!   where a piece of boundary meets a gridline, so the curvature condition can be
//!   impossible to meet exactly at a kink. The cap above ends such a search with the best
//!   sufficient-decrease step, the behaviour A. S. Lewis, M. L. Overton (2013),
//!   *Nonsmooth optimization via quasi-Newton methods*, Math. Program. 141:135–163,
//!   <https://doi.org/10.1007/s10107-012-0514-2>, found reliable for BFGS on such
//!   functions (they use the weak Wolfe conditions with bisection; Moré–Thuente's
//!   interpolation reaches a Wolfe step in fewer energy evaluations on the smooth pieces,
//!   which is where almost every step lands).
//!
//! The search is written in reverse-communication form, as `dcsrch` is: the caller
//! evaluates `φ` and `φ'` at [`MoreThuente::stp`] and hands them to
//! [`MoreThuente::update`], so the energy evaluation (which owns the problem's scratch)
//! stays with the caller.
//!
//! Related work not used: the Hager–Zhang approximate Wolfe line search (W. W. Hager,
//! H. Zhang (2005), *A new conjugate gradient method with guaranteed descent and an
//! efficient line search*, SIAM J. Optim. 16(1), <https://doi.org/10.1137/030601880>) was
//! measured with conjugate gradients when this solver was chosen and took more energy
//! evaluations than L-BFGS for the same result (see `docs/algorithm/08-boundary-solve.md`).

/// The sufficient-decrease constant `μ` (Nocedal & Wright's `c1`), the value of the
/// Armijo search this replaced.
pub(super) const FTOL: f64 = 1e-4;
/// The curvature constant `η` (`c2`). 0.9 is the value recommended for quasi-Newton
/// directions (Nocedal & Wright §3.1; Moré & Thuente §1): loose enough that the unit
/// quasi-Newton step is usually accepted at once, so most iterations cost one evaluation.
pub(super) const GTOL: f64 = 0.9;
/// Relative width of the interval of uncertainty below which the search stops (MINPACK-2's
/// `xtol`, the value in its driver).
const XTOL: f64 = 0.1;
/// Most trials one search may take. MINPACK-2's L-BFGS-B driver allows 20; here every
/// trial renders the band, and ten is far more than a search on this energy takes when it
/// succeeds: of 13,285 accepted steps on 41 icons of the 128 px screen set (2026-10-03),
/// 96.3 % took one trial, 99.7 % at most two, and none more than four.
pub(super) const MAX_EVALS: usize = 10;
/// Extrapolation bounds before a bracket exists: the next trial lies between
/// `stp + XTRAPL·(stp − stx)` and `stp + XTRAPU·(stp − stx)` (MINPACK-2's `xtrapl`,
/// `xtrapu`).
const XTRAPL: f64 = 1.1;
const XTRAPU: f64 = 4.0;
/// Required shrink of a bracketing interval over two trials, else bisect (Moré & Thuente
/// §4; MINPACK-2's `p66`).
const SHRINK: f64 = 0.66;

/// What [`MoreThuente::update`] asks the caller to do next.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum Next {
    /// Evaluate `φ` and `φ'` at this step and call `update` again.
    Eval(f64),
    /// The step last evaluated satisfies the strong Wolfe conditions.
    Converged,
    /// No further progress is possible in this search (rounding, the interval has
    /// shrunk to `XTOL`, the step hit `stpmax` while still descending, or the trial cap);
    /// `best()` holds the best sufficient-decrease step, if any.
    Stop,
}

/// The state of one Moré–Thuente search (MINPACK-2 `dcsrch`'s saved variables).
#[derive(Clone, Debug)]
pub(super) struct MoreThuente {
    /// The trial step to evaluate next.
    stp: f64,
    /// `φ(0)` and `φ'(0)`.
    finit: f64,
    ginit: f64,
    /// `μ·φ'(0)`, the slope of the sufficient-decrease line.
    gtest: f64,
    /// Whether the minimiser is bracketed by `[stx, sty]`.
    brackt: bool,
    /// Stage 1 (steps chosen on the auxiliary `ψ`) or 2 (on `φ` itself).
    stage1: bool,
    /// The best step so far, and `φ`, `φ'` there.
    stx: f64,
    fx: f64,
    gx: f64,
    /// The other end of the interval of uncertainty, and `φ`, `φ'` there.
    sty: f64,
    fy: f64,
    gy: f64,
    /// The bounds the next trial is kept within.
    stmin: f64,
    stmax: f64,
    /// The interval's width now and one trial ago (for the shrink test).
    width: f64,
    width1: f64,
    /// The bounds on every step.
    stpmin: f64,
    stpmax: f64,
    /// Trials evaluated so far.
    evals: usize,
    /// The best step evaluated that satisfies sufficient decrease: `(a, φ(a))`.
    best: Option<(f64, f64)>,
}

impl MoreThuente {
    /// Start a search from `φ(0) = f0` with slope `φ'(0) = g0 < 0`, first trying `stp`,
    /// never stepping beyond `stpmax`. `stp` is clamped into `(0, stpmax]`.
    pub(super) fn new(f0: f64, g0: f64, stp: f64, stpmax: f64) -> Self {
        let stp = stp.min(stpmax).max(0.0);
        let width = stpmax;
        MoreThuente {
            stp,
            finit: f0,
            ginit: g0,
            gtest: FTOL * g0,
            brackt: false,
            stage1: true,
            stx: 0.0,
            fx: f0,
            gx: g0,
            sty: 0.0,
            fy: f0,
            gy: g0,
            stmin: 0.0,
            stmax: stp + XTRAPU * stp,
            width,
            width1: 2.0 * width,
            stpmin: 0.0,
            stpmax,
            evals: 0,
            best: None,
        }
    }

    /// The trial step to evaluate.
    pub(super) fn stp(&self) -> f64 {
        self.stp
    }

    /// The best step evaluated that satisfies sufficient decrease, and the energy there.
    pub(super) fn best(&self) -> Option<(f64, f64)> {
        self.best
    }

    /// Trials evaluated so far.
    pub(super) fn evals(&self) -> usize {
        self.evals
    }

    /// Hand in `φ(stp) = f` and `φ'(stp) = g` for the step last asked for, and get what to
    /// do next. This is `dcsrch`'s body between two evaluations.
    pub(super) fn update(&mut self, f: f64, g: f64) -> Next {
        self.evals += 1;
        let stp = self.stp;
        let ftest = self.finit + stp * self.gtest;
        // A NaN energy (it cannot arise from finite positions, but a degenerate input
        // must not hang the search) fails every comparison below: treat it as no decrease.
        let f = if f.is_finite() { f } else { f64::INFINITY };
        let g = if g.is_finite() { g } else { 0.0 };
        if f <= ftest && self.best.is_none_or(|(_, fb)| f < fb) {
            self.best = Some((stp, f));
        }
        if self.stage1 && f <= ftest && g >= 0.0 {
            self.stage1 = false;
        }
        // The conditions under which no better step can be found (MINPACK-2's warnings):
        // rounding has made the interval useless, the interval is within XTOL, or the step
        // is pinned at a bound while the conditions still ask to go beyond it.
        if self.brackt && (stp <= self.stmin || stp >= self.stmax) {
            return Next::Stop;
        }
        if self.brackt && self.stmax - self.stmin <= XTOL * self.stmax {
            return Next::Stop;
        }
        if stp >= self.stpmax && f <= ftest && g <= self.gtest {
            return Next::Stop;
        }
        if stp <= self.stpmin && (f > ftest || g >= self.gtest) {
            return Next::Stop;
        }
        // The strong Wolfe conditions.
        if f <= ftest && g.abs() <= GTOL * (-self.ginit) {
            return Next::Converged;
        }
        if self.evals >= MAX_EVALS {
            return Next::Stop;
        }
        // The next trial. In stage 1, while the step has lowered φ below the best so far
        // but not below the sufficient-decrease line, choose it on ψ = φ − φ(0) − μ·a·φ'(0)
        // (Moré & Thuente §3): ψ's values and slopes are φ's less the line's.
        if self.stage1 && f <= self.fx && f > ftest {
            let gt = self.gtest;
            let mut fxm = self.fx - self.stx * gt;
            let mut fym = self.fy - self.sty * gt;
            let mut gxm = self.gx - gt;
            let mut gym = self.gy - gt;
            let fm = f - stp * gt;
            let gm = g - gt;
            let mut s = Step {
                stx: self.stx,
                fx: &mut fxm,
                dx: &mut gxm,
                sty: self.sty,
                fy: &mut fym,
                dy: &mut gym,
            };
            let (stx, sty, next, brackt) =
                step(&mut s, stp, fm, gm, self.brackt, self.stmin, self.stmax);
            self.stx = stx;
            self.sty = sty;
            self.brackt = brackt;
            self.fx = fxm + stx * gt;
            self.fy = fym + sty * gt;
            self.gx = gxm + gt;
            self.gy = gym + gt;
            self.stp = next;
        } else {
            let (mut fx, mut gx, mut fy, mut gy) = (self.fx, self.gx, self.fy, self.gy);
            let mut s = Step {
                stx: self.stx,
                fx: &mut fx,
                dx: &mut gx,
                sty: self.sty,
                fy: &mut fy,
                dy: &mut gy,
            };
            let (stx, sty, next, brackt) =
                step(&mut s, stp, f, g, self.brackt, self.stmin, self.stmax);
            self.stx = stx;
            self.sty = sty;
            self.brackt = brackt;
            self.fx = fx;
            self.gx = gx;
            self.fy = fy;
            self.gy = gy;
            self.stp = next;
        }
        // Force a sufficient shrink of a bracketing interval: if two trials have not
        // shrunk it by SHRINK, bisect.
        if self.brackt {
            if (self.sty - self.stx).abs() >= SHRINK * self.width1 {
                self.stp = self.stx + 0.5 * (self.sty - self.stx);
            }
            self.width1 = self.width;
            self.width = (self.sty - self.stx).abs();
        }
        // The bounds of the next trial: the interval once bracketed, else an
        // extrapolation window beyond the last step.
        if self.brackt {
            self.stmin = self.stx.min(self.sty);
            self.stmax = self.stx.max(self.sty);
        } else {
            self.stmin = self.stp + XTRAPL * (self.stp - self.stx);
            self.stmax = self.stp + XTRAPU * (self.stp - self.stx);
        }
        self.stp = self.stp.max(self.stpmin).min(self.stpmax);
        // When no progress is possible, the best step is the answer (`dcsrch` sets
        // stp = stx and lets the caller evaluate it once more).
        if (self.brackt && (self.stp <= self.stmin || self.stp >= self.stmax))
            || (self.brackt && self.stmax - self.stmin <= XTOL * self.stmax)
        {
            return Next::Stop;
        }
        Next::Eval(self.stp)
    }
}

/// The interval end points `step` updates in place: `stx` with `φ`, `φ'` there, and `sty`
/// likewise. (The steps themselves are returned.)
struct Step<'a> {
    stx: f64,
    fx: &'a mut f64,
    dx: &'a mut f64,
    sty: f64,
    fy: &'a mut f64,
    dy: &'a mut f64,
}

/// The safeguarded step of Moré & Thuente §4 (MINPACK-2 `dcstep`): from the interval end
/// `stx` (the best step, `φ = fx`, `φ' = dx`), the other end `sty` (`fy`, `dy`) and the
/// trial `stp` (`fp`, `dp`), choose the next trial and update the interval.
///
/// Four cases, by what the trial says about the minimiser:
///
/// 1. `fp > fx`: a higher value; the minimiser is bracketed between `stx` and `stp`. Take
///    the cubic step if it is closer to `stx` than the quadratic one (through `fx`, `dx`,
///    `fp`), else their midpoint.
/// 2. `fp ≤ fx` and the slopes at `stx` and `stp` have opposite signs: bracketed. Take
///    whichever of the cubic and the secant (through `dx`, `dp`) steps is further from
///    `stp`.
/// 3. `fp ≤ fx`, same signs, `|dp| < |dx|`: the slope is shrinking. The cubic step when the
///    cubic has its minimiser beyond `stp`, else a bound; compared with the secant step.
///    Within a bracket, never closer to `sty` than 0.66 of the way.
/// 4. `fp ≤ fx`, same signs, `|dp| ≥ |dx|`: the slope is not shrinking. Within a bracket,
///    the cubic through `stp` and `sty`; else a bound.
///
/// The cubic through two points with values `fa`, `fb` and slopes `da`, `db` has its
/// minimiser at `a + r·(b − a)` with `θ = 3(fa − fb)/(b − a) + da + db`,
/// `γ = ±sqrt(θ² − da·db)` and `r = (γ − da + θ)/(2γ − da + db)`; each case below writes it
/// scaled by `s = max(|θ|, |da|, |db|)` against overflow, exactly as `dcstep` does.
///
/// Returns the new `stx`, `sty`, the next trial, and whether the minimiser is bracketed.
fn step(
    s: &mut Step<'_>,
    stp: f64,
    fp: f64,
    dp: f64,
    brackt: bool,
    stpmin: f64,
    stpmax: f64,
) -> (f64, f64, f64, bool) {
    let (stx, sty) = (s.stx, s.sty);
    let (fx, dx) = (*s.fx, *s.dx);
    let (fy, dy) = (*s.fy, *s.dy);
    // The sign of φ' at stp relative to φ' at stx.
    let sgnd = dp * (dx / dx.abs());
    let (stpf, brackt) = next_trial(
        (stx, fx, dx),
        (sty, fy, dy),
        (stp, fp, dp),
        brackt,
        (stpmin, stpmax),
    );
    // Update the interval of uncertainty: a higher value replaces sty; otherwise stp
    // becomes the best step, and the old best becomes sty when the slopes changed sign.
    let (mut nstx, mut nsty) = (stx, sty);
    if fp > fx {
        nsty = stp;
        *s.fy = fp;
        *s.dy = dp;
    } else {
        if sgnd < 0.0 {
            nsty = stx;
            *s.fy = fx;
            *s.dy = dx;
        }
        nstx = stp;
        *s.fx = fp;
        *s.dx = dp;
    }
    // A NaN from a degenerate cubic (0/0 when every value and slope is equal) would stall
    // the search; fall back to the midpoint of the interval, or the far bound.
    let stpf = if stpf.is_finite() {
        stpf
    } else if brackt {
        0.5 * (nstx + nsty)
    } else {
        stpmax
    };
    (nstx, nsty, stpf, brackt)
}

/// `θ` and `|γ|` of the cubic through `(a, fa, da)` and `(b, fb, db)` (step, value, slope):
/// `θ = 3(fa − fb)/(b − a) + da + db`, `|γ| = sqrt(θ² − da·db)`, computed scaled by
/// `s = max(|θ|, |da|, |db|)` against overflow and with the radicand clamped at zero, as
/// `dcstep` does. The caller gives `γ` its sign.
fn cubic_terms(a: (f64, f64, f64), b: (f64, f64, f64)) -> (f64, f64) {
    let ((xa, fa, da), (xb, fb, db)) = (a, b);
    let theta = 3.0 * (fa - fb) / (xb - xa) + da + db;
    let sc = theta.abs().max(da.abs()).max(db.abs());
    let gamma = sc
        * ((theta / sc).powi(2) - (da / sc) * (db / sc))
            .max(0.0)
            .sqrt();
    (theta, gamma)
}

/// The next trial of [`step`]'s four cases (see there), from `x = (stx, fx, dx)`,
/// `y = (sty, fy, dy)` and the trial `p = (stp, fp, dp)`, within `bounds = (stpmin,
/// stpmax)`. Returns the trial and whether the minimiser is now bracketed.
fn next_trial(
    x: (f64, f64, f64),
    y: (f64, f64, f64),
    p: (f64, f64, f64),
    brackt: bool,
    bounds: (f64, f64),
) -> (f64, bool) {
    let ((stx, fx, dx), (sty, _, _), (stp, fp, dp)) = (x, y, p);
    let (stpmin, stpmax) = bounds;
    let sgnd = dp * (dx / dx.abs());
    if fp > fx {
        // Case 1: the cubic step if it is closer to stx than the quadratic one, else their
        // midpoint.
        let (theta, g) = cubic_terms(x, p);
        let gamma = if stp < stx { -g } else { g };
        let r = ((gamma - dx) + theta) / (((gamma - dx) + gamma) + dp);
        let stpc = stx + r * (stp - stx);
        let stpq = stx + ((dx / ((fx - fp) / (stp - stx) + dx)) / 2.0) * (stp - stx);
        let stpf = if (stpc - stx).abs() < (stpq - stx).abs() {
            stpc
        } else {
            stpc + (stpq - stpc) / 2.0
        };
        (stpf, true)
    } else if sgnd < 0.0 {
        // Case 2: whichever of the cubic and secant steps is further from stp.
        let (theta, g) = cubic_terms(x, p);
        let gamma = if stp > stx { -g } else { g };
        let r = ((gamma - dp) + theta) / (((gamma - dp) + gamma) + dx);
        let stpc = stp + r * (stx - stp);
        let stpq = stp + (dp / (dp - dx)) * (stx - stp);
        let stpf = if (stpc - stp).abs() > (stpq - stp).abs() {
            stpc
        } else {
            stpq
        };
        (stpf, true)
    } else if dp.abs() < dx.abs() {
        // Case 3. The cubic step is taken only if the cubic tends to infinity in the
        // direction of the step or its minimiser lies beyond stp; otherwise a bound.
        let (theta, g) = cubic_terms(x, p);
        let gamma = if stp > stx { -g } else { g };
        let r = ((gamma - dp) + theta) / ((gamma + (dx - dp)) + gamma);
        let stpc = if r < 0.0 && gamma != 0.0 {
            stp + r * (stx - stp)
        } else if stp > stx {
            stpmax
        } else {
            stpmin
        };
        let stpq = stp + (dp / (dp - dx)) * (stx - stp);
        let stpf = if brackt {
            let pick = if (stpc - stp).abs() < (stpq - stp).abs() {
                stpc
            } else {
                stpq
            };
            if stp > stx {
                pick.min(stp + SHRINK * (sty - stp))
            } else {
                pick.max(stp + SHRINK * (sty - stp))
            }
        } else {
            let pick = if (stpc - stp).abs() > (stpq - stp).abs() {
                stpc
            } else {
                stpq
            };
            pick.min(stpmax).max(stpmin)
        };
        (stpf, brackt)
    } else if brackt {
        // Case 4, bracketed: the cubic through stp and sty.
        let (_, _, dy) = y;
        let (theta, g) = cubic_terms(p, y);
        let gamma = if stp > sty { -g } else { g };
        let r = ((gamma - dp) + theta) / (((gamma - dp) + gamma) + dy);
        (stp + r * (sty - stp), brackt)
    } else if stp > stx {
        // Case 4, not bracketed: the bound in the direction of the step.
        (stpmax, brackt)
    } else {
        (stpmin, brackt)
    }
}

#[cfg(test)]
#[path = "linesearch_tests.rs"]
mod tests;
