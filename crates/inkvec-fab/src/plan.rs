//! From visible colours to sheets: the four modes, and what every sheet gets added.

use crate::corners;
use crate::dxf;
use crate::gcode;
use crate::geom::{self, Region};
use crate::lines;
use crate::options::{Check, CutStyle, Layer, Mode, Options, Plan};
use crate::preflight;
use crate::regions::{delta_e, hex, ColourRegion};
use crate::stencil;
use crate::write;

/// Distance of the registration marks and weed border from the artwork's bounds.
const MARK_GAP_MM: f64 = 4.0;
/// Arm length of a registration cross.
const MARK_ARM_MM: f64 = 5.0;
/// Width of a registration cross's bars and of a weed border.
const MARK_BAR_MM: f64 = 0.8;

fn parse_hex(s: &str) -> Option<[u8; 3]> {
    let s = s.trim().trim_start_matches('#');
    if s.len() != 6 {
        return None;
    }
    let b = |i: usize| u8::from_str_radix(&s[i..i + 2], 16).ok();
    Some([b(0)?, b(2)?, b(4)?])
}

/// The colours to cut, in the order asked for (bottom first).
fn chosen<'a>(colours: &'a [ColourRegion], o: &Options) -> Vec<&'a ColourRegion> {
    let pick = |list: &[String]| -> Vec<&'a ColourRegion> {
        list.iter()
            .filter_map(|h| parse_hex(h))
            .filter_map(|rgb| {
                colours
                    .iter()
                    .filter(|c| delta_e(c.rgb, rgb) <= o.merge_delta_e.max(0.5))
                    .min_by(|a, b| delta_e(a.rgb, rgb).total_cmp(&delta_e(b.rgb, rgb)))
            })
            .collect()
    };
    let mut set: Vec<&ColourRegion> = if o.include.is_empty() {
        colours.iter().filter(|c| !c.background).collect()
    } else {
        pick(&o.include)
    };
    set.dedup_by(|a, b| a.rgb == b.rgb);
    if !o.order.is_empty() {
        let order = pick(&o.order);
        set.sort_by_key(|c| {
            order
                .iter()
                .position(|x| x.rgb == c.rgb)
                .unwrap_or(usize::MAX)
        });
    }
    set
}

/// How far inside the colour above a bleed stops, so it cannot peek out past it.
const BLEED_HIDE_MM: f64 = 0.2;

/// Print bleed of a sticker: its outer colours run this far under the contour, so a cut a
/// little off the line shows colour, not paper.
const PRINT_BLEED_MM: f64 = 1.5;

/// LightBurn's first layer colours (00 to 05), exactly as its palette has them: a laser
/// program maps an imported colour to a layer only on an exact match.
const LIGHTBURN: [&str; 6] = [
    "#000000", "#0000ff", "#ff0000", "#00e000", "#d0d000", "#ff8000",
];

/// Colour a stencil sheet is shown in: the blue-grey of Mylar.
const STENCIL_HEX: &str = "#5f7489";

