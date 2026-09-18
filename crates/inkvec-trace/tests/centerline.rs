//! Does centreline recovery actually get the line back, and does it refuse when the
//! shape is not a line?
//!
//! Both halves matter equally. Recovering a stroke that is there is the feature; *not*
//! recovering one that is not there is what makes the feature safe to switch on by
//! default, because a filled shape re-emitted as a stroke is a silent corruption of the
//! drawing rather than a visible one.
//!
//! Images are rendered by supersampling the exact indicator function, as `subpixel.rs`
//! does, so the ground truth is analytic and the error measured is the pipeline's.

use inkvec_core::Point;
use inkvec_trace::centerline::{self, Stroke};
use inkvec_trace::coverage::{bilevel_coverage, Rgba};

/// Render a black shape on white with analytic coverage, by supersampling the indicator
/// function.
fn render(w: usize, h: usize, inside: impl Fn(f64, f64) -> bool) -> Rgba {
    const SS: usize = 16;
    let mut data = vec![0.0f32; w * h * 4];
    for y in 0..h {
        for x in 0..w {
            let mut hits = 0;
            for sy in 0..SS {
                for sx in 0..SS {
                    let px = x as f64 - 0.5 + (sx as f64 + 0.5) / SS as f64;
                    let py = y as f64 - 0.5 + (sy as f64 + 0.5) / SS as f64;
                    if inside(px, py) {
                        hits += 1;
                    }
                }
            }
            let v = 1.0 - hits as f32 / (SS * SS) as f32;
            let i = (y * w + x) * 4;
            data[i] = v;
            data[i + 1] = v;
            data[i + 2] = v;
            data[i + 3] = 1.0;
        }
    }
    Rgba {
        width: w,
        height: h,
        data,
    }
}

fn analyse(img: &Rgba) -> centerline::StrokeAnalysis {
    let field = bilevel_coverage(img);
    let labels = centerline::bilevel_labels(&field);
    centerline::analyse(&field, &labels, img.width, img.height)
}

/// Distance from a point to the segment `a-b`, the ground-truth centreline.
fn dist_to_seg(p: Point, a: Point, b: Point) -> f64 {
    let ab = b - a;
    let l2 = ab.dot(ab);
    if l2 <= 1e-18 {
        return p.dist(a);
    }
    let t = ((p - a).dot(ab) / l2).clamp(0.0, 1.0);
    p.dist(Point::new(a.x + ab.x * t, a.y + ab.y * t))
}

/// A round-capped, round-joined stroke through `pts` of the given width — the same shape
/// an SVG renderer draws for `stroke-linecap="round"`.
fn polystroke(pts: &[Point], width: f64) -> impl Fn(f64, f64) -> bool + '_ {
    move |x, y| {
        let p = Point::new(x, y);
        pts.windows(2)
            .any(|s| dist_to_seg(p, s[0], s[1]) <= width * 0.5)
    }
}

fn worst_centreline_error(s: &Stroke, a: Point, b: Point) -> f64 {
    s.path
        .iter()
        .map(|&p| dist_to_seg(p, a, b))
        .fold(0.0f64, f64::max)
}

// --- the core claim ------------------------------------------------------------------

#[test]
fn straight_stroke_recovers_one_centreline_and_its_width() {
    let (a, b) = (Point::new(12.0, 31.0), Point::new(83.0, 31.0));
    let img = render(96, 64, polystroke(&[a, b], 3.0));

    let r = analyse(&img);
    assert_eq!(r.strokes.len(), 1, "expected exactly one stroke");
    let s = &r.strokes[0];

    assert!(
        (s.width - 3.0).abs() < 0.15,
        "width {:.4} +- {:.4}, want 3.0 within 0.15",
        s.width,
        s.width_sigma
    );
    let err = worst_centreline_error(s, a, b);
    assert!(err < 0.1, "worst centreline error {err:.4}px, want < 0.1");
    assert!(
        s.length() > 60.0,
        "centreline length {:.2}, want most of the 71px stroke",
        s.length()
    );
    assert!(!s.closed);
    assert!(
        r.stroke_fraction > 0.95,
        "stroke_fraction {:.3}",
        r.stroke_fraction
    );
    assert!(
        r.strokes[0].width_sigma < 0.2,
        "width_sigma {:.4} implausibly large on noise-free input",
        r.strokes[0].width_sigma
    );
}

