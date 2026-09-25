//! Colour groups: several fills a user wants drawn as one.
//!
//! Recolouring the finished SVG cannot do this. Two fills painted the same are still two
//! sets of faces: the boundary between them is still traced, its nodes are still in the
//! file, and a region the artist drew as one is still two shapes. The merge has to happen
//! before the tracer decides what the faces are.
//!
//! It happens to the image. [`apply`] takes a first trace of the image, finds the faces
//! each group names by their fitted fills -- the fills the caller saw, so a colour picked off
//! the output matches exactly, even where the fill is not one of the palette's inks -- and
//! repaints those faces as the group's one fill. The unchanged pipeline then traces the
//! repainted image, and the boundaries inside the group are gone because there is nothing
//! left to see.
//!
//! A group becomes:
//!
//! * **a flat colour** -- the target colour, or the target member's -- painted over every
//!   member face. A gradient member collapses into it.
//! * **a gradient**, when the target member is one: each connected region of member faces
//!   gets one gradient of the target's kind (linear or radial), fitted to that region's own
//!   pixels. Fitting rather than extending is the point: a gradient fitted to one small face
//!   extrapolates into bands and blotches over a larger one, where a fit to the whole region
//!   follows the colours actually there.
//!
//! Anti-aliasing survives because each pixel is unmixed first: a pixel on an edge is taken
//! as a blend of its own face's fill and the neighbouring face's fill that best explains
//! it, and only the ends that belong to a group are replaced. An edge against a face left
//! alone stays a correct blend; an edge inside a group collapses to the group's fill.

use crate::color::rgb_to_oklab;
use crate::coverage::Rgba;
use crate::gradient::{self, FillModel};
use crate::ColorTrace;
use rayon::prelude::*;

/// How far, in OKLab, a named flat colour may sit from a face's fill and still name it.
///
/// Colours are normally picked off a trace's own fills, which are 8-bit hex of the fitted
/// colour, so they land within rounding of it; this leaves room for that and for a trace
/// at another size, and stays below the distance between shades an artist drew apart.
pub const MATCH_DISTANCE: f32 = 0.03;

/// How closely, in OKLab per stop, a face's gradient must match a named gradient's stops.
pub const STOP_MATCH: f32 = 0.05;

/// One fill in a group, as the caller saw it in a trace.
#[derive(Debug, Clone, PartialEq)]
pub enum Member {
    /// A flat colour, sRGB `[0, 1]`.
    Flat([f32; 3]),
    /// A gradient, by its stop colours in order, sRGB `[0, 1]`.
    Gradient(Vec<[f32; 3]>),
}

impl Member {
    fn first_colour(&self) -> [f32; 3] {
        match self {
            Member::Flat(c) => *c,
            Member::Gradient(stops) => stops.first().copied().unwrap_or([0.0; 3]),
        }
    }
}

/// What a group is drawn as.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Target {
    /// The member that covers the most of the image, flat or gradient: what the artist most
    /// likely meant the group to be.
    Auto,
    /// This flat colour, sRGB `[0, 1]`.
    Colour([f32; 3]),
    /// This member, by index. A gradient here is refitted over the whole group.
    Member(usize),
}

/// Fills to draw as one.
#[derive(Debug, Clone, PartialEq)]
pub struct InkGroup {
    /// The fills to merge.
    pub members: Vec<Member>,
    /// What they become.
    pub target: Target,
}

/// What became of one group.
#[derive(Debug, Clone, PartialEq)]
pub struct GroupOutcome {
    /// The flat colour the group became, or `None` when it became a gradient or did nothing.
    pub target: Option<[f32; 3]>,
    /// Whether the group became a gradient.
    pub gradient: bool,
    /// The members that matched faces, by their first colour.
    pub merged: Vec<[f32; 3]>,
    /// Members that matched no face, or only faces an earlier group had already claimed.
    pub unmatched: Vec<Member>,
}

/// A face's gradient stops, first to last, or `None` for a flat fill.
fn stops_of(model: &FillModel) -> Option<Vec<[f32; 3]>> {
    match model {
        FillModel::Flat(_) => None,
        FillModel::Linear { c0, c1, mids, .. } | FillModel::Radial { c0, c1, mids, .. } => {
            let mut v = vec![*c0];
            v.extend(mids.iter().map(|m| m.1));
            v.push(*c1);
            Some(v)
        }
    }
}

