//! Multi-ribbon width pooling for thin strokes under a shared pen width.

use inkvec_core::Point;
use inkvec_fit::FitConfig;

use crate::gradient::{FillFit, FillModel, PARAMS_FLAT};
use crate::planar::{face_edge_order, PlanarMap};

use super::{
    build_problem, candidate_orders, is_polygonal, ring_of, varpro, write_back, EdgeSpan, Problem,
    Report, RingPt, MAX_BBOX_PIXELS, PARAMS_PER_RIBBON, SHARE_MAX_PX, THIN_PX,
};
use crate::clip::{coverage, simple, Bbox};

/// One straight ribbon: which face it is, the polygon that describes it, and the width
/// that polygon currently has.
pub(crate) struct Ribbon {
    pub(crate) face: usize,
    pub(crate) spans: Vec<EdgeSpan>,
    pub(crate) ring: Vec<RingPt>,
    pub(crate) cuts: Vec<usize>,
    pub(crate) verts: Vec<Point>,
    pub(crate) normal: (f64, f64),
    pub(crate) centre: f64,
    pub(crate) width: f64,
}

/// Width of a quadrilateral ribbon, with the normal it is measured along and where its
/// centreline sits on that normal. `None` for anything that is not a four-sided ribbon.
pub(crate) fn ribbon_width(v: &[Point]) -> Option<((f64, f64), f64, f64)> {
    if v.len() != 4 {
        return None;
    }
    let mut best = (0usize, 0.0f64);
    for i in 0..4 {
        let d = v[i].dist(v[(i + 1) % 4]);
        if d > best.1 {
            best = (i, d);
        }
    }
    let (a, b) = (v[best.0], v[(best.0 + 1) % 4]);
    let (dx, dy) = (b.x - a.x, b.y - a.y);
    let len = (dx * dx + dy * dy).sqrt();
    if len < 4.0 {
        return None;
    }
    let n = (-dy / len, dx / len);
    let (mut lo, mut hi) = (f64::MAX, f64::MIN);
    for p in v {
        let t = p.x * n.0 + p.y * n.1;
        lo = lo.min(t);
        hi = hi.max(t);
    }
    let w = hi - lo;
    // Negated deliberately: a degenerate ribbon can make `w` NaN, and NaN must fail this
    // test rather than pass it. `w <= 0.05` would let it through.
    #[allow(clippy::neg_cmp_op_on_partial_ord)]
    if !(w > 0.05) || w > 12.0 {
        return None;
    }
    Some((n, (lo + hi) * 0.5, w))
}

/// The same polygon with its width set to `w`, both sides moved symmetrically about the
/// ribbon's own centreline so the stroke keeps its position and its direction.
pub(crate) fn set_width(v: &[Point], n: (f64, f64), centre: f64, w: f64) -> Vec<Point> {
    let half = w * 0.5;
    v.iter()
        .map(|p| {
            let t = p.x * n.0 + p.y * n.1 - centre;
            let target = if t >= 0.0 { half } else { -half };
            let d = target - t;
            Point::new(p.x + n.0 * d, p.y + n.1 * d)
        })
        .collect()
}

