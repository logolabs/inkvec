//! Label-map clean-up: connected components, blend absorption, despeckle and the
//! research-only saddle join (region proposal, `docs/DESIGN.md` S1).
//!
//! # Where this sits
//!
//! The Quality pipeline in [`crate::trace_color_full_with_alpha`] turns the image into a
//! *label map* (one palette index per pixel, from [`crate::color::label_image`]) and then
//! cleans it here before gradients are merged and faces are built:
//!
//! 1. [`despeckle`] folds components smaller than the minimum region into their commonest
//!    neighbour;
//! 2. [`absorb_blend_slivers`] dissolves thin components whose pixels are anti-aliasing
//!    blends of the inks around them;
//! 3. [`reassign_blend_pixels`] does the same pixel by pixel along the remaining
//!    boundaries;
//! 4. after the gradient stages, [`split_components`] turns the label map (one id per
//!    ink) into a face map (one id per 4-connected component).
//!
//! In: `labels` (`u16` palette index per pixel, row-major, `w * h`), the image as sRGB
//! `[0, 1]` composited onto white, the source alpha and the [`Palette`]. Out: the same
//! label map, edited in place, or a face map.
//!
//! All colour distances here are plain Euclidean distances in sRGB `[0, 1]`. That is on
//! purpose: anti-aliasing mixes *encoded* values linearly (SVG rasterisers, resvg included,
//! blend the stored sRGB values), so a blend pixel lies on the straight sRGB segment between
//! its two inks and
//! the residual to that segment is measured in the same units as the pixel noise
//! `sigma_noise`.
//!
//! The four-channel (colour, alpha) counterparts for images with transparency live in
//! [`crate::native`].

use crate::color::Palette;

mod components;
#[cfg(feature = "research")]
use crate::gradient;
pub(crate) use components::Components;

/// How far from the half-way mark a corner must sit before the image is taken to have
/// answered: three sigma of the propagated coverage noise. Below that the two readings
/// are indistinguishable, and the corner keeps the junction it has always had.
#[cfg(feature = "research")]
pub const SADDLE_SIGMAS: f64 = 3.0;

