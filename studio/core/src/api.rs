//! The commands' platform-independent bodies.
//!
//! Every Studio command that is not about a window, a file on disk or a thread lives here,
//! once, and the two shells call it: the desktop app from a Tauri command, the browser build
//! from its worker. Their names and shapes are the contract `studio/src/lib/ipc.ts` types,
//! so a change here is a change to both apps at once.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::{export, lost, options, quality, trace};

/// What this build can do, sent once at startup so the interface never has to guess.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Capabilities {
    /// The app's version.
    pub version: &'static str,
    /// The engine's version: the same tree, but worth naming separately on About.
    pub engine_version: &'static str,
    /// The target the engine was compiled for. Two builds write byte-identical SVG only
    /// when this string matches, which is worth showing where anyone compares outputs.
    pub build_target: String,
    /// The operating system, for the few places the interface says ⌘ rather than Ctrl;
    /// `web` for the browser build, which the interface reads to hide what a tab cannot do.
    pub platform: &'static str,
    /// The advanced drawer's rows.
    /// A list rather than the static table itself, so a shell can narrow a range to what it
    /// can run (the browser build offers trace sizes up to 2048 px).
    pub controls: Vec<options::Control>,
    /// The presets, with their names and subtitles.
    pub presets: Vec<PresetInfo>,
    /// The stage names the progress list shows, in order.
    pub stages: [&'static str; 9],
    /// Where the denoiser stands.
    pub denoiser: DenoiserStatus,
}

/// One preset, as the tray shows it.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PresetInfo {
    /// Its identifier, as the frontend sends it back.
    pub id: options::Preset,
    /// Its plain-language name.
    pub name: &'static str,
    /// The one line under the name.
    pub subtitle: &'static str,
    /// The settings it means, so the drawer can show them without a round trip.
    pub settings: options::Settings,
    /// Whether it wants the denoiser to do what it says.
    pub wants_denoiser: bool,
}

/// An image that has been opened.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceInfo {
    /// The file's name.
    pub name: String,
    /// Where it came from, if it came from disk.
    pub path: Option<PathBuf>,
    /// Its width as it arrived.
    pub width: u32,
    /// Its height as it arrived.
    pub height: u32,
    /// What the file is: "PNG", "JPEG", ...
    pub container: &'static str,
    /// Whether the container compressed it lossily.
    pub lossy: bool,
    /// The pixels, as a `data:` URL, for the viewer's left pane. Capped on the longer
    /// side: the pane is a few hundred pixels and an 8000 px source would cost tens of
    /// megabytes to hand across for no visible gain.
    pub preview: String,
}

/// One bundled sample.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SampleInfo {
    /// Its file name, which is what `open_sample` takes.
    pub file: String,
    /// What the first-run screen calls it.
    pub label: String,
    /// The image itself, for the thumbnail.
    pub preview: String,
}

/// Where the denoiser stands right now.
///
/// The desktop app answers from the weights file on disk; the browser build from its cache
/// of the same file. Both check the same SHA-256, which is `inkvec_restore`'s.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DenoiserStatus {
    /// Whether this build can run the model at all. A desktop build without the `denoiser`
    /// feature has no ONNX Runtime linked in, and a browser that cannot block a worker on a
    /// shared buffer cannot hand the network's answer back to the trace; either says so
    /// rather than offering a download that would achieve nothing.
    pub supported: bool,
    /// Whether the weights are present and hash to the published value.
    pub installed: bool,
    /// Where they are, or where they would go. None in a browser.
    pub path: Option<PathBuf>,
    /// Their size, in bytes, if present.
    pub bytes: Option<u64>,
    /// The Hugging Face repository they come from.
    pub repo: &'static str,
    /// The SHA-256 the download is checked against.
    pub sha256: &'static str,
}

/// What this build can do, with the denoiser's status and the platform as given.
pub fn describe(denoiser: DenoiserStatus, platform: &'static str) -> Capabilities {
    Capabilities {
        version: env!("CARGO_PKG_VERSION"),
        engine_version: env!("CARGO_PKG_VERSION"),
        build_target: build_target(),
        platform,
        controls: options::CONTROLS.to_vec(),
        presets: options::Preset::ALL
            .iter()
            .map(|p| {
                let (name, subtitle) = p.labels();
                PresetInfo {
                    id: *p,
                    name,
                    subtitle,
                    settings: p.settings(),
                    wants_denoiser: p.wants_denoiser(),
                }
            })
            .collect(),
        stages: trace::STAGES,
        denoiser,
    }
}