/// Pool the straight ribbons of one image under a single shared width.
#[allow(clippy::too_many_arguments)]
pub(crate) fn share_widths(
    map: &mut PlanarMap,
    rgb: &[[f32; 3]],
    labels: &[u16],
    face_fill: &mut [FillFit],
    lambda: f64,
    rep: &mut Report,
    dbg: bool,
    _leak: f64,
) {
    let (w, h) = (map.width, map.height);
    let thin_px = inkvec_core::env::number("INKVEC_DECODE_THIN").unwrap_or(THIN_PX);
    let order = face_edge_order(map);
    let n_faces = face_fill.len();
    let mut area = vec![0usize; n_faces];
    for &l in labels.iter() {
        let f = l as usize;
        if f < n_faces {
            area[f] += 1;
        }
    }

    let extent = (w.max(h)) as f64;
    let cfg = FitConfig::from_precision(extent, 0.1, 2.0);
    let mut ribs: Vec<Ribbon> = Vec::new();
    for f in 0..n_faces.min(order.len()) {
        if area[f] == 0 || area[f] > MAX_BBOX_PIXELS {
            continue;
        }
        let Some((ring, spans)) = ring_of(map, &order[f]) else {
            continue;
        };
        let pts: Vec<Point> = ring.iter().map(|r| r.p).collect();
        let perim: f64 = (0..ring.len())
            .map(|i| ring[i].p.dist(ring[(i + 1) % ring.len()].p))
            .sum();
        if perim < 1e-9 || 2.0 * area[f] as f64 / perim > thin_px {
            continue;
        }
        let (orders, _) = candidate_orders(map, &ring, &spans, &cfg);
        let Some((cuts, verts)) = orders.into_iter().find(|(c, _)| c.len() == 4) else {
            continue;
        };
        if !is_polygonal(&pts, &verts, &cuts) {
            continue;
        }
        // A corner that is a junction is shared with faces this pass is not solving, and
        // cannot be moved by this one alone.
        if cuts.iter().any(|&i| ring[i].junction) {
            continue;
        }
        let Some((normal, centre, width)) = ribbon_width(&verts) else {
            continue;
        };
        // Pool only where a ribbon cannot be measured on its own. Above about two pixels
        // the label map anchors the width well enough that a single stroke is already
        // exact -- measured: four 2 px bars come back with zero spread and zero error --
        // and forcing them onto a common value can then only move them off it. The
        // ambiguity this pass exists to break lives below that, which is where h3b found
        // the recovered width depending on where the solver started.
        if width > SHARE_MAX_PX {
            continue;
        }
        ribs.push(Ribbon {
            face: f,
            spans,
            ring,
            cuts,
            verts,
            normal,
            centre,
            width,
        });
    }

    if ribs.len() < 2 {
        return;
    }
    let mut ws: Vec<f64> = ribs.iter().map(|r| r.width).collect();
    ws.sort_by(f64::total_cmp);
    let med = ws[ws.len() / 2];

    let mut probs: Vec<Problem> = Vec::new();
    let mut keep: Vec<usize> = Vec::new();
    for (i, r) in ribs.iter().enumerate() {
        let pts: Vec<Point> = r.ring.iter().map(|p| p.p).collect();
        let Some(bb) = Bbox::of(&pts, w, h, 2.0) else {
            continue;
        };
        let Some(p) = build_problem(map, &order, rgb, labels, face_fill, r.face, bb, w, h) else {
            continue;
        };
        probs.push(p);
        keep.push(i);
    }
    if keep.len() < 2 {
        return;
    }

    let cap = probs.iter().map(|p| p.bb.len()).max().unwrap_or(0);
    let mut cov = vec![0.0f64; cap];
    let mut mark = vec![false; cap];

    let mut base_sse = 0.0;
    for (slot, &i) in keep.iter().enumerate() {
        base_sse += probs[slot].eval(&ribs[i].verts, &mut cov, &mut mark).0;
    }
    let base_j =
        base_sse + lambda * (keep.len() as f64 * PARAMS_PER_RIBBON + keep.len() as f64 * 3.0);

    let joint = |wid: f64, cov: &mut [f64], mark: &mut [bool]| -> f64 {
        let k = keep.len();
        let mut covers: Vec<Vec<f64>> = Vec::with_capacity(k);
        for &i in keep.iter() {
            let v = set_width(&ribs[i].verts, ribs[i].normal, ribs[i].centre, wid);
            if !simple(&v) {
                return f64::INFINITY;
            }
            let slot = covers.len();
            coverage(&v, probs[slot].bb, cov, mark);
            covers.push(cov[..probs[slot].bb.len()].to_vec());
        }
        // Columns: 0 is the shared ink, 1..=k is each ribbon's surround.
        let total: usize = probs.iter().map(|p| p.target.len()).sum();
        let mut cols: Vec<Vec<f64>> = vec![vec![0.0; total]; k + 1];
        let mut target: Vec<[f32; 3]> = Vec::with_capacity(total);
        let mut base = 0usize;
        for slot in 0..k {
            let p = &probs[slot];
            for q in 0..p.target.len() {
                let a = covers[slot][q].clamp(0.0, 1.0);
                cols[0][base + q] = a;
                cols[slot + 1][base + q] = (1.0 - a - p.rest[q]).max(0.0);
                let mut t = p.target[q];
                for ch in 0..3 {
                    t[ch] -= p.rest_rgb[q][ch];
                }
                target.push(t);
            }
            base += p.target.len();
        }
        let mut prior: Vec<[f32; 3]> = Vec::with_capacity(k + 1);
        prior.push(probs[0].prior[0]);
        for slot in 0..k {
            prior.push(*probs[slot].prior.get(1).unwrap_or(&probs[slot].prior[0]));
        }
        varpro(&cols, &target, &prior).1
    };
    let score = joint;

    let (lo, hi) = (0.4 * med, 2.0 * med);
    let steps = 40;
    let mut best = (f64::INFINITY, med);
    for k in 0..=steps {
        let wid = lo + (hi - lo) * k as f64 / steps as f64;
        let s = score(wid, &mut cov, &mut mark);
        if s < best.0 {
            best = (s, wid);
        }
    }
    let step = (hi - lo) / steps as f64;
    for k in -8..=8i32 {
        let wid = best.1 + step * k as f64 / 8.0;
        if wid <= 0.05 {
            continue;
        }
        let s = score(wid, &mut cov, &mut mark);
        if s < best.0 {
            best = (s, wid);
        }
    }
    if !best.0.is_finite() {
        return;
    }

    let params_after = keep.len() as f64 * PARAMS_PER_RIBBON - (keep.len() as f64 - 1.0) + 3.0;
    let new_j = best.0 + lambda * params_after;

    #[allow(clippy::neg_cmp_op_on_partial_ord)]
    if !(new_j < base_j) || !(best.0 < base_sse) {
        if dbg {
            eprintln!(
                "  [decode] share: {} ribbons at w {:.3}, sse {base_sse:.3} -> {:.3}, J {base_j:.3} -> {new_j:.3}, refused",
                keep.len(),
                best.1,
                best.0
            );
        }
        return;
    }

    let shared_ink = {
        let k = keep.len();
        let mut covers: Vec<Vec<f64>> = Vec::with_capacity(k);
        for &i in keep.iter() {
            let v = set_width(&ribs[i].verts, ribs[i].normal, ribs[i].centre, best.1);
            let slot = covers.len();
            coverage(&v, probs[slot].bb, &mut cov, &mut mark);
            covers.push(cov[..probs[slot].bb.len()].to_vec());
        }
        let total: usize = probs.iter().map(|p| p.target.len()).sum();
        let mut cols: Vec<Vec<f64>> = vec![vec![0.0; total]; k + 1];
        let mut target: Vec<[f32; 3]> = Vec::with_capacity(total);
        let mut base = 0usize;
        for slot in 0..k {
            let p = &probs[slot];
            for q in 0..p.target.len() {
                let a = covers[slot][q].clamp(0.0, 1.0);
                cols[0][base + q] = a;
                cols[slot + 1][base + q] = (1.0 - a - p.rest[q]).max(0.0);
                let mut t = p.target[q];
                for ch in 0..3 {
                    t[ch] -= p.rest_rgb[q][ch];
                }
                target.push(t);
            }
            base += p.target.len();
        }
        let mut prior: Vec<[f32; 3]> = Vec::with_capacity(k + 1);
        prior.push(probs[0].prior[0]);
        for slot in 0..k {
            prior.push(*probs[slot].prior.get(1).unwrap_or(&probs[slot].prior[0]));
        }
        varpro(&cols, &target, &prior).0[0]
    };
    for &i in keep.iter() {
        let v = set_width(&ribs[i].verts, ribs[i].normal, ribs[i].centre, best.1);
        write_back(map, &ribs[i].ring, &ribs[i].spans, &ribs[i].cuts, &v);
        face_fill[ribs[i].face].model = FillModel::Flat(shared_ink);
        face_fill[ribs[i].face].params = PARAMS_FLAT;
    }
    rep.pooled = keep.len();
    rep.shared_width = best.1;
    rep.d_objective += new_j - base_j;
    if dbg {
        eprintln!(
            "  [decode] share: {} ribbons pooled at w {:.3}, J {base_j:.3} -> {new_j:.3}",
            keep.len(),
            best.1
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ribbon_width_and_set_width() {
        // A horizontal rectangle of width 1.0, length 10.0 from x=0 to 10, y=2.0 to 3.0
        let rect = vec![
            Point::new(0.0, 2.0),
            Point::new(10.0, 2.0),
            Point::new(10.0, 3.0),
            Point::new(0.0, 3.0),
        ];
        let info = ribbon_width(&rect);
        assert!(info.is_some());
        let (normal, centre, width) = info.unwrap();
        assert!((width - 1.0).abs() < 1e-6, "width was {}", width);
        assert!(
            (centre - 2.5).abs() < 1e-6 || (centre + 2.5).abs() < 1e-6,
            "centre was {}",
            centre
        );

        // Adjust width to 2.0
        let widened = set_width(&rect, normal, centre, 2.0);
        assert_eq!(widened.len(), 4);
        let info_wide = ribbon_width(&widened);
        assert!(info_wide.is_some());
        let (_, _, w2) = info_wide.unwrap();
        assert!((w2 - 2.0).abs() < 1e-6, "w2 was {}", w2);
    }

    #[test]
    fn non_quad_is_not_ribbon() {
        let tri = vec![
            Point::new(0.0, 0.0),
            Point::new(10.0, 0.0),
            Point::new(5.0, 5.0),
        ];
        assert!(ribbon_width(&tri).is_none());
    }
}