/// Base sheets, bottom first, before marks: `(name, hex, region)`.
fn sheets(
    set: &[&ColourRegion],
    o: &Options,
    checks: &mut Vec<Check>,
) -> Vec<(String, String, Region)> {
    match o.mode {
        // Built by `plan_lines` instead; it never reaches the sheet builder.
        Mode::Lines => Vec::new(),
        Mode::Stencil => {
            let all = geom::union_all(set.iter().map(|c| &c.region));
            let (sheet, bridges) = stencil::stencil(&all, o.stencil_margin_mm, o.bridge_mm);
            if !bridges.is_empty() {
                checks.push(Check {
                    level: crate::options::Level::Info,
                    code: "bridges",
                    message: format!(
                        "Loose pieces that would fall out of the stencil are held by {} bridge(s), {} mm wide; large ones by two.",
                        bridges.len(),
                        o.bridge_mm
                    ),
                });
            }
            vec![("Stencil".into(), STENCIL_HEX.into(), sheet)]
        }
        Mode::SingleColour => {
            let all = geom::union_all(set.iter().map(|c| &c.region));
            let colour = if set.len() == 1 {
                hex(set[0].rgb)
            } else {
                "#000000".into()
            };
            vec![("Cut".into(), colour, all)]
        }
        Mode::Inlay => set
            .iter()
            .map(|c| (hex(c.rgb), hex(c.rgb), c.region.clone()))
            .collect(),
        Mode::Layered => (0..set.len())
            .map(|k| {
                let own = &set[k].region;
                let above = geom::union_all(set[k + 1..].iter().map(|c| &c.region));
                // Clipped short of the colour above's own edge, so the bleed never shows
                // where that colour is narrower than the bleed is long.
                let hidden = geom::offset(&above, -BLEED_HIDE_MM);
                let under = geom::intersection(&geom::offset(own, o.bleed_mm), &hidden);
                // A hole the layers above cover completely is filled, not shrunk: the
                // colour on top hides it either way, and a filled sheet has nothing to
                // weed and nothing to line up there.
                let covered = covered_holes(own, &above);
                let grown = geom::union(&geom::union(own, &under), &covered);
                (hex(set[k].rgb), hex(set[k].rgb), grown)
            })
            .collect(),
        Mode::Sticker => {
            let all = geom::union_all(set.iter().map(|c| &c.region));
            // Grow by the margin, then close the small notches a contour knife cannot
            // follow, and drop holes: a sticker is one piece.
            let grown = geom::offset(&all, o.sticker_margin_mm);
            let smooth = geom::closing(&grown, 2.0 * o.sticker_margin_mm);
            vec![(
                "Contour".into(),
                "#000000".into(),
                geom::fill_holes(&smooth),
            )]
        }
    }
}

/// The holes of `own` that `above` covers (to within a hundredth of their area).
fn covered_holes(own: &Region, above: &Region) -> Region {
    let holes = geom::difference(&geom::fill_holes(own), own);
    let kept: Region = holes
        .into_iter()
        .filter(|h| {
            let hole = vec![h.clone()];
            geom::area(&geom::difference(&hole, above)) <= 0.01 * geom::shape_area(h)
        })
        .collect();
    kept
}

/// Registration crosses at the four corners outside `b`.
fn registration_marks(b: [f64; 4]) -> Region {
    let g = MARK_GAP_MM + MARK_ARM_MM / 2.0;
    let (h, a) = (MARK_BAR_MM / 2.0, MARK_ARM_MM / 2.0);
    let corners = [
        [b[0] - g, b[1] - g],
        [b[2] + g, b[1] - g],
        [b[0] - g, b[3] + g],
        [b[2] + g, b[3] + g],
    ];
    let bars: Vec<Region> = corners
        .iter()
        .flat_map(|c| {
            [
                geom::rect(c[0] - a, c[1] - h, c[0] + a, c[1] + h),
                geom::rect(c[0] - h, c[1] - a, c[0] + h, c[1] + a),
            ]
        })
        .collect();
    geom::union_all(bars.iter())
}

/// The G-code settings `o` asks for, with the frame's left edge at x = 0.
fn gcode_settings(o: &Options, origin: [f64; 2]) -> gcode::Settings {
    gcode::Settings {
        feed_mm_min: o.gcode_feed_mm_min,
        power: o.gcode_power,
        passes: o.gcode_passes,
        x0: origin[0],
    }
}

/// Gap between the design (with its marks and border) and the size-check square.
const SIZE_CHECK_GAP_MM: f64 = 3.0;

/// The size-check square of side `side`, below the lower left of `b`.
fn size_check(b: [f64; 4], side: f64) -> Region {
    let y = b[3] + SIZE_CHECK_GAP_MM;
    geom::rect(b[0], y, b[0] + side, y + side)
}

/// The finding that says what the square is for.
fn size_check_note(side: f64) -> Check {
    Check {
        level: crate::options::Level::Info,
        code: "sizeCheck",
        message: format!(
            "A {side:.1} mm square is cut below the design. Measure it once it is cut: if it is not {side:.1} mm, the cutter's program changed the size on import."
        ),
    }
}

