//! Editability passes that rewrite a ring's curves with their nodes at the x and y extrema:
//! circular arcs written as the cubics a drawing tool would write, and smooth runs
//! re-segmented at their extrema with a stronger preference than the default fit's.
//!
//! A drawing tool writes a circle as four cubics with their nodes at the top, bottom, left
//! and right and every handle horizontal or vertical, each `κ = 4/3·tan(θ/4)` of the radius
//! long (0.5523 for a quarter), and an arc as the same pieces cut at its ends. Type design
//! asks the same of every curve: points at the extrema, handles on the axes (Adobe, *Type 1
//! Font Format*, 1990, and the Apple and Microsoft font tools since: an outline without
//! points at its extrema fails their checks). The fit writes arcs as `A`, which an editor
//! shows as two nodes with no handles at all, wherever on the circle they fall.

use inkvec_core::{Point, Polyline};
use inkvec_fit::curves::{arc_ellipse_center, Segment};
use inkvec_fit::extrema::Preference;
use inkvec_fit::{FitConfig, FittedPath};

use super::{chain_starts, guarded, EditStats};

/// How `--editability` prefers nodes at extrema: three parameters credited per anchored
/// node against the fit's one (`inkvec_fit::extrema::FIT`), and the written count free to
/// change, so a run gains a node where a segment spans an extremum. The mode spends
/// parameters on structure; the fidelity budget is the same as every other editability
/// pass's.
pub(super) const EDIT_PREFERENCE: Preference = Preference {
    credit: 3.0,
    same_count: false,
};

/// Write every circular arc of the ring as cubics cut at its x and y extrema
/// ([`arc_as_cubics`]). The pieces are within `1.1e-3` of the radius of the arc, so the
/// guard holds wherever the radius is under a few thousand pixels; it is checked anyway.
pub(super) fn pass_arcs(fp: &mut FittedPath, poly: &Polyline, st: &mut EditStats) {
    if !fp
        .segments
        .iter()
        .any(|s| matches!(s, Segment::Arc { .. }) && s.is_circular())
    {
        return;
    }
    let before = fp.segments.clone();
    let mut out = Vec::with_capacity(before.len() + 4);
    let mut cur = fp.start;
    let mut arcs = 0;
    for s in &before {
        match arc_as_cubics(s, cur) {
            Some(pieces) => {
                out.extend(pieces);
                arcs += 1;
            }
            None => out.push(s.clone()),
        }
        cur = s.end();
    }
    fp.segments = out;
    if guarded(&fp.segments, &chain_starts(fp), &poly.points, &poly.sigma) {
        st.arcs_split += arcs;
    } else {
        fp.segments = before;
        st.reverted += 1;
    }
}

/// A cut closer than this to an arc's end, along the arc (px), is not made: the arc's end
/// is then all but at the extremum, and the cut would leave a sliver whose handles are too
/// short for two decimals to point them (a 0.4 px piece on `material-icons/dining`). The
/// same gap as the fit's for an extremum near a run's end (`inkvec_fit::extrema`).
const ARC_CUT_GAP_PX: f64 = 1.0;

