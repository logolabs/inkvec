//! Export: the formats, the asset pack, and the README that goes inside it.
//!
//! Everything is produced in memory first, so the sheet can show a real byte count beside
//! each format instead of an estimate, and so a failure to render one PNG does not leave
//! half a folder behind. Writing happens once, at the end, when every artifact exists.
//!
//! The asset pack's README is a design surface, not a formality. It is the one artifact
//! the app leaves inside someone else's project folder, and someone else on their team
//! will open it — so it is laid out deliberately, carries the numbers that were measured,
//! and says plainly what tracing could not recover before it says anything about LogoLabs.

use std::io::Write;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::lost::Loss;
use crate::quality::{self, Ink, Report};

/// Which formats to write.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Formats {
    /// The SVG as traced.
    pub svg: bool,
    /// The same drawing, minified.
    pub svg_minified: bool,
    /// PNG at each of these widths.
    pub png_sizes: Vec<u32>,
    /// A `.ico` plus 16/32/48 PNGs.
    pub favicon: bool,
    /// One `.zip` holding all of the above plus `palette.json` and the README.
    pub asset_pack: bool,
}

impl Default for Formats {
    fn default() -> Self {
        Self {
            svg: true,
            svg_minified: true,
            png_sizes: vec![512, 1024, 2048],
            favicon: false,
            asset_pack: true,
        }
    }
}

/// One file the export would write.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Artifact {
    /// Path relative to the destination folder, using `/` on every platform.
    pub name: String,
    /// Its size in bytes.
    pub bytes: usize,
    /// Which checklist row it belongs to, so the sheet can total them per format.
    pub group: &'static str,
    /// The bytes themselves, kept out of the JSON sent to the interface.
    #[serde(skip)]
    pub data: Vec<u8>,
}

/// Everything an export needs to know about the trace it is exporting.
pub struct Subject<'a> {
    /// The file the image came from.
    pub stem: &'a str,
    /// What it was called when it arrived.
    pub source_name: &'a str,
    /// The SVG as traced.
    pub svg: &'a str,
    /// The measurements.
    pub report: &'a Report,
    /// The inks.
    pub palette: &'a [Ink],
    /// What could not be recovered.
    pub losses: &'a [Loss],
    /// Size of the source as it arrived.
    pub source_px: (u32, u32),
}

/// Build every file the chosen formats ask for.
pub fn build(subject: &Subject<'_>, formats: &Formats) -> Result<Vec<Artifact>, String> {
    let stem = sanitise_stem(subject.stem);
    let mut out: Vec<Artifact> = Vec::new();

    let minified = inkvec_svgmin::compact(
        subject.svg,
        &inkvec_svgmin::Options {
            decimals: Some(2),
            ..Default::default()
        },
    )
    .map(|(s, _)| s)
    .unwrap_or_else(|_| subject.svg.to_string());

    if formats.svg {
        out.push(artifact(
            format!("{stem}.svg"),
            "svg",
            subject.svg.as_bytes().to_vec(),
        ));
    }
    if formats.svg_minified {
        out.push(artifact(
            format!("{stem}.min.svg"),
            "svgMinified",
            minified.clone().into_bytes(),
        ));
    }

    for size in formats.png_sizes.iter().copied().filter(|s| *s > 0) {
        let size = size.clamp(16, 8192);
        let png = render_png(subject.svg, size, size)?;
        out.push(artifact(format!("png/{stem}-{size}.png"), "png", png));
    }

    if formats.favicon {
        for size in [16u32, 32, 48] {
            let png = render_png(subject.svg, size, size)?;
            out.push(artifact(
                format!("favicon/favicon-{size}.png"),
                "favicon",
                png,
            ));
        }
        out.push(artifact(
            "favicon/favicon.ico".into(),
            "favicon",
            ico(subject.svg)?,
        ));
    }

    let palette_json = palette_json(subject.palette);
    let readme = readme(subject, &minified, &stem);

    if formats.asset_pack {
        // The pack holds the whole set, whether or not the loose copies were ticked:
        // a pack with only half the formats in it is a surprise inside someone else's
        // project folder.
        let mut inside: Vec<Artifact> = Vec::new();
        inside.push(artifact(
            format!("{stem}.svg"),
            "pack",
            subject.svg.as_bytes().to_vec(),
        ));
        inside.push(artifact(
            format!("{stem}.min.svg"),
            "pack",
            minified.into_bytes(),
        ));
        for size in [512u32, 1024, 2048] {
            inside.push(artifact(
                format!("png/{stem}-{size}.png"),
                "pack",
                render_png(subject.svg, size, size)?,
            ));
        }
        for size in [16u32, 32, 48] {
            inside.push(artifact(
                format!("favicon/favicon-{size}.png"),
                "pack",
                render_png(subject.svg, size, size)?,
            ));
        }
        inside.push(artifact(
            "favicon/favicon.ico".into(),
            "pack",
            ico(subject.svg)?,
        ));
        inside.push(artifact(
            "palette.json".into(),
            "pack",
            palette_json.clone().into_bytes(),
        ));
        inside.push(artifact(
            "README.txt".into(),
            "pack",
            readme.clone().into_bytes(),
        ));
        out.push(artifact(
            format!("{stem}-assets.zip"),
            "assetPack",
            zip(&inside)?,
        ));
    } else {
        out.push(artifact(
            "palette.json".into(),
            "svg",
            palette_json.into_bytes(),
        ));
    }

    Ok(out)
}