/// A rectangular frame `gap` outside `b`, one bar wide.
fn weed_border(b: [f64; 4], gap: f64) -> Region {
    let outer = geom::rect(
        b[0] - gap - MARK_BAR_MM,
        b[1] - gap - MARK_BAR_MM,
        b[2] + gap + MARK_BAR_MM,
        b[3] + gap + MARK_BAR_MM,
    );
    let inner = geom::rect(b[0] - gap, b[1] - gap, b[2] + gap, b[3] + gap);
    geom::difference(&outer, &inner)
}

/// Colour of everything the preflight points at on the stage.
const PROBLEM_HEX: &str = "#e5484d";
/// Colour of the narrow waste gaps, told apart from thin parts.
const GAP_HEX: &str = "#f5a524";

/// Where the physical problems are, drawn over the sheets: every part narrower than the
/// minimum feature filled in red, every waste gap narrower than it in amber, every speck
/// ringed, so a warning in the list has a place
/// on the drawing.
fn problems_body(base: &[(String, String, Region)], o: &Options) -> String {
    let w = o.min_feature_mm;
    let speck = 4.0 * w * w;
    let mut body = String::new();
    for (_, _, r) in base {
        if !o.remove_thin {
            let thin = geom::difference(r, &geom::opening(r, w));
            let (d, _) = write::region_d(&thin, o.tolerance_mm.min(0.02));
            if !d.is_empty() {
                body.push_str(&format!(
                    "<path d=\"{d}\" fill=\"{PROBLEM_HEX}\" fill-rule=\"evenodd\"/>"
                ));
            }
        }
        let gaps = geom::difference(&geom::closing(r, w), r);
        let (d, _) = write::region_d(&gaps, o.tolerance_mm.min(0.02));
        if !d.is_empty() {
            body.push_str(&format!("<path d=\"{d}\" fill=\"{GAP_HEX}\"/>"));
        }
        for s in r.iter().filter(|s| geom::shape_area(s) < speck) {
            if let Some(b) = geom::bounds(&vec![s.clone()]) {
                let (cx, cy) = ((b[0] + b[2]) / 2.0, (b[1] + b[3]) / 2.0);
                let rad = ((b[2] - b[0]).hypot(b[3] - b[1]) / 2.0 + 1.0).max(1.5);
                body.push_str(&format!(
                    "<circle cx=\"{cx:.3}\" cy=\"{cy:.3}\" r=\"{rad:.3}\" fill=\"none\" stroke=\"{PROBLEM_HEX}\" stroke-width=\"0.35\"/>"
                ));
            }
        }
    }
    body
}

/// Every sheet in one file, each its own group in its own colour: what Cricut Design Space
/// and Silhouette Studio split into layers on import. Hairlines take LightBurn's layer
/// colours here, one per sheet, because in a laser program colour is the operation.
fn combined_body(sheets: &[(String, String, String)], o: &Options) -> String {
    sheets
        .iter()
        .enumerate()
        .map(|(i, (name, hex, d))| {
            let el = match o.cut_style {
                CutStyle::Filled => write::filled(d, hex),
                CutStyle::Hairline => write::hairline_in(d, LIGHTBURN[i % LIGHTBURN.len()]),
            };
            let id: String = name.chars().filter(|c| c.is_ascii_alphanumeric()).collect();
            format!("<g id=\"sheet-{}-{id}\">{el}</g>", i + 1)
        })
        .collect()
}

/// A sticker's printable face: every colour first spread by the print bleed and clipped to
/// the contour, then every colour as drawn on top. Inside the artwork the spreads are
/// covered; at its edge they carry the outer colours out under the cut line.
fn sticker_print(set: &[&ColourRegion], contour: &Region, o: &Options) -> String {
    let mut out = String::new();
    for c in set {
        let spread = geom::intersection(&geom::offset(&c.region, PRINT_BLEED_MM), contour);
        out.push_str(&write::filled(
            &write::region_d(&spread, o.tolerance_mm).0,
            &hex(c.rgb),
        ));
    }
    for c in set {
        out.push_str(&write::filled(
            &write::region_d(&c.region, o.tolerance_mm).0,
            &hex(c.rgb),
        ));
    }
    out
}