/// Join the faces that four pixels meeting at one corner say are one shape.
///
/// An experiment: compiled only with the `research` feature, and even then a no-op unless
/// `INKVEC_SADDLE` is set.
///
/// # The idea
///
/// Where a 2x2 block reads ink A on one diagonal and ink C on the other (`nw, se` = A,
/// `ne, sw` = C), the pixel grid cannot say whether the two A pixels touch through the
/// corner or the two C pixels do. The pixel *values* can: each is a coverage of A over C,
/// and the bilinear surface through the four coverages has a saddle whose height says
/// which diagonal is connected. This is the asymptotic decider used to resolve ambiguous
/// cells in marching squares.
///
/// # The formula
///
/// Coverage of pixel `p` along the ink axis, in sRGB `[0, 1]`:
/// `t(p) = clamp(((p − C) · (A − C)) / |A − C|², 0, 1)`.
/// With `c_nw, c_ne, c_sw, c_se` the four coverages, the saddle of the bilinear patch is
/// `s = (c_nw c_se − c_ne c_sw) / (c_nw + c_se − c_ne − c_sw)`.
/// When the denominator vanishes the patch has no saddle and `s` is the mean of the four.
/// The per-pixel noise `σ` (sRGB units) projects onto the axis as `σ_t = σ / |A − C|`, and
/// first-order error propagation gives `σ_s = σ_t · |∇s|`, with
/// `∂s/∂c_nw = (c_se·den − num) / den²` and so on (the mean's `σ_t / 2` in the degenerate
/// case). The corner is decided only when `|s − 0.5| > SADDLE_SIGMAS · σ_s`; `s > 0.5`
/// joins the A faces, `s < 0.5` the C faces. Faces are joined with a union-find.
///
/// # Edge cases
///
/// Images narrower or shorter than 2 px, faces with no ink and inks with no palette colour
/// are skipped. Only `labels` is rewritten; `face_fill`, `face_color` and `n_faces` are
/// returned unchanged, so absorbed faces keep their (now unused) entries.
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
///
/// Returns `(faces, face_color)`: `faces[p]` is the component id of pixel `p`, numbered in
/// raster order of each component's first pixel, and `face_color[f]` is the label (palette
/// index) that component `f` was made of. Face ids are `u16` with `u16::MAX` as the
/// "not yet visited" marker, so at most 65 535 faces are numbered; the pixels of any
/// further component fall into face 0.
///
/// 4-connectivity (not 8) is what the planar map and the boundary tracer assume: two
/// same-ink pixels that touch only at a corner are two faces, and the corner is a junction.
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
///
/// Returns `Some((r, who))`: `r` is the Euclidean distance (sRGB `[0, 1]`) from `c` to the
/// nearest point of any segment between two inks or any triangle spanned by three, and
/// `who` is the index in `cols` of the ink with the largest weight in that nearest mixture
/// (the ink the pixel "mostly is"). `None` when there are fewer than two inks, or when every
/// pair and triple is degenerate (coincident or collinear inks).
///
/// # The formula
///
/// For a pair `(a, b)`: `t = clamp(((c − a) · (b − a)) / |b − a|², 0, 1)`, nearest point
/// `q = a + t (b − a)`, dominant ink `a` if `t < 0.5` else `b`. The clamp makes each end
/// point a candidate too, so a pixel that is simply one ink scores `r` = its distance to it.
///
/// For a triple `(a, b, e)` with `u = b − a`, `v = e − a`, `w = c − a`, the orthogonal
/// projection onto the plane solves the 2x2 normal equations of least squares:
/// `s = (|v|² (w·u) − (u·v)(w·v)) / det`, `t = (|u|² (w·v) − (u·v)(w·u)) / det`,
/// `det = |u|²|v|² − (u·v)²`. Only projections that land inside the triangle
/// (`s, t ≥ 0`, `s + t ≤ 1`) are offered; a point outside is nearer to an edge, which the
/// pair pass already covered. The dominant ink is the largest of the barycentric weights
/// `(1 − s − t, s, t)`.
///
/// # Why
///
/// An anti-aliased edge pixel is a convex blend of the inks whose coverage it averages:
/// two at a boundary, three where boundaries meet. Asking "is this pixel on a blend of its
/// neighbours, to within the noise?" is how [`absorb_blend_slivers`] and
/// [`reassign_blend_pixels`] tell anti-aliasing from a genuine third ink. The search is
/// exhaustive (`O(k³)`), which is fine because callers pass at most five inks.
///
/// Ties keep the first candidate found (pairs before triples, lower indices first).
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

/// 4-connected components of equal label, by flood fill: the reference [`Components`] is
/// tested against.
///
/// Returns `(comp, members)`: `comp[p]` is the component id of pixel `p`, and
/// `members[id]` lists that component's pixels in flood-fill order. Components are numbered
/// in raster order of their first pixel.
#[cfg(test)]
pub(crate) fn label_components(labels: &[u16], w: usize, h: usize) -> (Vec<u32>, Vec<Vec<usize>>) {
    let n = w * h;
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
    (comp, members)
}

/// The inputs [`absorb_blend_slivers`] shares with each component it examines.
struct SliverScene<'a> {
    /// The image, sRGB `[0, 1]` composited onto [`BACKDROP`].
    rgb: &'a [[f32; 3]],
    /// Source alpha per pixel, `[0, 1]`.
    alpha: &'a [f32],
    w: usize,
    h: usize,
    pal: &'a Palette,
    /// Largest mixture residual (sRGB `[0, 1]`) at which a pixel still counts as a blend.
    tol: f32,
    /// `INKVEC_ABSDBG`: say why each sizeable component was kept.
    debug: bool,
}

/// Count how component `id` touches the rest of the image.
///
/// Returns `(interior, foreign)`: `interior` is the number of its pixels whose in-image
/// 4-neighbours all belong to the component (the image border does not make a pixel a
/// boundary pixel), and `foreign` the number of pixel edges it shares with other
/// components. `contacts[l]` is overwritten with the number of those edges whose outside
/// pixel carries label `l`, read from `labels` as it is *now*, so a neighbour absorbed
/// earlier in the same round already counts under its new label.
pub(crate) fn tally_contacts(
    group: &[usize],
    id: u32,
    comp: &[u32],
    labels: &[u16],
    w: usize,
    h: usize,
    contacts: &mut [usize],
) -> (usize, usize) {
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
    (interior, foreign)
}

