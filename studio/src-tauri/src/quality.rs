//! The quality report: what the trace actually cost, measured rather than asserted.
//!
//! No other free tracer tells the user whether the trace was any good, and the whole
//! panel is worthless if the headline number is a guess. So the colour difference is a
//! measurement: the SVG that was just written is rendered back to pixels at the size it
//! was traced at, and compared with the source raster the tracer saw, pixel for pixel, in
//! CIEDE2000.
//!
//! Three things that keep the comparison honest:
//!
//! * **The same raster.** `inkvec_cli::intake` may cap or resample the input; the tracer
//!   works on the result, so that is what the render is compared against, not the file on
//!   disk.
//! * **The same ground.** Both sides are composited onto one neutral matte before the
//!   conversion, so a transparency the trace got wrong shows up as colour error instead
//!   of being skipped.
//! * **Mean and median.** A trace can be excellent everywhere and wrong along one seam;
//!   the mean says so and the median does not, and the panel shows both.

use resvg::tiny_skia;
use resvg::usvg;
use serde::Serialize;

/// The mid-grey both sides are composited onto before the comparison. Neutral, and far
/// from both ends of the range, so neither a white nor a black artwork is flattered.
const MATTE: [f32; 3] = [0.5, 0.5, 0.5];

/// What one trace produced, as numbers.
#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Report {
    /// Mean colour difference, dE00, against the source. `None` when the SVG could not be
    /// rendered back — the panel then says so rather than showing a made-up number.
    pub mean_de00: Option<f64>,
    /// Median colour difference, dE00.
    pub median_de00: Option<f64>,
    /// The 99th percentile: where the trace is at its worst.
    pub worst_de00: Option<f64>,
    /// Coordinates in the path data.
    pub coordinates: usize,
    /// Drawn elements: paths, circles, ellipses and rects.
    pub paths: usize,
    /// Segments (move, line, cubic, arc, close) in the path data.
    pub segments: usize,
    /// Distinct inks.
    pub colours: usize,
    /// Bytes of the SVG as written.
    pub bytes: usize,
    /// Bytes after the minifier, for the same drawing.
    pub minified_bytes: Option<usize>,
    /// Seconds the trace took.
    pub seconds: f64,
    /// The longer side the trace ran at, in pixels.
    pub traced_px: u32,
}

/// One ink in the palette.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Ink {
    /// The colour the tracer measured, `#rrggbb`.
    pub traced: String,
    /// What it is painted as now: the traced value, or a colour the user snapped it to.
    pub hex: String,
    /// Share of the canvas it covers, 0..1.
    pub share: f64,
    /// Colour difference between `traced` and `hex`, when the user has snapped it.
    pub snapped_de00: Option<f64>,
}

/// Where the trace and the source disagree most: the region the viewer can jump to.
#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorstCorner {
    /// Centre of the window, in traced-raster pixels.
    pub x: u32,
    /// Centre of the window, in traced-raster pixels.
    pub y: u32,
    /// Mean dE00 inside the window.
    pub de00: f64,
}

/// Render an SVG document to straight RGBA8 at `w` x `h`.
///
/// The SVG's own viewBox is mapped onto the whole pixmap, so the render lines up with the
/// raster whatever `width`/`height` the document declares.
pub fn render(svg: &str, w: u32, h: u32) -> Result<Vec<u8>, String> {
    if w == 0 || h == 0 {
        return Err("cannot render to a zero-sized pixmap".into());
    }
    // No font database is loaded on purpose: the tracer never writes <text>, and a render
    // that silently substituted a font would be comparing against something the SVG does
    // not actually say.
    let opt = usvg::Options::default();
    let tree = usvg::Tree::from_str(svg, &opt).map_err(|e| format!("cannot parse the SVG: {e}"))?;
    let size = tree.size();
    if size.width() <= 0.0 || size.height() <= 0.0 {
        return Err("the SVG declares an empty canvas".into());
    }
    let mut pixmap =
        tiny_skia::Pixmap::new(w, h).ok_or_else(|| format!("{w}x{h} is too large to render"))?;
    let scale = tiny_skia::Transform::from_scale(w as f32 / size.width(), h as f32 / size.height());
    resvg::render(&tree, scale, &mut pixmap.as_mut());

    // tiny-skia stores premultiplied RGBA; the comparison wants straight.
    let mut out = Vec::with_capacity((w as usize) * (h as usize) * 4);
    for px in pixmap.pixels() {
        let a = px.alpha();
        if a == 0 {
            out.extend_from_slice(&[0, 0, 0, 0]);
        } else {
            let un = |c: u8| ((c as u32 * 255 + a as u32 / 2) / a as u32).min(255) as u8;
            out.extend_from_slice(&[un(px.red()), un(px.green()), un(px.blue()), a]);
        }
    }
    Ok(out)
}

