//! The two-ground representation `native` traces transparency in: what it has to tell
//! apart, and what it has to reduce to.

use inkvec_trace::color::{rgb_to_oklab, Palette};
use inkvec_trace::native::{self, Ink2, CLEAR};

const WHITE: [f32; 3] = [1.0, 1.0, 1.0];

#[test]
fn white_paint_and_the_clear_ground_are_two_inks() {
    // Composited onto white, which is what the classic path traces, they are one colour.
    let p = native::pixel_points(&[WHITE, WHITE], &[1.0, 0.0]);
    assert_eq!(p[0].w, p[1].w);
    // Over the second ground only the paint is still white.
    assert!(p[0].dist(p[1]) > 0.2, "{}", p[0].dist(p[1]));
}

#[test]
fn opaque_colours_measure_as_plain_oklab() {
    let (a, b) = (rgb_to_oklab([0.8, 0.3, 0.2]), rgb_to_oklab([0.2, 0.4, 0.9]));
    assert_eq!(Ink2::opaque(a).dist(Ink2::opaque(b)), a.dist(b));
    let p = native::pixel_points(&[[0.8, 0.3, 0.2]], &[1.0])[0];
    assert_eq!(p, Ink2::opaque(a));
}

#[test]
fn a_washes_opacity_is_read_back_from_the_two_grounds() {
    let paint = [0.2f32, 0.4, 0.7];
    for a in [0.25f32, 0.5, 0.8] {
        let over_white = paint.map(|c| a * c + (1.0 - a));
        let p = native::pixel_points(&[over_white], &[a])[0];
        assert!((p.alpha() - a).abs() < 0.01, "{a} -> {}", p.alpha());
    }
    assert_eq!(native::snap_alpha(0.9996), 1.0);
    assert_eq!(native::snap_alpha(0.003), 0.0);
    assert_eq!(native::snap_alpha(0.5), 0.5);
}

#[test]
fn an_edge_pixel_is_a_straight_blend_in_four_channels() {
    // Anti-aliased paint over the clear ground, as colour-over-white and alpha: both are
    // linear in coverage, so the pixel lies on the segment between the two inks.
    let paint = [0.9f32, 0.5, 0.3, 1.0];
    let other = [0.1f32, 0.1, 0.1, 1.0];
    let c: [f32; 4] = std::array::from_fn(|k| 0.3 * paint[k] + 0.7 * CLEAR[k]);
    let (residual, dominant) = native::mixture(c, &[paint, CLEAR, other]).expect("three inks");
    assert!(residual < 1e-4, "{residual}");
    assert_eq!(dominant, 1, "70% of the pixel is ground");
}

#[test]
fn every_pixel_goes_to_the_ink_it_looks_like_on_both_grounds() {
    let pal = Palette {
        colors: vec![rgb_to_oklab(WHITE), rgb_to_oklab(WHITE)],
        rgb: vec![WHITE, WHITE],
        weight: vec![0.5, 0.5],
        alpha: vec![1.0, 0.0],
    };
    // White paint, the ground, and a near-opaque off-white fringe of the paint.
    let labels = native::label_image(&[WHITE, WHITE, [0.97, 0.97, 0.97]], &[1.0, 0.0, 0.9], &pal);
    assert_eq!(labels, vec![0, 1, 0]);
}
