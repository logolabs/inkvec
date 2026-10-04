//! The stroke solve's linear system: the Jacobian rows of the boundary residuals, the
//! normal equations `JᵀJ`, `Jᵀr` they sum to, and the envelope (profile) that holds them.
//!
//! **The rows.** A boundary point's residual `r_p = (d(p, C) - h)/σ_p` reads the half-width
//! and the variables of the one segment its foot lies on (two neighbouring segments' at a
//! miter vertex). By the envelope theorem the foot's own movement adds nothing to first
//! order, so `∂d/∂θ = -n·∂C(t*)/∂θ`, `n` the unit vector from the foot to the point: written
//! out per segment kind in [`super::super::dist`] and assembled by [`analytic_row`] into a
//! fixed-size row ([`JacRow`]). The miter gauge at a vertex and degenerate feet keep central
//! differences of the exact distance (step 1e-5 px).
//!
//! **The system.** Every variable couples only to its own segment's neighbours and to `h`,
//! so the lower triangle of `JᵀJ` lives inside an envelope -- per variable, the first
//! variable it couples to ([`profile`]) -- and is stored and factorised in that envelope
//! ([`super::super::skyline`]), which gives the dense Cholesky's result bit for bit. Rows
//! are measured in parallel and added in row order, so every sum is the sequential one.
//!
//! Method from: the envelope theorem as stated by Milgrom, Segal (2002), Envelope theorems
//! for arbitrary choice sets, Econometrica 70(2), doi:10.1111/1468-0262.00296; and the
//! envelope (profile) storage of a symmetric band matrix from Jennings (1966), A compact
//! storage scheme for the solution of symmetric linear simultaneous equations, The
//! Computer Journal 9(3), doi:10.1093/comjnl/9.3.281.
//! See also: Triggs, McLauchlan, Hartley, Fitzgibbon (2000), Bundle adjustment -- a modern
//! synthesis, Vision Algorithms: Theory and Practice, doi:10.1007/3-540-44480-7_21, on
//! exploiting the sparsity of `JᵀJ` in least-squares problems of this shape.

use inkvec_core::{Point, Vec2};
use inkvec_fit::curves::Segment;
use rayon::prelude::*;

use super::{
    local_dist, local_eval, locals, neighbour, pt, seg_end, Foot, Model, Row, SegVar, Shape,
};
use crate::ribbon::boundary::Boundary;
use crate::ribbon::dist::{
    circle_grad, circular_arc_grad, cubic_grad, ellipse_arc_grad, line_grad,
};
use crate::ribbon::join::{Cap, Join, Style};
use crate::ribbon::skyline::Skyline;

/// The envelope of the normal matrix `JᵀJ` for `model` ([`Skyline`]): for each variable,
/// the first (lowest-index) variable any residual couples it to.
///
/// A residual reads one segment's variables and its start point ([`locals`]), or under
/// miter joins two neighbouring segments' (the gauge at a vertex, both neighbours of
/// every segment, round a closed path's start too), or a circle's three; every set is
/// listed here, so the envelope holds every nonzero `JᵀJ` can have whatever the rows'
/// assignment to segments. The half-width (last variable) is in every residual: its row
/// is full. Fixed shapes have no variables. Cost O(segments).
pub(super) fn profile(model: &Model) -> Vec<usize> {
    let n = model.theta.len();
    let mut first: Vec<usize> = (0..n).collect();
    let mut note = |v: &[usize]| {
        if let Some(&lo) = v.iter().min() {
            for &u in v {
                first[u] = first[u].min(lo);
            }
        }
    };
    for (si, s) in model.shapes.iter().enumerate() {
        match s {
            Shape::Path { segs, closed, .. } => {
                let m = segs.len();
                for k in 0..m {
                    note(&locals(model, si, k, None));
                    if model.style.join == Join::Miter {
                        for foot in [Foot::Start, Foot::End] {
                            if let Some(j) = neighbour(m, *closed, k, foot) {
                                note(&locals(model, si, k, Some(j)));
                            }
                        }
                    }
                }
            }
            Shape::Circle { at, .. } => note(&[*at, at + 1, at + 2]),
            Shape::Fixed(_) => {}
        }
    }
    if let Some(f) = first.last_mut() {
        *f = 0;
    }
    first
}

