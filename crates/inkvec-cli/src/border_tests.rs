//! The border pad: which rasters it applies to, the pad itself, and the translation back,
//! form by form, including the forms it must refuse.

use super::*;

/// A `w x h` raster, every pixel `px`.
fn flat(w: usize, h: usize, px: [f32; 4]) -> Rgba {
    Rgba {
        width: w,
        height: h,
        data: px.repeat(w * h),
    }
}

fn set(img: &mut Rgba, x: usize, y: usize, px: [f32; 4]) {
    let i = (y * img.width + x) * 4;
    img.data[i..i + 4].copy_from_slice(&px);
}

const INK: [f32; 4] = [0.2, 0.4, 0.6, 1.0];
const CLEAR: [f32; 4] = [0.0, 0.0, 0.0, 0.0];

#[test]
fn only_transparent_rasters_with_ink_on_the_ring_are_padded() {
    // Opaque everywhere: never padded, whatever is on the border.
    assert!(!touches_border(&flat(6, 5, INK)));
    // Transparent, ink well inside: not padded.
    let mut inside = flat(6, 5, CLEAR);
    set(&mut inside, 2, 2, INK);
    assert!(!touches_border(&inside));
    // Ink on each side of the ring in turn.
    for (x, y) in [(3, 0), (3, 4), (0, 2), (5, 2), (0, 0), (5, 4)] {
        let mut img = flat(6, 5, CLEAR);
        set(&mut img, x, y, INK);
        assert!(touches_border(&img), "ink at ({x}, {y})");
    }
    // A faint fringe counts: any alpha above zero.
    let mut faint = flat(6, 5, CLEAR);
    set(&mut faint, 0, 3, [0.0, 0.0, 0.0, 1.0 / 255.0]);
    assert!(touches_border(&faint));
    // An opaque sticker filling the canvas but for one transparent corner.
    let mut sticker = flat(6, 5, INK);
    set(&mut sticker, 0, 0, CLEAR);
    assert!(touches_border(&sticker));
    // One row, one column, empty.
    assert!(touches_border(&flat(1, 1, [0.0, 0.0, 0.0, 0.5])));
    assert!(!touches_border(&flat(0, 0, CLEAR)));
}

#[test]
fn the_pad_embeds_the_raster_at_pad_pad_in_clear_pixels() {
    let mut img = flat(3, 2, CLEAR);
    set(&mut img, 0, 0, INK);
    set(&mut img, 2, 1, [1.0, 0.0, 0.0, 0.5]);
    let p = pad(&img);
    assert_eq!((p.width, p.height), (3 + 2 * PAD, 2 + 2 * PAD));
    for y in 0..p.height {
        for x in 0..p.width {
            let inside = (PAD..PAD + 3).contains(&x) && (PAD..PAD + 2).contains(&y);
            let want = if inside {
                img.pixel(x - PAD, y - PAD)
            } else {
                CLEAR
            };
            assert_eq!(p.pixel(x, y), want, "({x}, {y})");
        }
    }
}

/// The emitter's header for a padded `w x h` canvas.
fn head(w: usize, h: usize) -> String {
    let (pw, ph) = (w + 2 * PAD, h + 2 * PAD);
    format!("<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"-0.5 -0.5 {pw} {ph}\" width=\"{pw}\" height=\"{ph}\">")
}

