//! Prepare vector art for physical output: vinyl cutters, print-and-cut stickers, and the
//! preflight that says what will go wrong on the machine before it does.
//!
//! Two calls. [`analyze`] reads any SVG and reports its colours (which one is the page,
//! which are gradients), so an interface can offer choices before asking for anything.
//! [`prepare`] builds the sheets:
//!
//! * [`Mode::SingleColour`]: everything chosen, joined, one sheet.
//! * [`Mode::Layered`]: one sheet per colour, each running under the colours above it by
//!   the bleed, with registration marks, so a stack has no gaps where colours meet.
//! * [`Mode::Inlay`]: one sheet per colour, cut exactly, to lay edge to edge.
//! * [`Mode::Sticker`]: the artwork with a smoothed contour around it for print-and-cut.
//! * [`Mode::Stencil`]: a sheet with the artwork cut out, every loose counter bridged.
//!
//! The work happens in millimetres on polygons: overlaps are cut back to what shows
//! ([`regions`]), layers are grown and clipped ([`geom`]), and every contour is refitted as
//! lines and cubics before it is written ([`fitcurve`], [`write`]), so a
//! sheet carries a few hundred path segments rather than the tens of thousands of points
//! that make cutter software refuse a traced file.

pub mod biarc;
pub mod dxf;
pub mod fitcurve;
pub mod gcode;
pub mod geom;
pub mod lines;
pub mod load;
pub mod options;
pub mod plan;
pub mod preflight;
pub mod regions;
pub mod stencil;
pub mod write;

pub use load::LoadError;
pub use options::{
    Analysis, Check, ColourInfo, CutStyle, FileUnits, Layer, Level, Mode, Options, Plan,
};

/// Width used to measure an artwork whose physical size has not been asked for yet.
const ANALYSIS_WIDTH_MM: f64 = 100.0;

/// What is in `svg`: its colours and anything that cannot be cut.
pub fn analyze(svg: &str) -> Result<Analysis, LoadError> {
    let art = load::load(svg, ANALYSIS_WIDTH_MM, 0.05)?;
    let colours = regions::visible_colours(&art, Options::default().merge_delta_e);
    let canvas = art.size_mm[0] * art.size_mm[1];
    Ok(Analysis {
        size_px: art.size_px,
        aspect: art.size_px[1] / art.size_px[0],
        colours: colours
            .iter()
            .map(|c| ColourInfo {
                hex: regions::hex(c.rgb),
                coverage: c.area_mm2 / canvas,
                background: c.background,
                gradient: c.gradient,
                translucent: c.translucent,
            })
            .collect(),
        items: art.items.len(),
        nodes: art.items.iter().map(|i| i.nodes).sum(),
        unsupported: art.unsupported,
    })
}

/// Build the sheets `o` asks for from `svg`.
pub fn prepare(svg: &str, o: &Options) -> Result<Plan, LoadError> {
    if o.width_mm.is_nan() || o.width_mm <= 0.0 {
        return Err(LoadError("the width must be positive".into()));
    }
    // Flatten a little finer than the output tolerance, so the refit has room.
    let timing = std::env::var_os("INKVEC_FAB_TIMING").is_some();
    let t = std::time::Instant::now();
    let art = load::load(svg, o.width_mm, (o.tolerance_mm / 4.0).max(0.002))?;
    if timing {
        let pts: usize = art
            .items
            .iter()
            .flat_map(|i| i.region.iter().flatten())
            .map(Vec::len)
            .sum();
        eprintln!(
            "load {:.0} ms, {pts} points",
            t.elapsed().as_secs_f64() * 1e3
        );
    }
    let t = std::time::Instant::now();
    let colours = regions::visible_colours(&art, o.merge_delta_e);
    if timing {
        eprintln!("visible colours {:.0} ms", t.elapsed().as_secs_f64() * 1e3);
    }
    let mut p = plan::plan(&colours, art.size_mm, &art.unsupported, o);
    // The files people save state their size the way their program reads it; the preview
    // and overlay are only ever drawn here and stay in millimetres.
    for l in &mut p.layers {
        l.svg = write::with_units(&l.svg, o.file_units);
    }
    p.combined_svg = write::with_units(&p.combined_svg, o.file_units);
    Ok(p)
}

#[cfg(test)]
mod tests {
    use super::*;

    const LOGO: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="60">
        <rect width="100" height="60" fill="#ffffff"/>
        <circle cx="30" cy="30" r="20" fill="#1e3a8a"/>
        <circle cx="30" cy="30" r="8" fill="#f59e0b"/>
        <rect x="60" y="15" width="30" height="30" fill="#f59e0b"/></svg>"##;