/// A diagonal stroke is the harder case: the skeleton is a staircase and the initial
/// centreline is wrong by up to half a diagonal, so this is a test of the ridge walk
/// rather than of the thinning.
#[test]
fn diagonal_stroke_is_recovered_to_sub_pixel_accuracy() {
    let (a, b) = (Point::new(14.0, 12.0), Point::new(74.0, 60.0));
    let img = render(88, 76, polystroke(&[a, b], 3.0));

    let r = analyse(&img);
    assert_eq!(r.strokes.len(), 1, "expected exactly one stroke");
    let s = &r.strokes[0];
    assert!(
        (s.width - 3.0).abs() < 0.15,
        "width {:.4}, want 3.0 within 0.15",
        s.width
    );
    let err = worst_centreline_error(s, a, b);
    assert!(err < 0.15, "worst centreline error {err:.4}px");
}

/// An L is one stroke with a corner. Splitting it into two would lose the join, which is
/// exactly the structure the artist drew.
#[test]
fn l_shape_is_one_stroke_with_a_corner() {
    let a = Point::new(16.0, 16.0);
    let b = Point::new(16.0, 72.0);
    let c = Point::new(72.0, 72.0);
    let img = render(90, 90, polystroke(&[a, b, c], 3.0));

    let r = analyse(&img);
    assert_eq!(
        r.strokes.len(),
        1,
        "an L is one stroke, got {} — a spur at the corner was not pruned",
        r.strokes.len()
    );
    let s = &r.strokes[0];
    assert!((s.width - 3.0).abs() < 0.15, "width {:.4}", s.width);
    assert!(
        s.length() > 100.0,
        "length {:.2}, want both arms",
        s.length()
    );

    // The corner must survive the fit as a corner: two long segments, not one curve
    // rounding it off.
    let cfg = inkvec_fit::FitConfig::default();
    let path = s.fit(&cfg);
    assert!(
        path.segments.len() >= 2,
        "fitted {} segments, want at least the two arms",
        path.segments.len()
    );
    let near_corner = s
        .path
        .iter()
        .map(|&p| p.dist(b))
        .fold(f64::INFINITY, f64::min);
    assert!(
        near_corner < 1.0,
        "centreline passes {near_corner:.2}px from the corner"
    );
}

/// A circle outline is a *closed* stroke. Recovering it as one closed centreline plus a
/// width is the whole point: the filled description needs two concentric contours.
#[test]
fn annulus_is_one_closed_stroke_of_the_right_radius() {
    let (cx, cy, r0, wid) = (48.0, 48.0, 30.0, 4.0);
    let img = render(96, 96, |x, y| {
        ((x - cx).hypot(y - cy) - r0).abs() <= wid * 0.5
    });

    let res = analyse(&img);
    assert_eq!(
        res.strokes.len(),
        1,
        "expected one closed stroke, got {}",
        res.strokes.len()
    );
    let s = &res.strokes[0];
    assert!(s.closed, "the ring must come back closed");
    assert!(
        (s.width - wid).abs() < 0.15,
        "width {:.4}, want {wid} within 0.15",
        s.width
    );

    let radii: Vec<f64> = s.path.iter().map(|p| (p.x - cx).hypot(p.y - cy)).collect();
    let mean = radii.iter().sum::<f64>() / radii.len() as f64;
    assert!(
        (mean - r0).abs() < 0.1,
        "mean radius {mean:.4}, want {r0} within 0.1"
    );
    let worst = radii.iter().map(|r| (r - r0).abs()).fold(0.0f64, f64::max);
    assert!(worst < 0.25, "worst radial error {worst:.4}px");
    assert!(
        (s.length() - std::f64::consts::TAU * r0).abs() < 2.0,
        "length {:.2}, want {:.2}",
        s.length(),
        std::f64::consts::TAU * r0
    );
}

// --- the refusals --------------------------------------------------------------------

/// The load-bearing negative. A disc is not a stroke, and no amount of skeletonization
/// makes it one; emitting it as a stroke would replace a filled circle with a fat dot.
#[test]
fn filled_disc_is_not_a_stroke() {
    let img = render(80, 80, |x, y| (x - 39.5).hypot(y - 39.5) <= 24.0);
    let r = analyse(&img);
    assert!(
        r.strokes.is_empty(),
        "a filled disc was reported as {} stroke(s)",
        r.strokes.len()
    );
    assert_eq!(r.stroke_fraction, 0.0);
    assert!(
        r.residual_regions.len() >= 2,
        "the disc and the background should both be left for filled tracing"
    );
}