#[test]
fn every_emitted_form_moves_by_minus_pad() {
    let svg = format!(
        "{}<defs><linearGradient id=\"a\" gradientUnits=\"userSpaceOnUse\" x1=\"3.000\" y1=\"4.500\" x2=\"10.250\" y2=\"-1.000\"><stop offset=\"0\" stop-color=\"#fff\"/></linearGradient>\
<radialGradient id=\"b\" gradientUnits=\"userSpaceOnUse\" cx=\"6.000\" cy=\"7.000\" r=\"3.000\"><stop offset=\"1\" stop-color=\"#000\"/></radialGradient>\
<radialGradient id=\"c\" gradientUnits=\"userSpaceOnUse\" gradientTransform=\"translate(5.000 6.000) rotate(30.00) scale(1 0.5000) translate(-5.000 -6.000)\" cx=\"5.000\" cy=\"6.000\" r=\"2.000\"/>\
<linearGradient id=\"d\" x1=\"0\" y1=\"0\" x2=\"1\" y2=\"0\"/></defs>\
<g id=\"x-group\"><path id=\"p\" d=\"M2.00,3.50L12.25,3.50C13.00,4.00 14.00,5.00 15.00,6.00A1.50,1.50 0.000 0,1 16.00,7.00H1.00V0.00Q2.00,2.00 3.00,3.00Z\" fill=\"url(#a)\"/>\
<rect x=\"1.50\" y=\"2.50\" width=\"4.00\" height=\"5.00\" rx=\"1.00\" fill=\"#f00\"/>\
<circle cx=\"8.00\" cy=\"9.00\" r=\"2.00\" fill=\"#0f0\"/>\
<ellipse cx=\"5.00\" cy=\"6.00\" rx=\"3.00\" ry=\"1.00\" fill=\"#00f\" transform=\"rotate(12.500 5.00 6.00)\"/></g></svg>",
        head(12, 10)
    );
    let out = crop(&svg, 12, 10).expect("every form is known");
    assert!(out.starts_with(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"-0.5 -0.5 12 10\" width=\"12\" height=\"10\">"
    ));
    for want in [
        "x1=\"1.000\" y1=\"2.500\" x2=\"8.250\" y2=\"-3.000\"",
        "cx=\"4.000\" cy=\"5.000\" r=\"3.000\"",
        "gradientTransform=\"translate(3.000 4.000) rotate(30.00) scale(1 0.5000) translate(-5.000 -6.000)\" cx=\"5.000\" cy=\"6.000\"",
        // An objectBoundingBox gradient is relative to the shape: untouched.
        "<linearGradient id=\"d\" x1=\"0\" y1=\"0\" x2=\"1\" y2=\"0\"/>",
        "d=\"M0.00,1.50L10.25,1.50C11.00,2.00 12.00,3.00 13.00,4.00A1.50,1.50 0.000 0,1 14.00,5.00H-1.00V-2.00Q0.00,0.00 1.00,1.00Z\"",
        "<rect x=\"-0.50\" y=\"0.50\" width=\"4.00\" height=\"5.00\" rx=\"1.00\"",
        "<circle cx=\"6.00\" cy=\"7.00\" r=\"2.00\"",
        "<ellipse cx=\"3.00\" cy=\"4.00\" rx=\"3.00\" ry=\"1.00\" fill=\"#00f\" transform=\"rotate(12.500 3.00 4.00)\"",
    ] {
        assert!(out.contains(want), "missing {want}\nin {out}");
    }
}

#[test]
fn relative_commands_keep_their_deltas_but_a_leading_m_moves() {
    assert_eq!(
        shift_path_data("m5,6l1,1c1,2 3,4 5,6z", 2.0).unwrap(),
        "m3,4l1,1c1,2 3,4 5,6z"
    );
    // Several pairs after one M are linetos, all absolute.
    assert_eq!(shift_path_data("M5,6 7,8", 2.0).unwrap(), "M3,4 5,6");
    // After the first pair, a relative m's further pairs are relative linetos.
    assert_eq!(shift_path_data("m5,6 7,8", 2.0).unwrap(), "m3,4 7,8");
}

#[test]
fn a_gradient_transform_that_does_not_start_with_translate_is_prefixed() {
    assert_eq!(
        prepend_translate("rotate(10) scale(2)", 2.0).unwrap(),
        "translate(-2 -2) rotate(10) scale(2)"
    );
    assert_eq!(
        prepend_translate("translate(1,2)", 2.0).unwrap(),
        "translate(-1 0)"
    );
}

#[test]
fn unknown_or_untranslatable_forms_are_refused() {
    let h = head(4, 4);
    for body in [
        "<use href=\"#s\" transform=\"matrix(1 0 0 1 3 4)\"/>",
        "<symbol id=\"s\"><path d=\"M0,0L1,1Z\"/></symbol>",
        "<defs><path id=\"s\" d=\"M0,0L1,1Z\"/></defs>",
        "<g transform=\"translate(1 1)\"><path d=\"M0,0L1,1Z\"/></g>",
        "<path d=\"M0,0L1,1Z\" transform=\"scale(2)\"/>",
        "<ellipse cx=\"1\" cy=\"1\" rx=\"1\" ry=\"1\" transform=\"rotate(30)\"/>",
        "<polygon points=\"0,0 1,1 1,0\"/>",
        "<image href=\"x.png\" x=\"0\" y=\"0\"/>",
        "<text x=\"0\" y=\"0\">a</text>",
        "<path d=\"M0,0L1\"/>",
        "<path d=\"M0,0X1,1\"/>",
    ] {
        let svg = format!("{h}{body}</svg>");
        assert!(crop(&svg, 4, 4).is_none(), "accepted {body}");
    }
    // A header that is not the padded canvas's.
    let wrong = "<svg viewBox=\"-0.5 -0.5 4 4\" width=\"4\" height=\"4\"><path d=\"M0,0Z\"/></svg>";
    assert!(crop(wrong, 4, 4).is_none());
}

#[test]
fn comments_metadata_and_text_between_tags_pass_through() {
    let svg = format!(
        "{}\n<!-- a <path d=\"M9,9\"/> comment -->\n<path d=\"M3,3Z\"/>\n</svg>",
        head(4, 4)
    );
    let out = crop(&svg, 4, 4).unwrap();
    assert!(out.contains("<!-- a <path d=\"M9,9\"/> comment -->"));
    assert!(out.contains("<path d=\"M1,1Z\"/>"));
    assert!(out.ends_with("\n</svg>"));
}
