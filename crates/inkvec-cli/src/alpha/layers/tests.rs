//! Tests of the `--layers` guard, [`layers_reproduce`]: the layered document is only written
//! when compositing every layer over each covered face's ground, in sRGB as a renderer does,
//! gives back the colour the flat document paints that face.

use super::*;
use inkvec_trace::alpha::{AlphaAnalysis, Layer};

/// One layer of colour `c` at opacity `a` over the faces `faces`, with the given grounds.
fn analysis(c: [f32; 3], a: f32, faces: Vec<usize>, base: Vec<[f32; 3]>) -> AlphaAnalysis {
    AlphaAnalysis {
        layers: vec![Layer {
            color: c,
            alpha: a,
            faces,
            residual: 0.0,
        }],
        opaque_faces: Vec::new(),
        base_rgb: base,
    }
}

/// `a·C + (1 − a)·G` per channel: the sRGB "over" a renderer draws.
fn over(c: [f32; 3], a: f32, g: [f32; 3]) -> [f32; 3] {
    [
        a * c[0] + (1.0 - a) * g[0],
        a * c[1] + (1.0 - a) * g[1],
        a * c[2] + (1.0 - a) * g[2],
    ]
}

/// Grounds that the layer composites back onto the observed colours exactly: nothing to
/// report, so the layered document may be written.
#[test]
fn an_exact_layer_reproduces_every_face() {
    let (c, a) = ([0.2, 0.4, 0.8], 0.6);
    let grounds = vec![[1.0, 1.0, 1.0], [0.9, 0.2, 0.1], [0.3, 0.3, 0.3]];
    let observed: Vec<[f32; 3]> = grounds.iter().map(|&g| over(c, a, g)).collect();
    let an = analysis(c, a, vec![0, 1], grounds);
    // Face 2 is not covered, and its colour is not the composite: it must not count.
    let mut rgb = observed.clone();
    rgb[2] = [0.0, 1.0, 0.0];
    assert!(layers_reproduce(&an, &rgb) < 1e-3);
}

/// The hair on `noto-emoji/emoji_u1f469_1f3fb_200d_1f52c`: dark grey faces read as black at
/// 0.924 over a ground that, in sRGB, would have to be 2.5 times white. The decomposition
/// clamps the ground to white, and the composite comes out near black, far from the face.
#[test]
fn a_clamped_ground_is_caught() {
    let (c, a) = ([0.0, 0.0, 0.0], 0.924);
    let hair = [0.192, 0.176, 0.176];
    let an = analysis(c, a, vec![0], vec![[1.0, 1.0, 1.0]]);
    let drawn = over(c, a, [1.0, 1.0, 1.0]);
    assert!(drawn[0] < 0.08, "{drawn:?}");
    assert!(layers_reproduce(&an, &[hair]) > LAYER_MAX_DE00);
}

/// Two layers over one face are composited back to front: `layers[0]` is frontmost, so it
/// goes on last. Applying them in the other order gives a different colour, which the
/// guard would then wrongly reject.
#[test]
fn stacked_layers_composite_back_to_front() {
    let (front, back) = (([0.9, 0.1, 0.1], 0.5), ([0.1, 0.1, 0.9], 0.7));
    let g = [1.0, 1.0, 1.0];
    let seen = over(front.0, front.1, over(back.0, back.1, g));
    let an = AlphaAnalysis {
        layers: vec![
            Layer {
                color: front.0,
                alpha: front.1,
                faces: vec![0],
                residual: 0.0,
            },
            Layer {
                color: back.0,
                alpha: back.1,
                faces: vec![0],
                residual: 0.0,
            },
        ],
        opaque_faces: Vec::new(),
        base_rgb: vec![g],
    };
    assert!(layers_reproduce(&an, &[seen]) < 1e-3);
    let wrong_order = over(back.0, back.1, over(front.0, front.1, g));
    assert!(layers_reproduce(&an, &[wrong_order]) > LAYER_MAX_DE00);
}

/// No layer covers anything: nothing can be wrong.
#[test]
fn no_layers_reproduce_trivially() {
    let an = AlphaAnalysis {
        layers: Vec::new(),
        opaque_faces: vec![0],
        base_rgb: vec![[0.5, 0.5, 0.5]],
    };
    assert_eq!(layers_reproduce(&an, &[[0.5, 0.5, 0.5]]), 0.0);
}

fn empty_palette() -> inkvec_trace::color::Palette {
    inkvec_trace::color::Palette {
        colors: Vec::new(),
        rgb: Vec::new(),
        weight: Vec::new(),
        alpha: Vec::new(),
    }
}

