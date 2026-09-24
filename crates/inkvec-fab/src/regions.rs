//! From painted items to what can be cut: the visible area of each colour.
//!
//! An SVG paints in order and later paint hides earlier paint, but a sheet of vinyl has no
//! "under": a shape that is half covered is cut whole, the covered half is cut twice, and
//! the knife lifts the vinyl where it crosses. So each item is first cut back to what shows
//! — every item minus the union of everything above it — and the visible pieces of one
//! colour are joined. On the tracer's own output this changes nothing (its faces already
//! tile the canvas); on a drawing made in an editor it is the step every cutting tutorial
//! tells people to do by hand with Weld and Slice.
//!
//! Colours closer than a threshold in CIELAB are one colour: nobody owns two vinyls that
//! differ by a JND, and a trace can carry both.

use crate::geom::{self, Region};
use crate::load::{Artwork, Centreline, PaintKind};

/// Everything visible in one colour.
#[derive(Clone, Debug)]
pub struct ColourRegion {
    /// The colour, sRGB bytes.
    pub rgb: [u8; 3],
    /// Its visible area, in millimetres.
    pub region: Region,
    /// Visible area in square millimetres.
    pub area_mm2: f64,
    /// True when any of it was a gradient or pattern.
    pub gradient: bool,
    /// True when any of it was translucent.
    pub translucent: bool,
    /// True when it looks like the page rather than the artwork: it spans the canvas.
    pub background: bool,
    /// The centrelines of this colour's strokes, clipped to where the colour shows.
    pub strokes: Vec<Centreline>,
}

fn lab(rgb: [u8; 3]) -> [f64; 3] {
    let lin = |c: u8| {
        let c = c as f64 / 255.0;
        if c <= 0.04045 {
            c / 12.92
        } else {
            ((c + 0.055) / 1.055).powf(2.4)
        }
    };
    let (r, g, b) = (lin(rgb[0]), lin(rgb[1]), lin(rgb[2]));
    let x = (0.4124 * r + 0.3576 * g + 0.1805 * b) / 0.95047;
    let y = 0.2126 * r + 0.7152 * g + 0.0722 * b;
    let z = (0.0193 * r + 0.1192 * g + 0.9505 * b) / 1.08883;
    let f = |t: f64| {
        if t > 0.008856 {
            t.cbrt()
        } else {
            7.787 * t + 16.0 / 116.0
        }
    };
    let (fx, fy, fz) = (f(x), f(y), f(z));
    [116.0 * fy - 16.0, 500.0 * (fx - fy), 200.0 * (fy - fz)]
}