/// The pixel-for-pixel comparison, kept whole.
///
/// The per-pixel differences are the expensive part and more than one panel reads them:
/// the report wants three summary statistics, the viewer wants the worst window, and the
/// honesty panel wants to know *where* the disagreement is and what the source looks like
/// there. So the map is computed once and handed around.
pub struct Analysis {
    /// dE00 per pixel, row-major, `width * height` long.
    pub deltas: Vec<f32>,
    /// The SVG rendered back to straight RGBA8, the same size as the source.
    pub rendered: Vec<u8>,
    /// Width of both, in pixels.
    pub width: u32,
    /// Height of both, in pixels.
    pub height: u32,
    /// Mean dE00 over the canvas.
    pub mean: f64,
    /// Median dE00.
    pub median: f64,
    /// 99th percentile dE00.
    pub worst: f64,
    /// The window the two disagree in most, if any is worth going to.
    pub corner: Option<WorstCorner>,
}

/// Compare the raster the tracer saw with a render of the SVG it wrote.
pub fn analyse(source: &inkvec_trace::Rgba, svg: &str) -> Result<Analysis, String> {
    let (w, h) = (source.width as u32, source.height as u32);
    let rendered = render(svg, w, h)?;

    let n = (w as usize) * (h as usize);
    if source.data.len() < n * 4 {
        return Err("the source raster is shorter than its own dimensions".into());
    }
    let mut deltas = vec![0.0f32; n];
    for (i, delta) in deltas.iter_mut().enumerate() {
        let s = &source.data[i * 4..i * 4 + 4];
        let r = &rendered[i * 4..i * 4 + 4];
        let lab_s = lab(over_matte([s[0], s[1], s[2]], s[3]));
        let lab_r = lab(over_matte(
            [
                r[0] as f32 / 255.0,
                r[1] as f32 / 255.0,
                r[2] as f32 / 255.0,
            ],
            r[3] as f32 / 255.0,
        ));
        *delta = ciede2000(lab_s, lab_r) as f32;
    }

    let mean = deltas.iter().map(|d| *d as f64).sum::<f64>() / n as f64;
    let mut sorted = deltas.clone();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let median = sorted[n / 2] as f64;
    let worst = sorted[((n as f64 * 0.99) as usize).min(n - 1)] as f64;
    let corner = worst_corner(&deltas, w, h);

    Ok(Analysis {
        deltas,
        rendered,
        width: w,
        height: h,
        mean,
        median,
        worst,
        corner,
    })
}

/// The window, on a coarse grid, whose mean dE00 is highest.
///
/// This is what "find the worst corner" jumps to. A grid rather than a search: the point
/// is to land somewhere the disagreement is visible at 12x, not to find a global optimum.
fn worst_corner(deltas: &[f32], w: u32, h: u32) -> Option<WorstCorner> {
    let win = (w.min(h) / 12).clamp(8, 96);
    if w < win || h < win {
        return None;
    }
    let step = (win / 2).max(1);
    let mut best: Option<WorstCorner> = None;
    let mut y = 0;
    while y + win <= h {
        let mut x = 0;
        while x + win <= w {
            let mut sum = 0.0f64;
            for yy in y..y + win {
                let row = (yy as usize) * (w as usize);
                for xx in x..x + win {
                    sum += deltas[row + xx as usize] as f64;
                }
            }
            let mean = sum / (win as f64 * win as f64);
            if best.is_none_or(|b| mean > b.de00) {
                best = Some(WorstCorner {
                    x: x + win / 2,
                    y: y + win / 2,
                    de00: mean,
                });
            }
            x += step;
        }
        y += step;
    }
    // A perfectly clean trace has no worst corner worth going to.
    best.filter(|b| b.de00 > 0.05)
}

