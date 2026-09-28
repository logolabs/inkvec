//! `--monochrome`: black artwork on a white ground, or with `--no-background` on real
//! transparency. Every test renders the SVG and reads pixels, in both engines.

use inkvec_cli::{post_process, trace_image, Args, TraceMode};
use inkvec_trace::Rgba;

const W: usize = 96;
const H: usize = 64;
const WHITE: [f32; 4] = [1.0, 1.0, 1.0, 1.0];
const BLACK: [f32; 4] = [0.0, 0.0, 0.0, 1.0];
const CLEAR: [f32; 4] = [0.0, 0.0, 0.0, 0.0];
const RED: [f32; 4] = [0.85, 0.12, 0.12, 1.0];
const YELLOW: [f32; 4] = [1.0, 0.85, 0.0, 1.0];

/// A raster drawn by `paint` at sub-pixel positions and box-filtered 4x4, as a renderer
/// anti-aliases: colours are mixed premultiplied, so a transparent ground stays clear.
fn draw(paint: impl Fn(f32, f32) -> [f32; 4]) -> Rgba {
    const SS: usize = 4;
    let mut data = Vec::with_capacity(W * H * 4);
    for y in 0..H {
        for x in 0..W {
            let mut acc = [0.0f32; 4];
            for sy in 0..SS {
                for sx in 0..SS {
                    let c = paint(
                        x as f32 + (sx as f32 + 0.5) / SS as f32,
                        y as f32 + (sy as f32 + 0.5) / SS as f32,
                    );
                    for k in 0..3 {
                        acc[k] += c[k] * c[3];
                    }
                    acc[3] += c[3];
                }
            }
            let a = acc[3] / (SS * SS) as f32;
            let un = |v: f32| {
                if a > 0.0 {
                    v / (SS * SS) as f32 / a
                } else {
                    0.0
                }
            };
            data.extend_from_slice(&[un(acc[0]), un(acc[1]), un(acc[2]), a]);
        }
    }
    Rgba {
        width: W,
        height: H,
        data,
    }
}

/// A letter O: a ring centred at (cx, 32), outer radius 20, inner 10.
fn ring(px: f32, py: f32, cx: f32) -> bool {
    let r = (px - cx).hypot(py - 32.0);
    (10.0..20.0).contains(&r)
}

/// "O" at x = 30 and a solid dot at x = 72, in `ink` on `ground`.
fn letters(ground: [f32; 4], ink: [f32; 4]) -> Rgba {
    draw(|x, y| {
        if ring(x, y, 30.0) || (x - 72.0).hypot(y - 32.0) < 12.0 {
            ink
        } else {
            ground
        }
    })
}

fn trace(img: Rgba, mode: TraceMode, no_background: bool) -> Rgba {
    let args = Args {
        monochrome: true,
        no_background,
        mode,
        ..Args::default()
    };
    let t = trace_image(img, &args).expect("trace succeeds");
    let svg = post_process(&args, t.svg, t.width, t.height);
    assert!(
        !svg.contains("fill=\"#ffffff\"") || !no_background,
        "no white under --no-background: {svg}"
    );
    inkvec_sr::detect::render_svg(&svg, W, H).expect("the SVG renders")
}

fn px(r: &Rgba, x: usize, y: usize) -> [f32; 4] {
    let i = (y * W + x) * 4;
    [r.data[i], r.data[i + 1], r.data[i + 2], r.data[i + 3]]
}

/// Every pixel that carries any paint is black: over transparency only the alpha of an
/// edge varies, so nothing pale can show as a halo on a dark page.
fn no_halo(r: &Rgba) -> usize {
    (0..W * H)
        .filter(|&i| {
            let a = r.data[i * 4 + 3];
            a > 1.0 / 255.0 && (0..3).any(|k| r.data[i * 4 + k] > 2.0 / 255.0)
        })
        .count()
}

/// Black where the letters are, transparent in the counter and on the ground.
fn assert_black_letters_on_clear(r: &Rgba, what: &str) {
    assert_eq!(no_halo(r), 0, "{what}: painted pixels that are not black");
    assert_eq!(px(r, 15, 32)[3], 1.0, "{what}: the O's stroke is solid");
    assert_eq!(px(r, 72, 32)[3], 1.0, "{what}: the dot is solid");
    assert_eq!(px(r, 30, 32)[3], 0.0, "{what}: the O's counter is a hole");
    assert_eq!(px(r, 3, 3)[3], 0.0, "{what}: the ground is gone");
    assert_eq!(px(r, 52, 60)[3], 0.0, "{what}: the ground is gone");
}

const ENGINES: [TraceMode; 2] = [TraceMode::Quality, TraceMode::Fast];

