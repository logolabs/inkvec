//! The SVG Minimizer tab.
//!
//! A different emotional register from tracing, and worth exploiting. A trace takes about
//! a second; this takes about thirty milliseconds. The tab is instant and should feel it,
//! so everything here is synchronous — there is no draft tier, no progress, and no stage
//! list, because there is nothing to wait for.
//!
//! The tolerance control explains itself in its own label: *nothing moves more than 0.1 px
//! when the drawing is 1024 px wide*. That sentence is the interface. What backs it up is
//! the guarantee the minifier itself makes — a run whose fit strayed past the tolerance is
//! replaced by its source rather than shipped — plus a measurement, taken the same way the
//! quality report takes its own: both drawings are rendered and compared in CIEDE2000, so
//! "nothing moved" is a number rather than a claim.

use serde::{Deserialize, Serialize};

use crate::quality;

/// The three controls the tab has.
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct MinifySettings {
    /// Nothing moves more than this many pixels...
    pub tolerance_px: f64,
    /// ...when the drawing is this many pixels wide.
    pub judge_px: f64,
    /// Turn between two source segments, in degrees, above which their join is a corner
    /// that must survive exactly.
    pub corner_degrees: f64,
    /// Also shorten everything that is not path geometry.
    pub document_cleanup: bool,
}

impl Default for MinifySettings {
    fn default() -> Self {
        let d = inkvec_svgmin::Options::default();
        Self {
            tolerance_px: d.tolerance_px,
            judge_px: d.judge,
            corner_degrees: d.corner_degrees,
            document_cleanup: d.document,
        }
    }
}

impl MinifySettings {
    fn to_options(self) -> inkvec_svgmin::Options {
        inkvec_svgmin::Options {
            tolerance_px: self.tolerance_px.clamp(0.0, 5.0),
            judge: self.judge_px.clamp(16.0, 16384.0),
            corner_degrees: self.corner_degrees.clamp(1.0, 179.0),
            decimals: None,
            document: self.document_cleanup,
        }
    }
}

/// One line of the "Removed" breakdown.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Removed {
    /// What was removed.
    pub what: &'static str,
    /// How much of it, already formatted with its unit.
    pub amount: String,
}

/// What a minify run produced.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MinifyResult {
    /// The rewritten document.
    pub svg: String,
    /// Bytes before.
    pub bytes_before: usize,
    /// Bytes after.
    pub bytes_after: usize,
    /// Numbers in the path data before.
    pub numbers_before: usize,
    /// Numbers in the path data after.
    pub numbers_after: usize,
    /// Drawn elements before.
    pub paths_before: usize,
    /// Drawn elements after.
    pub paths_after: usize,
    /// Segments before.
    pub segments_before: usize,
    /// Segments after.
    pub segments_after: usize,
    /// Paths written as a `<circle>`, `<ellipse>` or `<rect>` instead of a path.
    pub primitives: usize,
    /// Runs whose fit strayed past the tolerance and were replaced by their source. The
    /// guarantee is kept by falling back, and saying how often that happened is part of
    /// keeping it.
    pub guarded: usize,
    /// The tolerance actually applied, in the document's own units.
    pub tolerance_units: f64,
    /// Measured colour difference between a render of the two documents. The number
    /// behind "nothing moved"; `None` if either document would not render.
    pub difference_de00: Option<f64>,
    /// Milliseconds the rewrite took.
    pub ms: f64,
    /// The breakdown.
    pub removed: Vec<Removed>,
}

/// Rewrite one SVG.
pub fn run(svg: &str, settings: MinifySettings) -> Result<MinifyResult, String> {
    let started = std::time::Instant::now();
    let opts = settings.to_options();
    let (out, report) = inkvec_svgmin::minify(svg, &opts)?;
    let ms = started.elapsed().as_secs_f64() * 1e3;

    let (numbers_before, _, paths_before, _) = quality::count(svg);
    let (numbers_after, _, paths_after, _) = quality::count(&out);

    // Rendered at the size the tolerance is stated at, which is the size the promise is
    // about. Both documents are put through the same renderer, so any difference is the
    // rewrite's and not the renderer's.
    let judge = opts.judge.clamp(16.0, 2048.0) as u32;
    let difference_de00 = match (
        quality::render(svg, judge, judge),
        quality::render(&out, judge, judge),
    ) {
        (Ok(a), Ok(b)) if a.len() == b.len() => {
            let mut sum = 0.0f64;
            let n = a.len() / 4;
            for i in 0..n {
                let pa = to_lab(&a[i * 4..i * 4 + 4]);
                let pb = to_lab(&b[i * 4..i * 4 + 4]);
                sum += quality::ciede2000(pa, pb);
            }
            Some(sum / n.max(1) as f64)
        }
        _ => None,
    };

    Ok(MinifyResult {
        bytes_before: svg.len(),
        bytes_after: out.len(),
        numbers_before,
        numbers_after,
        paths_before,
        paths_after,
        segments_before: report.segments_before,
        segments_after: report.segments_after,
        primitives: report.primitives,
        guarded: report.guarded,
        tolerance_units: report.tolerance_units,
        difference_de00,
        ms,
        removed: breakdown(svg, &out, &report),
        svg: out,
    })
}