/// Whether a face's fill is the member the caller named.
fn names(model: &FillModel, member: &Member) -> bool {
    match (member, model) {
        (Member::Flat(c), FillModel::Flat(f)) => {
            rgb_to_oklab(*f).dist(rgb_to_oklab(*c)) <= MATCH_DISTANCE
        }
        (Member::Gradient(want), _) => stops_of(model).is_some_and(|have| {
            have.len() == want.len()
                && want
                    .iter()
                    .zip(&have)
                    .all(|(a, b)| rgb_to_oklab(*a).dist(rgb_to_oklab(*b)) <= STOP_MATCH)
        }),
        _ => false,
    }
}

/// What the faces of one group are painted with.
enum Paint {
    Flat([f32; 3]),
    /// One fitted model per connected region of the group, indexed by region id.
    Fitted(Vec<FillModel>),
}

/// Which group each face belongs to, each group's paint, and the groups whose gradients
/// still have to be fitted: `(group, radial)`.
struct Plan {
    group_of: Vec<Option<usize>>,
    paints: Vec<Option<Paint>>,
    needs_fit: Vec<(usize, bool)>,
}

/// The image repainted so that each group is one fill, and what each group did.
///
/// `trace` is a trace of `img` itself, made with the options the final trace will use. A
/// face belongs to the first group that names it; asking for one face to be in two groups
/// has no single answer, and the first is the one the caller listed first. Alpha is never
/// touched, and a fully transparent pixel is left exactly as it was.
pub fn apply(img: &Rgba, trace: &ColorTrace, groups: &[InkGroup]) -> (Rgba, Vec<GroupOutcome>) {
    let mut area = vec![0usize; trace.face_fill.len()];
    for &f in &trace.labels {
        if let Some(a) = area.get_mut(f as usize) {
            *a += 1;
        }
    }
    let (mut plan, outcomes) = plan(trace, &area, groups);
    let mut out = img.clone();
    if plan.group_of.iter().all(Option::is_none) {
        return (out, outcomes);
    }
    let region = fit_regions(img, trace, &mut plan);
    repaint(&mut out, trace, &plan, &region);
    (out, outcomes)
}

/// Match each group's members to faces and decide what the group becomes.
fn plan(trace: &ColorTrace, area: &[usize], groups: &[InkGroup]) -> (Plan, Vec<GroupOutcome>) {
    let n_faces = trace.face_fill.len();
    let mut plan = Plan {
        group_of: vec![None; n_faces],
        paints: Vec::with_capacity(groups.len()),
        needs_fit: Vec::new(),
    };
    let mut outcomes = Vec::with_capacity(groups.len());
    for (gi, g) in groups.iter().enumerate() {
        let faces_of: Vec<Vec<usize>> = g
            .members
            .iter()
            .map(|m| {
                (0..n_faces)
                    .filter(|&f| plan.group_of[f].is_none() && names(&trace.face_fill[f].model, m))
                    .collect()
            })
            .collect();
        let merged: Vec<[f32; 3]> = g
            .members
            .iter()
            .zip(&faces_of)
            .filter(|(_, f)| !f.is_empty())
            .map(|(m, _)| m.first_colour())
            .collect();
        let unmatched: Vec<Member> = g
            .members
            .iter()
            .zip(&faces_of)
            .filter(|(_, f)| f.is_empty())
            .map(|(m, _)| m.clone())
            .collect();
        let pick = match g.target {
            Target::Colour(_) => None,
            Target::Member(i) => Some(i),
            Target::Auto => (0..g.members.len())
                .max_by_key(|&i| faces_of[i].iter().map(|&f| area[f]).sum::<usize>()),
        };
        let matched = faces_of.iter().filter(|f| !f.is_empty()).count();
        let explicit = !matches!(g.target, Target::Auto);
        // One fill and nothing of its own to become is not a merge; leave it alone.
        if matched == 0 || (matched == 1 && !explicit) {
            plan.paints.push(None);
            outcomes.push(GroupOutcome {
                target: None,
                gradient: false,
                merged: Vec::new(),
                unmatched,
            });
            continue;
        }
        let paint = match (g.target, pick.and_then(|i| g.members.get(i))) {
            (Target::Colour(c), _) => Paint::Flat(c),
            (_, Some(Member::Gradient(_))) => {
                // The kind of gradient to fit: the target member's own, read off its
                // largest face.
                let radial = pick
                    .and_then(|i| faces_of[i].iter().copied().max_by_key(|&f| area[f]))
                    .is_some_and(|f| matches!(trace.face_fill[f].model, FillModel::Radial { .. }));
                plan.needs_fit.push((gi, radial));
                Paint::Fitted(Vec::new())
            }
            (_, Some(Member::Flat(c))) => Paint::Flat(*c),
            (_, None) => Paint::Flat(g.members[0].first_colour()),
        };
        for faces in &faces_of {
            for &f in faces {
                plan.group_of[f] = Some(gi);
            }
        }
        outcomes.push(GroupOutcome {
            target: match paint {
                Paint::Flat(c) => Some(c),
                Paint::Fitted(_) => None,
            },
            gradient: matches!(paint, Paint::Fitted(_)),
            merged,
            unmatched,
        });
        plan.paints.push(Some(paint));
    }
    (plan, outcomes)
}