/// Most entries a Jacobian row can hold: the half-width, and two segments' variables
/// (a miter gauge reads a vertex's two segments: two start points, two cubics' handles
/// and ends, two radii) -- at most 1 + 2·(2 + 6 + 1) = 19.
const ROW_CAP: usize = 20;

/// One residual's Jacobian entries, `(variable, ∂r/∂θ)`, in a fixed array so the
/// thousands of rows of a measurement allocate nothing.
#[derive(Clone, Copy)]
struct JacRow {
    /// Entries in use.
    n: usize,
    /// The entries; the first `n` are meaningful.
    e: [(usize, f64); ROW_CAP],
}

impl JacRow {
    /// A row holding `(i, v)` alone.
    fn with(i: usize, v: f64) -> JacRow {
        let mut r = JacRow {
            n: 0,
            e: [(0, 0.0); ROW_CAP],
        };
        r.push(i, v);
        r
    }

    /// Append `(i, v)`.
    fn push(&mut self, i: usize, v: f64) {
        self.e[self.n] = (i, v);
        self.n += 1;
    }

    /// The entries in use.
    fn entries(&self) -> &[(usize, f64)] {
        &self.e[..self.n]
    }
}

/// The derivatives of row (`shape`, `seg`)'s distance term at boundary point `p` with
/// respect to the variables it reads, in closed form ([`super::super::dist`]'s gradients,
/// by the envelope theorem), appended to `out` as `(variable, ∂d/∂θ)` with every variable
/// once. `false` (and `out` untouched) where the closed form is not used: the degenerate
/// cases the gradients decline and -- decided by the caller -- a miter gauge at a vertex;
/// the caller then takes central differences.
///
/// A segment's start point is the previous segment's end variable (the path's start for
/// the first); a closed path's last end *is* its start, so a variable can receive two
/// contributions (a one-segment loop), which are summed.
fn analytic_row(
    model: &Model,
    shape: usize,
    seg: usize,
    p: Point,
    t_hint: f64,
    out: &mut JacRow,
) -> bool {
    let t = &model.theta;
    // At most four points' two coordinates and a radius.
    let mut g = JacRow {
        n: 0,
        e: [(0, 0.0); ROW_CAP],
    };
    let put = |g: &mut JacRow, i: usize, v: Vec2| {
        g.push(i, v.x);
        g.push(i + 1, v.y);
    };
    match &model.shapes[shape] {
        Shape::Path { start, segs, .. } => {
            let a_i = if seg == 0 {
                *start
            } else {
                seg_end(&segs[seg - 1])
            };
            let a = pt(t, a_i);
            match segs[seg] {
                SegVar::Line { end } => {
                    let Some((_, d)) = line_grad(p, a, pt(t, end)) else {
                        return false;
                    };
                    put(&mut g, a_i, d[0]);
                    put(&mut g, end, d[1]);
                }
                SegVar::Cubic { c1, c2, end } => {
                    let q = [a, pt(t, c1), pt(t, c2), pt(t, end)];
                    let Some((_, d)) = cubic_grad(p, q, t_hint) else {
                        return false;
                    };
                    for (i, v) in [a_i, c1, c2, end].into_iter().zip(d) {
                        put(&mut g, i, v);
                    }
                }
                SegVar::Arc {
                    end,
                    r: Some(ri),
                    large,
                    sweep,
                    ..
                } => {
                    let Some((_, d, dr)) = circular_arc_grad(p, a, t[ri], large, sweep, pt(t, end))
                    else {
                        return false;
                    };
                    put(&mut g, a_i, d[0]);
                    put(&mut g, end, d[1]);
                    g.push(ri, dr);
                }
                SegVar::Arc {
                    end,
                    r: None,
                    rx,
                    ry,
                    phi,
                    large,
                    sweep,
                } => {
                    let arc = Segment::Arc {
                        rx,
                        ry,
                        phi,
                        large_arc: large,
                        sweep,
                        end: pt(t, end),
                    };
                    let Some((_, d)) = ellipse_arc_grad(p, a, &arc, t_hint) else {
                        return false;
                    };
                    put(&mut g, a_i, d[0]);
                    put(&mut g, end, d[1]);
                }
            }
        }
        Shape::Circle { at, .. } => {
            let Some((_, dc, dr)) = circle_grad(p, pt(t, *at), t[at + 2]) else {
                return false;
            };
            put(&mut g, *at, dc);
            g.push(at + 2, dr);
        }
        Shape::Fixed(_) => return false,
    }
    // A stable insertion sort by variable (as `sort_by_key` was): a variable entered
    // twice -- a one-segment loop's start and end -- keeps its two terms in entry order.
    let e = &mut g.e[..g.n];
    for k in 1..e.len() {
        let mut j = k;
        while j > 0 && e[j - 1].0 > e[j].0 {
            e.swap(j - 1, j);
            j -= 1;
        }
    }
    for &(i, v) in g.entries() {
        match out.n.checked_sub(1).map(|l| &mut out.e[l]) {
            Some(last) if last.0 == i => last.1 += v,
            _ => out.push(i, v),
        }
    }
    true
}

