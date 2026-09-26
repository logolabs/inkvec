//! Connected components, blend absorption, saddle disambiguation, and despeckle (DESIGN.md S1/S2).
//!
//! This module represents Stage 4 ("Regions") of the raster analysis pipeline,
//! partitioning quantized pixel labels into connected faces and resolving ambiguities
//! prior to planar map construction.

use crate::color::Palette;
#[cfg(feature = "research")]
use crate::gradient;

/// How far from the half-way mark a corner must sit before the image is taken to have
/// answered: three sigma of the propagated coverage noise. Below that the two readings
/// are indistinguishable, and the corner keeps the junction it has always had.
#[cfg(feature = "research")]
pub const SADDLE_SIGMAS: f64 = 3.0;

/// Join the faces that four pixels meeting at one corner say are one shape.
///
/// An experiment: compiled only with the `research` feature, and even then a no-op unless
/// `INKVEC_SADDLE` is set.
#[cfg(feature = "research")]
#[allow(clippy::too_many_arguments)]
pub fn merge_saddle_faces(
    mut labels: Vec<u16>,
    w: usize,
    h: usize,
    rgb: &[[f32; 3]],
    pal: &Palette,
    face_fill: Vec<gradient::FillFit>,
    face_color: Vec<usize>,
    n_faces: usize,
    sigma_noise: f64,
) -> (Vec<u16>, Vec<gradient::FillFit>, Vec<usize>, usize) {
    if !inkvec_core::env::flag("INKVEC_SADDLE") || w < 2 || h < 2 {
        return (labels, face_fill, face_color, n_faces);
    }
    let dbg = inkvec_core::env::flag("INKVEC_SADDLEDBG");
    let mut parent: Vec<u16> = (0..n_faces as u16).collect();
    fn find(parent: &mut [u16], x: u16) -> u16 {
        let mut r = x;
        while parent[r as usize] != r {
            r = parent[r as usize];
        }
        let mut c = x;
        while parent[c as usize] != r {
            let next = parent[c as usize];
            parent[c as usize] = r;
            c = next;
        }
        r
    }

    let mut joined = 0usize;
    for j in 1..h {
        for i in 1..w {
            let at = |x: usize, y: usize| labels[y * w + x];
            let (nw, ne) = (at(i - 1, j - 1), at(i, j - 1));
            let (sw, se) = (at(i - 1, j), at(i, j));
            if nw == ne || nw == sw || ne == se || sw == se {
                continue;
            }
            let ink = |f: u16| face_color.get(f as usize).copied();
            let (Some(a), Some(b), Some(c), Some(d)) = (ink(nw), ink(se), ink(ne), ink(sw)) else {
                continue;
            };
            if a != b || c != d || a == c {
                continue;
            }
            let (Some(ca), Some(cb)) = (pal.rgb.get(a), pal.rgb.get(c)) else {
                continue;
            };
            let sep = [ca[0] - cb[0], ca[1] - cb[1], ca[2] - cb[2]];
            let norm = (sep[0] * sep[0] + sep[1] * sep[1] + sep[2] * sep[2]) as f64;
            if norm < 1e-6 {
                continue;
            }
            let cover = |x: usize, y: usize| -> f64 {
                let p = rgb[y * w + x];
                let t = ((p[0] - cb[0]) * sep[0]
                    + (p[1] - cb[1]) * sep[1]
                    + (p[2] - cb[2]) * sep[2]) as f64
                    / norm;
                t.clamp(0.0, 1.0)
            };
            let (cnw, cne) = (cover(i - 1, j - 1), cover(i, j - 1));
            let (csw, cse) = (cover(i - 1, j), cover(i, j));
            let den = cnw + cse - cne - csw;
            let num = cnw * cse - cne * csw;
            let degenerate = den.abs() < 1e-9;
            let at_corner = if degenerate {
                (cnw + cne + csw + cse) / 4.0
            } else {
                num / den
            };
            let sigma_t = sigma_noise / norm.sqrt();
            let sigma_corner = if degenerate {
                sigma_t / 2.0
            } else {
                let d2 = den * den;
                let g = [
                    (cse * den - num) / d2,
                    (cnw * den - num) / d2,
                    (num - csw * den) / d2,
                    (num - cne * den) / d2,
                ];
                sigma_t * g.iter().map(|x| x * x).sum::<f64>().sqrt()
            };
            let decisive = (at_corner - 0.5).abs() > SADDLE_SIGMAS * sigma_corner;
            let (x, y) = if at_corner > 0.5 { (nw, se) } else { (ne, sw) };
            if dbg {
                let verdict = if decisive {
                    format!("joining faces {x} and {y}")
                } else {
                    "TIED, left a junction".to_string()
                };
                eprintln!(
                    "  [saddle] ({i},{j}) inks {a}/{c} corner {at_corner:.4} \
+-{sigma_corner:.4} -> {verdict}"
                );
            }
            if !decisive {
                continue;
            }

            let (rx, ry) = (find(&mut parent, x), find(&mut parent, y));
            if rx != ry {
                parent[rx.max(ry) as usize] = rx.min(ry);
                joined += 1;
            }
        }
    }
    if joined == 0 {
        return (labels, face_fill, face_color, n_faces);
    }

    for l in labels.iter_mut() {
        *l = find(&mut parent, *l);
    }
    if dbg {
        eprintln!(
            "  [saddle] joined {joined} corner(s), {} face(s) absorbed",
            joined
        );
    }
    (labels, face_fill, face_color, n_faces)
}