/// A circular arc as cubics: cut at every angle that is a multiple of 90 degrees inside it
/// and at least [`ARC_CUT_GAP_PX`] from its ends, so each piece turns by at most a quarter
/// (a little more beside a cut left out) and every cut is an x or y extremum with its
/// handles exactly on an axis. Each piece is the standard approximation, handles along the
/// circle's tangents and `4/3·tan(Δ/4)·r` long, whose largest radial error is `2.7e-4·r`
/// for a quarter (Riškus 2006; Goldapp 1991). The first piece starts on the arc's stored
/// start and the last ends on its stored end, so the ring stays watertight. `None` for
/// anything that is not a circular arc, or a degenerate one.
pub(super) fn arc_as_cubics(seg: &Segment, start: Point) -> Option<Vec<Segment>> {
    let Segment::Arc {
        rx,
        ry,
        phi,
        large_arc,
        sweep,
        end,
    } = *seg
    else {
        return None;
    };
    if !seg.is_circular() {
        return None;
    }
    let f = arc_ellipse_center(start, rx, ry, phi, large_arc, sweep, end);
    if f.delta.abs() < 1e-9 || f.rx <= 1e-9 {
        return None;
    }
    let quarter = std::f64::consts::FRAC_PI_2;
    let (a0, a1) = (f.theta1, f.theta1 + f.delta);
    // Cut angles inside, clear of the ends, in the direction of travel, as whole quarter
    // turns `k`. The gap is at most an eighth of a half turn, so no piece turns by more
    // than 112.5 degrees (radial error under 1.1e-3 of the radius).
    let gap = (ARC_CUT_GAP_PX / f.rx).clamp(1e-9, std::f64::consts::FRAC_PI_8);
    let mut cuts: Vec<i64> = Vec::new();
    let (lo, hi) = (a0.min(a1), a0.max(a1));
    let mut k = (lo / quarter).floor() as i64 + 1;
    while (k as f64) * quarter < hi - gap {
        if (k as f64) * quarter > lo + gap {
            cuts.push(k);
        }
        k += 1;
    }
    if f.delta < 0.0 {
        cuts.reverse();
    }
    let sgn = f.delta.signum();
    let r = f.rx;
    // A point of the circle and the unit direction of travel there; exact at a cut.
    let at = |th: f64, k: Option<i64>| -> (Point, (f64, f64)) {
        let (s, c) = match k {
            Some(k) => match k.rem_euclid(4) {
                0 => (0.0, 1.0),
                1 => (1.0, 0.0),
                2 => (0.0, -1.0),
                _ => (-1.0, 0.0),
            },
            None => th.sin_cos(),
        };
        (
            Point::new(f.c.x + r * c, f.c.y + r * s),
            (-sgn * s, sgn * c),
        )
    };
    let mut angles: Vec<(f64, Option<i64>)> = vec![(a0, None)];
    angles.extend(cuts.iter().map(|&k| (k as f64 * quarter, Some(k))));
    angles.push((a1, None));
    let mut out = Vec::with_capacity(angles.len() - 1);
    for (i, w) in angles.windows(2).enumerate() {
        let (pa, ta) = at(w[0].0, w[0].1);
        let (pb, tb) = at(w[1].0, w[1].1);
        let p0 = if i == 0 { start } else { pa };
        let p3 = if i + 2 == angles.len() { end } else { pb };
        let h = 4.0 / 3.0 * ((w[1].0 - w[0].0).abs() / 4.0).tan() * r;
        out.push(Segment::Cubic(
            Point::new(p0.x + h * ta.0, p0.y + h * ta.1),
            Point::new(p3.x - h * tb.0, p3.y - h * tb.1),
            p3,
        ));
    }
    Some(out)
}