/// `arch-os-env`, the string two builds must share to write byte-identical SVG.
pub fn build_target() -> String {
    if cfg!(target_arch = "wasm32") {
        // The engine's own spelling for both browser builds (`inkvec::build_target`).
        return "wasm32-unknown".to_string();
    }
    let env = if cfg!(target_env = "msvc") {
        "-msvc"
    } else if cfg!(target_env = "gnu") {
        "-gnu"
    } else if cfg!(target_env = "musl") {
        "-musl"
    } else {
        ""
    };
    format!("{}-{}{env}", std::env::consts::ARCH, std::env::consts::OS)
}

/// Whether `name` is a plain file name that cannot climb out of its directory.
pub fn is_bare_name(name: &str) -> bool {
    !name.is_empty() && !name.contains(['/', '\\']) && !name.contains("..")
}

/// What the interface is told about an image that has just been opened.
pub fn source_info(source: &trace::Source) -> Result<SourceInfo, String> {
    Ok(SourceInfo {
        name: source.name(),
        path: source.path.clone(),
        width: source.width,
        height: source.height,
        container: source.container.name(),
        lossy: source.container.is_lossy(),
        preview: preview_of(source)?,
    })
}

/// The source as a `data:` URL the viewer can show, capped on the longer side.
///
/// A file the webview can show as it is, and small enough, goes across as it is: decoding
/// a JPEG only to encode it again as a larger PNG costs a noticeable moment on every open.
/// Only PNG, JPEG, WebP, GIF and BMP qualify, and those other than PNG only when they carry
/// no EXIF rotation, because the webview honours one and the tracer does not; shown
/// rotated, the source would not line up with its own trace.
pub fn preview_of(source: &trace::Source) -> Result<String, String> {
    use lost::Container;
    const CAP: u32 = 2048;
    if source.width.max(source.height) <= CAP {
        let mime = match source.container {
            Container::Png => Some("image/png"),
            Container::Jpeg => Some("image/jpeg"),
            Container::WebpLossy | Container::WebpLossless => Some("image/webp"),
            Container::Gif => Some("image/gif"),
            Container::Bmp => Some("image/bmp"),
            Container::Tiff | Container::Unknown => None,
        };
        if let Some(mime) = mime {
            if source.container == Container::Png || !is_reoriented(&source.bytes) {
                return Ok(data_url(&source.bytes, mime));
            }
        }
    }
    // The raster a trace at this size reads anyway, so the decode is not wasted.
    let raster = source
        .raster(CAP as usize)
        .map_err(|e| format!("cannot read the image: {e}"))?;
    let (w, h) = (raster.width as u32, raster.height as u32);
    let bytes: Vec<u8> = raster
        .data
        .iter()
        .map(|c| (c.clamp(0.0, 1.0) * 255.0).round() as u8)
        .collect();
    let img = image::RgbaImage::from_raw(w, h, bytes)
        .ok_or_else(|| "the decoded image did not fill its buffer".to_string())?;
    let mut out = std::io::Cursor::new(Vec::new());
    image::DynamicImage::ImageRgba8(img)
        .write_to(&mut out, image::ImageFormat::Png)
        .map_err(|e| format!("cannot encode the preview: {e}"))?;
    Ok(data_url(&out.into_inner(), "image/png"))
}

/// Whether the file asks to be shown rotated or mirrored, which is any EXIF Orientation
/// but the first. A file whose orientation cannot be read counts as rotated, which only
/// costs it the re-encoded preview.
fn is_reoriented(bytes: &[u8]) -> bool {
    use image::ImageDecoder;
    image::ImageReader::new(std::io::Cursor::new(bytes))
        .with_guessed_format()
        .ok()
        .and_then(|r| r.into_decoder().ok())
        .and_then(|mut d| d.orientation().ok())
        .is_none_or(|o| o != image::metadata::Orientation::NoTransforms)
}

pub fn data_url(bytes: &[u8], mime: &str) -> String {
    format!("data:{mime};base64,{}", base64(bytes))
}