/// Relabel a palette-indexed image by 4-connected component.
pub fn split_components(labels: &[u16], w: usize, h: usize) -> (Vec<u16>, Vec<usize>) {
    let n = w * h;
    let mut out = vec![u16::MAX; n];
    let mut face_color: Vec<usize> = Vec::new();
    let mut stack: Vec<usize> = Vec::new();

    for seed in 0..n {
        if out[seed] != u16::MAX {
            continue;
        }
        if face_color.len() >= u16::MAX as usize {
            break;
        }
        let lab = labels[seed];
        let id = face_color.len() as u16;
        face_color.push(lab as usize);

        out[seed] = id;
        stack.push(seed);
        while let Some(p) = stack.pop() {
            let (x, y) = (p % w, p / w);
            let visit = |q: usize, stack: &mut Vec<usize>, out: &mut Vec<u16>| {
                if out[q] == u16::MAX && labels[q] == lab {
                    out[q] = id;
                    stack.push(q);
                }
            };
            if x > 0 {
                visit(p - 1, &mut stack, &mut out);
            }
            if x + 1 < w {
                visit(p + 1, &mut stack, &mut out);
            }
            if y > 0 {
                visit(p - w, &mut stack, &mut out);
            }
            if y + 1 < h {
                visit(p + w, &mut stack, &mut out);
            }
        }
    }

    for v in out.iter_mut() {
        if *v == u16::MAX {
            *v = 0;
        }
    }
    (out, face_color)
}

/// Colour of the backdrop the image was composited onto.
pub const BACKDROP: [f32; 3] = [1.0, 1.0, 1.0];

/// Explain `c` as a convex mixture of the inks in `cols`.
pub fn mixture(c: [f32; 3], cols: &[[f32; 3]]) -> Option<(f32, usize)> {
    let k = cols.len();
    if k < 2 {
        return None;
    }
    let d2 = |a: [f32; 3], b: [f32; 3]| {
        let e = [a[0] - b[0], a[1] - b[1], a[2] - b[2]];
        e[0] * e[0] + e[1] * e[1] + e[2] * e[2]
    };
    let mut best: Option<(f32, usize)> = None;
    let mut offer = |r2: f32, who: usize| {
        if best.is_none_or(|(b, _)| r2 < b) {
            best = Some((r2, who));
        }
    };
    for i in 0..k {
        for j in i + 1..k {
            let (a, b) = (cols[i], cols[j]);
            let u = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
            let uu = u[0] * u[0] + u[1] * u[1] + u[2] * u[2];
            if uu < 1e-12 {
                continue;
            }
            let w = [c[0] - a[0], c[1] - a[1], c[2] - a[2]];
            let t = ((w[0] * u[0] + w[1] * u[1] + w[2] * u[2]) / uu).clamp(0.0, 1.0);
            let q = [a[0] + u[0] * t, a[1] + u[1] * t, a[2] + u[2] * t];
            offer(d2(c, q), if t < 0.5 { i } else { j });
        }
    }
    for i in 0..k {
        for j in i + 1..k {
            for m in j + 1..k {
                let (a, b, e) = (cols[i], cols[j], cols[m]);
                let u = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
                let v = [e[0] - a[0], e[1] - a[1], e[2] - a[2]];
                let w = [c[0] - a[0], c[1] - a[1], c[2] - a[2]];
                let uu = u[0] * u[0] + u[1] * u[1] + u[2] * u[2];
                let vv = v[0] * v[0] + v[1] * v[1] + v[2] * v[2];
                let uv = u[0] * v[0] + u[1] * v[1] + u[2] * v[2];
                let det = uu * vv - uv * uv;
                if det.abs() < 1e-12 {
                    continue;
                }
                let wu = w[0] * u[0] + w[1] * u[1] + w[2] * u[2];
                let wv = w[0] * v[0] + w[1] * v[1] + w[2] * v[2];
                let sc = (vv * wu - uv * wv) / det;
                let tc = (uu * wv - uv * wu) / det;
                if sc < 0.0 || tc < 0.0 || sc + tc > 1.0 {
                    continue;
                }
                let q = [
                    a[0] + u[0] * sc + v[0] * tc,
                    a[1] + u[1] * sc + v[1] * tc,
                    a[2] + u[2] * sc + v[2] * tc,
                ];
                let wa = 1.0 - sc - tc;
                let who = if wa >= sc && wa >= tc {
                    i
                } else if sc >= tc {
                    j
                } else {
                    m
                };
                offer(d2(c, q), who);
            }
        }
    }
    best.map(|(r2, who)| (r2.sqrt(), who))
}