/// The connected regions of each gradient group, one gradient fitted to each, stored in the
/// group's paint. Returns each pixel's region id (`u32::MAX` outside every gradient group).
fn fit_regions(img: &Rgba, trace: &ColorTrace, plan: &mut Plan) -> Vec<u32> {
    let (w, h) = (img.width, img.height);
    let labels = &trace.labels;
    let rgb = img.composited([1.0, 1.0, 1.0]);
    let mut region = vec![u32::MAX; w * h];
    let lambda = gradient::bic_lambda(w * h);
    for &(gi, radial) in &plan.needs_fit {
        let group_of = &plan.group_of;
        let in_group = |p: usize| group_of.get(labels[p] as usize).copied().flatten() == Some(gi);
        let mut models = Vec::new();
        for start in 0..w * h {
            if region[start] != u32::MAX || !in_group(start) {
                continue;
            }
            let id = models.len() as u32;
            let mask = flood(&mut region, w, h, start, id, &in_group);
            let cands = gradient::fit_candidates(&rgb, w, h, &mask, 1, trace.sigma_noise, lambda);
            models.push(best_gradient(&cands, radial));
        }
        if let Some(Some(Paint::Fitted(m))) = plan.paints.get_mut(gi) {
            *m = models;
        }
    }
    region
}

/// Mark the 4-connected region of `start` whose pixels pass `inside` with `id` in `region`,
/// and return it as a label mask (1 inside, 0 elsewhere).
fn flood(
    region: &mut [u32],
    w: usize,
    h: usize,
    start: usize,
    id: u32,
    inside: &impl Fn(usize) -> bool,
) -> Vec<u16> {
    let mut stack = vec![start];
    region[start] = id;
    let mut mask = vec![0u16; w * h];
    while let Some(p) = stack.pop() {
        mask[p] = 1;
        let (x, y) = (p % w, p / w);
        let mut push = |q: usize| {
            if region[q] == u32::MAX && inside(q) {
                region[q] = id;
                stack.push(q);
            }
        };
        if x > 0 {
            push(p - 1);
        }
        if x + 1 < w {
            push(p + 1);
        }
        if y > 0 {
            push(p - w);
        }
        if y + 1 < h {
            push(p + w);
        }
    }
    mask
}

/// The cheapest candidate of the target's kind, else the cheapest gradient, else the
/// cheapest fill at all.
fn best_gradient(cands: &[gradient::FillFit], radial: bool) -> FillModel {
    let kind = |m: &FillModel| {
        if radial {
            matches!(m, FillModel::Radial { .. })
        } else {
            matches!(m, FillModel::Linear { .. })
        }
    };
    cands
        .iter()
        .filter(|c| kind(&c.model))
        .min_by(|a, b| a.cost.total_cmp(&b.cost))
        .or_else(|| {
            cands
                .iter()
                .filter(|c| c.model.is_gradient())
                .min_by(|a, b| a.cost.total_cmp(&b.cost))
        })
        .or_else(|| cands.iter().min_by(|a, b| a.cost.total_cmp(&b.cost)))
        .map(|c| c.model.clone())
        .unwrap_or(FillModel::Flat([0.0; 3]))
}