/// A filled square is the other blob shape whose skeleton is long enough to be tempting:
/// thinning gives it two full diagonals. Each branch is still shorter than the width and
/// its width varies from the full side down to nothing.
#[test]
fn filled_square_is_not_a_stroke() {
    let img = render(72, 72, |x, y| {
        (16.0..=56.0).contains(&x) && (16.0..=56.0).contains(&y)
    });
    let r = analyse(&img);
    assert!(
        r.strokes.is_empty(),
        "a filled square was reported as {} stroke(s)",
        r.strokes.len()
    );
}

/// A stroke whose width varies is not a constant-width stroke. Forcing one would pick
/// some average width and silently redraw the shape; returning it as a region keeps it.
#[test]
fn tapered_stroke_is_rejected_as_non_constant_width() {
    let (x0, x1) = (14.0, 86.0);
    let cy = 32.0;
    let img = render(100, 64, |x, y| {
        if !(x0..=x1).contains(&x) {
            return false;
        }
        let t = (x - x0) / (x1 - x0);
        let w = 2.0 + 4.0 * t;
        (y - cy).abs() <= w * 0.5
    });

    let r = analyse(&img);
    assert!(
        r.strokes.is_empty(),
        "a 2->6px taper was accepted as a constant-width stroke of width {:?}",
        r.strokes.iter().map(|s| s.width).collect::<Vec<_>>()
    );
    assert_eq!(r.stroke_fraction, 0.0);
}

/// The same geometry with a constant width *is* accepted — otherwise the test above
/// would pass for the wrong reason.
#[test]
fn the_untapered_control_is_accepted() {
    let img = render(100, 64, |x, y| {
        (14.0..=86.0).contains(&x) && (y - 32.0).abs() <= 2.0
    });
    let r = analyse(&img);
    assert_eq!(r.strokes.len(), 1, "the constant-width control must pass");
    assert!(
        (r.strokes[0].width - 4.0).abs() < 0.15,
        "width {:.4}",
        r.strokes[0].width
    );
}

// --- topology ------------------------------------------------------------------------

/// Two strokes crossing. The junction is the structure that filled tracing destroys, so
/// what matters is that it comes back as a junction: four arms of the right width, and
/// no fifth short edge invented by the thinning.
#[test]
fn crossing_strokes_give_four_arms_and_no_spurs() {
    let a0 = Point::new(14.0, 14.0);
    let a1 = Point::new(82.0, 82.0);
    let b0 = Point::new(82.0, 14.0);
    let b1 = Point::new(14.0, 82.0);
    let img = render(96, 96, |x, y| {
        let p = Point::new(x, y);
        dist_to_seg(p, a0, a1) <= 1.5 || dist_to_seg(p, b0, b1) <= 1.5
    });

    let r = analyse(&img);
    assert_eq!(
        r.strokes.len(),
        4,
        "an X is four arms about one junction, got {} of lengths {:?}",
        r.strokes.len(),
        r.strokes.iter().map(|s| s.length()).collect::<Vec<_>>()
    );
    for s in &r.strokes {
        assert!(
            (s.width - 3.0).abs() < 0.2,
            "arm width {:.4}, want 3.0",
            s.width
        );
        assert!(
            s.length() > 2.0 * s.width,
            "arm of length {:.2} against width {:.2} is a spur, not an arm",
            s.length(),
            s.width
        );
    }

    // All four arms must end at the same junction point.
    let ends: Vec<Point> = r
        .strokes
        .iter()
        .map(|s| {
            let (f, l) = (s.path[0], s.path[s.path.len() - 1]);
            let c = Point::new(48.0, 48.0);
            if f.dist(c) < l.dist(c) {
                f
            } else {
                l
            }
        })
        .collect();
    for e in &ends {
        assert!(
            e.dist(ends[0]) < 2.0,
            "arms meet at {:?} and {:?}, more than a width apart",
            e,
            ends[0]
        );
    }
    assert!(
        r.stroke_fraction > 0.9,
        "stroke_fraction {:.3}",
        r.stroke_fraction
    );
}