/// Composite a straight RGBA colour onto the shared matte.
fn over_matte(rgb: [f32; 3], a: f32) -> [f32; 3] {
    let a = a.clamp(0.0, 1.0);
    [
        rgb[0] * a + MATTE[0] * (1.0 - a),
        rgb[1] * a + MATTE[1] * (1.0 - a),
        rgb[2] * a + MATTE[2] * (1.0 - a),
    ]
}

/// sRGB (0..1) to CIE L*a*b* under D65, the space dE00 is defined in.
pub fn lab(rgb: [f32; 3]) -> [f64; 3] {
    let lin = |c: f32| -> f64 {
        let c = c.clamp(0.0, 1.0) as f64;
        if c <= 0.04045 {
            c / 12.92
        } else {
            ((c + 0.055) / 1.055).powf(2.4)
        }
    };
    let (r, g, b) = (lin(rgb[0]), lin(rgb[1]), lin(rgb[2]));
    // sRGB -> XYZ (D65), then normalised by the D65 white point.
    let x = (0.412_456_4 * r + 0.357_576_1 * g + 0.180_437_5 * b) / 0.950_47;
    let y = 0.212_672_9 * r + 0.715_152_2 * g + 0.072_175_0 * b;
    let z = (0.019_333_9 * r + 0.119_192_0 * g + 0.950_304_1 * b) / 1.088_83;
    let f = |t: f64| {
        if t > 216.0 / 24389.0 {
            t.cbrt()
        } else {
            (24389.0 / 27.0 * t + 16.0) / 116.0
        }
    };
    let (fx, fy, fz) = (f(x), f(y), f(z));
    [116.0 * fy - 16.0, 500.0 * (fx - fy), 200.0 * (fy - fz)]
}

/// CIEDE2000 between two L*a*b* colours.
///
/// The standard formulation (Sharma, Wu & Dalal 2005), including the hue-rotation term
/// that the simplified versions drop. Around 1.0 is where a trained eye starts to see a
/// difference; a good trace lands near 0.1.
pub fn ciede2000(l1: [f64; 3], l2: [f64; 3]) -> f64 {
    const K_L: f64 = 1.0;
    const K_C: f64 = 1.0;
    const K_H: f64 = 1.0;

    let (l_1, a_1, b_1) = (l1[0], l1[1], l1[2]);
    let (l_2, a_2, b_2) = (l2[0], l2[1], l2[2]);

    let c1 = (a_1 * a_1 + b_1 * b_1).sqrt();
    let c2 = (a_2 * a_2 + b_2 * b_2).sqrt();
    let c_bar = (c1 + c2) / 2.0;

    let c_bar7 = c_bar.powi(7);
    let g = 0.5 * (1.0 - (c_bar7 / (c_bar7 + 25f64.powi(7))).sqrt());

    let a1p = (1.0 + g) * a_1;
    let a2p = (1.0 + g) * a_2;
    let c1p = (a1p * a1p + b_1 * b_1).sqrt();
    let c2p = (a2p * a2p + b_2 * b_2).sqrt();

    let hp = |ap: f64, b: f64| -> f64 {
        if ap == 0.0 && b == 0.0 {
            0.0
        } else {
            let h = b.atan2(ap).to_degrees();
            if h < 0.0 {
                h + 360.0
            } else {
                h
            }
        }
    };
    let h1p = hp(a1p, b_1);
    let h2p = hp(a2p, b_2);

    let dlp = l_2 - l_1;
    let dcp = c2p - c1p;

    let dhp = if c1p * c2p == 0.0 {
        0.0
    } else {
        let d = h2p - h1p;
        if d > 180.0 {
            d - 360.0
        } else if d < -180.0 {
            d + 360.0
        } else {
            d
        }
    };
    let dhp_big = 2.0 * (c1p * c2p).sqrt() * (dhp.to_radians() / 2.0).sin();

    let lp_bar = (l_1 + l_2) / 2.0;
    let cp_bar = (c1p + c2p) / 2.0;

    let hp_bar = if c1p * c2p == 0.0 {
        h1p + h2p
    } else {
        let d = (h1p - h2p).abs();
        if d <= 180.0 {
            (h1p + h2p) / 2.0
        } else if h1p + h2p < 360.0 {
            (h1p + h2p + 360.0) / 2.0
        } else {
            (h1p + h2p - 360.0) / 2.0
        }
    };

    let t = 1.0 - 0.17 * (hp_bar - 30.0).to_radians().cos()
        + 0.24 * (2.0 * hp_bar).to_radians().cos()
        + 0.32 * (3.0 * hp_bar + 6.0).to_radians().cos()
        - 0.20 * (4.0 * hp_bar - 63.0).to_radians().cos();

    let d_theta = 30.0 * (-(((hp_bar - 275.0) / 25.0).powi(2))).exp();
    let cp_bar7 = cp_bar.powi(7);
    let r_c = 2.0 * (cp_bar7 / (cp_bar7 + 25f64.powi(7))).sqrt();
    let s_l = 1.0 + (0.015 * (lp_bar - 50.0).powi(2)) / (20.0 + (lp_bar - 50.0).powi(2)).sqrt();
    let s_c = 1.0 + 0.045 * cp_bar;
    let s_h = 1.0 + 0.015 * cp_bar * t;
    let r_t = -(2.0 * d_theta.to_radians()).sin() * r_c;

    let term_l = dlp / (K_L * s_l);
    let term_c = dcp / (K_C * s_c);
    let term_h = dhp_big / (K_H * s_h);

    (term_l * term_l + term_c * term_c + term_h * term_h + r_t * term_c * term_h).sqrt()
}