/// Repaint every pixel whose face, or the neighbouring face it is a blend with, is in a
/// group: unmix it between the two fills and replace the ends that belong to a group.
fn repaint(out: &mut Rgba, trace: &ColorTrace, plan: &Plan, region: &[u32]) {
    let (w, h) = (out.width, out.height);
    let labels = &trace.labels;
    // What a pixel of face `f` at pixel `p` is repainted as, if its face is in a group.
    let paint_at = |f: usize, p: usize, x: f64, y: f64| -> Option<[f32; 3]> {
        let gi = plan.group_of.get(f).copied().flatten()?;
        match plan.paints[gi].as_ref()? {
            Paint::Flat(c) => Some(*c),
            Paint::Fitted(models) => models.get(region[p] as usize).map(|m| m.color_at(x, y)),
        }
    };
    let fill_at = |f: usize, x: f64, y: f64| -> [f32; 3] {
        trace
            .face_fill
            .get(f)
            .map_or([0.0; 3], |ff| ff.model.color_at(x, y))
    };
    out.data
        .par_chunks_mut(4 * w)
        .enumerate()
        .for_each(|(y, row)| {
            for x in 0..w {
                let px = &mut row[4 * x..4 * x + 4];
                if px[3] <= 0.0 {
                    continue;
                }
                let p = y * w + x;
                let own = labels[p] as usize;
                let (fx, fy) = (x as f64, y as f64);
                let c = [px[0], px[1], px[2]];
                let a = fill_at(own, fx, fy);
                // The neighbouring face whose fill, with the pixel's own, best explains it.
                let mut best: Option<(usize, usize, f32, f32)> = None; // (face, pixel, t, error)
                for dy in -1i64..=1 {
                    for dx in -1i64..=1 {
                        let (nx, ny) = (x as i64 + dx, y as i64 + dy);
                        if nx < 0 || ny < 0 || nx >= w as i64 || ny >= h as i64 {
                            continue;
                        }
                        let q = ny as usize * w + nx as usize;
                        let other = labels[q] as usize;
                        if other == own || best.is_some_and(|b| b.0 == other) {
                            continue;
                        }
                        let b = fill_at(other, fx, fy);
                        let d = sub(b, a);
                        let len2 = dot(d, d);
                        if len2 <= 1e-12 {
                            continue;
                        }
                        let t = (dot(sub(c, a), d) / len2).clamp(0.0, 1.0);
                        let off = sub(c, [a[0] + t * d[0], a[1] + t * d[1], a[2] + t * d[2]]);
                        let e = dot(off, off);
                        if best.is_none_or(|b| e < b.3) {
                            best = Some((other, q, t, e));
                        }
                    }
                }
                let na = paint_at(own, p, fx, fy);
                let nb = best.and_then(|(o, q, _, _)| paint_at(o, q, fx, fy));
                if na.is_none() && nb.is_none() {
                    continue;
                }
                // Keep the pixel's own deviation from the model (noise, texture) and move only
                // the modelled part of it.
                let t = best.map_or(0.0, |b| b.2);
                let ob = best.map_or(a, |(o, _, _, _)| fill_at(o, fx, fy));
                let (na, nb) = (na.unwrap_or(a), nb.unwrap_or(ob));
                for k in 0..3 {
                    let before = (1.0 - t) * a[k] + t * ob[k];
                    let after = (1.0 - t) * na[k] + t * nb[k];
                    px[k] = (c[k] + after - before).clamp(0.0, 1.0);
                }
            }
        });
}

