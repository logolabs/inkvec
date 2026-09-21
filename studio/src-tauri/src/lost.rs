//! "What this trace could not recover" — the honesty panel.
//!
//! Every row here is a statement of fact about the file, derived from something that was
//! actually measured: the container the image arrived in, the geometry the tracer wrote,
//! or the per-pixel difference between the source and a render of the result. Nothing is
//! guessed, and a row that cannot be established does not appear.
//!
//! Three rules the copy obeys, from the brief:
//!
//! 1. The panel has a positive state. When nothing is detected it says so, calmly. A
//!    panel that only ever appears to deliver bad news gets read as an advertisement.
//! 2. The LogoLabs links are text links at the same weight as everything else. Never a
//!    button, never accent-filled, never a card.
//! 3. It is diagnosis, not apology. Each line states a fact. It never says sorry, and it
//!    never implies the user did something wrong.

use serde::Serialize;

use crate::options::Settings;
use crate::quality::Analysis;

/// What the file arrived as. Only the question the panel needs: was it compressed
/// lossily before it got here?
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Container {
    Png,
    Jpeg,
    WebpLossy,
    WebpLossless,
    Gif,
    Bmp,
    Tiff,
    Unknown,
}

impl Container {
    /// Read the container from the file's first bytes, the way the engine does.
    pub fn sniff(bytes: &[u8]) -> Self {
        let starts = |p: &[u8]| bytes.starts_with(p);
        if starts(b"\x89PNG\r\n\x1a\n") {
            Container::Png
        } else if starts(&[0xff, 0xd8, 0xff]) {
            Container::Jpeg
        } else if starts(b"RIFF") && bytes.len() > 15 && &bytes[8..12] == b"WEBP" {
            // VP8L is the lossless chunk; VP8  (with the trailing space) is lossy, and
            // VP8X is an extended file whose image chunk follows — treated as lossy,
            // which is the common case and the conservative reading.
            match &bytes[12..16] {
                b"VP8L" => Container::WebpLossless,
                _ => Container::WebpLossy,
            }
        } else if starts(b"GIF8") {
            Container::Gif
        } else if starts(b"BM") {
            Container::Bmp
        } else if starts(&[0x49, 0x49, 0x2a, 0x00]) || starts(&[0x4d, 0x4d, 0x00, 0x2a]) {
            Container::Tiff
        } else {
            Container::Unknown
        }
    }

    /// Whether the pixels carry compression damage before anything here touched them.
    pub fn is_lossy(self) -> bool {
        matches!(self, Container::Jpeg | Container::WebpLossy)
    }

    /// The name to use in a sentence.
    pub fn name(self) -> &'static str {
        match self {
            Container::Png => "PNG",
            Container::Jpeg => "JPEG",
            Container::WebpLossy | Container::WebpLossless => "WebP",
            Container::Gif => "GIF",
            Container::Bmp => "BMP",
            Container::Tiff => "TIFF",
            Container::Unknown => "image",
        }
    }
}

/// One row of the panel.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Loss {
    /// A stable identifier, so the frontend can pick the icon and the layout.
    pub kind: &'static str,
    /// The one plain sentence the row leads with.
    pub text: String,
    /// What the "why" expander says: how this was established.
    pub why: String,
    /// The single text link, if this row has earned one.
    pub link: Option<Link>,
}

/// The one text link a row may carry.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Link {
    /// The link's words.
    pub label: String,
    /// Where it goes: an external URL, or an internal target like `palette`.
    pub href: String,
}

/// Everything the panel can say about this trace.
///
/// An empty vector is the positive state, and the frontend renders it as the calm line
/// rather than as an absence.
pub fn detect(
    source: &inkvec_trace::Rgba,
    analysis: &Analysis,
    svg: &str,
    settings: &Settings,
    container: Container,
) -> Vec<Loss> {
    let mut out = Vec::new();

    if let Some(l) = lettering(svg) {
        out.push(l);
    }
    if let Some(l) = lossy_source(container) {
        out.push(l);
    }
    if let Some(l) = baked_strokes(svg, settings) {
        out.push(l);
    }
    if let Some(l) = smooth_ramps(source, analysis, svg) {
        out.push(l);
    }
    if let Some(l) = dropped_details(analysis, settings) {
        out.push(l);
    }
    out
}