/// Count what the SVG holds: coordinates, segments, drawn elements, distinct inks.
pub fn count(svg: &str) -> (usize, usize, usize, usize) {
    let mut coordinates = 0usize;
    let mut segments = 0usize;
    let mut elements = 0usize;
    let mut inks: Vec<String> = Vec::new();

    for tag in ["<path", "<circle", "<ellipse", "<rect", "<polygon", "<line"] {
        elements += svg.matches(tag).count();
    }
    // <svg ...> itself is not a drawn element, and neither is a <rect> inside <defs>;
    // the first is worth subtracting because it is always there.
    elements = elements.saturating_sub(svg.matches("<rect").count().min(0));

    for d in attribute_values(svg, "d") {
        let (c, s) = count_path(d);
        coordinates += c;
        segments += s;
    }
    for key in ["fill", "stroke"] {
        for v in attribute_values(svg, key) {
            let v = v.trim();
            if v.starts_with('#') && !inks.iter().any(|i| i == v) {
                inks.push(v.to_ascii_lowercase());
            }
        }
    }
    (coordinates, segments, elements, inks.len())
}

/// Numbers and commands in one `d` attribute.
fn count_path(d: &str) -> (usize, usize) {
    let mut numbers = 0usize;
    let mut commands = 0usize;
    let mut in_number = false;
    for ch in d.chars() {
        if ch.is_ascii_digit() || ch == '.' || ch == '-' || ch == '+' || ch == 'e' || ch == 'E' {
            if !in_number {
                in_number = true;
                numbers += 1;
            }
        } else {
            in_number = false;
            if ch.is_ascii_alphabetic() {
                commands += 1;
            }
        }
    }
    (numbers, commands)
}

/// Every value of `name="..."` in the document, in order.
///
/// A scan rather than a parse: this runs on the tracer's own output, which is generated,
/// flat and free of namespaces, and the numbers it feeds are counts for a readout.
fn attribute_values<'a>(svg: &'a str, name: &str) -> Vec<&'a str> {
    let needle = format!(" {name}=\"");
    let mut out = Vec::new();
    let mut rest = svg;
    while let Some(i) = rest.find(&needle) {
        let after = &rest[i + needle.len()..];
        match after.find('"') {
            Some(j) => {
                out.push(&after[..j]);
                rest = &after[j + 1..];
            }
            None => break,
        }
    }
    out
}