/// `JᵀJ` (lower triangle in the envelope `first`, [`profile`]) and `Jᵀr` for the data
/// rows and the anchor.
///
/// Each row's `J` is the closed form of [`analytic_row`] where it applies -- every row
/// under round joins except at degenerate feet -- and otherwise central
/// differences of the segment's exact distance (step 1e-5 px; see the module docs): the
/// miter gauge at a vertex, whose tangent-dependent derivative is not written out. The
/// closed form costs one foot per row where the differences cost two exact distances per
/// variable (16 Newton solves for a cubic's row); on lucide at 512 px this pass took
/// 10-25 ms per iteration.
pub(super) fn normal_equations(
    model: &mut Model,
    b: &Boundary,
    rws: &[Option<Row>],
    theta0: &[f64],
    sig: &[f64],
    first: &[usize],
) -> (Skyline, Vec<f64>) {
    let n = model.theta.len();
    let hi = n - 1;
    let mut ata = Skyline::zeros(first.to_vec());
    let mut atr = vec![0.0; n];
    let eps = 1e-5;
    let h = model.h();
    // Each row's residual and Jacobian entries depend on the model and that row alone, so
    // they are computed in parallel (each worker on its own copy of the model, which the
    // central differences perturb and restore exactly); the products are then added into
    // `JᵀJ` and `Jᵀr` in row order, as before, so every sum is the sequential one.
    let jacs: Vec<Option<(f64, JacRow)>> = rws
        .par_iter()
        .enumerate()
        .map_init(
            || model.clone(),
            |m, (i, row)| {
                let row = row.as_ref()?;
                let (r, s, active) = b.residual(i, row.d, h);
                if !active {
                    return None;
                }
                let mut jac = JacRow::with(hi, -1.0 / s);
                if row.seg != usize::MAX {
                    // Under round joins and caps every row reads one segment's plain
                    // distance; a row whose foot is a miter vertex reads the gauge of two
                    // segments, and one beyond a butt end the end's gauge, instead.
                    let ev = match m.style {
                        Style {
                            join: Join::Round,
                            cap: Cap::Round,
                        } => None,
                        _ => Some(local_eval(m, row.shape, row.seg, b.pts[i], row.t)),
                    };
                    let with = ev.and_then(|e| e.with);
                    if ev.is_some_and(|e| e.h_free) {
                        // A butt end's face does not move with the width.
                        jac.e[0].1 = 0.0;
                    }
                    if !ev.is_some_and(|e| e.gauge)
                        && analytic_row(m, row.shape, row.seg, b.pts[i], row.t, &mut jac)
                    {
                        for e in jac.e[1..jac.n].iter_mut() {
                            e.1 /= s;
                        }
                    } else {
                        for v in locals(m, row.shape, row.seg, with) {
                            let x = m.theta[v];
                            m.theta[v] = x + eps;
                            let dp = local_dist(m, row.shape, row.seg, b.pts[i], row.t);
                            m.theta[v] = x - eps;
                            let dm = local_dist(m, row.shape, row.seg, b.pts[i], row.t);
                            m.theta[v] = x;
                            jac.push(v, (dp - dm) / (2.0 * eps * s));
                        }
                    }
                }
                Some((r, jac))
            },
        )
        .collect();
    for (r, jac) in jacs.into_iter().flatten() {
        let jac = jac.entries();
        for &(u, ju) in jac {
            atr[u] += ju * r;
            for &(v, jv) in jac {
                if v <= u {
                    ata.add(u, v, ju * jv);
                }
            }
        }
    }
    for j in 0..n {
        let w = 1.0 / (sig[j] * sig[j]);
        ata.add_diag(j, w);
        atr[j] += w * (model.theta[j] - theta0[j]);
    }
    (ata, atr)
}