/// Standard base64, written out rather than pulled in: this is the only place the app
/// needs it, and it is twenty lines.
pub fn base64(bytes: &[u8]) -> String {
    const SET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b = [
            chunk[0],
            *chunk.get(1).unwrap_or(&0),
            *chunk.get(2).unwrap_or(&0),
        ];
        let n = ((b[0] as u32) << 16) | ((b[1] as u32) << 8) | b[2] as u32;
        out.push(SET[(n >> 18) as usize & 63] as char);
        out.push(SET[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 {
            SET[(n >> 6) as usize & 63] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            SET[n as usize & 63] as char
        } else {
            '='
        });
    }
    out
}

/// What the interface asks for when it wants a trace.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TraceRequest {
    /// The controls as they stand.
    pub settings: options::Settings,
    /// Draft or final.
    pub tier: trace::Tier,
}

// ------------------------------------------------------------------------- palette ---

/// A traced ink and what to paint it as instead.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Snap {
    /// The colour the tracer measured.
    pub from: String,
    /// The colour to paint it.
    pub to: String,
}

/// The rewritten drawing and its palette.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SnapResult {
    /// The SVG with the new fills.
    pub svg: String,
    /// The palette as it now stands.
    pub inks: Vec<quality::Ink>,
}

/// How far a snap's measured colour may be from an ink of a new trace and still name it.
///
/// A snap is kept with the image and applied again to every trace that lands after it
/// (the export's own fresh trace among them), and a re-trace measures each ink afresh: a
/// draft and a full trace, or two traces either side of a small control change, land a
/// hair apart. Well under the distance at which two inks of one logo read as different
/// colours, so a snap never jumps to a neighbour.
pub const SNAP_REACH_DE00: f64 = 2.0;

/// Rewrite the SVG's fills, and say how far each ink was moved from its measurement.
///
/// This is the one thing in the app that is genuinely instant: it rewrites fill strings
/// and nothing else, so there is no re-trace and no new measurement. The distance moved is
/// shown beside every snapped swatch, because the user is overriding something that was
/// measured and should be able to see by how much.
///
/// `svg` is the trace's own drawing and `snaps` every snap the user has made to it, all at
/// once: the fills are rewritten in one pass, so a snap from A to B and another from B to C
/// paint A as B, never as C. Each snap names the flat ink it was measured as, or failing an
/// exact match the nearest one within [`SNAP_REACH_DE00`]; one that names nothing in this
/// drawing (its ink merged away, say) is left out rather than guessed at.
pub fn snap_inks(
    svg: String,
    snaps: Vec<Snap>,
    width: u32,
    height: u32,
) -> Result<SnapResult, String> {
    let (width, height) = (width.max(1), height.max(1));
    let mut wanted: Vec<([f64; 3], String, String)> = Vec::new();
    for s in &snaps {
        match (quality::parse_hex(&s.from), quality::parse_hex(&s.to)) {
            (Some(from), Some(to)) => wanted.push((
                quality::lab(from),
                quality::to_hex(from),
                quality::to_hex(to),
            )),
            _ => return Err(format!("{} or {} is not a colour", s.from, s.to)),
        }
    }

    // Which ink each snap names: the one measured as exactly its colour, else the nearest in
    // reach that no other snap names exactly. An ink named by two snaps takes the closer.
    let before = quality::palette(&svg, width, height)?;
    let flats: Vec<(&quality::Ink, [f64; 3])> = before
        .iter()
        .filter(|i| i.kind == quality::InkKind::Flat)
        .filter_map(|i| Some((i, quality::lab(quality::parse_hex(&i.hex)?))))
        .collect();
    let exact = |hex: &str| wanted.iter().any(|(_, from, _)| from == hex);
    let mut taken: Vec<Option<(f64, &str)>> = vec![None; flats.len()];
    for (from_lab, from, to) in &wanted {
        let named = match flats.iter().position(|(i, _)| i.hex == *from) {
            Some(at) => Some((at, 0.0)),
            None => flats
                .iter()
                .enumerate()
                .filter(|(_, (i, _))| !exact(&i.hex))
                .map(|(at, (_, lab))| (at, quality::ciede2000(*from_lab, *lab)))
                .filter(|(_, d)| *d <= SNAP_REACH_DE00)
                .min_by(|a, b| a.1.total_cmp(&b.1)),
        };
        if let Some((at, d)) = named {
            if taken[at].is_none_or(|(had, _)| d < had) {
                taken[at] = Some((d, to.as_str()));
            }
        }
    }
    let mut repaint: std::collections::HashMap<String, String> = Default::default();
    let mut moved: Vec<(String, String)> = Vec::new();
    for ((ink, _), to) in flats.iter().zip(&taken) {
        let Some((_, to)) = *to else { continue };
        if to == ink.hex {
            continue;
        }
        for key in &ink.keys {
            repaint.insert(key.trim().to_string(), to.to_string());
        }
        moved.push((ink.hex.clone(), to.to_string()));
    }

    let paint = |value: &str| repaint.get(value.trim()).cloned();
    let out = quality::rewrite_attr(&svg, "fill", paint);
    let out = quality::rewrite_attr(&out, "stroke", paint);

    let mut inks = quality::palette(&out, width, height)?;
    // Only a flat ink can have been snapped: a gradient's `hex` is its first stop, which a
    // snap to the same colour must not be mistaken for.
    for ink in inks.iter_mut().filter(|i| i.kind == quality::InkKind::Flat) {
        if let Some((from, _)) = moved.iter().find(|(_, to)| *to == ink.hex) {
            ink.traced = from.clone();
            ink.snapped_de00 = quality::hex_distance(&ink.traced, &ink.hex);
        }
    }
    Ok(SnapResult { svg: out, inks })
}