/// The palette: every ink the SVG paints with, and how much of the canvas it covers.
///
/// The share is measured from a render rather than inferred from the geometry, because
/// what the user is being shown is the picture, and overlap, holes and opacity all move
/// the answer.
pub fn palette(svg: &str, w: u32, h: u32) -> Result<Vec<Ink>, String> {
    let declared: Vec<[f32; 3]> = {
        let mut seen: Vec<String> = Vec::new();
        for key in ["fill", "stroke"] {
            for v in attribute_values(svg, key) {
                let v = v.trim().to_ascii_lowercase();
                if v.starts_with('#') && parse_hex(&v).is_some() && !seen.contains(&v) {
                    seen.push(v);
                }
            }
        }
        seen.iter().filter_map(|s| parse_hex(s)).collect()
    };
    if declared.is_empty() {
        return Ok(Vec::new());
    }

    // Render small: the share is a percentage, and a 256-px render settles it to well
    // inside the 1% the readout shows while costing nothing.
    let scale = 256.0 / w.max(h).max(1) as f64;
    let (rw, rh) = (
        ((w as f64 * scale).round() as u32).max(1),
        ((h as f64 * scale).round() as u32).max(1),
    );
    let px = render(svg, rw, rh)?;

    let labs: Vec<[f64; 3]> = declared.iter().map(|c| lab(*c)).collect();
    let mut counts = vec![0usize; declared.len()];
    let mut covered = 0usize;
    for chunk in px.chunks_exact(4) {
        if chunk[3] < 8 {
            continue;
        }
        covered += 1;
        let here = lab([
            chunk[0] as f32 / 255.0,
            chunk[1] as f32 / 255.0,
            chunk[2] as f32 / 255.0,
        ]);
        let mut best = (f64::MAX, 0usize);
        for (i, l) in labs.iter().enumerate() {
            let d = ciede2000(here, *l);
            if d < best.0 {
                best = (d, i);
            }
        }
        counts[best.1] += 1;
    }

    let total = covered.max(1) as f64;
    let mut inks: Vec<Ink> = declared
        .iter()
        .zip(counts.iter())
        .map(|(c, n)| {
            let hex = to_hex(*c);
            Ink {
                traced: hex.clone(),
                hex,
                share: *n as f64 / total,
                snapped_de00: None,
            }
        })
        .collect();
    inks.sort_by(|a, b| {
        b.share
            .partial_cmp(&a.share)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    Ok(inks)
}

/// `#rgb` or `#rrggbb` to linear 0..1 sRGB components.
pub fn parse_hex(s: &str) -> Option<[f32; 3]> {
    let s = s.trim().trim_start_matches('#');
    let v = |a: &str| u8::from_str_radix(a, 16).ok().map(|n| n as f32 / 255.0);
    match s.len() {
        3 => {
            let d: Vec<char> = s.chars().collect();
            Some([
                v(&format!("{}{}", d[0], d[0]))?,
                v(&format!("{}{}", d[1], d[1]))?,
                v(&format!("{}{}", d[2], d[2]))?,
            ])
        }
        6 => Some([v(&s[0..2])?, v(&s[2..4])?, v(&s[4..6])?]),
        _ => None,
    }
}

/// 0..1 sRGB components back to `#rrggbb`.
pub fn to_hex(c: [f32; 3]) -> String {
    let b = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
    format!("#{:02x}{:02x}{:02x}", b(c[0]), b(c[1]), b(c[2]))
}

/// The colour difference between two hex strings, for the "you are overriding a
/// measurement" line beside a snapped swatch.
pub fn hex_distance(a: &str, b: &str) -> Option<f64> {
    Some(ciede2000(lab(parse_hex(a)?), lab(parse_hex(b)?)))
}

#[cfg(test)]
mod tests {
    use super::*;

    const SVG: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" width="64" height="64" viewBox="0 0 64 64"><rect width="64" height="64" fill="#ffffff"/><path d="M8 8L56 8L56 56L8 56Z" fill="#14453f"/></svg>"##;

    #[test]
    fn identical_colours_are_zero_apart() {
        assert!(ciede2000(lab([0.2, 0.4, 0.6]), lab([0.2, 0.4, 0.6])).abs() < 1e-9);
    }

    #[test]
    fn ciede2000_matches_the_sharma_reference_pairs() {
        // Rows from the Sharma, Wu & Dalal (2005) test data, Lab in, dE00 out.
        let cases = [
            ([50.0, 2.6772, -79.7751], [50.0, 0.0, -82.7485], 2.0425),
            ([50.0, 3.1571, -77.2803], [50.0, 0.0, -82.7485], 2.8615),
            ([50.0, 2.8361, -74.0200], [50.0, 0.0, -82.7485], 3.4412),
            ([50.0, -1.3802, -84.2814], [50.0, 0.0, -82.7485], 1.0000),
            ([50.0, 2.5, 0.0], [50.0, 0.0, -2.5], 4.3065),
            (
                [60.2574, -34.0099, 36.2677],
                [60.4626, -34.1751, 39.4387],
                1.2644,
            ),
            (
                [22.7233, 20.0904, -46.6940],
                [23.0331, 14.9730, -42.5619],
                2.0373,
            ),
            (
                [2.0776, 0.0795, -1.1350],
                [0.9033, -0.0636, -0.5514],
                0.9082,
            ),
        ];
        for (a, b, want) in cases {
            let got = ciede2000(a, b);
            assert!(
                (got - want).abs() < 1e-3,
                "dE00({a:?}, {b:?}) = {got}, want {want}"
            );
        }
    }

    #[test]
    fn white_and_black_are_a_hundred_apart() {
        let d = ciede2000(lab([1.0, 1.0, 1.0]), lab([0.0, 0.0, 0.0]));
        assert!((d - 100.0).abs() < 0.5, "{d}");
    }

    #[test]
    fn hex_round_trips() {
        assert_eq!(to_hex(parse_hex("#14453F").unwrap()), "#14453f");
        assert_eq!(to_hex(parse_hex("#abc").unwrap()), "#aabbcc");
        assert!(parse_hex("#12345").is_none());
    }

    #[test]
    fn counting_reads_the_path_data() {
        let (coords, segments, elements, inks) = count(SVG);
        assert_eq!(coords, 8, "four points, two numbers each");
        assert_eq!(segments, 5, "M L L L Z");
        assert_eq!(elements, 2);
        assert_eq!(inks, 2);
    }

    #[test]
    fn a_render_of_the_svg_has_the_colours_the_svg_declares() {
        let px = render(SVG, 64, 64).unwrap();
        // Middle pixel is inside the dark square.
        let i = ((32 * 64) + 32) * 4;
        assert_eq!(&px[i..i + 3], &[0x14, 0x45, 0x3f]);
        // A corner is the white ground.
        assert_eq!(&px[0..3], &[0xff, 0xff, 0xff]);
    }

    #[test]
    fn the_palette_sums_to_the_whole_canvas() {
        let inks = palette(SVG, 64, 64).unwrap();
        assert_eq!(inks.len(), 2);
        let total: f64 = inks.iter().map(|i| i.share).sum();
        assert!((total - 1.0).abs() < 1e-6, "{total}");
        // Sorted largest first: the inner square is 48/64 squared, which is more than
        // half the canvas, so it leads and the white ground follows.
        assert!(inks[0].share > inks[1].share);
        assert_eq!(inks[0].traced, "#14453f");
        assert!(
            (inks[0].share - (48.0 * 48.0) / (64.0 * 64.0)).abs() < 0.02,
            "{:?}",
            inks[0]
        );
    }

    #[test]
    fn comparing_an_svg_with_itself_is_zero() {
        let rendered = render(SVG, 64, 64).unwrap();
        let source = inkvec_trace::rgba8_capped(&rendered, 64, 64, 0);
        let a = analyse(&source, SVG).unwrap();
        assert!(a.mean < 0.01, "{}", a.mean);
        assert!(a.median < 0.01, "{}", a.median);
        assert!(a.corner.is_none(), "a perfect trace has no worst corner");
    }

    #[test]
    fn comparing_against_a_different_drawing_is_not_zero() {
        let other = SVG.replace("#14453f", "#8a2f2f");
        let rendered = render(SVG, 64, 64).unwrap();
        let source = inkvec_trace::rgba8_capped(&rendered, 64, 64, 0);
        let a = analyse(&source, &other).unwrap();
        assert!(a.mean > 1.0, "{}", a.mean);
        assert!(a.corner.is_some());
    }

    #[test]
    fn an_unparseable_document_is_an_error_not_a_number() {
        assert!(render("not an svg at all", 8, 8).is_err());
    }
}