/// Write a built set into `destination`, creating the subfolders it needs.
pub fn write_all(artifacts: &[Artifact], destination: &Path) -> Result<Vec<PathBuf>, String> {
    let mut written = Vec::new();
    for a in artifacts {
        let path = destination.join(a.name.replace('/', std::path::MAIN_SEPARATOR_STR));
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("cannot create {}: {e}", parent.display()))?;
        }
        std::fs::write(&path, &a.data)
            .map_err(|e| format!("cannot write {}: {e}", path.display()))?;
        written.push(path);
    }
    Ok(written)
}

fn artifact(name: String, group: &'static str, data: Vec<u8>) -> Artifact {
    Artifact {
        bytes: data.len(),
        name,
        group,
        data,
    }
}

/// A file name that will survive being written on any of the three platforms.
fn sanitise_stem(stem: &str) -> String {
    let cleaned: String = stem
        .chars()
        .map(|c| {
            if "\\/:*?\"<>|".contains(c) || c.is_control() {
                '-'
            } else {
                c
            }
        })
        .collect();
    let cleaned = cleaned.trim().trim_matches('.').to_string();
    if cleaned.is_empty() {
        "image".to_string()
    } else {
        cleaned
    }
}

fn render_png(svg: &str, w: u32, h: u32) -> Result<Vec<u8>, String> {
    let rgba = quality::render(svg, w, h)?;
    let buf = image::RgbaImage::from_raw(w, h, rgba)
        .ok_or_else(|| "the render did not fill the buffer".to_string())?;
    let mut out = std::io::Cursor::new(Vec::new());
    image::DynamicImage::ImageRgba8(buf)
        .write_to(&mut out, image::ImageFormat::Png)
        .map_err(|e| format!("cannot encode the PNG: {e}"))?;
    Ok(out.into_inner())
}

/// A `.ico` carrying 16, 32 and 48 px.
///
/// Written here rather than left to an encoder: the ICO container is a header, a directory
/// and a run of embedded PNGs, and every decoder since Vista reads PNG-in-ICO.
fn ico(svg: &str) -> Result<Vec<u8>, String> {
    let sizes = [16u32, 32, 48];
    let images: Vec<Vec<u8>> = sizes
        .iter()
        .map(|s| render_png(svg, *s, *s))
        .collect::<Result<_, _>>()?;

    let mut out = Vec::new();
    out.extend_from_slice(&[0, 0, 1, 0]); // reserved, type 1 (icon)
    out.extend_from_slice(&(sizes.len() as u16).to_le_bytes());
    let mut offset = 6 + 16 * sizes.len() as u32;
    for (size, png) in sizes.iter().zip(&images) {
        // 0 in the width/height byte means 256; none of these sizes needs that, but the
        // cast is written explicitly so it is obvious why it is safe.
        out.push(if *size >= 256 { 0 } else { *size as u8 });
        out.push(if *size >= 256 { 0 } else { *size as u8 });
        out.extend_from_slice(&[0, 0]); // palette count, reserved
        out.extend_from_slice(&1u16.to_le_bytes()); // colour planes
        out.extend_from_slice(&32u16.to_le_bytes()); // bits per pixel
        out.extend_from_slice(&(png.len() as u32).to_le_bytes());
        out.extend_from_slice(&offset.to_le_bytes());
        offset += png.len() as u32;
    }
    for png in &images {
        out.extend_from_slice(png);
    }
    Ok(out)
}

fn zip(files: &[Artifact]) -> Result<Vec<u8>, String> {
    let mut cursor = std::io::Cursor::new(Vec::new());
    {
        let mut w = zip::ZipWriter::new(&mut cursor);
        let options: zip::write::FileOptions<'_, ()> =
            zip::write::FileOptions::default().compression_method(zip::CompressionMethod::Deflated);
        for f in files {
            w.start_file(f.name.clone(), options)
                .map_err(|e| format!("cannot add {} to the pack: {e}", f.name))?;
            w.write_all(&f.data)
                .map_err(|e| format!("cannot write {} into the pack: {e}", f.name))?;
        }
        w.finish()
            .map_err(|e| format!("cannot close the pack: {e}"))?;
    }
    Ok(cursor.into_inner())
}

