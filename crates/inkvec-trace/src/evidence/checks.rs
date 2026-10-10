//! Checker call sites: where a result a theorem speaks about is accepted only if a checker
//! generated from Lean agrees (`docs/theory/verification.md`).
//!
//! * [`certify_run_chi2_polyline`]: the `χ²` of a polyline candidate on a set of run windows.
//!   The candidate is cut, per window, at the window's strip and ends (the certificate: the
//!   straight pieces and the clamped height of each); every piece's area is the generated
//!   `trapezoid_iv` (`Inkvec.Gen.trapezoidK_integral`: the integral of the piece's height above
//!   the window's end), each window's term the generated `window_term_iv`, and the sum an
//!   outward-rounded enclosure. The window identity (`Inkvec.window_sum_eq_area`) is what
//!   makes the measured sums areas at all.
//! * [`certify_corner`]: a corner proposal's fourth difference is at least `z` of its standard
//!   deviations (`corner_excess_iv`, `Inkvec.Gen.cornerExcessK_eq`; `fourthDiff_cubic` says a
//!   smooth run reads zero there).
//!
//! What is trusted beyond the generated kernels: the cutting (where each piece meets the
//! strip's sides and the window's ends, computed in `f64`; a cut a rounding away from the
//! exact one moves the area by the same rounding), and the combination of the per-piece sums
//! into the left face's area by the runtime's interval operations.

use inkvec_core::likelihood::{Axis, RunObs, Window};
use inkvec_core::Point;
use inkvec_verified::generated::evidence::{corner_excess_iv, trapezoid_iv, window_term_iv};
use inkvec_verified::iv::{self, Iv};

/// Why a checker refused a result.
#[derive(Debug, Clone, PartialEq)]
pub enum Refused {
    /// The candidate did not cross a window's strip exactly once.
    NotACrossing {
        /// Index of the window in the range.
        window: usize,
    },
    /// The enclosure could not be formed (a non-finite input, an overflow).
    Arithmetic,
    /// The claimed value lies outside the certified enclosure.
    Mismatch {
        /// The solver's value.
        claimed: f64,
        /// The certified enclosure.
        lo: f64,
        /// The certified enclosure.
        hi: f64,
    },
}

/// The straight pieces of segment `a → b` (strip coordinates `(u, v)`) inside the strip
/// `u ∈ [u0, u1]`, cut where `v` crosses `vlo` and `vhi`, with each piece's heights clamped to
/// `[vlo, vhi]`: the certificate `trapezoid` is evaluated on.
fn pieces(a: (f64, f64), b: (f64, f64), u0: f64, u1: f64, vlo: f64, vhi: f64) -> Vec<[f64; 4]> {
    let (du, dv) = (b.0 - a.0, b.1 - a.1);
    if du == 0.0 {
        return Vec::new();
    }
    let (ta, tb) = {
        let t0 = (u0 - a.0) / du;
        let t1 = (u1 - a.0) / du;
        (t0.min(t1).max(0.0), t0.max(t1).min(1.0))
    };
    if tb <= ta {
        return Vec::new();
    }
    let mut ts = vec![ta, tb];
    if dv != 0.0 {
        for lv in [vlo, vhi] {
            let t = (lv - a.1) / dv;
            if t > ta && t < tb {
                ts.push(t);
            }
        }
    }
    ts.sort_by(f64::total_cmp);
    ts.windows(2)
        .map(|w| {
            let (t0, t1) = (w[0], w[1]);
            let clamp = |v: f64| v.clamp(vlo, vhi);
            [
                a.0 + t0 * du,
                clamp(a.1 + t0 * dv),
                a.0 + t1 * du,
                clamp(a.1 + t1 * dv),
            ]
        })
        .collect()
}