#[test]
fn test_recover_layers_disabled() {
    let args = Args {
        layers: false,
        ..Args::default()
    };
    let map = planar::PlanarMap::default();
    let res = recover_layers(&args, &map, &[], &[], &empty_palette(), &[]);
    assert!(res.is_none());
}

#[test]
fn test_recover_layers_empty_and_gradient_exclusion() {
    let args = Args {
        layers: true,
        quiet: false,
        ..Args::default()
    };
    let mut map = planar::PlanarMap::default();
    map.edges.push(planar::Edge {
        points: vec![],
        sigma: vec![],
        left: 0,
        right: 1,
        start_node: 0,
        end_node: 1,
        closed: false,
        lambda_scale: 1.0,
    });
    // Face 0: flat fill; Face 1: non-flat gradient fill (Rule 2); Face 2: no fill, palette fallback
    let fills = vec![
        gradient::FillFit {
            model: gradient::FillModel::Flat([1.0, 1.0, 1.0]),
            chi2: 0.0,
            params: 0.0,
            cost: 0.0,
        },
        gradient::FillFit {
            model: gradient::FillModel::Linear {
                p0: (0.0, 0.0),
                p1: (1.0, 1.0),
                c0: [0.0, 0.0, 0.0],
                c1: [1.0, 1.0, 1.0],
                interp: gradient::Interp::Srgb,
                mids: vec![],
            },
            chi2: 0.0,
            params: 0.0,
            cost: 0.0,
        },
    ];
    let pal = inkvec_trace::color::Palette {
        colors: Vec::new(),
        rgb: vec![[0.2, 0.4, 0.6]],
        weight: vec![1.0],
        alpha: vec![1.0],
    };
    let face_color = vec![0, 0, 0];
    let labels = vec![0u16, 1, 2, 0];
    let res = recover_layers(&args, &map, &face_color, &fills, &pal, &labels);
    // Decompose on these simple faces yields no valid layer stack
    assert!(res.is_none());
}

#[test]
fn test_layers_reproduce_missing_face_in_base_rgb() {
    let an_missing = AlphaAnalysis {
        layers: vec![Layer {
            color: [1.0, 0.0, 0.0],
            alpha: 0.5,
            faces: vec![999],
            residual: 0.0,
        }],
        opaque_faces: vec![],
        base_rgb: vec![[1.0, 1.0, 1.0]],
    };
    assert_eq!(layers_reproduce(&an_missing, &[[1.0, 1.0, 1.0]]), 0.0);
}

#[test]
fn test_recover_layers_successful_stack() {
    let white = [1.0f32, 1.0, 1.0];
    let red = [0.9f32, 0.1, 0.1];
    let blue = [0.1f32, 0.1, 0.9];
    let green = [0.1f32, 0.9, 0.1];
    let bw = over(blue, 0.85, white);
    let br = over(blue, 0.85, red);
    let rgb = [
        white,
        red,
        bw,
        over(green, 0.85, white),
        br,
        over(green, 0.85, red),
        over(green, 0.85, bw),
        over(green, 0.85, br),
    ];
    let area = [37834usize, 5846, 5846, 5902, 2318, 2262, 2262, 3266];
    let adj = [
        (0, 1),
        (0, 2),
        (0, 3),
        (1, 4),
        (1, 5),
        (2, 4),
        (2, 6),
        (3, 5),
        (3, 6),
        (4, 7),
        (5, 7),
        (6, 7),
        (0, 4),
        (0, 5),
        (0, 6),
        (1, 7),
        (3, 7),
    ];
    let mut map = planar::PlanarMap::default();
    for &(l, r) in &adj {
        map.edges.push(planar::Edge {
            points: vec![],
            sigma: vec![],
            left: l as u16,
            right: r as u16,
            start_node: 0,
            end_node: 1,
            closed: false,
            lambda_scale: 1.0,
        });
    }
    let fills: Vec<_> = rgb
        .iter()
        .map(|&c| gradient::FillFit {
            model: gradient::FillModel::Flat(c),
            chi2: 0.0,
            params: 0.0,
            cost: 0.0,
        })
        .collect();
    let mut labels = Vec::new();
    for (f, &count) in area.iter().enumerate() {
        labels.extend(std::iter::repeat_n(f as u16, count / 10));
    }
    let args = Args {
        layers: true,
        quiet: false,
        ..Args::default()
    };
    let pal = empty_palette();
    let face_color = vec![0; 8];
    let res = recover_layers(&args, &map, &face_color, &fills, &pal, &labels);
    assert!(res.is_some());
    let an = res.unwrap();
    assert_eq!(an.layers.len(), 2);
}