/// Composite onto white and convert, so a transparent pixel in one document and a white
/// one in the other are not called identical.
fn to_lab(px: &[u8]) -> [f64; 3] {
    let a = px[3] as f32 / 255.0;
    quality::lab([
        (px[0] as f32 / 255.0) * a + (1.0 - a),
        (px[1] as f32 / 255.0) * a + (1.0 - a),
        (px[2] as f32 / 255.0) * a + (1.0 - a),
    ])
}

/// What came out, counted by comparing the two documents.
///
/// Every line is a difference between the before and the after, not an estimate: if a row
/// would be zero it is left out rather than shown as "0", because a list of noughts reads
/// as a tool that did nothing.
fn breakdown(before: &str, after: &str, report: &inkvec_svgmin::Report) -> Vec<Removed> {
    let mut out = Vec::new();

    let metadata_bytes =
        tagged_bytes(before, "metadata") + tagged_bytes(before, "desc") + comment_bytes(before)
            - tagged_bytes(after, "metadata")
            - tagged_bytes(after, "desc")
            - comment_bytes(after);
    if metadata_bytes > 0 {
        out.push(Removed {
            what: "editor metadata",
            amount: format_bytes(metadata_bytes),
        });
    }

    let ids = count_minus(before, after, " id=\"");
    let groups = count_minus(before, after, "<g");
    if ids + groups > 0 {
        out.push(Removed {
            what: "ids and groups",
            amount: (ids + groups).to_string(),
        });
    }

    let zeros = trailing_zeros(before).saturating_sub(trailing_zeros(after));
    if zeros > 0 {
        out.push(Removed {
            what: "trailing zeros",
            amount: zeros.to_string(),
        });
    }

    let transforms = count_minus(before, after, " transform=\"");
    if transforms > 0 {
        out.push(Removed {
            what: "redundant transforms",
            amount: transforms.to_string(),
        });
    }

    let segments = report.segments_before.saturating_sub(report.segments_after);
    if segments > 0 {
        out.push(Removed {
            what: "segments",
            amount: segments.to_string(),
        });
    }
    if report.primitives > 0 {
        out.push(Removed {
            what: "paths written as shapes",
            amount: report.primitives.to_string(),
        });
    }
    out
}

fn count_minus(before: &str, after: &str, needle: &str) -> usize {
    before
        .matches(needle)
        .count()
        .saturating_sub(after.matches(needle).count())
}

/// Bytes inside `<tag ...>...</tag>` pairs, including the tags.
fn tagged_bytes(svg: &str, tag: &str) -> isize {
    let open = format!("<{tag}");
    let close = format!("</{tag}>");
    let mut total = 0isize;
    let mut rest = svg;
    while let Some(i) = rest.find(&open) {
        let after = &rest[i..];
        match after.find(&close) {
            Some(j) => {
                total += (j + close.len()) as isize;
                rest = &after[j + close.len()..];
            }
            None => break,
        }
    }
    total
}

fn comment_bytes(svg: &str) -> isize {
    let mut total = 0isize;
    let mut rest = svg;
    while let Some(i) = rest.find("<!--") {
        let after = &rest[i..];
        match after.find("-->") {
            Some(j) => {
                total += (j + 3) as isize;
                rest = &after[j + 3..];
            }
            None => break,
        }
    }
    total
}

/// Digits that sit after a decimal point and carry no information: `1.50`, `3.000`.
fn trailing_zeros(svg: &str) -> usize {
    let bytes = svg.as_bytes();
    let mut total = 0usize;
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'.' {
            let start = i + 1;
            let mut j = start;
            while j < bytes.len() && bytes[j].is_ascii_digit() {
                j += 1;
            }
            let mut k = j;
            while k > start && bytes[k - 1] == b'0' {
                k -= 1;
            }
            total += j - k;
            i = j;
        } else {
            i += 1;
        }
    }
    total
}