/// Re-segment the ring's smooth runs at their extrema with [`EDIT_PREFERENCE`]
/// (`inkvec_fit::extrema::anchor_extrema`), kept only if the whole ring still guards.
pub(super) fn pass_extrema(
    fp: &mut FittedPath,
    poly: &Polyline,
    cfg: &FitConfig,
    st: &mut EditStats,
) {
    let before = fp.clone();
    let runs = inkvec_fit::extrema::anchor_extrema(fp, poly, cfg, EDIT_PREFERENCE);
    if runs == 0 {
        return;
    }
    if guarded(&fp.segments, &chain_starts(fp), &poly.points, &poly.sigma) {
        st.extrema_runs += runs;
    } else {
        *fp = before;
        st.reverted += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use inkvec_fit::structural::eval_segment;

    fn start_of(k: usize, start: Point, segs: &[Segment]) -> Point {
        if k == 0 {
            start
        } else {
            segs[k - 1].end()
        }
    }

    #[test]
    fn an_arc_is_cut_at_its_extrema_with_axis_handles() {
        // Three eighths of a circle of radius 20 round (5, 5), from -45 to 90 degrees.
        let (c, r) = (Point::new(5.0, 5.0), 20.0);
        let at = |deg: f64| {
            let t = deg.to_radians();
            Point::new(c.x + r * t.cos(), c.y + r * t.sin())
        };
        let start = at(-45.0);
        let arc = Segment::circular_arc(r, false, true, at(90.0));
        let pieces = arc_as_cubics(&arc, start).expect("a circular arc");
        // Cut at 0 degrees only: 90 is the arc's end.
        assert_eq!(pieces.len(), 2, "{pieces:?}");
        let mid = pieces[0].end();
        assert!(
            mid.dist(Point::new(c.x + r, c.y)) < 1e-9,
            "the cut is the x extremum: {mid:?}"
        );
        let (Segment::Cubic(_, c2, _), Segment::Cubic(c1, _, e)) = (&pieces[0], &pieces[1]) else {
            panic!("cubics");
        };
        assert_eq!(c2.x, mid.x, "the handle into the cut is vertical");
        assert_eq!(c1.x, mid.x, "and so is the one out of it");
        assert_eq!(*e, at(90.0), "the arc's own end, as stored");
        // Within the standard approximation's error of the circle everywhere.
        for (k, p) in pieces.iter().enumerate() {
            let s = start_of(k, start, &pieces);
            for i in 0..=32 {
                let q = eval_segment(p, s, i as f64 / 32.0);
                assert!((q.dist(c) - r).abs() < 3e-4 * r, "piece {k} strays at {i}");
            }
        }
        assert!(arc_as_cubics(&Segment::Line(at(0.0)), start).is_none());
    }

    #[test]
    fn no_cut_leaves_a_sliver_at_the_arcs_end() {
        // A quarter of radius 10 that ends 0.4 px (along the arc) past the y extremum: the
        // cut there would leave a 0.4 px piece, so the arc is one piece.
        let (c, r) = (Point::new(0.0, 0.0), 10.0);
        let at = |rad: f64| Point::new(c.x + r * rad.cos(), c.y + r * rad.sin());
        let half_pi = std::f64::consts::FRAC_PI_2;
        let start = at(0.04 - half_pi);
        let arc = Segment::circular_arc(r, false, true, at(0.04));
        let pieces = arc_as_cubics(&arc, start).expect("a circular arc");
        assert_eq!(pieces.len(), 1, "{pieces:?}");
        for i in 0..=32 {
            let q = eval_segment(&pieces[0], start, i as f64 / 32.0);
            assert!((q.dist(c) - r).abs() < 1.1e-3 * r, "strays at {i}");
        }
        // With room on both sides the cut is made.
        let arc = Segment::circular_arc(r, false, true, at(0.2));
        assert_eq!(arc_as_cubics(&arc, start).expect("an arc").len(), 2);
    }

    #[test]
    fn a_ring_of_arcs_becomes_cubics_and_still_guards() {
        // A circle as two half arcs, sampled for the guard.
        let r = 30.0;
        let mut fp = FittedPath {
            start: Point::new(r, 0.0),
            segments: vec![
                Segment::circular_arc(r, false, true, Point::new(-r, 0.0)),
                Segment::circular_arc(r, false, true, Point::new(r, 0.0)),
            ],
            closed: true,
        };
        let pts: Vec<Point> = (0..240)
            .map(|k| {
                let t = std::f64::consts::TAU * k as f64 / 240.0;
                Point::new(r * t.cos(), r * t.sin())
            })
            .collect();
        let poly = Polyline::with_uniform_sigma(pts, 0.05, true);
        let mut st = EditStats::default();
        pass_arcs(&mut fp, &poly, &mut st);
        assert_eq!(st.arcs_split, 2);
        assert_eq!(fp.segments.len(), 4, "a quarter each side of every cut");
        assert!(fp.segments.iter().all(|s| matches!(s, Segment::Cubic(..))));
    }
}