fn palette_json(inks: &[Ink]) -> String {
    let inks: Vec<serde_json::Value> = inks
        .iter()
        .map(|i| {
            serde_json::json!({
                "hex": i.hex,
                "traced": i.traced,
                "share": (i.share * 10_000.0).round() / 10_000.0,
            })
        })
        .collect();
    serde_json::to_string_pretty(&serde_json::json!({
        "generator": "Inkvec Studio Lite",
        "inks": inks,
    }))
    .unwrap_or_else(|_| "{}".into())
}

/// The plain-text README that ships inside the asset pack.
fn readme(subject: &Subject<'_>, minified: &str, stem: &str) -> String {
    let r = subject.report;
    let rule = "─".repeat(48);
    let de00 = r
        .mean_de00
        .map(|d| format!("Mean colour difference {d:.2} dE00 · "))
        .unwrap_or_default();

    let mut lines = vec![
        format!("{stem} — vector asset pack"),
        rule.clone(),
        format!(
            "Traced from {} ({} × {}) at {} px",
            subject.source_name, subject.source_px.0, subject.source_px.1, r.traced_px
        ),
        format!(
            "{de00}{} coordinates · {} paths",
            thousands(r.coordinates),
            r.paths
        ),
        String::new(),
        row(
            &format!("{stem}.svg"),
            r.bytes,
            "editable, one path per colour",
        ),
        row(
            &format!("{stem}.min.svg"),
            minified.len(),
            "no ids, no groups, no trailing zeros",
        ),
        row("png/512, 1024, 2048", 0, ""),
        row("favicon/", 0, ".ico + 16/32/48 png"),
        row(
            "palette.json",
            0,
            &format!(
                "{} ink{}, hex + canvas share",
                subject.palette.len(),
                if subject.palette.len() == 1 { "" } else { "s" }
            ),
        ),
        String::new(),
        "Traced with Inkvec Studio Lite (Apache-2.0). Nothing was uploaded.".to_string(),
    ];

    // The honest line comes before the LogoLabs line, and only when there is one to make.
    if let Some(first) = subject.losses.first() {
        lines.push(first.text.clone());
    }
    lines.push("LogoLabs draws logos and brand assets — logolabs.org".to_string());
    lines.join("\n") + "\n"
}

fn row(name: &str, bytes: usize, note: &str) -> String {
    let size = if bytes > 0 {
        format_kb(bytes)
    } else {
        String::new()
    };
    format!("  {name:<28}{size:>9}   {note}")
        .trim_end()
        .to_string()
}

fn format_kb(bytes: usize) -> String {
    format!("{:.1} KB", bytes as f64 / 1024.0)
}

