//! `INKVEC_GRADDBG=x0,y0,x1,y1`: after band merging, print every surviving component whose
//! bounding box meets the window, and a fresh union fit with each neighbour -- including the
//! flat-flat pairs the merge never looks at. Diagnostic only.

use super::*;
use std::sync::atomic::{AtomicBool, Ordering};

/// Set while `dump` fits a union, so `fit_samples` prints its candidates.
pub(crate) static VERBOSE: AtomicBool = AtomicBool::new(false);

pub(crate) fn verbose() -> bool {
    VERBOSE.load(Ordering::Relaxed)
}

/// While a union is being dumped: the candidates the fit weighed, and the step test.
pub(crate) fn candidates(n: usize, out: &[FillFit], two: f64, best_grad: f64) {
    if !verbose() {
        return;
    }
    let rest: Vec<String> = out[1..]
        .iter()
        .map(|f| format!("{} {:.1}/{:.1}", f.model.kind(), f.chi2, f.cost))
        .collect();
    eprintln!(
        "[gd]      n={n} flat chi2 {:.1} | two-flats {two:.1} vs best grad {best_grad:.1} | {}",
        out[0].chi2,
        rest.join(", ")
    );
}

/// The window from `INKVEC_GRADDBG`, if set.
pub(crate) fn window() -> Option<[usize; 4]> {
    let v = inkvec_core::env::text("INKVEC_GRADDBG")?;
    let p: Vec<usize> = v.split(',').filter_map(|s| s.trim().parse().ok()).collect();
    (p.len() == 4).then(|| [p[0], p[1], p[2], p[3]])
}

fn bbox(px: &[usize], w: usize) -> [usize; 4] {
    let mut b = [usize::MAX, usize::MAX, 0, 0];
    for &p in px {
        let (x, y) = (p % w, p / w);
        b[0] = b[0].min(x);
        b[1] = b[1].min(y);
        b[2] = b[2].max(x);
        b[3] = b[3].max(y);
    }
    b
}

/// Dump the components in the window. `fit` fits the union of two components. The
/// merger's state, passed piece by piece: it lives in separate locals there.
#[allow(clippy::too_many_arguments)]
pub(crate) fn dump(
    win: [usize; 4],
    w: usize,
    members: &[Vec<usize>],
    alive: &[bool],
    fits: &[FillFit],
    adj: &[HashMap<u32, u32>],
    rgb: &[[f32; 3]],
    fit: &dyn Fn(u32, u32) -> FillFit,
) {
    let meets =
        |b: [usize; 4]| b[0] <= win[2] && b[2] >= win[0] && b[1] <= win[3] && b[3] >= win[1];
    for c in 0..members.len() {
        if !alive[c] || members[c].is_empty() {
            continue;
        }
        let b = bbox(&members[c], w);
        if !meets(b) {
            continue;
        }
        let m = mean_rgb(rgb, &members[c]);
        eprintln!(
            "[gd] comp {c} n={} bbox={b:?} mean={} {} chi2 {:.1} cost {:.1}",
            members[c].len(),
            hex(m),
            fits[c].model.kind(),
            fits[c].chi2,
            fits[c].cost
        );
        let mut nb: Vec<(u32, u32)> = adj[c].iter().map(|(&k, &v)| (k, v)).collect();
        nb.sort();
        for (o, shared) in nb {
            if (o as usize) < c && meets(bbox(&members[o as usize], w)) {
                continue;
            }
            if shared < 3 || !alive[o as usize] {
                continue;
            }
            VERBOSE.store(true, Ordering::Relaxed);
            let u = fit(c as u32, o);
            VERBOSE.store(false, Ordering::Relaxed);
            eprintln!(
                "[gd]    + {o} (n={} {} {}) shared {shared}: union {} chi2 {:.1} cost {:.1} gain {:.1}",
                members[o as usize].len(),
                hex(mean_rgb(rgb, &members[o as usize])),
                fits[o as usize].model.kind(),
                u.model.kind(),
                u.chi2,
                u.cost,
                fits[c].cost + fits[o as usize].cost - u.cost
            );
        }
    }
}

fn mean_rgb(rgb: &[[f32; 3]], px: &[usize]) -> [f32; 3] {
    let mut m = [0.0f32; 3];
    for &p in px {
        for k in 0..3 {
            m[k] += rgb[p][k];
        }
    }
    let n = px.len().max(1) as f32;
    [m[0] / n, m[1] / n, m[2] / n]
}

fn hex(c: [f32; 3]) -> String {
    format!(
        "#{:02x}{:02x}{:02x}",
        (c[0] * 255.0).round() as u8,
        (c[1] * 255.0).round() as u8,
        (c[2] * 255.0).round() as u8
    )
}
