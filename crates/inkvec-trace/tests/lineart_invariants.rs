//! Automated visual detail and regression invariant tests for fine lineart.
//!
//! Validates preservation of fine lineart details through tracing:
//! - Subpixel thin strokes (<1.5px) retention, component count, mass conservation, and turning bounds
//! - Narrow gaps (<2px) preservation, component separation, and minimum polygon distance
//! - Acute tips (20° and 45°) apex recovery without chamfering
//! - Small glyph counters (9px ring with 1.2px crossbar) topological hole and component retention
//!
//! Uses robust mathematical invariants (topological genus/hole count, component count,
//! continuous mass conservation, turning angle bounds, Euclidean apex distance) rather than
//! brittle pixel diffs.

use std::f64::consts::PI;

use inkvec_core::Point;
use inkvec_trace::coverage::Rgba;
use inkvec_trace::planar::face_edge_order;
use inkvec_trace::{
    contour, trace_bilevel, trace_color, trace_color_full, ColorOptions, TraceOptions,
};

/// High-fidelity analytic ground truth rasterizer using 16x16 supersampling.
/// `inside(x, y)` evaluates the continuous analytic indicator function.
fn render_analytic(w: usize, h: usize, inside: impl Fn(f64, f64) -> bool) -> Rgba {
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
            let a = hits as f32 / (SS * SS) as f32;
            let v = 1.0 - a; // Black shape on white ground
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

/// Compute total turning angle of a closed polyline in radians.
/// A simple convex closed ring has turning angle 2*pi.
fn polyline_total_turning(pts: &[Point]) -> f64 {
    let n = pts.len();
    if n < 3 {
        return 0.0;
    }
    let mut total = 0.0;
    for i in 0..n {
        let p0 = pts[i];
        let p1 = pts[(i + 1) % n];
        let p2 = pts[(i + 2) % n];
        let d1x = p1.x - p0.x;
        let d1y = p1.y - p0.y;
        let d2x = p2.x - p1.x;
        let d2y = p2.y - p1.y;
        let l1 = d1x.hypot(d1y);
        let l2 = d2x.hypot(d2y);
        if l1 < 1e-9 || l2 < 1e-9 {
            continue;
        }
        let cross = (d1x * d2y - d1y * d2x) / (l1 * l2);
        let dot = ((d1x * d2x + d1y * d2y) / (l1 * l2)).clamp(-1.0, 1.0);
        let angle = cross.atan2(dot).abs();
        total += angle;
    }
    total
}

/// Distance from point p to line segment (a, b).
fn dist_to_segment(p: Point, a: Point, b: Point) -> f64 {
    let dx = b.x - a.x;
    let dy = b.y - a.y;
    let l2 = dx * dx + dy * dy;
    if l2 <= 1e-12 {
        return p.dist(a);
    }
    let t = (((p.x - a.x) * dx + (p.y - a.y) * dy) / l2).clamp(0.0, 1.0);
    let proj = Point::new(a.x + t * dx, a.y + t * dy);
    p.dist(proj)
}

/// Minimum Euclidean distance between two closed polygons.
fn min_polygon_separation(poly_a: &[Point], poly_b: &[Point]) -> f64 {
    let mut min_d = f64::INFINITY;
    let na = poly_a.len();
    let nb = poly_b.len();
    for &p in poly_a {
        for j in 0..nb {
            let d = dist_to_segment(p, poly_b[j], poly_b[(j + 1) % nb]);
            if d < min_d {
                min_d = d;
            }
        }
    }
    for &q in poly_b {
        for i in 0..na {
            let d = dist_to_segment(q, poly_a[i], poly_a[(i + 1) % na]);
            if d < min_d {
                min_d = d;
            }
        }
    }
    min_d
}

#[test]
fn test_thin_stroke_retention_subpixel() {
    let widths = [0.5f64, 0.8, 1.0, 1.2, 1.5];
    let len = 40.0f64;
    let canvas = 64;
    let y0 = 12.0f64;
    let y1 = y0 + len;
    let x_center = 32.0f64;

    for &w in &widths {
        let x0 = x_center - w / 2.0;
        let x1 = x_center + w / 2.0;

        let img = render_analytic(canvas, canvas, |x, y| {
            x >= x0 && x < x1 && y >= y0 && y < y1
        });

        // 1. Bilevel tracing test
        let (polys, _) = trace_bilevel(&img, &TraceOptions { min_area: 0.01 });
        assert_eq!(
            polys.len(),
            1,
            "bilevel trace must retain exactly 1 stroke polygon for width {w}"
        );

        let stroke = &polys[0];
        let recovered_area = contour::signed_area(stroke).abs();
        let expected_area = len * w;

        // Mass conservation invariant: for resolved strokes (w >= 1.0), < 15% width/area error
        if w >= 1.0 {
            let area_err = (recovered_area - expected_area).abs() / expected_area;
            assert!(
                area_err < 0.15,
                "bilevel stroke width {w} mass conservation error {area_err:.3} exceeds 15% (recovered {recovered_area:.2}, expected {expected_area:.2})"
            );
        } else {
            // For sub-pixel strokes w < 1.0, feature must still survive with positive mass
            assert!(
                recovered_area > 0.0,
                "sub-pixel stroke width {w} must produce positive mass"
            );
        }

        // Turning ratio invariant: total turning <= 2.2 * 2pi (no teeth or ripple wiggles)
        let total_turning = polyline_total_turning(&stroke.points);
        assert!(
            total_turning <= 2.2 * (2.0 * PI),
            "stroke width {w} total turning {total_turning:.2} exceeds 2.2 * 2pi"
        );

        // 2. Color tracing test
        let color_opts = ColorOptions {
            min_region: 1,
            ..Default::default()
        };
        let (map, palette) = trace_color(&img, &color_opts);
        assert!(
            palette.len() >= 2,
            "color trace must recover foreground and background palette entries"
        );
        assert!(
            map.n_labels >= 2,
            "planar map must subdivide foreground from background"
        );

        let color_trace = trace_color_full(&img, &color_opts);
        // Exactly 1 foreground shape (anything darker than the white background)
        let fg_faces: Vec<usize> = (0..color_trace.face_color.len())
            .filter(|&f| color_trace.face_rgb[f][0] < 0.9)
            .collect();
        assert_eq!(
            fg_faces.len(),
            1,
            "color trace must have exactly 1 foreground component for width {w}"
        );
    }
}

#[test]
fn test_narrow_gap_preservation() {
    let gaps = [1.0f64, 1.5, 2.0];
    let bar_w = 10.0f64;
    let bar_h = 40.0f64;
    let canvas = 64;
    let y0 = 12.0f64;
    let y1 = y0 + bar_h;
    let cx = 32.0f64;

    for &g in &gaps {
        // Two vertical bars separated by gap g centered around cx
        let left_x0 = cx - g / 2.0 - bar_w;
        let left_x1 = cx - g / 2.0;
        let right_x0 = cx + g / 2.0;
        let right_x1 = cx + g / 2.0 + bar_w;

        let img = render_analytic(canvas, canvas, |x, y| {
            y >= y0 && y < y1 && ((x >= left_x0 && x < left_x1) || (x >= right_x0 && x < right_x1))
        });

        // 1. Bilevel trace
        let (polys, _) = trace_bilevel(&img, &TraceOptions { min_area: 0.1 });
        assert_eq!(
            polys.len(),
            2,
            "bilevel trace must find 2 distinct components across gap {g}px (not bridged)"
        );

        let sep_bilevel = min_polygon_separation(&polys[0].points, &polys[1].points);
        assert!(
            sep_bilevel > 0.5 * g,
            "bilevel separation {sep_bilevel:.3}px must exceed 0.5 * gap ({:.3}px)",
            0.5 * g
        );

        // 2. Color trace
        let color_opts = ColorOptions {
            min_region: 1,
            absorb_blends: false,
            ..Default::default()
        };
        let (map, _) = trace_color(&img, &color_opts);
        assert_eq!(
            map.n_labels, 3,
            "trace_color must identify 3 regions (1 backdrop + 2 distinct bars)"
        );

        let trace = trace_color_full(&img, &color_opts);
        let fg_faces: Vec<usize> = (0..trace.face_color.len())
            .filter(|&f| trace.face_rgb[f][0] < 0.5)
            .collect();
        assert_eq!(
            fg_faces.len(),
            2,
            "color trace must find 2 distinct foreground components across gap {g}px"
        );
    }
}

#[test]
fn test_acute_corner_apex_recovery() {
    let angles = [20.0f64, 45.0f64];
    let canvas = 128;
    let apex = Point::new(64.27, 22.63);
    let arm = 88.0f64;

    for &angle_deg in &angles {
        let half = angle_deg.to_radians() / 2.0;
        let p_left = Point::new(apex.x - arm * half.sin(), apex.y + arm * half.cos());
        let p_right = Point::new(apex.x + arm * half.sin(), apex.y + arm * half.cos());

        let img = render_analytic(canvas, canvas, |x, y| {
            // Point (x, y) inside triangle (apex, p_left, p_right)
            let pt = Point::new(x, y);
            let sign = |p1: Point, p2: Point, p3: Point| {
                (p1.x - p3.x) * (p2.y - p3.y) - (p2.x - p3.x) * (p1.y - p3.y)
            };
            let d1 = sign(pt, apex, p_left);
            let d2 = sign(pt, p_left, p_right);
            let d3 = sign(pt, p_right, apex);
            let has_neg = (d1 < 0.0) || (d2 < 0.0) || (d3 < 0.0);
            let has_pos = (d1 > 0.0) || (d2 > 0.0) || (d3 > 0.0);
            !(has_neg && has_pos)
        });

        let trace = trace_color_full(&img, &ColorOptions::default());
        let fg_faces: Vec<usize> = (0..trace.face_color.len())
            .filter(|&f| trace.face_rgb[f][0] < 0.5)
            .collect();
        assert_eq!(
            fg_faces.len(),
            1,
            "wedge must form exactly 1 foreground face"
        );

        let rings_per_face = face_edge_order(&trace.map);
        let fg_rings = &rings_per_face[fg_faces[0]];
        assert!(
            !fg_rings.is_empty(),
            "foreground face must have at least one boundary ring"
        );

        let cfg = inkvec_fit::FitConfig::default();
        let mut min_apex_dist = f64::INFINITY;

        for ring in fg_rings {
            for &(edge_idx, _) in ring {
                let edge = &trace.map.edges[edge_idx];
                let poly = edge.as_polyline();
                let fitted = inkvec_fit::multimodel::optimal_multimodel(&poly, &cfg);

                let mut vertices = vec![fitted.start];
                for seg in &fitted.segments {
                    vertices.push(seg.end());
                }

                for v in vertices {
                    let d = v.dist(apex);
                    if d < min_apex_dist {
                        min_apex_dist = d;
                    }
                }
            }
        }

        assert!(
            min_apex_dist < 0.18,
            "wedge {angle_deg}° apex recovery error {min_apex_dist:.4}px exceeds 0.18px limit"
        );
    }
}

#[test]
fn test_small_counter_preservation_ring_bar() {
    let canvas = 64;
    let cx = 32.27f64;
    let cy = 31.73f64;
    let r_outer = 4.5f64;
    let r_inner = 3.0f64;
    let bar_half_h = 0.6f64; // 1.2px bar

    let img = render_analytic(canvas, canvas, |x, y| {
        let d = (x - cx).hypot(y - cy);
        if d <= r_outer {
            d >= r_inner || (y - cy).abs() <= bar_half_h
        } else {
            false
        }
    });

    // 1. Bilevel trace
    let (polys, _) = trace_bilevel(&img, &TraceOptions { min_area: 0.05 });
    // In marching squares (y-down): outer boundary is clockwise (signed_area < 0),
    // inner counter holes are counter-clockwise (signed_area > 0).
    let outer_contours: Vec<_> = polys
        .iter()
        .filter(|p| contour::signed_area(p) < 0.0)
        .collect();
    let inner_holes: Vec<_> = polys
        .iter()
        .filter(|p| contour::signed_area(p) > 0.0)
        .collect();

    assert_eq!(
        outer_contours.len(),
        1,
        "ring-bar glyph must have exactly 1 outer contour"
    );
    assert_eq!(
        inner_holes.len(),
        2,
        "ring-bar glyph must have exactly 2 inner counter holes"
    );

    // 2. Color trace
    let color_opts = ColorOptions {
        min_region: 1,
        ..Default::default()
    };
    let (map, palette) = trace_color(&img, &color_opts);
    assert_eq!(palette.len(), 2, "palette must contain backdrop and ink");
    assert_eq!(
        map.n_labels, 4,
        "map must have 4 regions (backdrop, ring-bar foreground, and 2 inner counters)"
    );

    let trace = trace_color_full(&img, &color_opts);
    let fg_faces: Vec<usize> = (0..trace.face_color.len())
        .filter(|&f| trace.face_rgb[f][0] < 0.5)
        .collect();
    assert_eq!(
        fg_faces.len(),
        1,
        "ring-bar glyph must be 1 connected foreground shape in color trace"
    );

    let rings_per_face = face_edge_order(&trace.map);
    let fg_rings = &rings_per_face[fg_faces[0]];
    assert_eq!(
        fg_rings.len(),
        3,
        "ring-bar glyph face must have 3 rings (1 outer ring + 2 counter hole rings), got {}",
        fg_rings.len()
    );
}