/// CIEDE2000 colour difference (Sharma, Wu and Dalal's formulation), the measure the tracer
/// uses: CIE76 merges dark colours that look different and splits blues that look alike.
pub fn delta_e(a: [u8; 3], b: [u8; 3]) -> f64 {
    let (p, q) = (lab(a), lab(b));
    let (l1, a1, b1) = (p[0], p[1], p[2]);
    let (l2, a2, b2) = (q[0], q[1], q[2]);
    let c1 = a1.hypot(b1);
    let c2 = a2.hypot(b2);
    let cm = (c1 + c2) / 2.0;
    let g = 0.5 * (1.0 - (cm.powi(7) / (cm.powi(7) + 25f64.powi(7))).sqrt());
    let (a1p, a2p) = ((1.0 + g) * a1, (1.0 + g) * a2);
    let (c1p, c2p) = (a1p.hypot(b1), a2p.hypot(b2));
    let hue = |bb: f64, aa: f64| {
        if bb == 0.0 && aa == 0.0 {
            0.0
        } else {
            bb.atan2(aa).to_degrees().rem_euclid(360.0)
        }
    };
    let (h1p, h2p) = (hue(b1, a1p), hue(b2, a2p));
    let dlp = l2 - l1;
    let dcp = c2p - c1p;
    let dhp = if c1p * c2p == 0.0 {
        0.0
    } else if (h2p - h1p).abs() <= 180.0 {
        h2p - h1p
    } else if h2p - h1p > 180.0 {
        h2p - h1p - 360.0
    } else {
        h2p - h1p + 360.0
    };
    let dhp_big = 2.0 * (c1p * c2p).sqrt() * (dhp.to_radians() / 2.0).sin();
    let lpm = (l1 + l2) / 2.0;
    let cpm = (c1p + c2p) / 2.0;
    let hpm = if c1p * c2p == 0.0 {
        h1p + h2p
    } else if (h1p - h2p).abs() <= 180.0 {
        (h1p + h2p) / 2.0
    } else if h1p + h2p < 360.0 {
        (h1p + h2p + 360.0) / 2.0
    } else {
        (h1p + h2p - 360.0) / 2.0
    };
    let t = 1.0 - 0.17 * (hpm - 30.0).to_radians().cos()
        + 0.24 * (2.0 * hpm).to_radians().cos()
        + 0.32 * (3.0 * hpm + 6.0).to_radians().cos()
        - 0.20 * (4.0 * hpm - 63.0).to_radians().cos();
    let dtheta = 30.0 * (-((hpm - 275.0) / 25.0).powi(2)).exp();
    let rc = 2.0 * (cpm.powi(7) / (cpm.powi(7) + 25f64.powi(7))).sqrt();
    let sl = 1.0 + 0.015 * (lpm - 50.0).powi(2) / (20.0 + (lpm - 50.0).powi(2)).sqrt();
    let sc = 1.0 + 0.045 * cpm;
    let sh = 1.0 + 0.015 * cpm * t;
    let rt = -(2.0 * dtheta).to_radians().sin() * rc;
    ((dlp / sl).powi(2)
        + (dcp / sc).powi(2)
        + (dhp_big / sh).powi(2)
        + rt * (dcp / sc) * (dhp_big / sh))
        .sqrt()
}

/// Lightness, 0 (black) to 100 (white).
pub fn lightness(rgb: [u8; 3]) -> f64 {
    lab(rgb)[0]
}

/// `#rrggbb`.
pub fn hex(rgb: [u8; 3]) -> String {
    format!("#{:02x}{:02x}{:02x}", rgb[0], rgb[1], rgb[2])
}

/// The visible area of every colour, colours within `merge_de` of each other joined,
/// largest first.
pub fn visible_colours(art: &Artwork, merge_de: f64) -> Vec<ColourRegion> {
    // Top down: what each item shows is what the items above it have not covered.
    let mut covered: Region = Vec::new();
    let mut shown: Vec<(usize, Region)> = Vec::new();
    for (i, item) in art.items.iter().enumerate().rev() {
        let vis = geom::difference(&item.region, &covered);
        covered = geom::union(&covered, &item.region);
        if geom::area(&vis) > 1e-9 {
            shown.push((i, vis));
        }
    }
    shown.reverse();
    // Group by colour, first come first served.
    let mut out: Vec<(ColourRegion, Vec<Region>, Vec<Centreline>)> = Vec::new();
    for (i, vis) in shown {
        let item = &art.items[i];
        let slot = out
            .iter()
            .position(|(c, _, _)| delta_e(c.rgb, item.rgb) <= merge_de);
        let (c, parts, lines) = match slot {
            Some(k) => &mut out[k],
            None => {
                out.push((
                    ColourRegion {
                        rgb: item.rgb,
                        region: Vec::new(),
                        area_mm2: 0.0,
                        gradient: false,
                        translucent: false,
                        background: false,
                        strokes: Vec::new(),
                    },
                    Vec::new(),
                    Vec::new(),
                ));
                out.last_mut().expect("just pushed")
            }
        };
        c.gradient |= item.kind != PaintKind::Solid;
        c.translucent |= item.opacity < 0.999;
        parts.push(vis);
        lines.extend(item.centrelines.iter().cloned());
    }
    let canvas = art.size_mm;
    let mut colours: Vec<ColourRegion> = out
        .into_iter()
        .map(|(mut c, parts, lines)| {
            c.region = geom::union_all(parts.iter());
            // Clipped to the colour as a whole, not to the item: where two strokes of one
            // colour cross, the lower is covered by the upper and a pen still draws both.
            c.strokes = lines
                .iter()
                .flat_map(|l| {
                    geom::clip_line(&l.path, l.closed, &c.region)
                        .into_iter()
                        .map(move |(path, closed)| Centreline {
                            path,
                            closed,
                            width_mm: l.width_mm,
                        })
                })
                .collect();
            c.area_mm2 = geom::area(&c.region);
            c.background = spans_canvas(&c.region, canvas);
            c
        })
        .collect();
    colours.sort_by(|a, b| b.area_mm2.total_cmp(&a.area_mm2));
    // Only one colour can be the page: the largest that spans it.
    let mut seen = false;
    for c in &mut colours {
        if c.background && seen {
            c.background = false;
        }
        seen |= c.background;
    }
    colours
}