/// Where every document of one plan sits: its size and origin, millimetres.
struct Frame {
    size: [f64; 2],
    origin: [f64; 2],
}

/// Every sheet written: its file and numbers, the stacked preview's body, and each sheet's
/// path data for the combined file.
#[allow(clippy::type_complexity)]
fn write_sheets(
    mut sheet_regions: Vec<(String, String, Region)>,
    extents: &[[f64; 2]],
    frame: &Frame,
    print: &str,
    o: &Options,
    checks: &mut Vec<Check>,
) -> (Vec<Layer>, String, Vec<(String, String, String)>) {
    let (size, origin) = (frame.size, frame.origin);
    let mut layers = Vec::new();
    let mut preview = String::new();
    let mut drawn: Vec<(String, String, String)> = Vec::new();
    for (k, (name, colour, r)) in sheet_regions.drain(..).enumerate() {
        let (d, nodes) =
            write::region_d_ordered(&r, o.tolerance_mm, o.cut_style == CutStyle::Hairline);
        drawn.push((name.clone(), colour.clone(), d.clone()));
        let paths = r.iter().map(Vec::len).sum();
        checks.extend(preflight::node_checks(&name, paths, nodes));
        let element = match o.cut_style {
            CutStyle::Filled => write::filled(&d, &colour),
            CutStyle::Hairline => write::hairline(&d),
        };
        let body = if o.mode == Mode::Sticker {
            // The printable artwork under its contour, as print-and-cut expects.
            format!("{print}{}", write::hairline(&d))
        } else {
            element
        };
        preview.push_str(&if o.mode == Mode::Sticker {
            format!("<path d=\"{d}\" fill=\"#ffffff\" stroke=\"#c9754a\" stroke-width=\"0.3\" fill-rule=\"evenodd\"/>{print}")
        } else {
            write::filled(&d, &colour)
        });
        layers.push(Layer {
            svg: write::document(size, origin, &body),
            parts: r.len(),
            area_mm2: geom::area(&r),
            material_mm: extents.get(k).copied().unwrap_or([0.0, 0.0]),
            nodes,
            name,
            hex: colour,
        });
    }
    (layers, preview, drawn)
}

