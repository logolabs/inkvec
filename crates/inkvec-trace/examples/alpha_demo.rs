//! Layer peeling on the benchmark's worst case, `stack_overlap`.
//!
//!     cargo run -p inkvec-trace --example alpha_demo
//!
//! The source is three circles on white:
//!
//! ```text
//!   <circle cx="96"  cy="110" r="66" fill="#e53e3e"/>
//!   <circle cx="160" cy="110" r="66" fill="#3182ce" fill-opacity="0.85"/>
//!   <circle cx="128" cy="166" r="66" fill="#38a169" fill-opacity="0.85"/>
//! ```
//!
//! Three mutually overlapping discs, two of them translucent, cut the canvas into eight
//! flat regions. A tracer with no alpha model emits every one of them as an opaque fill,
//! which is where 13.2x the ground-truth parameter count comes from. No image is loaded
//! here: the eight region colours are *computed* from the compositing algebra, the
//! region areas are counted by classifying pixel centres, and the adjacency is the one
//! the three circles actually induce.

use inkvec_trace::alpha::{
    composite, composite_in, decompose, decompose_with, AlphaOptions, Layer, Space,
};
use inkvec_trace::color::to_hex;

const CANVAS: usize = 256;

const WHITE: [f32; 3] = [1.0, 1.0, 1.0];
const RED: [f32; 3] = [
    0xe5 as f32 / 255.0,
    0x3e as f32 / 255.0,
    0x3e as f32 / 255.0,
];
const BLUE: [f32; 3] = [
    0x31 as f32 / 255.0,
    0x82 as f32 / 255.0,
    0xce as f32 / 255.0,
];
const GREEN: [f32; 3] = [
    0x38 as f32 / 255.0,
    0xa1 as f32 / 255.0,
    0x69 as f32 / 255.0,
];
const A_BLUE: f32 = 0.85;
const A_GREEN: f32 = 0.85;

/// `(cx, cy, r)` for the red, blue and green discs.
const DISCS: [(f64, f64, f64); 3] = [
    (96.0, 110.0, 66.0),
    (160.0, 110.0, 66.0),
    (128.0, 166.0, 66.0),
];

/// Region id from disc membership `(red, blue, green)`.
fn region(r: bool, b: bool, g: bool) -> usize {
    match (r, b, g) {
        (false, false, false) => 0, // white
        (true, false, false) => 1,  // red
        (false, true, false) => 2,  // blue over white
        (false, false, true) => 3,  // green over white
        (true, true, false) => 4,   // blue over red
        (true, false, true) => 5,   // green over red
        (false, true, true) => 6,   // green over blue over white
        (true, true, true) => 7,    // green over blue over red
    }
}

fn describe(layer: &Layer) -> String {
    format!(
        "fill={} fill-opacity={:.3}  faces={:?}  residual={:.3e}",
        to_hex(layer.color),
        layer.alpha,
        layer.faces,
        layer.residual
    )
}