/// Dissolve thin components that are colour blends of their dominant neighbours.
pub fn absorb_blend_slivers(
    labels: &mut [u16],
    rgb: &[[f32; 3]],
    alpha: &[f32],
    w: usize,
    h: usize,
    pal: &Palette,
    sigma_noise: f64,
) -> usize {
    let n = w * h;
    let tol = (3.0 * sigma_noise).max(0.025) as f32;
    let mut absorbed = 0usize;

    for _round in 0..2 {
        let mut comp = vec![u32::MAX; n];
        let mut members: Vec<Vec<usize>> = Vec::new();
        for start in 0..n {
            if comp[start] != u32::MAX {
                continue;
            }
            let id = members.len() as u32;
            let lab = labels[start];
            let mut stack = vec![start];
            let mut group = Vec::new();
            comp[start] = id;
            while let Some(p) = stack.pop() {
                group.push(p);
                let (x, y) = (p % w, p / w);
                let mut visit = |q: usize| {
                    if comp[q] == u32::MAX && labels[q] == lab {
                        comp[q] = id;
                        stack.push(q);
                    }
                };
                if x > 0 {
                    visit(p - 1);
                }
                if x + 1 < w {
                    visit(p + 1);
                }
                if y > 0 {
                    visit(p - w);
                }
                if y + 1 < h {
                    visit(p + w);
                }
            }
            members.push(group);
        }

        let mut changed = 0usize;
        let n_labels = labels
            .iter()
            .copied()
            .max()
            .map_or(1, |m| m as usize + 1)
            .max(pal.rgb.len());
        let mut contacts: Vec<usize> = vec![0; n_labels];
        for group in &members {
            let area = group.len();
            if area == 0 {
                continue;
            }
            let id = comp[group[0]];
            let mut interior = 0usize;
            contacts.iter_mut().for_each(|c| *c = 0);
            let mut foreign = 0usize;
            for &p in group {
                let (x, y) = (p % w, p / w);
                let mut inside = true;
                for q in [
                    (x > 0).then(|| p - 1),
                    (x + 1 < w).then(|| p + 1),
                    (y > 0).then(|| p - w),
                    (y + 1 < h).then(|| p + w),
                ]
                .into_iter()
                .flatten()
                {
                    if comp[q] != id {
                        inside = false;
                        contacts[labels[q] as usize] += 1;
                        foreign += 1;
                    }
                }
                if inside {
                    interior += 1;
                }
            }
            let absdbg = inkvec_core::env::flag("INKVEC_ABSDBG");
            if interior * 5 >= area || foreign == 0 {
                if absdbg && area > 15 && interior * 5 >= area {
                    eprintln!("abs: comp area {area} NOT thin (interior {interior})");
                }
                continue;
            }
            let mut tally: Vec<(usize, u16)> = contacts
                .iter()
                .enumerate()
                .filter(|(_, &c)| c > 0)
                .map(|(l, &c)| (c, l as u16))
                .collect();
            tally.sort_by_key(|&(c, l)| (std::cmp::Reverse(c), l));
            tally.truncate(3);
            let covered: usize = tally.iter().map(|&(c, _)| c).sum();
            if covered * 5 < foreign * 4 {
                if absdbg && area > 15 {
                    eprintln!(
                        "abs: comp area {area} contacts too spread (top {covered} of {foreign})"
                    );
                }
                continue;
            }
            let mut labs: Vec<Option<u16>> = tally.iter().map(|&(_, l)| Some(l)).collect();
            let mut cols: Vec<[f32; 3]> = Vec::with_capacity(4);
            for l in &labs {
                let Some(&c) = pal.rgb.get(l.unwrap() as usize) else {
                    continue;
                };
                cols.push(c);
            }
            if cols.len() != labs.len() || cols.len() < 2 {
                continue;
            }
            if group.iter().any(|&p| alpha[p] < 0.99) && !cols.contains(&BACKDROP) {
                cols.push(BACKDROP);
                labs.push(None);
            }

            let mut pass = 0usize;
            let mut dest: Vec<u16> = Vec::with_capacity(area);
            for &p in group {
                let Some((r, who)) = mixture(rgb[p], &cols) else {
                    dest.push(labels[p]);
                    continue;
                };
                if r <= tol {
                    pass += 1;
                }
                dest.push(match labs[who] {
                    Some(l) => l,
                    None => {
                        let real: Vec<[f32; 3]> = cols[..labs.len() - 1].to_vec();
                        match mixture(rgb[p], &real) {
                            Some((_, w2)) => labs[w2].unwrap(),
                            None => tally[0].1,
                        }
                    }
                });
            }
            if pass * 5 < area * 4 {
                if absdbg && area > 15 {
                    eprintln!("abs: comp area {area} blend fail ({pass} of {area} pass)");
                }
                continue;
            }
            for (&p, &l) in group.iter().zip(&dest) {
                labels[p] = l;
            }
            changed += 1;
        }
        absorbed += changed;
        if changed == 0 {
            break;
        }
    }
    absorbed
}