/// A zig-zag of sharp turns is still one stroke. This is the case the naive prune rule
/// gets wrong: thinning puts the junction on the *notch* side of a sharp join, where the
/// region is nearly pinched shut, so "shorter than the local width" measured at the
/// junction compares a 2px branch against a 0.7px threshold and keeps it. Measuring the
/// local width as the widest point of the branch instead keeps the stroke whole.
#[test]
fn sharp_zig_zag_stays_one_stroke() {
    let v = [
        Point::new(14.0, 60.0),
        Point::new(42.0, 20.0),
        Point::new(70.0, 60.0),
        Point::new(98.0, 20.0),
        Point::new(114.0, 42.0),
    ];
    let img = render(128, 80, polystroke(&v, 3.0));

    let r = analyse(&img);
    assert_eq!(
        r.strokes.len(),
        1,
        "a zig-zag is one stroke, got {} of lengths {:?}",
        r.strokes.len(),
        r.strokes.iter().map(|s| s.length()).collect::<Vec<_>>()
    );
    let s = &r.strokes[0];
    assert!((s.width - 3.0).abs() < 0.15, "width {:.4}", s.width);
    assert!(
        s.length() > 155.0,
        "length {:.2}, want most of 173",
        s.length()
    );

    // Every interior turn must be reached: a skeleton rounds a sharp join, and the ridge
    // walk is what pulls the vertex back out to the apex.
    for corner in &v[1..4] {
        let d = s
            .path
            .iter()
            .map(|&q| q.dist(*corner))
            .fold(f64::INFINITY, f64::min);
        assert!(
            d < 1.2,
            "centreline misses the corner {corner:?} by {d:.3}px"
        );
    }
    let cfg = inkvec_fit::FitConfig::default();
    assert!(
        s.fit(&cfg).segments.len() >= 4,
        "four drawn segments should survive the fit"
    );
}

/// Two separate strokes are two strokes, and the fraction is still 1.
#[test]
fn two_disjoint_strokes_are_two_strokes() {
    let img = render(96, 64, |x, y| {
        ((10.0..=86.0).contains(&x) && (y - 20.0).abs() <= 1.5)
            || ((10.0..=86.0).contains(&x) && (y - 44.0).abs() <= 1.5)
    });
    let r = analyse(&img);
    assert_eq!(r.strokes.len(), 2);
    assert!(r.stroke_fraction > 0.95);
}

// --- what the saving actually is ------------------------------------------------------

/// The argument for the whole module, as a number: the stroke description is materially
/// cheaper than the filled outline of the same drawing, and it carries the width as one
/// editable parameter instead of burying it in coordinates.
#[test]
fn stroke_description_is_cheaper_than_the_filled_outline() {
    let img = render(96, 96, |x, y| {
        let p = Point::new(x, y);
        dist_to_seg(p, Point::new(16.0, 16.0), Point::new(80.0, 16.0)) <= 1.5
            || dist_to_seg(p, Point::new(80.0, 16.0), Point::new(80.0, 80.0)) <= 1.5
    });

    let cfg = inkvec_fit::FitConfig::default();
    let r = analyse(&img);
    assert_eq!(r.strokes.len(), 1);
    let stroke_params: f64 = r.strokes.iter().map(|s| s.params(&cfg)).sum();

    let (polys, _) = inkvec_trace::trace_bilevel(&img, &inkvec_trace::TraceOptions::default());
    let filled_params: f64 = polys
        .iter()
        .map(|p| inkvec_fit::multimodel::optimal_multimodel(p, &cfg).params())
        .sum();

    assert!(
        stroke_params < filled_params,
        "stroke {stroke_params} params vs filled {filled_params} — no saving"
    );
}

// --- uncertainty is carried, not invented ---------------------------------------------

#[test]
fn centreline_carries_a_per_point_sigma() {
    let img = render(96, 64, |x, y| {
        (12.0..=84.0).contains(&x) && (y - 32.0).abs() <= 1.5
    });
    let r = analyse(&img);
    let s = &r.strokes[0];
    assert_eq!(s.path.len(), s.sigma.len());
    assert!(s.sigma.iter().all(|v| v.is_finite() && *v > 0.0));
    // A centreline is the midpoint of two boundary measurements, so it is better
    // localized than either — but not by more than the sqrt(2) that averaging buys.
    let worst = s.sigma.iter().cloned().fold(0.0f64, f64::max);
    assert!(worst < 0.5, "worst centreline sigma {worst:.4}px");
    let poly = s.polyline();
    assert_eq!(poly.len(), s.path.len());
    assert_eq!(poly.closed, s.closed);
}