    #[test]
    fn analysis_finds_the_page_and_the_inks() {
        let a = analyze(LOGO).unwrap();
        assert_eq!(a.colours.len(), 3);
        assert!(a.colours[0].background && a.colours[0].hex == "#ffffff");
        assert!(a.colours.iter().skip(1).all(|c| !c.background));
    }

    #[test]
    fn layered_sheets_run_under_the_colours_above() {
        let o = Options {
            mode: Mode::Layered,
            width_mm: 100.0,
            order: vec!["#1e3a8a".into(), "#f59e0b".into()],
            ..Options::default()
        };
        let p = prepare(LOGO, &o).unwrap();
        assert_eq!(p.layers.len(), 2);
        // The blue disc is a ring (r 20 minus the r 8 orange) plus the bleed under the
        // orange: more than the ring alone, less than the full disc; marks add a little.
        let ring = std::f64::consts::PI * (400.0 - 64.0);
        let blue = p.layers[0].area_mm2;
        assert!(blue > ring + 10.0, "{blue} vs ring {ring}");
        // Every sheet is small: circles and squares refit as a handful of segments.
        assert!(
            p.layers.iter().all(|l| l.nodes < 80),
            "{:?}",
            p.layers.iter().map(|l| l.nodes).collect::<Vec<_>>()
        );
        assert!(p.layers[0].svg.contains("width=\""));
    }

    #[test]
    fn a_stroked_drawing_is_drawn_along_its_own_strokes() {
        // An arrow drawn with strokes, a crossing, and one filled square beside it.
        let svg = r##"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="50">
            <path d="M10 25H40M20 15 10 25 20 35M25 10V40" fill="none" stroke="#000" stroke-width="3"/>
            <rect x="60" y="10" width="30" height="30" fill="#000"/></svg>"##;
        let o = Options {
            mode: Mode::Lines,
            width_mm: 100.0,
            ..Options::default()
        };
        let p = prepare(svg, &o).unwrap();
        assert_eq!(p.layers.len(), 1);
        let pen = &p.layers[0].svg;
        // Three strokes as three lines at the pen width, the square as its outline.
        assert_eq!(pen.matches("stroke-width=\"0.400\"").count(), 4, "{pen}");
        let info = p
            .checks
            .iter()
            .find(|c| c.code == "lines")
            .expect("lines info");
        assert!(info.message.contains("3 line(s)"), "{}", info.message);
        assert!(
            info.message.contains("the file's own strokes"),
            "{}",
            info.message
        );
    }

    #[test]
    fn the_size_check_square_is_cut_below_and_the_file_states_its_size_as_asked() {
        let o = Options {
            mode: Mode::SingleColour,
            width_mm: 100.0,
            include: vec!["#1e3a8a".into()],
            size_check_mm: 25.4,
            file_units: FileUnits::Px72,
            ..Options::default()
        };
        let p = prepare(LOGO, &o).unwrap();
        // The disc (r 20 less the r 8 orange) plus one inch square.
        let ring = std::f64::consts::PI * (400.0 - 64.0);
        let area = p.layers[0].area_mm2;
        assert!((area - ring - 25.4 * 25.4).abs() < 8.0, "{area}");
        // The material extent is the design's, not the square's.
        assert!(p.layers[0].material_mm[1] < 41.0);
        assert!(p.checks.iter().any(|c| c.code == "sizeCheck"));
        // 72 px an inch: the job's millimetre width, restated.
        let svg = &p.layers[0].svg;
        let w: f64 = svg
            .split("width=\"")
            .nth(1)
            .unwrap()
            .split('"')
            .next()
            .unwrap()
            .parse()
            .unwrap();
        assert!(
            (w - p.size_mm[0] / 25.4 * 72.0).abs() < 0.01,
            "{w} in {svg:.120}"
        );
        assert!(!svg[..200].contains("mm\""), "no millimetre size left");
        assert!(
            p.preview_svg.contains("mm\""),
            "the preview stays in millimetres"
        );
    }

    #[test]
    fn a_sticker_is_one_piece_around_everything() {
        let o = Options {
            mode: Mode::Sticker,
            width_mm: 100.0,
            ..Options::default()
        };
        let p = prepare(LOGO, &o).unwrap();
        assert_eq!(p.layers.len(), 1);
        assert_eq!(
            p.layers[0].parts, 1,
            "the disc and square are joined by the contour"
        );
    }
}
