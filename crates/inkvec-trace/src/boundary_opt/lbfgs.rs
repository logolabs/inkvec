//! The descent on the band energy: limited-memory BFGS with a backtracking line search,
//! run separately on each independent part of the boundary.
//!
//! * **Direction.** Limited-memory BFGS, the two-loop recursion over the last `MEMORY`
//!   steps with the initial scaling `γ = sᵀy / yᵀy`: D. C. Liu, J. Nocedal (1989), *On the
//!   limited memory BFGS method for large scale optimization*, Math. Programming 45,
//!   <https://doi.org/10.1007/BF01589116>; as in J. Nocedal, S. J. Wright (2006),
//!   *Numerical Optimization*, Springer, Algorithm 7.4 and §7.2,
//!   <https://doi.org/10.1007/978-0-387-40065-5>. A pair with `yᵀs ≤ 10⁻¹²·‖y‖·‖s‖` is not
//!   stored, which keeps the implied Hessian positive definite without a curvature condition.
//! * **Line search.** Backtracking to the Armijo (sufficient decrease) condition from the
//!   unit step (Nocedal & Wright, Algorithm 3.1). The energy is continuous but only
//!   piecewise smooth (its slope jumps where a piece meets a gridline), the setting of
//!   A. S. Lewis, M. L. Overton (2013), *Nonsmooth optimization via quasi-Newton methods*,
//!   Math. Program. 141:135–163, <https://doi.org/10.1007/s10107-012-0514-2>, who found BFGS
//!   with an inexact line search reliable there. Adapted: every trial point is projected
//!   back into the 1-px disc around its start and onto the image frame where it is pinned,
//!   and a step never moves a point more than `MAX_STEP` px.
//! * **Per component.** The unknowns split into groups that share no pixel and no prior
//!   term (a median of five per icon, hundreds on a page of text); the energy is the sum of
//!   the groups' energies, so each is minimised on its own and stops when it has converged,
//!   instead of every group paying for the slowest one: block-separable minimisation, as a
//!   sparse solver's independent residual blocks (Ceres Solver documentation,
//!   <https://github.com/ceres-solver/ceres-solver>, `docs/source/nnls_solving.rst`).
//!
//! Measured against what it replaces (Fletcher–Reeves conjugate gradients taking the first
//! trial that lowered the energy, from a fresh 0.35 px step every iteration): on the
//! continuous band energy that solver moved the furthest point 0.35 px on every one of its
//! 48 iterations while the energy fell by a hundredth of a percent -- it wandered along the
//! energy's flat directions. Levenberg–Marquardt with a Gauss–Newton Hessian did the same
//! at a hundred times the cost; preconditioning L-BFGS with the priors' exact banded Hessian
//! made the steps far too short, because the smoothed ℓ1 kink term is a hundred times
//! stiffer at a straight run than it is anywhere a real corner is.

use super::{Problem, Report, Vars, K_ANCHOR, K_KINK, MAX_STEP, MAX_TOTAL};
use inkvec_core::clock::Instant;
use inkvec_core::Point;

/// Steps remembered by the L-BFGS direction. Three did as well as seven, fifteen or thirty
/// on the standard inputs: the energy's kinks, not the curvature model, set the pace.
const MEMORY: usize = 3;
/// Armijo (sufficient decrease) constant.
const C1: f64 = 1e-4;
/// Trials one line search may take (halving the step each time).
const MAX_TRIALS: usize = 8;
/// Iteration ceiling per component. 48 read 0.3558 on the screen set and 32 read 0.3585
/// (the per-pixel solver it replaces: 0.3873); 32 keeps the stage no slower than that solver
/// on the standard inputs, where 48 was a fifth to nine tenths slower.
const MAX_ITERS: usize = 32;
/// Stop once no point moved more than this in an iteration: half the 0.01 px the SVG
/// writes, so further steps cannot change the output coordinates they reach.
const PARAM_TOL: f64 = 0.005;
/// Stop once an iteration lowers the part of the energy the geometry can change by less
/// than this fraction.
const FUNC_TOL: f64 = 1e-4;