/// Reassign individual boundary pixels that a neighbour-pair blend explains strictly better.
pub fn reassign_blend_pixels(
    labels: &mut [u16],
    rgb: &[[f32; 3]],
    alpha: &[f32],
    w: usize,
    h: usize,
    pal: &Palette,
    sigma_noise: f64,
) -> usize {
    const ROUNDS: usize = 4;
    let n = w * h;
    let tol = (3.0 * sigma_noise).max(0.025) as f32;
    let mut moved_total = 0usize;

    for _ in 0..ROUNDS {
        let snap = labels.to_vec();
        let decide = |p: usize| -> Option<u16> {
            let (x, y) = (p % w, p / w);
            let own = snap[p];
            let mut labs = [own; 4];
            let mut nl = 1usize;
            for (dx, dy) in [
                (-1i32, -1i32),
                (0, -1),
                (1, -1),
                (-1, 0),
                (1, 0),
                (-1, 1),
                (0, 1),
                (1, 1),
            ] {
                let (qx, qy) = (x as i32 + dx, y as i32 + dy);
                if qx < 0 || qy < 0 || qx >= w as i32 || qy >= h as i32 {
                    continue;
                }
                let l = snap[qy as usize * w + qx as usize];
                if !labs[..nl].contains(&l) && nl < 4 {
                    labs[nl] = l;
                    nl += 1;
                }
            }
            if nl < 2 {
                return None;
            }
            let c = rgb[p];
            let col = |l: u16| -> Option<[f32; 3]> { pal.rgb.get(l as usize).copied() };
            let oc = col(own)?;
            let e0 = [c[0] - oc[0], c[1] - oc[1], c[2] - oc[2]];
            let resid_own = (e0[0] * e0[0] + e0[1] * e0[1] + e0[2] * e0[2]).sqrt();

            let mut cols: Vec<[f32; 3]> = Vec::with_capacity(5);
            let mut keep: Vec<Option<u16>> = Vec::with_capacity(5);
            for &l in &labs[..nl] {
                if let Some(cc) = col(l) {
                    cols.push(cc);
                    keep.push(Some(l));
                }
            }
            if alpha[p] < 0.99 && !cols.contains(&BACKDROP) {
                cols.push(BACKDROP);
                keep.push(None);
            }
            let (r, who) = mixture(c, &cols)?;
            let target = match keep[who] {
                Some(l) => l,
                None => {
                    let real = &cols[..cols.len() - 1];
                    match mixture(c, real) {
                        Some((_, w2)) => keep[w2].unwrap(),
                        None => return None,
                    }
                }
            };
            if target != own && r <= tol && r < 0.5 * resid_own {
                Some(target)
            } else {
                None
            }
        };
        use rayon::prelude::*;
        let decided: Vec<Option<u16>> = (0..n).into_par_iter().map(decide).collect();
        let mut moved = 0usize;
        for (p, d) in decided.into_iter().enumerate() {
            if let Some(t) = d {
                labels[p] = t;
                moved += 1;
            }
        }
        moved_total += moved;
        if moved == 0 {
            break;
        }
    }
    moved_total
}

