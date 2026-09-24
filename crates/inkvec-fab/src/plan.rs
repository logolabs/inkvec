//! From visible colours to sheets: the four modes, and what every sheet gets added.

use crate::geom::{self, Region};
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

/// Colour a stencil sheet is shown in: the blue-grey of Mylar.
const STENCIL_HEX: &str = "#5f7489";

/// Base sheets, bottom first, before marks: `(name, hex, region)`.
fn sheets(
    set: &[&ColourRegion],
    o: &Options,
    checks: &mut Vec<Check>,
) -> Vec<(String, String, Region)> {
    match o.mode {
        Mode::Stencil => {
            let all = geom::union_all(set.iter().map(|c| &c.region));
            let (sheet, bridges) = stencil::stencil(&all, o.stencil_margin_mm, o.bridge_mm);
            if !bridges.is_empty() {
                checks.push(Check {
                    level: crate::options::Level::Info,
                    code: "bridges",
                    message: format!(
                        "{} loose piece(s) would fall out of the stencil; each is held by a {} mm bridge.",
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
                let under = geom::intersection(&geom::offset(own, o.bleed_mm), &above);
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

/// Where the physical problems are, drawn over the sheets: every part narrower than the
/// minimum feature filled, and every speck ringed, so a warning in the list has a place
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
/// and Silhouette Studio split into layers on import. Hairlines keep their sheet's colour
/// here, because in a laser program colour is the operation.
fn combined_body(sheets: &[(String, String, String)], o: &Options) -> String {
    sheets
        .iter()
        .enumerate()
        .map(|(i, (name, hex, d))| {
            let el = match o.cut_style {
                CutStyle::Filled => write::filled(d, hex),
                CutStyle::Hairline => write::hairline_in(d, hex),
            };
            let id: String = name.chars().filter(|c| c.is_ascii_alphanumeric()).collect();
            format!("<g id=\"sheet-{}-{id}\">{el}</g>", i + 1)
        })
        .collect()
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
        let (d, nodes) = write::region_d(&r, o.tolerance_mm);
        drawn.push((name.clone(), colour.clone(), d.clone()));
        checks.extend(preflight::node_checks(&name, nodes));
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

/// Build the sheets for `o` from the artwork's visible colours.
pub fn plan(
    colours: &[ColourRegion],
    canvas_mm: [f64; 2],
    unsupported: &[String],
    o: &Options,
) -> Plan {
    let set = chosen(colours, o);
    // A sticker's printable face: the chosen colours, each filled, under the contour.
    let print: String = if o.mode == Mode::Sticker {
        set.iter()
            .map(|c| write::filled(&write::region_d(&c.region, o.tolerance_mm).0, &hex(c.rgb)))
            .collect()
    } else {
        String::new()
    };
    let mut checks: Vec<Check> = preflight::colour_checks(&set, unsupported);
    let mut base = sheets(&set, o, &mut checks);
    for (name, _, r) in &mut base {
        checks.extend(preflight::layer_checks(
            name,
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
    }
    let problems = problems_body(&base, o);
    // Everything the marks and borders are placed around.
    let art_bounds = geom::bounds(&geom::union_all(base.iter().map(|(_, _, r)| r))).unwrap_or([
        0.0,
        0.0,
        canvas_mm[0],
        canvas_mm[1],
    ]);
    let marks = (o.registration && base.len() > 1).then(|| registration_marks(art_bounds));
    let border = (o.weed_border_mm > 0.0).then(|| weed_border(art_bounds, o.weed_border_mm));
    // Each sheet's own extent, before the marks every sheet shares are added.
    let extents: Vec<[f64; 2]> = base
        .iter()
        .map(|(_, _, r)| geom::bounds(r).map_or([0.0, 0.0], |b| [b[2] - b[0], b[3] - b[1]]))
        .collect();
    let sheet_regions: Vec<(String, String, Region)> = base
        .into_iter()
        .map(|(n, h, r)| {
            let mut r = r;
            for extra in [&marks, &border].into_iter().flatten() {
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