#[inline]
fn sub(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

#[inline]
fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ColorOptions;

    const RED: [f32; 3] = [0.867, 0.180, 0.267];
    const DARK_RED: [f32; 3] = [0.651, 0.153, 0.220];
    const BLUE: [f32; 3] = [0.1, 0.2, 0.9];
    const WHITE: [f32; 3] = [1.0, 1.0, 1.0];

    /// Three vertical bands, each `band` pixels wide, on a white ground.
    fn bands(cols: [[f32; 3]; 3], band: usize) -> Rgba {
        let (w, h) = (band * 3 + 8, 24);
        let mut data = Vec::with_capacity(w * h * 4);
        for y in 0..h {
            for x in 0..w {
                let c = if y < 4 || y >= h - 4 || x < 4 || x >= w - 4 {
                    WHITE
                } else {
                    cols[((x - 4) / band).min(2)]
                };
                data.extend_from_slice(&[c[0], c[1], c[2], 1.0]);
            }
        }
        Rgba {
            width: w,
            height: h,
            data,
        }
    }

    fn at(img: &Rgba, x: usize, y: usize) -> [f32; 3] {
        let p = img.pixel(x, y);
        [p[0], p[1], p[2]]
    }

    fn near(a: [f32; 3], b: [f32; 3], tol: f32) -> bool {
        (0..3).all(|k| (a[k] - b[k]).abs() <= tol)
    }

    fn fill_of(trace: &ColorTrace, img: &Rgba, x: usize, y: usize) -> [f32; 3] {
        let f = trace.labels[y * img.width + x] as usize;
        match trace.face_fill[f].model {
            FillModel::Flat(c) => c,
            _ => panic!("expected a flat face"),
        }
    }

    #[test]
    fn two_reds_become_the_one_that_covers_more() {
        let img = bands([RED, RED, DARK_RED], 12);
        let trace = crate::trace_color_full(&img, &ColorOptions::default());
        let (red, dark) = (fill_of(&trace, &img, 8, 12), fill_of(&trace, &img, 36, 12));
        let group = InkGroup {
            members: vec![Member::Flat(dark), Member::Flat(red)],
            target: Target::Auto,
        };
        let (out, report) = apply(&img, &trace, &[group]);
        assert_eq!(report[0].merged.len(), 2, "{report:?}");
        assert!(near(report[0].target.unwrap(), red, 1e-6));
        assert!(
            near(at(&out, 36, 12), at(&out, 8, 12), 2.0 / 255.0),
            "dark red repainted red"
        );
        assert!(
            near(at(&out, 1, 1), WHITE, 1e-6),
            "the ground is left alone"
        );
    }

    #[test]
    fn an_explicit_colour_wins_and_other_faces_are_untouched() {
        let img = bands([RED, BLUE, DARK_RED], 12);
        let trace = crate::trace_color_full(&img, &ColorOptions::default());
        let (red, blue) = (fill_of(&trace, &img, 8, 12), fill_of(&trace, &img, 22, 12));
        let group = InkGroup {
            members: vec![Member::Flat(red)],
            target: Target::Colour(WHITE),
        };
        let (out, _) = apply(&img, &trace, &[group]);
        assert!(near(at(&out, 8, 12), WHITE, 2.0 / 255.0));
        assert!(near(at(&out, 22, 12), blue, 2.0 / 255.0), "blue stays blue");
    }

    #[test]
    fn a_colour_on_no_face_is_reported_and_nothing_moves() {
        let img = bands([RED, BLUE, DARK_RED], 12);
        let trace = crate::trace_color_full(&img, &ColorOptions::default());
        let green = [0.1, 0.8, 0.2];
        let red = fill_of(&trace, &img, 8, 12);
        let group = InkGroup {
            members: vec![Member::Flat(red), Member::Flat(green)],
            target: Target::Auto,
        };
        let (out, report) = apply(&img, &trace, &[group]);
        assert_eq!(report[0].unmatched, vec![Member::Flat(green)]);
        assert_eq!(out.data, img.data, "one fill and no target is not a merge");
    }

    #[test]
    fn a_face_belongs_to_the_first_group_that_names_it() {
        let img = bands([RED, BLUE, DARK_RED], 12);
        let trace = crate::trace_color_full(&img, &ColorOptions::default());
        let (red, blue, dark) = (
            fill_of(&trace, &img, 8, 12),
            fill_of(&trace, &img, 22, 12),
            fill_of(&trace, &img, 36, 12),
        );
        let groups = [
            InkGroup {
                members: vec![Member::Flat(red), Member::Flat(dark)],
                target: Target::Member(0),
            },
            InkGroup {
                members: vec![Member::Flat(dark), Member::Flat(blue)],
                target: Target::Auto,
            },
        ];
        let (_, report) = apply(&img, &trace, &groups);
        assert_eq!(report[1].unmatched, vec![Member::Flat(dark)]);
    }
}