/// Absorb connected components smaller than `min_size` into their largest neighbour.
pub fn despeckle(labels: &mut [u16], w: usize, h: usize, min_size: usize) {
    if min_size <= 1 {
        return;
    }
    let n = w * h;
    let mut comp = vec![u32::MAX; n];
    let mut sizes: Vec<usize> = Vec::new();
    let mut members: Vec<Vec<usize>> = Vec::new();

    for start in 0..n {
        if comp[start] != u32::MAX {
            continue;
        }
        let id = sizes.len() as u32;
        let lab = labels[start];
        let mut stack = vec![start];
        let mut group = Vec::new();
        comp[start] = id;
        while let Some(p) = stack.pop() {
            group.push(p);
            let (x, y) = (p % w, p / w);
            let push = |nx: isize, ny: isize, stack: &mut Vec<usize>, comp: &mut Vec<u32>| {
                if nx < 0 || ny < 0 || nx >= w as isize || ny >= h as isize {
                    return;
                }
                let q = ny as usize * w + nx as usize;
                if comp[q] == u32::MAX && labels[q] == lab {
                    comp[q] = id;
                    stack.push(q);
                }
            };
            push(x as isize - 1, y as isize, &mut stack, &mut comp);
            push(x as isize + 1, y as isize, &mut stack, &mut comp);
            push(x as isize, y as isize - 1, &mut stack, &mut comp);
            push(x as isize, y as isize + 1, &mut stack, &mut comp);
        }
        sizes.push(group.len());
        members.push(group);
    }

    for (id, size) in sizes.iter().enumerate() {
        if *size >= min_size {
            continue;
        }
        let mut tally: std::collections::HashMap<u16, usize> = std::collections::HashMap::new();
        for &p in &members[id] {
            let (x, y) = (p % w, p / w);
            for (nx, ny) in [
                (x as isize - 1, y as isize),
                (x as isize + 1, y as isize),
                (x as isize, y as isize - 1),
                (x as isize, y as isize + 1),
            ] {
                if nx < 0 || ny < 0 || nx >= w as isize || ny >= h as isize {
                    continue;
                }
                let q = ny as usize * w + nx as usize;
                if comp[q] != id as u32 {
                    *tally.entry(labels[q]).or_insert(0) += 1;
                }
            }
        }
        if let Some((&best, _)) = tally
            .iter()
            .max_by_key(|(&lab, &c)| (c, std::cmp::Reverse(lab)))
        {
            for &p in &members[id] {
                labels[p] = best;
            }
        }
    }
}

/// Diagnostic: write the label image as a binary PPM, each pixel its palette colour.
pub fn dump_labels(path: &std::ffi::OsStr, labels: &[u16], pal: &Palette, w: usize, h: usize) {
    let mut out = format!("P6\n{w} {h}\n255\n").into_bytes();
    for &l in labels {
        let c = pal.rgb.get(l as usize).copied().unwrap_or([1.0, 0.0, 1.0]);
        for k in 0..3 {
            out.push((c[k].clamp(0.0, 1.0) * 255.0).round() as u8);
        }
    }
    let _ = std::fs::write(path, out);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_split_components_disjoint() {
        let w = 4;
        let h = 4;
        let labels = vec![1, 1, 0, 1, 1, 1, 0, 1, 0, 0, 0, 0, 0, 0, 0, 0];
        let (comp, face_color) = split_components(&labels, w, h);
        assert_eq!(face_color.len(), 3);
        assert_ne!(comp[0], comp[3]);
    }

    #[test]
    fn test_despeckle_removes_single_pixel() {
        let w = 3;
        let h = 3;
        let mut labels = vec![0, 0, 0, 0, 1, 0, 0, 0, 0];
        despeckle(&mut labels, w, h, 2);
        assert_eq!(labels[4], 0);
    }
}