/// One traced ink and the pasted colour nearest to it.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Match {
    /// The traced colour.
    pub from: String,
    /// The nearest pasted colour.
    pub to: String,
    /// How far apart they are.
    pub de00: f64,
}

/// Match a pasted brand palette to the traced inks, without changing anything yet.
///
/// Each traced ink takes the nearest pasted colour, and the distance is reported so the
/// modal can show what each match would cost before the user commits to it.
pub fn match_palette(traced: Vec<String>, pasted: String) -> Vec<Match> {
    let wanted = pasted_colours(&pasted);
    traced
        .iter()
        .filter_map(|t| {
            let from = t.to_ascii_lowercase();
            let lab_from = quality::lab(quality::parse_hex(&from)?);
            let best = wanted
                .iter()
                .map(|w| {
                    let d = quality::parse_hex(w)
                        .map(|c| quality::ciede2000(lab_from, quality::lab(c)))
                        .unwrap_or(f64::MAX);
                    (d, w.clone())
                })
                .min_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal))?;
            Some(Match {
                from,
                to: best.1,
                de00: best.0,
            })
        })
        .collect()
}

/// Every colour in a pasted palette, in the order it was written.
///
/// People paste lists separated by newlines, commas, semicolons or spaces, and the commas
/// inside `rgb(207, 198, 180)` are not separators: the text is cut only outside
/// parentheses, so a functional colour reaches [`parse_colour`] whole. A piece that is not
/// a colour (a label, a variable's name) is skipped.
pub fn pasted_colours(text: &str) -> Vec<String> {
    let mut pieces: Vec<String> = Vec::new();
    let mut piece = String::new();
    let mut depth = 0usize;
    for c in text.chars() {
        match c {
            '(' => depth += 1,
            ')' => depth = depth.saturating_sub(1),
            _ => {}
        }
        if depth == 0 && (c == ',' || c == ';' || c.is_whitespace()) {
            pieces.push(std::mem::take(&mut piece));
        } else {
            piece.push(c);
        }
    }
    pieces.push(piece);
    pieces.iter().filter_map(|p| parse_colour(p)).collect()
}