/// Decide where each pixel of a sliver would go if the sliver were dissolved.
///
/// `cols` are the candidate inks (sRGB `[0, 1]`) and `labs` their labels, with `None` for
/// the white backdrop pseudo-ink, which is only ever the last entry. For every pixel the
/// nearest convex [`mixture`] of `cols` names the dominant ink, and the pixel's destination
/// is that ink's label. When the backdrop dominates, the pixel is a translucent edge
/// fading to nothing: it goes to the dominant *real* ink instead (mixture without the
/// backdrop), or to `fallback` if that fails. A pixel no mixture explains keeps its label.
///
/// Returns `(pass, dest)`: how many pixels lie within `tol` of a mixture, and the
/// destination label of every pixel in `group` order.
fn sliver_destinations(
    group: &[usize],
    scene: &SliverScene,
    labels: &[u16],
    cols: &[[f32; 3]],
    labs: &[Option<u16>],
    fallback: u16,
) -> (usize, Vec<u16>) {
    let mut pass = 0usize;
    let mut dest: Vec<u16> = Vec::with_capacity(group.len());
    for &p in group {
        let Some((r, who)) = mixture(scene.rgb[p], cols) else {
            dest.push(labels[p]);
            continue;
        };
        if r <= scene.tol {
            pass += 1;
        }
        dest.push(match labs[who] {
            Some(l) => l,
            None => {
                let real: Vec<[f32; 3]> = cols[..labs.len() - 1].to_vec();
                match mixture(scene.rgb[p], &real) {
                    Some((_, w2)) => labs[w2]
                        .expect("only the backdrop entry is None, and it is last, outside `real`"),
                    None => fallback,
                }
            }
        });
    }
    (pass, dest)
}

/// Try to dissolve one component; returns whether it was absorbed.
///
/// The three tests, in order, each on counts rather than colours so that one noisy pixel
/// cannot swing them:
///
/// 1. **Thin**: fewer than 20 % of its pixels are interior (`5 · interior < area`), and it
///    has at least one neighbour. A blob with a real interior is a shape, not a seam.
/// 2. **Between few inks**: its three most-touched neighbouring labels (ties to the lower
///    label) account for at least 80 % of its boundary contacts, and at least two of them
///    have a palette colour. A seam between two or three inks qualifies; a crumb in a busy
///    area does not.
/// 3. **A blend of those inks**: at least 80 % of its pixels lie within `tol` of a convex
///    [`mixture`] of the neighbour inks. If any pixel is translucent (`alpha < 0.99`) and
///    white is not already one of them, the white backdrop joins as a pseudo-ink, because a
///    translucent edge composited onto white is a blend with white.
///
/// On success each pixel moves to its own dominant ink (see [`sliver_destinations`]), so a
/// seam between A and B is split down the middle rather than handed wholesale to one side.
fn absorb_sliver(
    scene: &SliverScene,
    group: &[usize],
    comp: &[u32],
    labels: &mut [u16],
    contacts: &mut [usize],
) -> bool {
    let area = group.len();
    if area == 0 {
        return false;
    }
    let id = comp[group[0]];
    let (interior, foreign) = tally_contacts(group, id, comp, labels, scene.w, scene.h, contacts);
    if interior * 5 >= area || foreign == 0 {
        if scene.debug && area > 15 && interior * 5 >= area {
            eprintln!("abs: comp area {area} NOT thin (interior {interior})");
        }
        return false;
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
        if scene.debug && area > 15 {
            eprintln!("abs: comp area {area} contacts too spread (top {covered} of {foreign})");
        }
        return false;
    }
    let mut labs: Vec<Option<u16>> = tally.iter().map(|&(_, l)| Some(l)).collect();
    let mut cols: Vec<[f32; 3]> = Vec::with_capacity(4);
    for &(_, l) in &tally {
        let Some(&c) = scene.pal.rgb.get(l as usize) else {
            continue;
        };
        cols.push(c);
    }
    if cols.len() != labs.len() || cols.len() < 2 {
        return false;
    }
    if group.iter().any(|&p| scene.alpha[p] < 0.99) && !cols.contains(&BACKDROP) {
        cols.push(BACKDROP);
        labs.push(None);
    }

    let (pass, dest) = sliver_destinations(group, scene, labels, &cols, &labs, tally[0].1);
    if pass * 5 < area * 4 {
        if scene.debug && area > 15 {
            eprintln!("abs: comp area {area} blend fail ({pass} of {area} pass)");
        }
        return false;
    }
    for (&p, &l) in group.iter().zip(&dest) {
        labels[p] = l;
    }
    true
}