/// The point `d` along the border of the rectangle `inset..inset + w` by `inset..inset + h`,
/// clockwise from its top-left corner.
fn border_point(d: f64, inset: f64, w: f64, h: f64) -> [f64; 2] {
    if d < w {
        [inset + d, inset]
    } else if d < w + h {
        [inset + w, inset + d - w]
    } else if d < 2.0 * w + h {
        [inset + w - (d - w - h), inset + h]
    } else {
        [inset, inset + h - (d - 2.0 * w - h)]
    }
}

/// Share of the canvas border a background must cover.
const BACKGROUND_BORDER_SHARE: f64 = 0.75;

/// True when a region runs along most of the canvas border: the page, not a shape. Reaching
/// all four sides is not enough — a round logo touches every side at one point each.
fn spans_canvas(r: &Region, canvas: [f64; 2]) -> bool {
    let inset = 0.005 * canvas[0].max(canvas[1]);
    let (w, h) = (canvas[0] - 2.0 * inset, canvas[1] - 2.0 * inset);
    let n = 400;
    let perimeter = 2.0 * (w + h);
    let mut inside = 0;
    for k in 0..n {
        let p = border_point(perimeter * (k as f64 + 0.5) / n as f64, inset, w, h);
        if geom::contains(r, p) {
            inside += 1;
        }
    }
    inside as f64 >= BACKGROUND_BORDER_SHARE * n as f64
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::load::load;

    #[test]
    fn overlaps_are_cut_back_and_the_page_is_found() {
        let svg = r##"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100">
            <rect width="100" height="100" fill="#ffffff"/>
            <rect x="10" y="10" width="50" height="50" fill="#ff0000"/>
            <rect x="30" y="30" width="50" height="50" fill="#0000ff"/>
            <rect x="85" y="85" width="10" height="10" fill="#fe0101"/></svg>"##;
        let art = load(svg, 100.0, 0.01).unwrap();
        let cs = visible_colours(&art, 3.0);
        assert_eq!(cs.len(), 3, "the two reds are one colour");
        let red = cs.iter().find(|c| c.rgb == [255, 0, 0]).unwrap();
        // 2500 minus the 900 the blue covers, plus the 100 of the near-red square.
        assert!((red.area_mm2 - 1700.0).abs() < 1.0, "{}", red.area_mm2);
        assert!(
            cs.iter()
                .find(|c| c.rgb == [255, 255, 255])
                .unwrap()
                .background
        );
        assert!(!red.background);
    }

    #[test]
    fn a_round_logo_that_touches_every_edge_is_not_the_page() {
        let svg = r##"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100">
            <circle cx="50" cy="50" r="50" fill="#000000"/></svg>"##;
        let art = load(svg, 100.0, 0.01).unwrap();
        assert!(!visible_colours(&art, 3.0)[0].background);
    }

    #[test]
    fn ciede2000_matches_a_published_pair() {
        // Sharma et al.'s test data are in Lab; check the formula's shape on sRGB instead:
        // identical colours are 0, and pure red to pure blue is about 52.9 (reference value).
        assert!(delta_e([12, 34, 56], [12, 34, 56]).abs() < 1e-9);
        let d = delta_e([255, 0, 0], [0, 0, 255]);
        assert!((d - 52.88).abs() < 0.5, "{d}");
    }
}