fn thousands(n: usize) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const SVG: &str = "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"64\" height=\"64\" \
                       viewBox=\"0 0 64 64\"><path d=\"M8 8L56 8L56 56L8 56Z\" \
                       fill=\"#14453f\"/></svg>";

    fn subject<'a>(report: &'a Report, inks: &'a [Ink], losses: &'a [Loss]) -> Subject<'a> {
        Subject {
            stem: "northwind-mark",
            source_name: "northwind-mark.png",
            svg: SVG,
            report,
            palette: inks,
            losses,
            source_px: (1600, 1600),
        }
    }

    fn a_report() -> Report {
        Report {
            mean_de00: Some(0.12),
            median_de00: Some(0.09),
            worst_de00: Some(0.9),
            coordinates: 1382,
            paths: 11,
            segments: 266,
            colours: 4,
            bytes: SVG.len(),
            minified_bytes: Some(120),
            seconds: 1.18,
            traced_px: 1024,
        }
    }

    fn an_ink() -> Ink {
        Ink {
            traced: "#14453f".into(),
            hex: "#14453f".into(),
            share: 0.56,
            snapped_de00: None,
        }
    }

    #[test]
    fn the_default_set_writes_the_files_the_sheet_promises() {
        let (r, inks, losses) = (a_report(), vec![an_ink()], vec![]);
        let built = build(&subject(&r, &inks, &losses), &Formats::default()).unwrap();
        let names: Vec<&str> = built.iter().map(|a| a.name.as_str()).collect();
        assert!(names.contains(&"northwind-mark.svg"), "{names:?}");
        assert!(names.contains(&"northwind-mark.min.svg"), "{names:?}");
        assert!(names.contains(&"png/northwind-mark-1024.png"), "{names:?}");
        assert!(names.contains(&"northwind-mark-assets.zip"), "{names:?}");
        for a in &built {
            assert!(a.bytes > 0, "{} is empty", a.name);
            assert_eq!(a.bytes, a.data.len());
        }
    }

    #[test]
    fn the_pack_is_complete_even_when_the_loose_copies_are_not_ticked() {
        let (r, inks, losses) = (a_report(), vec![an_ink()], vec![]);
        let only_pack = Formats {
            svg: false,
            svg_minified: false,
            png_sizes: vec![],
            favicon: false,
            asset_pack: true,
        };
        let built = build(&subject(&r, &inks, &losses), &only_pack).unwrap();
        assert_eq!(built.len(), 1);
        let bytes = &built[0].data;
        let mut zip = zip::ZipArchive::new(std::io::Cursor::new(bytes.clone())).unwrap();
        let inside: Vec<String> = (0..zip.len())
            .map(|i| zip.by_index(i).unwrap().name().to_string())
            .collect();
        for wanted in [
            "northwind-mark.svg",
            "northwind-mark.min.svg",
            "png/northwind-mark-2048.png",
            "favicon/favicon.ico",
            "palette.json",
            "README.txt",
        ] {
            assert!(
                inside.contains(&wanted.to_string()),
                "{wanted} missing: {inside:?}"
            );
        }
    }

    #[test]
    fn the_readme_leads_with_the_measurements_and_ends_with_one_logolabs_line() {
        let (r, inks) = (a_report(), vec![an_ink()]);
        let losses = vec![Loss {
            kind: "lettering",
            text: "The lettering came back as outlines, not editable text.".into(),
            why: "…".into(),
            link: None,
        }];
        let text = readme(&subject(&r, &inks, &losses), "<svg/>", "northwind-mark");
        assert!(text.starts_with("northwind-mark — vector asset pack"));
        assert!(text.contains("0.12 dE00"));
        assert!(text.contains("1,382 coordinates"));
        assert!(text.contains("Nothing was uploaded."));
        assert!(text.contains("The lettering came back as outlines"));
        assert_eq!(text.matches("logolabs.org").count(), 1, "one link, no more");
        // The honest line comes before the pitch.
        assert!(text.find("outlines").unwrap() < text.find("logolabs.org").unwrap());
    }

    #[test]
    fn the_readme_says_nothing_about_losses_when_there_are_none() {
        let (r, inks, losses) = (a_report(), vec![an_ink()], vec![]);
        let text = readme(&subject(&r, &inks, &losses), "<svg/>", "x");
        assert!(text.contains("Nothing was uploaded."));
        assert_eq!(text.matches("logolabs.org").count(), 1);
    }

    #[test]
    fn an_ico_carries_three_sizes_as_png() {
        let bytes = ico(SVG).unwrap();
        assert_eq!(&bytes[0..4], &[0, 0, 1, 0]);
        assert_eq!(u16::from_le_bytes([bytes[4], bytes[5]]), 3);
        // The first directory entry points at a PNG.
        let offset = u32::from_le_bytes([bytes[18], bytes[19], bytes[20], bytes[21]]) as usize;
        assert_eq!(&bytes[offset..offset + 8], b"\x89PNG\r\n\x1a\n");
    }

    #[test]
    fn a_hostile_file_name_cannot_escape_the_destination() {
        // The exact replacement does not matter; what matters is that nothing that comes
        // out can still be read as a path or as a hidden file.
        for hostile in ["../../etc/passwd", "..\\..\\windows", "....", ".hidden"] {
            let safe = sanitise_stem(hostile);
            assert!(!safe.contains(['/', '\\']), "{hostile} -> {safe}");
            assert!(!safe.starts_with('.'), "{hostile} -> {safe}");
            assert!(!safe.is_empty(), "{hostile} -> empty");
        }
        assert_eq!(sanitise_stem("  "), "image");
        assert_eq!(sanitise_stem("a:b*c?"), "a-b-c-");
        assert_eq!(sanitise_stem("northwind-mark"), "northwind-mark");
    }

    #[test]
    fn writing_puts_every_file_where_its_name_says() {
        let dir = std::env::temp_dir().join(format!("inkvec-export-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let (r, inks, losses) = (a_report(), vec![an_ink()], vec![]);
        let built = build(&subject(&r, &inks, &losses), &Formats::default()).unwrap();
        let written = write_all(&built, &dir).unwrap();
        assert_eq!(written.len(), built.len());
        assert!(dir.join("png").join("northwind-mark-512.png").exists());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn thousands_puts_the_commas_where_a_reader_expects_them() {
        assert_eq!(thousands(0), "0");
        assert_eq!(thousands(999), "999");
        assert_eq!(thousands(1382), "1,382");
        assert_eq!(thousands(1234567), "1,234,567");
    }
}