/// Dissolve thin components that are colour blends of their dominant neighbours.
///
/// Anti-aliased pixels between two inks are blends of those inks, but nearest-ink labelling
/// gives them whichever palette entry is closest, often a third colour. They then form
/// one- or two-pixel slivers along every boundary, each minting junctions the boundary
/// fitter has to honour. This stage finds those slivers and hands their pixels back to the
/// inks they are blends of; the tests are listed on [`absorb_sliver`].
///
/// `labels` is edited in place; `rgb` is sRGB `[0, 1]` composited onto [`BACKDROP`], `alpha`
/// the source alpha `[0, 1]`, `sigma_noise` the per-channel pixel noise in sRGB units. A
/// pixel counts as a blend when its mixture residual is at most
/// `tol = max(3 σ_noise, 0.025)`; the floor keeps clean synthetic input, where `σ` is near
/// zero, from rejecting blends over float rounding in the rasteriser.
///
/// Runs at most two rounds, because dissolving one sliver can leave a neighbouring one
/// thinner or bounded by fewer inks. Components are visited in raster order and edits are
/// visible to later components in the same round. Returns the number of components
/// absorbed. The four-channel counterpart is [`crate::native::absorb_blend_slivers`].
pub fn absorb_blend_slivers(
    labels: &mut [u16],
    rgb: &[[f32; 3]],
    alpha: &[f32],
    w: usize,
    h: usize,
    pal: &Palette,
    sigma_noise: f64,
) -> usize {
    let scene = SliverScene {
        rgb,
        alpha,
        w,
        h,
        pal,
        tol: (3.0 * sigma_noise).max(0.025) as f32,
        debug: inkvec_core::env::flag("INKVEC_ABSDBG"),
    };
    let mut absorbed = 0usize;

    for _round in 0..2 {
        let round = SliverRound::of(labels, w, h);
        let mut changed = 0usize;
        let n_labels = labels
            .iter()
            .copied()
            .max()
            .map_or(1, |m| m as usize + 1)
            .max(pal.rgb.len());
        let mut contacts: Vec<usize> = vec![0; n_labels];
        for id in 0..round.comps.len() {
            let Some(group) = round.thin(id) else {
                let (area, interior) = (round.comps.size[id], round.interior[id]);
                if scene.debug && area > 15 && interior * 5 >= area {
                    eprintln!("abs: comp area {area} NOT thin (interior {interior})");
                }
                continue;
            };
            if absorb_sliver(&scene, group, &round.comps.comp, labels, &mut contacts) {
                changed += 1;
            }
        }
        absorbed += changed;
        if changed == 0 {
            break;
        }
    }
    absorbed
}

/// One round of sliver absorption's view of the label map: its components, and the member
/// lists of the thin ones only.
///
/// A component can be absorbed only when it is thin (`5 · interior < area`) and touches
/// another (`foreign > 0`); [`absorb_sliver`] turns every other one down on exactly those
/// counts, which depend on the components and not on the labels, so they are counted for
/// all components in one pass and only the thin ones get member lists.
pub(crate) struct SliverRound {
    /// The label map's components.
    pub(crate) comps: Components,
    /// Per component, its interior pixel count.
    pub(crate) interior: Vec<u32>,
    thin: Vec<bool>,
    members: components::Members,
}