/// A certified enclosure of the area polyline `pts` gives its left face in window `w`, from
/// its passes through the strip that come within a pixel of the window; `Err` when those do
/// not advance one strip width in all, either way, or the arithmetic refuses.
pub fn certified_left_area(pts: &[Point], w: &Window) -> Result<Iv, Refused> {
    let (u0, u1) = (w.line as f64 - 0.5, w.line as f64 + 0.5);
    let (vlo, vhi) = (w.lo as f64 - 0.5, w.hi as f64 + 0.5);
    let uv = |p: Point| match w.axis {
        Axis::Column => (p.x, p.y),
        Axis::Row => (p.y, p.x),
    };
    let inside = |u: f64| u >= u0 && u <= u1;
    // Passes: runs of consecutive segments that stay in the strip from one to the next.
    struct Pass {
        area: Iv,
        adv: f64,
        vmin: f64,
        vmax: f64,
    }
    let zero = Iv::point(0.0).ok_or(Refused::Arithmetic)?;
    let mut passes: Vec<Pass> = Vec::new();
    let mut open = false;
    for s in pts.windows(2) {
        let (a, b) = (uv(s[0]), uv(s[1]));
        let ps = pieces(a, b, u0, u1, vlo, vhi);
        if ps.is_empty() {
            open = false;
            continue;
        }
        let mut area = zero;
        let mut adv = 0.0;
        // The segment's unclamped heights where it is inside the strip decide nearness, as
        // in the evaluator.
        let (vmin, vmax) = {
            let du = b.0 - a.0;
            let (p, q) = ((u0 - a.0) / du, (u1 - a.0) / du);
            let (ta, tb) = (p.min(q).max(0.0), p.max(q).min(1.0));
            let (va, vb) = (a.1 + ta * (b.1 - a.1), a.1 + tb * (b.1 - a.1));
            (va.min(vb), va.max(vb))
        };
        for p in &ps {
            let x = [p[0], p[1], p[2], p[3], vlo].map(Iv::point);
            let x = [
                x[0].ok_or(Refused::Arithmetic)?,
                x[1].ok_or(Refused::Arithmetic)?,
                x[2].ok_or(Refused::Arithmetic)?,
                x[3].ok_or(Refused::Arithmetic)?,
                x[4].ok_or(Refused::Arithmetic)?,
            ];
            let [t] = trapezoid_iv(&x).ok_or(Refused::Arithmetic)?;
            area = iv::add(area, t).ok_or(Refused::Arithmetic)?;
            adv += p[2] - p[0];
        }
        match passes.last_mut() {
            Some(p) if open && inside(a.0) => {
                p.area = iv::add(p.area, area).ok_or(Refused::Arithmetic)?;
                p.adv += adv;
                p.vmin = p.vmin.min(vmin);
                p.vmax = p.vmax.max(vmax);
            }
            _ => passes.push(Pass {
                area,
                adv,
                vmin,
                vmax,
            }),
        }
        open = inside(b.0);
    }
    let (mut i, mut adv) = (zero, 0.0);
    for p in passes
        .iter()
        .filter(|p| p.vmax >= vlo - 1.0 && p.vmin <= vhi + 1.0)
    {
        i = iv::add(i, p.area).ok_or(Refused::Arithmetic)?;
        adv += p.adv;
    }
    let dir = if (adv - 1.0).abs() < 1e-9 {
        1.0
    } else if (adv + 1.0).abs() < 1e-9 {
        -1.0
    } else {
        return Err(Refused::NotACrossing { window: 0 });
    };
    let h = Iv::point(vhi - vlo).ok_or(Refused::Arithmetic)?;
    // Column: I + h(1 − Δ)/2; row: −I + h(1 + Δ)/2, with Δ = ±1 exactly.
    let a = match w.axis {
        Axis::Column if dir > 0.0 => Some(i),
        Axis::Column => iv::add(i, h),
        Axis::Row if dir > 0.0 => iv::sub(h, i),
        Axis::Row => iv::neg(i),
    }
    .ok_or(Refused::Arithmetic)?;
    Ok(a)
}

/// The area of the left face of polyline `pts` in window `w` (the midpoint of the certified
/// enclosure), or `None` when the checker refuses.
pub fn polyline_left_area(pts: &[Point], w: &Window) -> Option<f64> {
    certified_left_area(pts, w)
        .ok()
        .map(|a| 0.5 * (a.lo + a.hi))
}

/// Certify a solver's `χ²` of a polyline candidate on run windows: the generated kernels'
/// enclosure must contain it, widened by `tol` relative to `1 + χ²` (the cut points are
/// computed in `f64`). Returns the enclosure's midpoint.
pub fn certify_run_chi2_polyline(
    obs: &[RunObs],
    pts: &[Point],
    claimed: f64,
    tol: f64,
) -> Result<f64, Refused> {
    let mut chi = Iv::point(0.0).ok_or(Refused::Arithmetic)?;
    for (k, o) in obs.iter().enumerate() {
        let a = certified_left_area(pts, &o.window).map_err(|e| match e {
            Refused::NotACrossing { .. } => Refused::NotACrossing { window: k },
            other => other,
        })?;
        let s = Iv::point(o.sum).ok_or(Refused::Arithmetic)?;
        let v = Iv::point(o.var).ok_or(Refused::Arithmetic)?;
        let [term, _] = window_term_iv(&[s, a, v]).ok_or(Refused::Arithmetic)?;
        chi = iv::add(chi, term).ok_or(Refused::Arithmetic)?;
    }
    let slack = tol * (1.0 + chi.hi.abs());
    if claimed < chi.lo - slack || claimed > chi.hi + slack {
        return Err(Refused::Mismatch {
            claimed,
            lo: chi.lo,
            hi: chi.hi,
        });
    }
    Ok(0.5 * (chi.lo + chi.hi))
}

/// Certify a corner proposal: the fourth difference of five consecutive window means `h` with
/// variances `v` is at least `z` of its standard deviations (`corner_excess_iv` non-negative).
/// A refusal drops the proposal, which only costs information (the windows stay a run).
pub fn certify_corner(h: [f64; 5], v: [f64; 5], z: f64) -> bool {
    let mut x = [Iv { lo: 0.0, hi: 0.0 }; 11];
    for k in 0..5 {
        let (Some(a), Some(b)) = (Iv::point(h[k]), Iv::point(v[k])) else {
            return false;
        };
        x[k] = a;
        x[5 + k] = b;
    }
    let Some(z2) = Iv::point(z * z) else {
        return false;
    };
    x[10] = z2;
    corner_excess_iv(&x).is_some_and(|[e]| e.is_nonneg())
}