/// Read one colour out of a piece of pasted text.
///
/// Hex with a `#` (3, 4, 6 or 8 digits; an alpha channel is dropped, since a fill snaps to
/// an opaque colour), six hex digits without one, `rgb(...)` / `rgba(...)` with commas or
/// spaces and numbers or percentages, or a CSS custom property whose value is any of
/// those. Anything else is skipped rather than guessed at: three bare letters such as
/// `bad` or `add` are words far more often than colours.
pub fn parse_colour(line: &str) -> Option<String> {
    let line = line.trim().trim_end_matches(';');
    let value = line.rsplit(':').next().unwrap_or(line).trim();
    let lower = value.to_ascii_lowercase();
    if let Some(rest) = lower
        .strip_prefix("rgba")
        .or_else(|| lower.strip_prefix("rgb"))
    {
        let inside = rest
            .trim()
            .strip_prefix('(')?
            .trim_end()
            .strip_suffix(')')?;
        let channel = |s: &str| -> Option<u8> {
            let v = match s.strip_suffix('%') {
                Some(pct) => pct.trim().parse::<f32>().ok()? * 2.55,
                None => s.parse::<f32>().ok()?,
            };
            v.is_finite().then(|| v.clamp(0.0, 255.0).round() as u8)
        };
        // `rgb(r g b / a)` and `rgba(r, g, b, a)`: the alpha, when there is one, is fourth.
        let parts: Vec<&str> = inside
            .split(|c: char| c == ',' || c == '/' || c.is_whitespace())
            .filter(|s| !s.is_empty())
            .collect();
        if parts.len() < 3 {
            return None;
        }
        let (r, g, b) = (channel(parts[0])?, channel(parts[1])?, channel(parts[2])?);
        return Some(format!("#{r:02x}{g:02x}{b:02x}"));
    }
    let (hex, marked) = match lower.strip_prefix('#') {
        Some(h) => (h, true),
        None => (lower.as_str(), false),
    };
    if !hex.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    let rgb = match (hex.len(), marked) {
        (3 | 4, true) => &hex[..3],
        (6, _) => hex,
        (8, true) => &hex[..6],
        _ => return None,
    };
    quality::parse_hex(&format!("#{rgb}")).map(quality::to_hex)
}

// -------------------------------------------------------------------------- export ---

/// The subset of the report an export needs. Sent back rather than re-derived, so the
/// README quotes the numbers the user was actually looking at.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportReport {
    /// Mean colour difference, if one was measured.
    pub mean_de00: Option<f64>,
    /// Coordinates in the path data.
    pub coordinates: usize,
    /// Drawn elements.
    pub paths: usize,
    /// The longer side the trace ran at.
    pub traced_px: u32,
}

/// One ink, as the interface has it.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportInk {
    /// What it is painted as now.
    pub hex: String,
    /// What the tracer measured.
    pub traced: String,
    /// Share of the canvas.
    pub share: f64,
    /// A gradient's stop colours, in order; empty for a flat ink.
    #[serde(default)]
    pub stops: Vec<String>,
}

/// One honesty-panel row, as the interface has it.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportLoss {
    /// The row's sentence.
    pub text: String,
}

/// What the interface sends when it wants to export.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportRequest {
    /// The drawing to write, which may carry snapped fills.
    pub svg: String,
    /// The measurements, for the README.
    pub report: ExportReport,
    /// The palette, for `palette.json` and the README.
    pub palette: Vec<ExportInk>,
    /// The honesty panel's rows, for the README's one honest line.
    #[serde(default)]
    pub losses: Vec<ExportLoss>,
    /// Which formats to write.
    pub formats: export::Formats,
}

/// One file an export would write.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PlannedFile {
    /// Path relative to the destination.
    pub name: String,
    /// Its size, measured rather than estimated: the file already exists in memory.
    pub bytes: usize,
    /// Which checklist row it belongs to.
    pub group: &'static str,
}

/// What an export would write, with the sizes measured on the files already built.
pub fn planned(built: &[export::Artifact]) -> Vec<PlannedFile> {
    built
        .iter()
        .map(|a| PlannedFile {
            name: a.name.clone(),
            bytes: a.bytes,
            group: a.group,
        })
        .collect()
}