/// One independent part of the problem.
#[derive(Clone, Debug, Default)]
pub(super) struct Active {
    pub edges: Vec<u32>,
    pub runs: Vec<u32>,
    pub vars: Vec<u32>,
    /// The part of its band energy no boundary touched at the start.
    pub e_const: f64,
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
    let (mut before, mut after, mut iters) = (0.0, 0.0, 0usize);
    for comp in comps {
        prob.active = Some(comp);
        let (e0, e1, it) = solve(prob, vars, &mut pos, &mut gfull, deadline, dbg);
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
/// before and after, and the iterations taken.
fn solve(
    prob: &mut Problem,
    vars: &Vars,
    pos: &mut [Point],
    gfull: &mut [Point],
    deadline: Option<(Instant, u128)>,
    dbg: bool,
) -> (f64, f64, usize) {
    let ids: Vec<u32> = prob.active.as_ref().map_or(Vec::new(), |a| a.vars.clone());
    let e_const = prob.active.as_ref().map_or(0.0, |a| a.e_const);
    let m = ids.len();
    let gather = |src: &[Point]| -> Vec<Point> { ids.iter().map(|&v| src[v as usize]).collect() };
    let eval = |prob: &mut Problem,
                pos: &mut [Point],
                gfull: &mut [Point],
                x: &[Point],
                g: &mut [Point]|
     -> f64 {
        for (&v, p) in ids.iter().zip(x) {
            pos[v as usize] = *p;
        }
        let e = prob.energy(pos, Some(&mut *gfull));
        for (gi, &v) in g.iter_mut().zip(&ids) {
            *gi = gfull[v as usize];
        }
        e
    };
    let start = gather(&vars.start);
    let pin: Vec<u8> = ids.iter().map(|&v| vars.pin[v as usize]).collect();
    let mut x = gather(pos);
    let mut g = vec![Point::new(0.0, 0.0); m];
    let mut e = eval(prob, pos, gfull, &x, &mut g);
    let e0 = e;
    let mut dir = vec![Point::new(0.0, 0.0); m];
    let mut trial = x.clone();
    let mut tg = vec![Point::new(0.0, 0.0); m];
    let mut mem: Vec<(Vec<Point>, Vec<Point>, f64)> = Vec::new();
    let mut done = 0usize;
    for it in 0..MAX_ITERS {
        if deadline.is_some_and(|(t0, ms)| t0.elapsed().as_millis() > ms) {
            break;
        }
        lbfgs_direction(&g, &mem, &mut dir);
        let mut gd = dot(&g, &dir);
        if gd >= 0.0 {
            mem.clear();
            lbfgs_direction(&g, &mem, &mut dir);
            gd = dot(&g, &dir);
        }
        let dmax = dir.iter().map(|d| d.x.hypot(d.y)).fold(0.0f64, f64::max);
        if dmax < 1e-12 || gd >= 0.0 {
            break;
        }
        // The unit quasi-Newton step, or on the first iteration a move of `MAX_STEP`;
        // never more than `MAX_STEP` for the furthest point.
        let amax = MAX_STEP / dmax;
        let mut a = if it == 0 { amax } else { amax.min(1.0) };
        let mut accepted = None;
        for _ in 0..MAX_TRIALS {
            inkvec_core::progress::checkpoint();
            project(&start, &pin, &x, &dir, a, &mut trial);
            let et = eval(prob, pos, gfull, &trial, &mut tg);
            if et <= e + C1 * a * gd {
                accepted = Some(et);
                break;
            }
            a *= 0.5;
        }
        let Some(et) = accepted else {
            break;
        };
        let moved = x
            .iter()
            .zip(&trial)
            .map(|(p, q)| p.dist(*q))
            .fold(0.0f64, f64::max);
        let rel = (e - et) / (e - e_const).max(1e-12);
        if dbg {
            eprintln!("  [bopt] it {it} step {a:.4} E {et:.4} rel {rel:.2e} moved {moved:.4}");
        }
        let s: Vec<Point> = trial
            .iter()
            .zip(&x)
            .map(|(p, q)| Point::new(p.x - q.x, p.y - q.y))
            .collect();
        let y: Vec<Point> = tg
            .iter()
            .zip(&g)
            .map(|(p, q)| Point::new(p.x - q.x, p.y - q.y))
            .collect();
        std::mem::swap(&mut x, &mut trial);
        std::mem::swap(&mut g, &mut tg);
        e = et;
        done = it + 1;
        if moved < PARAM_TOL || rel < FUNC_TOL {
            break;
        }
        let ys = dot(&y, &s);
        if ys > 1e-12 * dot(&y, &y).sqrt() * dot(&s, &s).sqrt() {
            if mem.len() == MEMORY {
                mem.remove(0);
            }
            mem.push((s, y, 1.0 / ys));
        }
    }
    for (&v, p) in ids.iter().zip(&x) {
        pos[v as usize] = *p;
    }
    (e0, e, done)
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