impl SliverRound {
    /// The components of `labels` (`w x h`) and the members of the thin ones.
    pub(crate) fn of(labels: &[u16], w: usize, h: usize) -> Self {
        let comps = Components::of(labels, w, h);
        let (interior, foreign) = comps.shape(w, h);
        let thin: Vec<bool> = (0..comps.len())
            .map(|c| interior[c] * 5 < comps.size[c] && foreign[c] > 0)
            .collect();
        let members = comps.members(|c| thin[c]);
        SliverRound {
            comps,
            interior,
            thin,
            members,
        }
    }

    /// The pixels of component `id` when it is thin, else `None`.
    pub(crate) fn thin(&self, id: usize) -> Option<&[usize]> {
        self.thin[id].then(|| self.members.of(id))
    }
}

/// Reassign individual boundary pixels that a neighbour-pair blend explains strictly better.
///
/// The per-pixel sequel to [`absorb_blend_slivers`]: after whole slivers are gone, single
/// blend pixels remain along boundaries, carrying a third ink's label. For each pixel, the
/// candidate inks are its own label plus up to three other distinct labels among its eight
/// neighbours (first found in the scan order NW, N, NE, W, E, SW, S, SE), with the white
/// [`BACKDROP`] added when the pixel is translucent. The pixel moves to the dominant ink of
/// its nearest convex [`mixture`] when all three hold:
///
/// * that ink differs from its current one;
/// * the mixture residual `r ≤ tol = max(3 σ_noise, 0.025)` (sRGB `[0, 1]`);
/// * `r < 0.5 · |c − own|`: the blend explains the pixel at least twice as well as the ink
///   it has, so a pixel that is a plausible match for its own ink is left alone.
///
/// When the backdrop dominates, the dominant real ink is used instead, as in
/// [`absorb_blend_slivers`]. Each round decides every pixel from a snapshot (in parallel,
/// so the result does not depend on scan order) and then applies the moves; up to four
/// rounds, stopping early when nothing moves. Returns the number of moves.
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
                        Some((_, w2)) => keep[w2].expect(
                            "only the backdrop entry is None, and it is last, outside `real`",
                        ),
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
///
/// "Largest" means the neighbouring label sharing the most pixel edges with the component
/// (ties to the lower label), not the biggest region. Components are 4-connected and are
/// found once, before any is absorbed ([`Components`]: runs and union-find, with member
/// lists for the small ones only); each speckle's neighbours are then read from the labels
/// as they are at that moment, so a speckle absorbed earlier counts under its new label. A
/// component with no neighbour at all (the whole image one label) is left alone.
/// `min_size <= 1` is a no-op.
pub fn despeckle(labels: &mut [u16], w: usize, h: usize, min_size: usize) {
    if min_size <= 1 {
        return;
    }
    let comps = Components::of(labels, w, h);
    let small = comps.members(|c| (comps.size[c] as usize) < min_size);
    for id in 0..comps.len() {
        let group = small.of(id);
        if group.is_empty() {
            continue;
        }
        if let Some(best) = commonest_neighbour(group, id as u32, &comps.comp, labels, w, h) {
            for &p in group {
                labels[p] = best;
            }
        }
    }
}

/// The label sharing the most pixel edges with component `id` (pixels `group`), read from
/// `labels` as they are now; ties to the lower label. `None` when it touches nothing.
fn commonest_neighbour(
    group: &[usize],
    id: u32,
    comp: &[u32],
    labels: &[u16],
    w: usize,
    h: usize,
) -> Option<u16> {
    let mut tally: Vec<(u16, usize)> = Vec::new();
    for &p in group {
        let (x, y) = (p % w, p / w);
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
                let l = labels[q];
                match tally.iter_mut().find(|e| e.0 == l) {
                    Some(e) => e.1 += 1,
                    None => tally.push((l, 1)),
                }
            }
        }
    }
    tally
        .iter()
        .max_by_key(|&&(lab, c)| (c, std::cmp::Reverse(lab)))
        .map(|&(lab, _)| lab)
}

/// Diagnostic: write the label image as a binary PPM, each pixel its palette colour.
///
/// Labels with no palette entry are drawn magenta. Called when `INKVEC_DUMP_LABELS` names
/// a path; a write failure is ignored because the dump is a debugging aid, not output.
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
mod reference_tests;

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