fn format_bytes(n: isize) -> String {
    let n = n.max(0) as f64;
    if n >= 1024.0 {
        format!("{:.1} KB", n / 1024.0)
    } else {
        format!("{n:.0} B")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A circle written as sixteen exact cubics plus editor cruft: what an export from a
    /// drawing program actually looks like.
    fn wordy_svg() -> String {
        let mut d = String::new();
        let (cx, cy, r, n) = (100.0f64, 100.0f64, 70.0f64, 16usize);
        let step = std::f64::consts::TAU / n as f64;
        let k = 4.0 / 3.0 * (step / 4.0).tan() * r;
        for i in 0..n {
            let (a0, a1) = (i as f64 * step, (i + 1) as f64 * step);
            let p0 = (cx + r * a0.cos(), cy + r * a0.sin());
            let p3 = (cx + r * a1.cos(), cy + r * a1.sin());
            let c1 = (p0.0 - k * a0.sin(), p0.1 + k * a0.cos());
            let c2 = (p3.0 + k * a1.sin(), p3.1 - k * a1.cos());
            if i == 0 {
                d.push_str(&format!("M{:.4},{:.4}", p0.0, p0.1));
            }
            d.push_str(&format!(
                "C{:.4},{:.4} {:.4},{:.4} {:.4},{:.4}",
                c1.0, c1.1, c2.0, c2.1, p3.0, p3.1
            ));
        }
        d.push('Z');
        format!(
            "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"200\" height=\"200\" \
             viewBox=\"0 0 200 200\"><!-- Generator: a drawing program --><metadata>\
             <rdf>lots and lots and lots of editor metadata goes here</rdf></metadata>\
             <desc>Created with a drawing program</desc><g id=\"Layer_1\">\
             <path id=\"circle-1\" d=\"{d}\" fill=\"#14453f\"/></g></svg>"
        )
    }

    #[test]
    fn a_wordy_export_gets_smaller_without_moving() {
        let before = wordy_svg();
        let r = run(&before, MinifySettings::default()).unwrap();
        assert!(
            r.bytes_after < r.bytes_before,
            "{} -> {}",
            r.bytes_before,
            r.bytes_after
        );
        assert!(r.numbers_after < r.numbers_before);
        // The promise the tolerance makes, measured rather than asserted.
        let d = r.difference_de00.expect("both documents rendered");
        assert!(d < 0.5, "the drawing moved: {d} dE00");
    }

    #[test]
    fn the_breakdown_only_lists_what_was_actually_removed() {
        let r = run(&wordy_svg(), MinifySettings::default()).unwrap();
        let named: Vec<&str> = r.removed.iter().map(|x| x.what).collect();
        assert!(named.contains(&"editor metadata"), "{named:?}");
        assert!(named.contains(&"ids and groups"), "{named:?}");
        for row in &r.removed {
            assert_ne!(row.amount, "0", "{} was listed as nothing", row.what);
        }
    }

    #[test]
    fn a_document_that_is_already_tight_is_left_alone() {
        let tight = "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"10\" height=\"10\" \
                     viewBox=\"0 0 10 10\"><path d=\"M0 0L10 0L10 10L0 10Z\" fill=\"#000\"/></svg>";
        let r = run(tight, MinifySettings::default()).unwrap();
        // Nothing to win, so nothing is claimed: the drawing is unchanged and the
        // breakdown does not pad itself with noughts.
        assert!(r.difference_de00.unwrap() < 0.5, "{:?}", r.difference_de00);
        assert!(r.removed.iter().all(|x| x.amount != "0"), "{:?}", r.removed);
        assert!(
            r.bytes_after <= r.bytes_before + 64,
            "a tight document grew by {} bytes",
            r.bytes_after as i64 - r.bytes_before as i64
        );
    }

    #[test]
    fn trailing_zeros_are_counted_not_guessed() {
        assert_eq!(
            trailing_zeros("M1.50 3.000L2 4"),
            4,
            "one in 1.50, three in 3.000"
        );
        assert_eq!(trailing_zeros("M1.5 3.1"), 0);
        assert_eq!(trailing_zeros("M0.0"), 1);
    }

    #[test]
    fn a_broken_document_is_an_error_with_words() {
        assert!(run("<svg><path d=", MinifySettings::default()).is_err());
    }

    #[test]
    fn the_defaults_are_the_minifiers_own() {
        let m = MinifySettings::default();
        let d = inkvec_svgmin::Options::default();
        assert_eq!(m.tolerance_px, d.tolerance_px);
        assert_eq!(m.judge_px, d.judge);
        assert_eq!(m.corner_degrees, d.corner_degrees);
    }
}