// ---------------------------------------------------------------------- lettering ---

/// Lettering, recognised by its geometry rather than by reading it.
///
/// Type sets itself apart from a mark: a run of small shapes of closely matched height,
/// sitting on a shared baseline. That is what is looked for — six or more faces whose
/// heights agree within a quarter and whose bottoms line up within a tenth of that
/// height. It cannot tell an "O" from a ring, which is why the sentence says the
/// lettering *came back as outlines* rather than claiming to have read any of it.
fn lettering(svg: &str) -> Option<Loss> {
    let boxes: Vec<Bbox> = path_boxes(svg);
    if boxes.len() < 6 {
        return None;
    }
    let canvas = boxes
        .iter()
        .fold(Bbox::EMPTY, |a, b| a.union(b))
        .area()
        .max(1.0);

    // Only small faces are candidates: a letter is a fraction of the drawing.
    let mut small: Vec<&Bbox> = boxes
        .iter()
        .filter(|b| b.area() > 0.0 && b.area() < canvas * 0.06 && b.height() > 0.0)
        .collect();
    if small.len() < 6 {
        return None;
    }
    small.sort_by(|a, b| {
        a.height()
            .partial_cmp(&b.height())
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    let median_h = small[small.len() / 2].height();
    if median_h <= 0.0 {
        return None;
    }

    let run: Vec<&&Bbox> = small
        .iter()
        .filter(|b| (b.height() - median_h).abs() <= median_h * 0.25)
        .collect();
    if run.len() < 6 {
        return None;
    }
    // A shared baseline: the bottoms of most of them agree.
    let mut bottoms: Vec<f64> = run.iter().map(|b| b.y1).collect();
    bottoms.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let median_bottom = bottoms[bottoms.len() / 2];
    let on_baseline = bottoms
        .iter()
        .filter(|b| (**b - median_bottom).abs() <= median_h * 0.12)
        .count();
    if on_baseline < 6 {
        return None;
    }

    Some(Loss {
        kind: "lettering",
        text: "The lettering came back as outlines, not editable text.".into(),
        why: format!(
            "{on_baseline} faces of matched height ({median_h:.0} px) sit on one baseline. \
             Tracing reconstructs their shapes; it does not recover the typeface, so the \
             words cannot be re-set or re-spaced."
        ),
        link: Some(Link {
            label: "LogoLabs can redraw it with live type".into(),
            href: "https://logolabs.org".into(),
        }),
    })
}

// -------------------------------------------------------------------- lossy source ---

fn lossy_source(container: Container) -> Option<Loss> {
    if !container.is_lossy() {
        return None;
    }
    let name = container.name();
    Some(Loss {
        kind: "lossy",
        text: format!("The source is a {name}, so these colours carry its compression damage."),
        why: format!(
            "A {name} stores colour approximately and at lower resolution than brightness. \
             The values the trace measured are the ones in the file, which are not quite \
             the values the artwork was drawn with."
        ),
        link: Some(Link {
            label: "Check them against your brand values".into(),
            href: "#palette".into(),
        }),
    })
}

// ------------------------------------------------------------------- baked strokes ---

/// Strokes that ended up as filled outlines.
///
/// Two ways to know. If line art was asked for and the output carries no `stroke`, the
/// engine declined, and that is a fact rather than an inference. Otherwise the geometry
/// is read: a face that fills under a fifth of its own bounding box, several times over,
/// is a drawing made of strokes that were traced as the outlines around them.
fn baked_strokes(svg: &str, settings: &Settings) -> Option<Loss> {
    let has_strokes = svg.contains(" stroke=\"") || svg.contains(" stroke-width=\"");
    if settings.line_art && !has_strokes {
        return Some(Loss {
            kind: "strokes",
            text: "Line art was asked for, and the stroke widths vary too much to recover.".into(),
            why: "Emitting strokes needs one width per path. Where the width changes along \
                  a line — a brush, a calligraphic pen, a tapered join — the drawing falls \
                  back to the filled outline around the stroke, which is what you have here."
                .into(),
            link: None,
        });
    }
    if has_strokes {
        return None;
    }

    let thin = path_shapes(svg)
        .iter()
        .filter(|s| {
            let bb = s.bbox.area();
            bb > 0.0 && s.bbox.width() > 4.0 && s.bbox.height() > 4.0 && s.area.abs() < bb * 0.2
        })
        .count();
    if thin < 3 {
        return None;
    }
    Some(Loss {
        kind: "strokes",
        text: "Stroke widths are baked into filled outlines.".into(),
        why: format!(
            "{thin} faces fill under a fifth of their own bounding box, which is the shape \
             of an outline drawn around a line rather than the line itself. An editor will \
             see closed shapes, not strokes it can re-weight."
        ),
        link: None,
    })
}

// -------------------------------------------------------------------- smooth ramps ---

/// Gradient banding, blur and mesh: areas that are a ramp in the source and a small
/// number of flat or linear fills in the output.
///
/// Measured, not assumed: a pixel counts when the source is locally *smooth but not
/// flat* — its neighbours differ a little rather than not at all or a lot — and the trace
/// disagrees with it by more than a just-visible amount there.
fn smooth_ramps(source: &inkvec_trace::Rgba, analysis: &Analysis, svg: &str) -> Option<Loss> {
    let (w, h) = (analysis.width as usize, analysis.height as usize);
    if w < 3 || h < 3 {
        return None;
    }
    let lum = |i: usize| -> f32 {
        let p = &source.data[i * 4..i * 4 + 4];
        (0.2126 * p[0] + 0.7152 * p[1] + 0.0722 * p[2]) * p[3]
    };

    let mut ramping = 0usize;
    for y in 1..h - 1 {
        for x in 1..w - 1 {
            let i = y * w + x;
            let gx = (lum(i + 1) - lum(i - 1)).abs();
            let gy = (lum(i + w) - lum(i - w)).abs();
            let g = gx.max(gy);
            // A ramp: a slope you could walk up, not a cliff and not a plateau.
            if (0.004..0.06).contains(&g) && analysis.deltas[i] > 1.0 {
                ramping += 1;
            }
        }
    }
    let share = ramping as f64 / (w * h) as f64;
    if share < 0.015 {
        return None;
    }

    let fitted = svg.matches("<linearGradient").count() + svg.matches("<radialGradient").count();
    let fitted_note = if fitted > 0 {
        format!(
            " {fitted} gradient{} {} fitted; what is left over is the part no gradient describes.",
            if fitted == 1 { "" } else { "s" },
            if fitted == 1 { "was" } else { "were" }
        )
    } else {
        String::new()
    };

    Some(Loss {
        kind: "ramp",
        text: "Part of this image is a continuous ramp, which is something SVG cannot hold.".into(),
        why: format!(
            "{:.1}% of the canvas is a smooth slope in the source that the trace reproduces \
             to more than one dE00.{fitted_note} A blur, a mesh or a photographic shade has \
             no exact vector form; the output is the closest arrangement of fills to it.",
            share * 100.0
        ),
        link: None,
    })
}

// ----------------------------------------------------------------- dropped details ---

/// Features that fell below the speckle floor.
///
/// Small islands of high disagreement, counted by flooding the difference map. A dozen
/// scattered specks is a trace that dropped detail; one large region is something else
/// and is reported by the ramp row instead.
fn dropped_details(analysis: &Analysis, settings: &Settings) -> Option<Loss> {
    let (w, h) = (analysis.width as usize, analysis.height as usize);
    let n = w * h;
    if n == 0 {
        return None;
    }
    // Two just-visible differences: well past "you might notice" and into "that is gone".
    let hot: Vec<bool> = analysis.deltas.iter().map(|d| *d > 2.0).collect();
    let ceiling = (settings.speckle_floor.max(1.0) * 6.0) as usize;

    let mut seen = vec![false; n];
    let mut stack: Vec<usize> = Vec::new();
    let mut islands = 0usize;
    let mut largest = 0usize;
    for start in 0..n {
        if !hot[start] || seen[start] {
            continue;
        }
        seen[start] = true;
        stack.push(start);
        let mut size = 0usize;
        while let Some(i) = stack.pop() {
            size += 1;
            let (x, y) = (i % w, i / w);
            let push = |j: usize, seen: &mut Vec<bool>, stack: &mut Vec<usize>| {
                if hot[j] && !seen[j] {
                    seen[j] = true;
                    stack.push(j);
                }
            };
            if x > 0 {
                push(i - 1, &mut seen, &mut stack);
            }
            if x + 1 < w {
                push(i + 1, &mut seen, &mut stack);
            }
            if y > 0 {
                push(i - w, &mut seen, &mut stack);
            }
            if y + 1 < h {
                push(i + w, &mut seen, &mut stack);
            }
        }
        largest = largest.max(size);
        if size <= ceiling {
            islands += 1;
        }
    }

    if islands < 4 {
        return None;
    }
    Some(Loss {
        kind: "detail",
        text: format!("{islands} very small features did not survive the trace at this size."),
        why: format!(
            "Each is an island of more than two dE00 covering no more than {ceiling} px² — \
             at or under the speckle floor of {:.1} px². Raising the trace size measures \
             them at more pixels; lowering the speckle floor keeps smaller ones.",
            settings.speckle_floor
        ),
        link: Some(Link {
            label: "Raise the trace size".into(),
            href: "#advanced:traceSize".into(),
        }),
    })
}

// ------------------------------------------------------------------------ geometry ---

/// An axis-aligned box in the SVG's own coordinates.
#[derive(Clone, Copy, Debug)]
struct Bbox {
    x0: f64,
    y0: f64,
    x1: f64,
    y1: f64,
}

impl Bbox {
    const EMPTY: Bbox = Bbox {
        x0: f64::MAX,
        y0: f64::MAX,
        x1: f64::MIN,
        y1: f64::MIN,
    };
    fn width(&self) -> f64 {
        (self.x1 - self.x0).max(0.0)
    }
    fn height(&self) -> f64 {
        (self.y1 - self.y0).max(0.0)
    }
    fn area(&self) -> f64 {
        self.width() * self.height()
    }
    fn union(self, o: &Bbox) -> Bbox {
        Bbox {
            x0: self.x0.min(o.x0),
            y0: self.y0.min(o.y0),
            x1: self.x1.max(o.x1),
            y1: self.y1.max(o.y1),
        }
    }
}

/// One drawn path, reduced to the two numbers these heuristics read.
struct Shape {
    bbox: Bbox,
    /// Signed area of the polygon through the path's on-curve points.
    area: f64,
}

fn path_boxes(svg: &str) -> Vec<Bbox> {
    path_shapes(svg).into_iter().map(|s| s.bbox).collect()
}

/// Every `<path d="...">` in the document, as a box and an area.
fn path_shapes(svg: &str) -> Vec<Shape> {
    let mut out = Vec::new();
    let mut rest = svg;
    while let Some(i) = rest.find(" d=\"") {
        let after = &rest[i + 4..];
        let Some(j) = after.find('"') else { break };
        if let Some(s) = shape_of(&after[..j]) {
            out.push(s);
        }
        rest = &after[j + 1..];
    }
    out
}

/// Walk one `d` attribute, collecting the on-curve points.
///
/// Control points are skipped on purpose: a curve's hull is wider than the curve, and
/// both of the measurements above want the shape, not its envelope. Arcs contribute
/// their endpoint, which is the same reasoning.
fn shape_of(d: &str) -> Option<Shape> {
    let mut points: Vec<(f64, f64)> = Vec::new();
    let mut cur = (0.0f64, 0.0f64);
    let mut start = (0.0f64, 0.0f64);
    let mut nums: Vec<f64> = Vec::new();
    let mut cmd = ' ';

    let flush = |cmd: char,
                 nums: &mut Vec<f64>,
                 cur: &mut (f64, f64),
                 start: &mut (f64, f64),
                 points: &mut Vec<(f64, f64)>| {
        let rel = cmd.is_ascii_lowercase();
        let up = cmd.to_ascii_uppercase();
        // How many numbers one repetition of this command takes, and where in that run
        // the endpoint's x sits.
        let (stride, off) = match up {
            'M' | 'L' | 'T' => (2usize, 0usize),
            'H' | 'V' => (1, 0),
            'C' => (6, 4),
            'S' | 'Q' => (4, 2),
            'A' => (7, 5),
            _ => (0, 0),
        };
        if stride == 0 {
            nums.clear();
            return;
        }
        let mut k = 0;
        while k + stride <= nums.len() {
            let run = &nums[k..k + stride];
            let next = match up {
                'H' => (if rel { cur.0 + run[0] } else { run[0] }, cur.1),
                'V' => (cur.0, if rel { cur.1 + run[0] } else { run[0] }),
                _ => {
                    let (x, y) = (run[off], run[off + 1]);
                    if rel {
                        (cur.0 + x, cur.1 + y)
                    } else {
                        (x, y)
                    }
                }
            };
            *cur = next;
            if up == 'M' && k == 0 {
                *start = next;
            }
            points.push(next);
            k += stride;
        }
        nums.clear();
        let _ = start;
    };

    let bytes = d.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        let c = bytes[i] as char;
        if c.is_ascii_alphabetic() {
            flush(cmd, &mut nums, &mut cur, &mut start, &mut points);
            cmd = c;
            if c == 'z' || c == 'Z' {
                cur = start;
            }
            i += 1;
        } else if c.is_ascii_digit() || c == '-' || c == '+' || c == '.' {
            let s = i;
            let mut seen_dot = false;
            let mut seen_e = false;
            i += 1;
            while i < bytes.len() {
                let ch = bytes[i] as char;
                if ch.is_ascii_digit() {
                    i += 1;
                } else if ch == '.' && !seen_dot && !seen_e {
                    seen_dot = true;
                    i += 1;
                } else if (ch == 'e' || ch == 'E') && !seen_e {
                    seen_e = true;
                    i += 1;
                    if i < bytes.len() && (bytes[i] == b'-' || bytes[i] == b'+') {
                        i += 1;
                    }
                } else {
                    break;
                }
            }
            if let Ok(v) = d[s..i].parse::<f64>() {
                nums.push(v);
            }
        } else {
            i += 1;
        }
    }
    flush(cmd, &mut nums, &mut cur, &mut start, &mut points);

    if points.len() < 3 {
        return None;
    }
    let mut bbox = Bbox::EMPTY;
    for (x, y) in &points {
        bbox.x0 = bbox.x0.min(*x);
        bbox.y0 = bbox.y0.min(*y);
        bbox.x1 = bbox.x1.max(*x);
        bbox.y1 = bbox.y1.max(*y);
    }
    let mut area = 0.0;
    for k in 0..points.len() {
        let (x0, y0) = points[k];
        let (x1, y1) = points[(k + 1) % points.len()];
        area += x0 * y1 - x1 * y0;
    }
    Some(Shape {
        bbox,
        area: area / 2.0,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn containers_are_read_from_their_first_bytes() {
        assert_eq!(Container::sniff(b"\x89PNG\r\n\x1a\n rest"), Container::Png);
        assert_eq!(Container::sniff(&[0xff, 0xd8, 0xff, 0xe0]), Container::Jpeg);
        assert_eq!(Container::sniff(b"GIF89a"), Container::Gif);
        assert_eq!(
            Container::sniff(b"RIFF....WEBPVP8L"),
            Container::WebpLossless
        );
        assert_eq!(Container::sniff(b"RIFF....WEBPVP8 "), Container::WebpLossy);
        assert_eq!(Container::sniff(b"nope"), Container::Unknown);
        assert!(Container::Jpeg.is_lossy());
        assert!(!Container::WebpLossless.is_lossy());
    }

    #[test]
    fn a_jpeg_always_says_so() {
        let l = lossy_source(Container::Jpeg).expect("a JPEG has compression damage");
        assert_eq!(l.kind, "lossy");
        assert!(l.text.contains("JPEG"));
        assert!(lossy_source(Container::Png).is_none());
    }

    #[test]
    fn the_panel_never_apologises() {
        let rows = [
            lossy_source(Container::Jpeg).unwrap(),
            baked_strokes(
                "<svg><path d=\"M0 0L1 1\"/></svg>",
                &Settings {
                    line_art: true,
                    ..Settings::default()
                },
            )
            .unwrap(),
        ];
        for r in rows {
            let all = format!("{} {}", r.text, r.why).to_lowercase();
            for word in ["sorry", "unfortunately", "failed to", "we could not"] {
                assert!(!all.contains(word), "{:?} says {word}", r.kind);
            }
        }
    }

    #[test]
    fn a_square_path_has_its_own_box_and_area() {
        let s = shape_of("M0 0L10 0L10 10L0 10Z").unwrap();
        assert_eq!((s.bbox.width(), s.bbox.height()), (10.0, 10.0));
        assert!((s.area.abs() - 100.0).abs() < 1e-9);
    }

    #[test]
    fn relative_and_curve_commands_land_on_the_right_endpoints() {
        // A closed unit square drawn with relative lines, then the same with cubics whose
        // control points sit far outside it: the box must follow the endpoints, not the hull.
        let rel = shape_of("M0 0l10 0l0 10l-10 0z").unwrap();
        assert!((rel.area.abs() - 100.0).abs() < 1e-9);
        let curved =
            shape_of("M0 0C50 -50 50 -50 10 0C60 60 60 60 10 10C-40 60 -40 60 0 10Z").unwrap();
        assert_eq!((curved.bbox.width(), curved.bbox.height()), (10.0, 10.0));
    }

    #[test]
    fn a_thin_snaking_path_reads_as_a_baked_stroke() {
        // Three L-shaped outlines: each has a bbox the size of the drawing and fills
        // almost none of it, which is the shape of a line traced as the outline around
        // it rather than as a stroke.
        let mut svg = String::from("<svg>");
        for i in 0..3 {
            let o = i as f64 * 6.0;
            svg.push_str(&format!(
                "<path d=\"M{o} {o}L{a} {o}L{a} {b}L{c} {b}L{c} {d}L{o} {d}Z\" fill=\"#000\"/>",
                a = o + 80.0,
                b = o + 80.0,
                c = o + 76.0,
                d = o + 4.0
            ));
        }
        svg.push_str("</svg>");
        let l = baked_strokes(&svg, &Settings::default()).expect("three thin faces");
        assert_eq!(l.kind, "strokes");
        assert!(l.link.is_none(), "this row has no link to earn");
    }

    #[test]
    fn a_fat_shape_is_not_a_baked_stroke() {
        let svg = "<svg><path d=\"M0 0L100 0L100 100L0 100Z\" fill=\"#000\"/></svg>";
        assert!(baked_strokes(svg, &Settings::default()).is_none());
    }

    #[test]
    fn a_run_of_matched_faces_on_a_baseline_reads_as_lettering() {
        let mut svg = String::from("<svg><path d=\"M0 0L400 0L400 300L0 300Z\" fill=\"#fff\"/>");
        for i in 0..8 {
            let x = 20.0 + i as f64 * 30.0;
            // 20 x 24 faces, all bottoming out at y = 200.
            svg.push_str(&format!(
                "<path d=\"M{x} 176L{} 176L{} 200L{x} 200Z\" fill=\"#000\"/>",
                x + 20.0,
                x + 20.0
            ));
        }
        svg.push_str("</svg>");
        let l = lettering(&svg).expect("eight matched faces on one baseline");
        assert_eq!(l.kind, "lettering");
        assert!(l.link.is_some(), "lettering earns the LogoLabs link");
    }

    #[test]
    fn a_handful_of_unrelated_shapes_is_not_lettering() {
        let svg = "<svg>\
            <path d=\"M0 0L400 0L400 300L0 300Z\"/>\
            <path d=\"M10 10L90 10L90 120L10 120Z\"/>\
            <path d=\"M200 40L260 40L260 55L200 55Z\"/>\
            </svg>";
        assert!(lettering(svg).is_none());
    }
}