/// [`Mode::Lines`]: per colour, the line-shaped parts as single strokes along their
/// centre and everything else round its outline, all in the pen's width, lines ordered to
/// shorten pen-up travel. The preview draws each line at the width the drawing gave it.
fn plan_lines(
    set: &[&ColourRegion],
    canvas_mm: [f64; 2],
    mut checks: Vec<Check>,
    o: &Options,
) -> Plan {
    let pad = 1.0;
    let origin = [-pad, -pad];
    let side = o.size_check_mm.max(0.0);
    let check = (side > 0.0).then(|| {
        checks.push(size_check_note(side));
        size_check([0.0, 0.0, canvas_mm[0], canvas_mm[1]], side)
    });
    let below = if side > 0.0 {
        SIZE_CHECK_GAP_MM + side
    } else {
        0.0
    };
    let size = [
        canvas_mm[0].max(side) + 2.0 * pad,
        canvas_mm[1] + below + 2.0 * pad,
    ];
    let square = check
        .as_ref()
        .map(|r| write::region_d(r, o.tolerance_mm).0)
        .unwrap_or_default();
    let mut layers = Vec::new();
    let mut preview = String::new();
    let mut combined = String::new();
    let mut dxf_regions: Vec<(String, Region)> = Vec::new();
    let mut dxf_lines: Vec<Vec<Vec<geom::Pt>>> = Vec::new();
    let (mut total_lines, mut from_file) = (0, 0);
    for c in set {
        let (found, kept, own) =
            lines::colour_lines(&c.region, &c.strokes, o.max_line_mm, o.tolerance_mm);
        let found = lines::order(found);
        total_lines += found.len();
        from_file += own;
        let colour = hex(c.rgb);
        let (outline_d, outline_nodes) = write::region_d(&kept, o.tolerance_mm);
        let mut body = String::new();
        let mut shown = write::filled(&outline_d, &colour);
        if !outline_d.is_empty() {
            body.push_str(&write::pen(&outline_d, &colour, o.pen_mm));
        }
        if !square.is_empty() {
            body.push_str(&write::pen(&square, &colour, o.pen_mm));
            shown.push_str(&write::pen(&square, &colour, o.pen_mm));
        }
        let mut nodes = outline_nodes;
        let mut paths = Vec::new();
        for l in &found {
            let p = lines::simplify(&l.path, o.tolerance_mm);
            nodes += p.len().saturating_sub(1);
            let d = write::open_d(&p, l.closed);
            body.push_str(&write::pen(&d, &colour, o.pen_mm));
            shown.push_str(&write::pen(&d, &colour, l.width_mm));
            paths.push(p);
        }
        preview.push_str(&shown);
        let id = colour.trim_start_matches('#').to_string();
        combined.push_str(&format!("<g id=\"pen-{id}\">{body}</g>"));
        layers.push(Layer {
            svg: write::document(size, origin, &body),
            nodes,
            parts: kept.len() + found.len(),
            area_mm2: geom::area(&kept),
            material_mm: geom::bounds(&c.region).map_or([0.0, 0.0], |b| [b[2] - b[0], b[3] - b[1]]),
            name: colour.clone(),
            hex: colour.clone(),
        });
        dxf_regions.push((colour, kept));
        dxf_lines.push(paths);
    }
    if total_lines > 0 {
        let traced = total_lines - from_file;
        let source = match (from_file, traced) {
            (_, 0) => "all of them the file's own strokes".to_string(),
            (0, _) => "all of them found in filled shapes".to_string(),
            (f, t) => format!("{f} from the file's strokes, {t} found in filled shapes"),
        };
        checks.push(Check {
            level: crate::options::Level::Info,
            code: "lines",
            message: format!(
                "{total_lines} line(s) are drawn once along their centre instead of round both edges ({source})."
            ),
        });
    }
    let sheets: Vec<(String, &Region)> = dxf_regions.iter().map(|(n, r)| (n.clone(), r)).collect();
    let cam = dxf::cam_sheets(&sheets, &dxf_lines, o.tolerance_mm, origin[1] + size[1]);
    Plan {
        preview_svg: write::document(size, origin, &preview),
        problems_svg: write::document(size, origin, ""),
        combined_svg: write::document(size, origin, &combined),
        dxf: dxf::write(&cam),
        gcode: gcode::write(&cam, &gcode_settings(o, origin)),
        layers,
        size_mm: size,
        checks,
    }
}