#[test]
fn black_lettering_on_white_keeps_the_letters_and_loses_the_ground() {
    for mode in ENGINES {
        let r = trace(letters(WHITE, BLACK), mode, true);
        assert_black_letters_on_clear(&r, &format!("{mode:?}"));
        // Black & white: the same letters on an opaque white ground.
        let bw = trace(letters(WHITE, BLACK), mode, false);
        assert_eq!(px(&bw, 30, 32), WHITE, "{mode:?}: the counter is white");
        assert_eq!(px(&bw, 3, 3), WHITE, "{mode:?}");
        assert_eq!(px(&bw, 15, 32), BLACK, "{mode:?}");
    }
}

#[test]
fn black_lettering_on_a_coloured_ground_loses_the_ground() {
    for mode in ENGINES {
        let r = trace(letters(RED, BLACK), mode, true);
        assert_black_letters_on_clear(&r, &format!("{mode:?} on red"));
    }
}

#[test]
fn black_lettering_on_transparency_stays_on_transparency() {
    for mode in ENGINES {
        let r = trace(letters(CLEAR, BLACK), mode, true);
        assert_black_letters_on_clear(&r, &format!("{mode:?} on clear"));
        // White lettering on a transparent PNG is artwork too, and comes back black.
        let r = trace(letters(CLEAR, WHITE), mode, true);
        assert_black_letters_on_clear(&r, &format!("{mode:?} white on clear"));
    }
}

#[test]
fn white_lettering_on_black_is_the_artwork() {
    for mode in ENGINES {
        let r = trace(letters(BLACK, WHITE), mode, true);
        assert_black_letters_on_clear(&r, &format!("{mode:?} white on black"));
    }
}

#[test]
fn a_light_colour_on_white_is_kept_black() {
    // A yellow O beside a red dot: lightness alone would lose the yellow at the midpoint.
    let img = draw(|x, y| {
        if ring(x, y, 30.0) {
            YELLOW
        } else if (x - 72.0).hypot(y - 32.0) < 12.0 {
            RED
        } else {
            WHITE
        }
    });
    for mode in ENGINES {
        let r = trace(img.clone(), mode, true);
        assert_black_letters_on_clear(&r, &format!("{mode:?} yellow and red"));
    }
}

#[test]
fn lettering_on_a_panel_is_knocked_out_of_it() {
    // A black bar across a yellow panel on white: the panel is black, the bar a hole in it.
    let img = draw(|x, y| {
        let panel = (12.0..84.0).contains(&x) && (10.0..54.0).contains(&y);
        let bar = (24.0..72.0).contains(&x) && (26.0..38.0).contains(&y);
        match (panel, bar) {
            (true, true) => BLACK,
            (true, false) => YELLOW,
            _ => WHITE,
        }
    });
    for mode in ENGINES {
        let r = trace(img.clone(), mode, true);
        assert_eq!(no_halo(&r), 0, "{mode:?}");
        assert_eq!(px(&r, 16, 14)[3], 1.0, "{mode:?}: the panel is black");
        assert_eq!(px(&r, 48, 32)[3], 0.0, "{mode:?}: the bar is knocked out");
        assert_eq!(px(&r, 4, 4)[3], 0.0, "{mode:?}: the ground is gone");
    }
}

#[test]
fn edges_sit_where_the_source_drew_them() {
    // Over the O's anti-aliased rim the traced alpha matches the source's coverage.
    let src = letters(WHITE, BLACK);
    for mode in ENGINES {
        let r = trace(src.clone(), mode, true);
        let (mut err, mut n) = (0.0f32, 0);
        for i in 0..W * H {
            let cover = 1.0 - src.data[i * 4];
            if cover > 0.02 && cover < 0.98 {
                err += (r.data[i * 4 + 3] - cover).abs();
                n += 1;
            }
        }
        let mean = err / n as f32;
        eprintln!("{mode:?}: mean alpha error {mean:.4} over {n} edge pixels");
        assert!(n > 100, "{n} edge pixels");
        assert!(
            mean < 0.12,
            "{mode:?}: mean alpha error {mean:.3} on edge pixels"
        );
    }
}

#[test]
fn black_art_through_the_canvas_corners_survives_no_background() {
    // A black X from corner to corner on white: its outline visits all four canvas corners,
    // which the background knock-out would otherwise take for the canvas.
    let img = draw(|x, y| {
        let d1 = (y - x * H as f32 / W as f32).abs();
        let d2 = (y - (W as f32 - x) * H as f32 / W as f32).abs();
        if d1 < 5.0 || d2 < 5.0 {
            BLACK
        } else {
            WHITE
        }
    });
    for mode in ENGINES {
        let r = trace(img.clone(), mode, true);
        assert_eq!(px(&r, W / 2, H / 2)[3], 1.0, "{mode:?}: the X is drawn");
        assert_eq!(px(&r, W / 2, 4)[3], 0.0, "{mode:?}: the ground is gone");
    }
}