fn main() {
    // --- the eight region colours, straight from the compositing algebra -------------
    let blue_on_white = composite(BLUE, A_BLUE, WHITE);
    let blue_on_red = composite(BLUE, A_BLUE, RED);
    let face_rgb: [[f32; 3]; 8] = [
        WHITE,
        RED,
        blue_on_white,
        composite(GREEN, A_GREEN, WHITE),
        blue_on_red,
        composite(GREEN, A_GREEN, RED),
        composite(GREEN, A_GREEN, blue_on_white),
        composite(GREEN, A_GREEN, blue_on_red),
    ];

    // --- areas, by classifying pixel centres -----------------------------------------
    let mut face_area = [0usize; 8];
    for y in 0..CANVAS {
        for x in 0..CANVAS {
            let (px, py) = (x as f64 + 0.5, y as f64 + 0.5);
            let inside = |(cx, cy, r): (f64, f64, f64)| {
                (px - cx) * (px - cx) + (py - cy) * (py - cy) <= r * r
            };
            face_area[region(inside(DISCS[0]), inside(DISCS[1]), inside(DISCS[2]))] += 1;
        }
    }

    // --- adjacency the three circles induce ------------------------------------------
    // Crossing one circle's arc flips exactly one membership bit, so two regions share a
    // boundary iff their membership triples differ in one bit and the arc between them
    // is non-degenerate. All twelve such pairs are realised here.
    let adjacency = [
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
    ];

    println!("stack_overlap: 8 observed flat regions\n");
    let names = [
        "white",
        "red",
        "blue/white",
        "green/white",
        "blue/red",
        "green/red",
        "green/blue/white",
        "green/blue/red",
    ];
    for (i, name) in names.iter().enumerate() {
        println!(
            "  face {i}  {name:<17} {}  area {:>6}",
            to_hex(face_rgb[i]),
            face_area[i]
        );
    }

    let res = decompose(&face_rgb, &face_area, &adjacency);

    println!("\npeeled layers (frontmost first):");
    if res.layers.is_empty() {
        println!("  none");
    }
    for (i, l) in res.layers.iter().enumerate() {
        println!("  {i}. {}", describe(l));
    }

    println!("\nfaces claimed by no layer: {:?}", res.opaque_faces);

    // The opaque base each face resolves to, once its layers are lifted. Faces sharing a
    // base colour are the pieces of one opaque shape, which is what the emitter unions.
    println!("\nbase colour after peeling:");
    for (i, name) in names.iter().enumerate() {
        println!("  face {i}  {name:<17} -> {}", to_hex(res.base_rgb[i]));
    }

    // --- what the emitter would write -------------------------------------------------
    let mut bases: Vec<(String, Vec<usize>)> = Vec::new();
    for (i, c) in res.base_rgb.iter().enumerate() {
        let hex = to_hex(*c);
        match bases.iter_mut().find(|(h, _)| *h == hex) {
            Some((_, v)) => v.push(i),
            None => bases.push((hex, vec![i])),
        }
    }
    println!(
        "\nemitted document ({} shapes, bottom to top):",
        bases.len() + res.layers.len()
    );
    for (hex, faces) in &bases {
        println!("  <path fill=\"{hex}\"/>                    union of faces {faces:?}");
    }
    for l in res.layers.iter().rev() {
        println!(
            "  <path fill=\"{}\" fill-opacity=\"{:.2}\"/>  union of faces {:?}",
            to_hex(l.color),
            l.alpha,
            l.faces
        );
    }

    println!(
        "\nground truth: 1 background + 3 discs, two at fill-opacity 0.85.\n\
         ideal:        4 shapes.  observed flat regions: 8."
    );

    // --- and what the *real* raster looks like ----------------------------------------
    //
    // Everything above composites in linear light, which is where alpha blending is
    // physically linear. An SVG renderer does not: the spec's compositing space is sRGB,
    // and `out/input/stack_overlap.png` is composited there — blue over white comes out
    // #5095d5, not the #749ed6 linear light predicts. Trace that image and the linear
    // solver correctly finds nothing, because the model is wrong; `Space::Srgb` finds
    // both layers. Assume nothing about the compositing gamma (DESIGN.md S0).
    //
    // The traced map also carries five adjacencies the three circles do not induce, one
    // per lens tip, where an arc crossing lands on two or three pixel corners instead of
    // a point. Those are added here too.
    let srgb = |c, a, u| composite_in(c, a, u, Space::Srgb);
    let bw_s = srgb(BLUE, A_BLUE, WHITE);
    let br_s = srgb(BLUE, A_BLUE, RED);
    let face_srgb: [[f32; 3]; 8] = [
        WHITE,
        RED,
        bw_s,
        srgb(GREEN, A_GREEN, WHITE),
        br_s,
        srgb(GREEN, A_GREEN, RED),
        srgb(GREEN, A_GREEN, bw_s),
        srgb(GREEN, A_GREEN, br_s),
    ];
    let mut rasterised = adjacency.to_vec();
    rasterised.extend_from_slice(&[(0, 4), (0, 5), (0, 6), (1, 7), (3, 7)]);

    println!("\n--- the same picture as a renderer actually produces it ---");
    println!(
        "  blue over white: linear {}   sRGB {}",
        to_hex(blue_on_white),
        to_hex(bw_s)
    );
    for (label, opt) in [
        ("linear-light solver", AlphaOptions::default()),
        (
            "sRGB solver       ",
            AlphaOptions {
                space: Space::Srgb,
                ..Default::default()
            },
        ),
    ] {
        let r = decompose_with(&face_srgb, &face_area, &rasterised, &opt);
        print!("  {label} -> {} layer(s)", r.layers.len());
        for l in &r.layers {
            print!("  [{}]", describe(l));
        }
        println!();
    }
}