/// Build the sheets for `o` from the artwork's visible colours.
pub fn plan(
    colours: &[ColourRegion],
    canvas_mm: [f64; 2],
    unsupported: &[String],
    o: &Options,
) -> Plan {
    let set = chosen(colours, o);
    let mut checks: Vec<Check> = preflight::colour_checks(&set, unsupported);
    if o.mode == Mode::Lines {
        return plan_lines(&set, canvas_mm, checks, o);
    }
    let mut base = sheets(&set, o, &mut checks);
    let mut dogbones = 0;
    for (k, (name, _, r)) in base.iter_mut().enumerate() {
        // A layered sheet is judged by what shows of it: its bleed lies hidden under the
        // colours above, and a bleed clipped round a narrow upper colour is a thin fringe
        // that the tests would flag although nobody weeds or sees it.
        let seen = match (o.mode, set.get(k)) {
            (Mode::Layered, Some(c)) => &c.region,
            _ => &*r,
        };
        checks.extend(preflight::layer_checks(
            name,
            seen,
            r,
            o.min_feature_mm,
            o.remove_thin,
        ));
        if o.remove_thin {
            *r = geom::opening(r, o.min_feature_mm);
        }
        if o.mirror {
            *r = geom::mirror_x(r, canvas_mm[0] / 2.0);
        }
        if o.kerf_mm > 0.0 {
            *r = geom::offset(r, o.kerf_mm / 2.0);
        }
        if o.dogbone_mm > 0.0 {
            let (relieved, n) = corners::dogbones(r, o.dogbone_mm);
            *r = relieved;
            dogbones += n;
            // Waste narrower than the bit is somewhere it cannot go at all.
            let bit = 2.0 * o.dogbone_mm;
            let tight = geom::area(&geom::opening(
                &geom::difference(&geom::closing(r, bit), r),
                o.tolerance_mm,
            ));
            if tight > 0.01 * bit * bit {
                checks.push(Check {
                    level: crate::options::Level::Warn,
                    code: "bitTooBig",
                    message: format!(
                        "{name}: {tight:.1} mm² of holes and gaps are narrower than the {bit:.2} mm bit, which cannot enter them."
                    ),
                });
            }
        }
    }
    if dogbones > 0 {
        checks.push(Check {
            level: crate::options::Level::Info,
            code: "dogbones",
            message: format!(
                "{dogbones} inside corner(s) relieved for a {:.2} mm bit, so parts cut with it seat square.",
                2.0 * o.dogbone_mm
            ),
        });
    }
    let problems = problems_body(&base, o);
    let print = match (o.mode, base.first()) {
        (Mode::Sticker, Some((_, _, contour))) => sticker_print(&set, contour, o),
        _ => String::new(),
    };
    // Everything the marks and borders are placed around.
    let art_bounds = geom::bounds(&geom::union_all(base.iter().map(|(_, _, r)| r))).unwrap_or([
        0.0,
        0.0,
        canvas_mm[0],
        canvas_mm[1],
    ]);
    let marks = (o.registration && base.len() > 1).then(|| registration_marks(art_bounds));
    let border = (o.weed_border_mm > 0.0).then(|| weed_border(art_bounds, o.weed_border_mm));
    let check = (o.size_check_mm > 0.0).then(|| {
        let around = [&marks, &border]
            .into_iter()
            .flatten()
            .filter_map(geom::bounds)
            .fold(art_bounds, |a, b| {
                [
                    a[0].min(b[0]),
                    a[1].min(b[1]),
                    a[2].max(b[2]),
                    a[3].max(b[3]),
                ]
            });
        checks.push(size_check_note(o.size_check_mm));
        size_check(around, o.size_check_mm)
    });
    // Each sheet's own extent, before the marks every sheet shares are added.
    let extents: Vec<[f64; 2]> = base
        .iter()
        .map(|(_, _, r)| geom::bounds(r).map_or([0.0, 0.0], |b| [b[2] - b[0], b[3] - b[1]]))
        .collect();
    let sheet_regions: Vec<(String, String, Region)> = base
        .into_iter()
        .map(|(n, h, r)| {
            let mut r = r;
            for extra in [&marks, &border, &check].into_iter().flatten() {
                r = geom::union(&r, extra);
            }
            (n, h, r)
        })
        .collect();
    let all = geom::union_all(sheet_regions.iter().map(|(_, _, r)| r));
    let b = geom::bounds(&all).unwrap_or([0.0, 0.0, canvas_mm[0], canvas_mm[1]]);
    let pad = 1.0;
    let origin = [b[0].min(0.0) - pad, b[1].min(0.0) - pad];
    let size = [
        b[2].max(canvas_mm[0]) + pad - origin[0],
        b[3].max(canvas_mm[1]) + pad - origin[1],
    ];

    let frame = Frame { size, origin };
    let dxf_sheets: Vec<(String, &Region)> = sheet_regions
        .iter()
        .map(|(n, _, r)| (n.clone(), r))
        .collect();
    let cam = dxf::cam_sheets(&dxf_sheets, &[], o.tolerance_mm, origin[1] + size[1]);
    drop(dxf_sheets);
    let dxf = dxf::write(&cam);
    let gcode = gcode::write(&cam, &gcode_settings(o, origin));
    let (layers, preview, drawn) =
        write_sheets(sheet_regions, &extents, &frame, &print, o, &mut checks);
    let combined = if o.mode == Mode::Sticker {
        layers.first().map(|l| l.svg.clone()).unwrap_or_default()
    } else {
        write::document(size, origin, &combined_body(&drawn, o))
    };
    Plan {
        preview_svg: write::document(size, origin, &preview),
        problems_svg: write::document(size, origin, &problems),
        combined_svg: combined,
        dxf,
        gcode,
        layers,
        size_mm: size,
        checks,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn colour(rgb: [u8; 3], r: Region) -> ColourRegion {
        ColourRegion {
            rgb,
            area_mm2: geom::area(&r),
            region: r,
            gradient: false,
            translucent: false,
            background: false,
            strokes: Vec::new(),
        }
    }

    #[test]
    fn a_hole_the_colour_above_covers_is_filled() {
        // A 20 mm square with a 4 mm hole, and a 6 mm square on top covering it.
        let base = geom::difference(
            &geom::rect(0.0, 0.0, 20.0, 20.0),
            &geom::rect(8.0, 8.0, 12.0, 12.0),
        );
        let top = geom::rect(7.0, 7.0, 13.0, 13.0);
        let filled = covered_holes(&base, &top);
        assert!((geom::area(&filled) - 16.0).abs() < 1e-6);
        // Not covered: nothing is filled.
        assert!(covered_holes(&base, &geom::rect(30.0, 30.0, 31.0, 31.0)).is_empty());
    }

    #[test]
    fn the_background_is_left_out_and_the_order_is_kept() {
        let mut page = colour([255, 255, 255], geom::rect(0.0, 0.0, 50.0, 50.0));
        page.background = true;
        let cs = vec![
            page,
            colour([255, 0, 0], geom::rect(5.0, 5.0, 20.0, 20.0)),
            colour([0, 0, 255], geom::rect(25.0, 25.0, 30.0, 30.0)),
        ];
        let o = Options {
            order: vec!["#0000ff".into(), "#ff0000".into()],
            ..Options::default()
        };
        let set = chosen(&cs, &o);
        assert_eq!(
            set.iter().map(|c| c.rgb).collect::<Vec<_>>(),
            vec![[0, 0, 255], [255, 0, 0]]
        );
    }

    #[test]
    fn marks_and_border_sit_outside_the_artwork() {
        let b = [0.0, 0.0, 40.0, 30.0];
        let marks = registration_marks(b);
        assert_eq!(marks.len(), 4, "four crosses");
        let mb = geom::bounds(&marks).unwrap();
        assert!(mb[0] < -MARK_GAP_MM && mb[2] > 40.0 + MARK_GAP_MM);
        let border = weed_border(b, 3.0);
        assert!(geom::intersection(&border, &geom::rect(0.0, 0.0, 40.0, 30.0)).is_empty());
    }

    #[test]
    fn kerf_grows_the_outline_and_shrinks_the_hole() {
        let ring = geom::difference(
            &geom::rect(0.0, 0.0, 20.0, 20.0),
            &geom::rect(5.0, 5.0, 15.0, 15.0),
        );
        let cs = vec![colour([0, 0, 0], ring)];
        let o = Options {
            kerf_mm: 0.4,
            registration: false,
            ..Options::default()
        };
        let p = plan(&cs, [20.0, 20.0], &[], &o);
        // Outer 20.4 wide (rounded corners), hole 9.6 wide: area grows by about the
        // perimeter times 0.2 on each side.
        let grown = p.layers[0].area_mm2;
        assert!(
            grown > 300.0 + 0.2 * 80.0 - 1.0 && grown < 300.0 + 0.2 * 120.0 + 1.0,
            "{grown}"
        );
    }
}