/// Build every file of an export in memory, for the image `source`.
pub fn build_export(
    request: &ExportRequest,
    source: &trace::Source,
) -> Result<Vec<export::Artifact>, String> {
    let report = quality::Report {
        mean_de00: request.report.mean_de00,
        coordinates: request.report.coordinates,
        paths: request.report.paths,
        traced_px: request.report.traced_px,
        bytes: request.svg.len(),
        ..Default::default()
    };
    let inks: Vec<quality::Ink> = request
        .palette
        .iter()
        .map(|i| {
            let gradient = i.stops.len() > 1;
            quality::Ink {
                traced: i.traced.clone(),
                hex: i.hex.clone(),
                share: i.share,
                snapped_de00: None,
                kind: if gradient {
                    quality::InkKind::Gradient
                } else {
                    quality::InkKind::Flat
                },
                stops: i.stops.clone(),
                ..Default::default()
            }
        })
        .collect();
    let losses: Vec<lost::Loss> = request
        .losses
        .iter()
        .map(|l| lost::Loss {
            kind: "note",
            text: l.text.clone(),
            why: String::new(),
            link: None,
        })
        .collect();

    let (stem, name) = (source.stem(), source.name());
    export::build(
        &export::Subject {
            stem: &stem,
            source_name: &name,
            svg: &request.svg,
            report: &report,
            palette: &inks,
            losses: &losses,
            source_px: (source.width, source.height),
        },
        &request.formats,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_matches_the_standard_alphabet_and_padding() {
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foo"), "Zm9v");
        assert_eq!(base64(b"foob"), "Zm9vYg==");
        assert_eq!(base64(b"fooba"), "Zm9vYmE=");
        assert_eq!(base64(b"foobar"), "Zm9vYmFy");
    }

    #[test]
    fn a_data_url_names_its_type() {
        assert!(data_url(b"foo", "image/png").starts_with("data:image/png;base64,Zm9v"));
    }

    #[test]
    fn a_sample_name_cannot_be_a_path() {
        for bad in ["../secrets", "a/b.png", "..\\windows", ""] {
            assert!(!is_bare_name(bad), "{bad} was accepted");
        }
        assert!(is_bare_name("flat-logo.png"));
    }

    #[test]
    fn a_pasted_palette_is_read_in_the_forms_people_paste() {
        assert_eq!(parse_colour("#12443E").as_deref(), Some("#12443e"));
        assert_eq!(parse_colour("12443E").as_deref(), Some("#12443e"));
        assert_eq!(parse_colour("  #abc  ").as_deref(), Some("#aabbcc"));
        assert_eq!(
            parse_colour("rgb(207, 198, 180)").as_deref(),
            Some("#cfc6b4")
        );
        assert_eq!(
            parse_colour("--brand-ink: #12443E;").as_deref(),
            Some("#12443e")
        );
        assert_eq!(parse_colour("not a colour"), None);
        assert_eq!(parse_colour(""), None);
    }

    /// The dialog's own placeholder shows `rgb(207, 198, 180)`; its commas are not list
    /// separators. Before this, the paste was cut at them and `198` was read as `#119988`.
    #[test]
    fn a_pasted_rgb_colour_keeps_its_commas() {
        let matches = match_palette(vec!["#cfc6b5".into()], "rgb(207, 198, 180)".into());
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].to, "#cfc6b4", "{matches:?}");
        assert_eq!(
            pasted_colours("#12443E\n#E9B24C\nrgb(207, 198, 180)"),
            ["#12443e", "#e9b24c", "#cfc6b4"]
        );
    }

    #[test]
    fn a_pasted_list_is_read_whatever_separates_it() {
        let text = "rgb(207, 198, 180), #12443E; rgba(1, 2, 3, 0.5)\n\
                    --brand-ink: rgb(10 20 30 / 50%);\t#abc #E9B24C80\r\n\
                    RGB(100%, 0%, 50%), Accent: 0a0b0c; bad, add";
        assert_eq!(
            pasted_colours(text),
            [
                "#cfc6b4", "#12443e", "#010203", "#0a141e", "#aabbcc", "#e9b24c", "#ff0080",
                "#0a0b0c",
            ]
        );
        assert!(pasted_colours("").is_empty());
        assert!(
            pasted_colours("rgb(1, 2)").is_empty(),
            "two channels is not a colour"
        );
    }

    #[test]
    fn matching_a_palette_picks_the_nearest_and_says_how_far() {
        let traced = vec!["#14453f".to_string(), "#e7b04a".to_string()];
        let matches = match_palette(traced, "#12443E\n#E9B24C\n#ffffff".into());
        assert_eq!(matches.len(), 2);
        assert_eq!(matches[0].to, "#12443e");
        assert_eq!(matches[1].to, "#e9b24c");
        for m in &matches {
            assert!(m.de00 > 0.0 && m.de00 < 5.0, "{m:?}");
        }
    }

    #[test]
    fn snapping_rewrites_fills_and_reports_the_distance_moved() {
        let svg = "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"32\" height=\"32\" \
                   viewBox=\"0 0 32 32\"><path d=\"M0 0L32 0L32 32L0 32Z\" fill=\"#14453f\"/></svg>";
        let result = snap_inks(
            svg.into(),
            vec![Snap {
                from: "#14453f".into(),
                to: "#12443e".into(),
            }],
            32,
            32,
        )
        .unwrap();
        assert!(result.svg.contains("fill=\"#12443e\""));
        assert!(!result.svg.contains("#14453f"));
        let ink = result.inks.iter().find(|i| i.hex == "#12443e").unwrap();
        assert_eq!(ink.traced, "#14453f");
        let moved = ink
            .snapped_de00
            .expect("a snapped ink reports what it cost");
        assert!(moved > 0.0 && moved < 3.0, "{moved}");
    }

    #[test]
    fn snapping_a_flat_ink_leaves_a_gradient_that_starts_there_alone() {
        let svg = "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"32\" height=\"32\" viewBox=\"0 0 32 32\">\
                   <defs><linearGradient id=\"g1\"><stop offset=\"0\" stop-color=\"#12443e\"/>\
                   <stop offset=\"1\" stop-color=\"#ffffff\"/></linearGradient></defs>\
                   <path d=\"M0 0H16V32H0Z\" fill=\"#14453f\"/><path d=\"M16 0H32V32H16Z\" fill=\"url(#g1)\"/></svg>";
        let result = snap_inks(
            svg.into(),
            vec![Snap {
                from: "#14453f".into(),
                to: "#12443e".into(),
            }],
            32,
            32,
        )
        .unwrap();
        assert!(
            result.svg.contains("fill=\"url(#g1)\""),
            "the gradient is untouched"
        );
        assert_eq!(result.inks.len(), 2);
        let ramp = result
            .inks
            .iter()
            .find(|i| i.kind == quality::InkKind::Gradient)
            .unwrap();
        assert_eq!(ramp.hex, "#12443e", "the ramp starts at the snapped colour");
        assert_eq!(ramp.snapped_de00, None, "but it was not snapped");
        let flat = result
            .inks
            .iter()
            .find(|i| i.kind == quality::InkKind::Flat)
            .unwrap();
        assert!(flat.snapped_de00.is_some());
        assert!((flat.share - 0.5).abs() < 0.02 && (ramp.share - 0.5).abs() < 0.02);
    }

    /// Two flat inks side by side, left and right.
    fn two_inks(left: &str, right: &str) -> String {
        format!(
            "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"32\" height=\"32\" viewBox=\"0 0 32 32\">\
             <path d=\"M0 0H16V32H0Z\" fill=\"{left}\"/><path d=\"M16 0H32V32H16Z\" fill=\"{right}\"/></svg>"
        )
    }

    fn snap(from: &str, to: &str) -> Snap {
        Snap {
            from: from.into(),
            to: to.into(),
        }
    }

    /// A pasted palette snaps every ink at once. When one ink's new colour is another's old
    /// one, the old sequential rewrite ran them into each other and both came out as C.
    #[test]
    fn snaps_are_applied_all_at_once_not_one_after_another() {
        let svg = two_inks("#aa2222", "#2222aa");
        let result = snap_inks(
            svg,
            vec![snap("#aa2222", "#2222aa"), snap("#2222aa", "#22aa22")],
            32,
            32,
        )
        .unwrap();
        assert!(
            result.svg.contains("d=\"M0 0H16V32H0Z\" fill=\"#2222aa\""),
            "{}",
            result.svg
        );
        assert!(
            result
                .svg
                .contains("d=\"M16 0H32V32H16Z\" fill=\"#22aa22\""),
            "{}",
            result.svg
        );
        assert_eq!(result.inks.len(), 2);
        assert!(result.inks.iter().all(|i| i.snapped_de00.is_some()));
    }

    /// The export re-traces, and a re-trace measures each ink afresh: a snap made on one
    /// trace names the same ink in the next even when it landed a hair away.
    #[test]
    fn a_snap_follows_its_ink_into_a_trace_that_measured_it_a_hair_apart() {
        let result = snap_inks(
            two_inks("#15463f", "#e7b04a"),
            vec![snap("#14453f", "#12443e")],
            32,
            32,
        )
        .unwrap();
        assert!(result.svg.contains("fill=\"#12443e\""), "{}", result.svg);
        assert!(!result.svg.contains("#15463f"));
        assert!(
            result.svg.contains("fill=\"#e7b04a\""),
            "a far ink is untouched"
        );
        let ink = result.inks.iter().find(|i| i.hex == "#12443e").unwrap();
        assert_eq!(
            ink.traced, "#15463f",
            "the swatch says what this trace measured"
        );
    }

    #[test]
    fn a_snap_that_names_an_ink_exactly_leaves_its_near_neighbour_alone() {
        let result = snap_inks(
            two_inks("#aa0000", "#ab0101"),
            vec![snap("#aa0000", "#ff0000")],
            32,
            32,
        )
        .unwrap();
        assert!(result.svg.contains("fill=\"#ff0000\""));
        assert!(result.svg.contains("fill=\"#ab0101\""), "{}", result.svg);
    }

    #[test]
    fn a_snap_to_bare_hex_writes_a_colour_an_svg_can_read() {
        let result = snap_inks(
            two_inks("#aa0000", "#0000aa"),
            vec![snap("#AA0000", "ABCDEF")],
            32,
            32,
        )
        .unwrap();
        assert!(result.svg.contains("fill=\"#abcdef\""), "{}", result.svg);
    }

    #[test]
    fn snapping_to_something_that_is_not_a_colour_is_refused() {
        let e = snap_inks(
            "<svg/>".into(),
            vec![Snap {
                from: "#14453f".into(),
                to: "chartreuse".into(),
            }],
            8,
            8,
        )
        .unwrap_err();
        assert!(e.contains("chartreuse"), "{e}");
    }

    /// A JPEG of `w` x `h`, with an EXIF Orientation of `orientation` when one is given.
    fn jpeg(w: u32, h: u32, orientation: Option<u16>) -> Vec<u8> {
        let img = image::RgbImage::from_fn(w, h, |x, y| {
            image::Rgb([(x * 7 % 256) as u8, (y * 5 % 256) as u8, 90])
        });
        let mut out = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgb8(img)
            .write_to(&mut out, image::ImageFormat::Jpeg)
            .unwrap();
        let plain = out.into_inner();
        let Some(o) = orientation else {
            return plain;
        };
        // An APP1 segment right after the start marker: "Exif", then a little-endian TIFF
        // header and one IFD entry, 0x0112 Orientation, SHORT, count 1.
        let mut payload = b"Exif\0\0".to_vec();
        payload.extend_from_slice(&[b'I', b'I', 42, 0, 8, 0, 0, 0, 1, 0, 0x12, 0x01, 3, 0]);
        payload.extend_from_slice(&[1, 0, 0, 0, o as u8, (o >> 8) as u8, 0, 0, 0, 0, 0, 0]);
        let len = (payload.len() + 2) as u16;
        let mut jpeg = vec![0xff, 0xd8, 0xff, 0xe1, (len >> 8) as u8, len as u8];
        jpeg.extend_from_slice(&payload);
        jpeg.extend_from_slice(&plain[2..]);
        jpeg
    }

    #[test]
    fn a_jpeg_that_needs_no_rotation_is_previewed_as_itself() {
        let bytes = jpeg(120, 80, None);
        assert!(!is_reoriented(&bytes));
        let source = trace::Source::open(bytes.clone(), None).unwrap();
        let url = preview_of(&source).unwrap();
        assert_eq!(url, data_url(&bytes, "image/jpeg"));
    }

    /// The webview would turn a rotated JPEG upright and the tracer would not, so its
    /// preview is the pixels as the tracer reads them.
    #[test]
    fn a_rotated_jpeg_is_previewed_as_the_tracer_reads_it() {
        let upright = jpeg(120, 80, Some(1));
        assert!(!is_reoriented(&upright), "Orientation 1 is no rotation");
        let rotated = jpeg(120, 80, Some(6));
        assert!(is_reoriented(&rotated));
        let source = trace::Source::open(rotated, None).unwrap();
        assert_eq!(
            (source.width, source.height),
            (120, 80),
            "stored size, not rotated"
        );
        let url = preview_of(&source).unwrap();
        assert!(url.starts_with("data:image/png;base64,"), "{}", &url[..30]);
    }
}
