//! Line-shaped parts as single lines: drawn, scored or engraved once, along their centre.
//!
//! A traced drawing of a line is a thin filled shape, and a cutter, pen or laser that
//! follows its outline goes round it — down one side and back up the other. "It cuts every
//! line twice" is the second most common complaint about traced files in cutter forums.
//! A pen, a scoring blade or a laser line wants the centreline and one width.
//!
//! The tracer already recovers exactly that from pixels (`inkvec_trace::centerline`: a
//! sub-pixel distance field, a Zhang–Suen skeleton reduced to a graph, a stroke width with
//! its own uncertainty, and a verdict of stroke-or-not made against that uncertainty). So
//! the region is rasterised here at about ten pixels a millimetre with exact horizontal
//! coverage, handed to the tracer's analysis, and its strokes come back in millimetres.
//! Every part the analysis does not call a stroke keeps its outline.

use inkvec_trace::centerline;
use inkvec_trace::coverage::{CoverageField, DEFAULT_SIGMA_MODEL};

use crate::geom::{self, Pt, Region};

/// One single line: its path, whether it closes, and the width the drawing gave it.
#[derive(Clone, Debug)]
pub struct Line {
    /// Centreline points, millimetres.
    pub path: Vec<Pt>,
    /// Whether it is a closed loop.
    pub closed: bool,
    /// The drawn width it replaces, millimetres.
    pub width_mm: f64,
}

/// Most pixels per millimetre a rasterisation uses.
const PX_PER_MM: f64 = 10.0;
/// Pixels across the widest candidate line. The analysis's distance queries grow with
/// the width in pixels, so a 7 mm stroke at ten pixels a millimetre costs seconds; a
/// sixteen pixels measures a width to a few hundredths of it and takes milliseconds.
const PX_ACROSS_WIDEST: f64 = 16.0;
/// Candidates are analysed in groups whose widths lie within this factor, each at its own
/// density: one sliver beside a 7 mm line would otherwise set ten pixels a millimetre for
/// the whole drawing and cost seconds (lucide `earth`: 14.5 s).
const WIDTH_BUCKET: f64 = 2.0;
/// Most pixels a rasterisation may use.
const MAX_PIXELS: f64 = 4.0e6;
/// Most the lines, stroked at their width, may differ from the parts they replace, as a
/// share of the parts' area. Measured on lucide traced from pixels, 79% of regions redraw
/// within it; the rest (a stub the skeleton pruned, an arrowhead's lost arm) are drawings
/// the recovered lines would visibly change, and keep their outline instead.
const MAX_MISFIT: f64 = 0.10;
/// A part is a line candidate only when its length is at least this many widths.
const MIN_SLENDERNESS: f64 = 3.0;
/// Sub-sample rows per pixel for coverage.
const SUB_ROWS: usize = 4;

/// Coverage of `r` on a grid of `k` pixels per millimetre whose pixel (0, 0) is centred at
/// `origin`: exact along x, `SUB_ROWS` samples along y, even-odd over every contour.
fn rasterise(r: &Region, origin: Pt, k: f64, w: usize, h: usize) -> Vec<f32> {
    let mut data = vec![0.0f32; w * h];
    let edges: Vec<(Pt, Pt)> = r
        .iter()
        .flatten()
        .flat_map(|c| (0..c.len()).map(move |i| (c[i], c[(i + 1) % c.len()])))
        .collect();
    let mut xs: Vec<f64> = Vec::new();
    for y in 0..h {
        for j in 0..SUB_ROWS {
            let py = y as f64 - 0.5 + (j as f64 + 0.5) / SUB_ROWS as f64;
            let my = origin[1] + py / k;
            xs.clear();
            for &(a, b) in &edges {
                if (a[1] > my) != (b[1] > my) {
                    let x = a[0] + (my - a[1]) / (b[1] - a[1]) * (b[0] - a[0]);
                    xs.push((x - origin[0]) * k + 0.5);
                }
            }
            xs.sort_by(f64::total_cmp);
            for [a, b] in xs.as_chunks::<2>().0 {
                let (x0, x1) = (a.max(0.0), b.min(w as f64));
                if x1 <= x0 {
                    continue;
                }
                let (i0, i1) = (x0.floor() as usize, (x1.ceil() as usize).min(w));
                for i in i0..i1 {
                    let cover = (x1.min(i as f64 + 1.0) - x0.max(i as f64)).max(0.0);
                    data[y * w + i] += (cover / SUB_ROWS as f64) as f32;
                }
            }
        }
    }
    data.iter_mut().for_each(|v| *v = v.clamp(0.0, 1.0));
    data
}

/// Every line one colour draws, and the parts that keep their outline.
///
/// Where the file drew strokes, their own centrelines are the answer and nothing has to be
/// recovered: they are exact, and they do not break where the drawing has junctions or
/// sharp turns (arrowheads, crossings), which is where recovering a centreline from a shape
/// is hardest. What the strokes do not cover (fills, or strokes wider than the limit) goes
/// to [`split`]. Returns the lines, the kept parts, and how many lines came from the file.
pub fn colour_lines(
    region: &Region,
    strokes: &[crate::load::Centreline],
    max_width_mm: f64,
    tolerance_mm: f64,
) -> (Vec<Line>, Region, usize) {
    let limit = if max_width_mm > 0.0 {
        max_width_mm
    } else {
        f64::INFINITY
    };
    let own: Vec<Line> = strokes
        .iter()
        .filter(|s| s.width_mm <= limit && s.path.len() >= 2)
        .map(|s| Line {
            path: s.path.clone(),
            closed: s.closed,
            width_mm: s.width_mm,
        })
        .collect();
    let rest = if own.is_empty() {
        region.clone()
    } else {
        // What the file's strokes draw, a little wider so the flattening leaves no seam;
        // then slivers thinner than that margin are dropped.
        let margin = 2.0 * tolerance_mm;
        let drawn = geom::union_all(
            own.iter()
                .map(|l| geom::stroke(&l.path, l.width_mm + 2.0 * margin, l.closed))
                .collect::<Vec<_>>()
                .iter(),
        );
        geom::opening(&geom::difference(region, &drawn), 2.0 * margin)
    };
    let n_own = own.len();
    let (found, kept) = split(&rest, max_width_mm);
    let mut lines = own;
    lines.extend(found);
    (lines, kept, n_own)
}

/// A shape's mean width and length read as a line: for a band of length `l` and width `w`
/// the area is `l w` and the perimeter `2 l`, so `w = 2 A / P` and `l = P / 2`.
fn band_measure(s: &geom::Shape) -> (f64, f64) {
    let perimeter: f64 = s
        .iter()
        .map(|c| {
            (0..c.len())
                .map(|i| {
                    let (a, b) = (c[i], c[(i + 1) % c.len()]);
                    (b[0] - a[0]).hypot(b[1] - a[1])
                })
                .sum::<f64>()
        })
        .sum();
    let area = geom::shape_area(s);
    (2.0 * area / perimeter.max(1e-12), perimeter / 2.0)
}

/// Split `r` into single lines (its parts no wider than `max_width_mm`, or any width when
/// it is 0, that the tracer's analysis calls strokes) and the parts that keep their outline.
pub fn split(r: &Region, max_width_mm: f64) -> (Vec<Line>, Region) {
    let limit = if max_width_mm > 0.0 {
        max_width_mm
    } else {
        f64::INFINITY
    };
    // Only slender parts no wider than the limit go to the analysis; blocks, discs and wide
    // parts keep their outline without being rasterised at all.
    let mut kept: Region = Vec::new();
    let mut candidates: Vec<(f64, geom::Shape)> = Vec::new();
    for s in r {
        let (w, l) = band_measure(s);
        if w > 0.0 && w <= limit * 1.25 && l >= MIN_SLENDERNESS * w {
            candidates.push((w, s.clone()));
        } else {
            kept.push(s.clone());
        }
    }
    candidates.sort_by(|a, b| b.0.total_cmp(&a.0));
    let mut lines = Vec::new();
    let mut start = 0;
    while start < candidates.len() {
        let wide = candidates[start].0;
        let end = candidates[start..]
            .iter()
            .position(|(w, _)| *w * WIDTH_BUCKET < wide)
            .map_or(candidates.len(), |n| start + n);
        let group: Region = candidates[start..end]
            .iter()
            .map(|(_, s)| s.clone())
            .collect();
        let (found, rest) = split_group(&group, wide, limit);
        lines.extend(found);
        kept.extend(rest);
        start = end;
    }
    (lines, kept)
}

/// [`split`] for candidate parts no wider than `wide`, rasterised so that `wide` spans
/// [`PX_ACROSS_WIDEST`] pixels.
fn split_group(r: &Region, wide: f64, limit: f64) -> (Vec<Line>, Region) {
    let Some(b) = geom::bounds(r) else {
        return (Vec::new(), Vec::new());
    };
    let (bw, bh) = (b[2] - b[0], b[3] - b[1]);
    let k = (PX_ACROSS_WIDEST / wide)
        .min(PX_PER_MM)
        .min((MAX_PIXELS / (bw * bh).max(1e-9)).sqrt())
        .max(0.5);
    let pad = 3.0 / k;
    let origin = [b[0] - pad, b[1] - pad];
    let w = ((bw + 2.0 * pad) * k).ceil() as usize + 1;
    let h = ((bh + 2.0 * pad) * k).ceil() as usize + 1;
    let field = CoverageField {
        width: w,
        height: h,
        data: rasterise(r, origin, k, w, h),
        sigma_alpha: 0.0,
        sigma_model: DEFAULT_SIGMA_MODEL,
        fg: [0.0; 3],
        bg: [1.0; 3],
        saturation: 1.0,
    };
    let labels = centerline::bilevel_labels(&field);
    let analysis = centerline::analyse_with(&field, &labels, w, h, centerline::GRAPH_CRITERIA);
    let to_mm = |p: inkvec_core::Point| [origin[0] + p.x / k, origin[1] + p.y / k];
    let label_at = |p: Pt| {
        label_near(
            &field,
            &labels,
            (p[0] - origin[0]) * k,
            (p[1] - origin[1]) * k,
        )
    };
    // Each candidate part goes with the raster region under an interior point of it.
    let part_label: Vec<Option<u16>> = r
        .iter()
        .map(|s| interior_point(s).and_then(label_at))
        .collect();
    let mut region_labels: Vec<u16> = analysis.strokes.iter().map(|s| s.label).collect();
    region_labels.sort_unstable();
    region_labels.dedup();

    let mut lines = Vec::new();
    let mut kept: Region = Vec::new();
    let mut drawn: Vec<u16> = Vec::new();
    for &label in &region_labels {
        let found: Vec<Line> = analysis
            .strokes
            .iter()
            .filter(|s| s.label == label && s.path.len() >= 2)
            .map(|s| Line {
                path: s.path.iter().map(|&p| to_mm(p)).collect(),
                closed: s.closed,
                width_mm: s.width / k,
            })
            .collect();
        if found.is_empty() || found.iter().any(|l| l.width_mm > limit) {
            continue;
        }
        // The lines must redraw the parts they replace: the lines stroked at their width,
        // against the parts, may differ by at most MAX_MISFIT of the parts' area.
        let parts: Region = r
            .iter()
            .zip(&part_label)
            .filter(|(_, l)| **l == Some(label))
            .map(|(s, _)| s.clone())
            .collect();
        let Some(misfit) = misfit(&parts, &found) else {
            continue;
        };
        if inkvec_core::env::flag("INKVEC_FAB_TIMING") {
            eprintln!(
                "  lines: region {label}: {} line(s), {:.2} mm wide, misfit {misfit:.3}",
                found.len(),
                found[0].width_mm
            );
        }
        if misfit > MAX_MISFIT {
            continue;
        }
        drawn.push(label);
        lines.extend(found);
    }
    kept.extend(
        r.iter()
            .zip(&part_label)
            .filter(|(_, l)| !l.is_some_and(|l| drawn.contains(&l)))
            .map(|(s, _)| s.clone()),
    );
    (lines, kept)
}

/// The label of the most-covered inked pixel next to pixel position (`x`, `y`): an interior
/// point of a thin part can round to a pixel the part only grazes.
fn label_near(field: &CoverageField, labels: &[u16], x: f64, y: f64) -> Option<u16> {
    let (w, h) = (field.width as isize, field.height as isize);
    let (xi, yi) = (x.floor() as isize, y.floor() as isize);
    let mut best: Option<(f32, u16)> = None;
    for (dx, dy) in [
        (0, 0),
        (1, 0),
        (0, 1),
        (1, 1),
        (-1, 0),
        (0, -1),
        (2, 0),
        (0, 2),
    ] {
        let (i, j) = (xi + dx, yi + dy);
        if i < 0 || j < 0 || i >= w || j >= h {
            continue;
        }
        let at = (j * w + i) as usize;
        let c = field.data[at];
        if c >= 0.5 && best.is_none_or(|(b, _)| c > b) {
            best = Some((c, labels[at]));
        }
    }
    best.map(|(_, l)| l)
}

/// How far `lines`, stroked at their widths, are from `parts`: the area of their symmetric
/// difference as a share of the parts' area. `None` when there are no parts.
fn misfit(parts: &Region, lines: &[Line]) -> Option<f64> {
    let area = geom::area(parts);
    if area <= 0.0 {
        return None;
    }
    let redrawn = geom::union_all(
        lines
            .iter()
            .map(|l| geom::stroke(&l.path, l.width_mm, l.closed))
            .collect::<Vec<_>>()
            .iter(),
    );
    let missed = geom::area(&geom::difference(parts, &redrawn));
    let spilled = geom::area(&geom::difference(&redrawn, parts));
    Some((missed + spilled) / area)
}

/// A point inside a shape: the midpoint of the widest span on a horizontal line through
/// its middle, which lies inside even for a ring or a crescent.
fn interior_point(s: &geom::Shape) -> Option<Pt> {
    let b = geom::bounds(&vec![s.clone()])?;
    let mut best: Option<(f64, Pt)> = None;
    for frac in [0.5, 0.3, 0.7, 0.2, 0.8] {
        let y = b[1] + frac * (b[3] - b[1]);
        let mut xs: Vec<f64> = Vec::new();
        for c in s {
            for i in 0..c.len() {
                let (a, q) = (c[i], c[(i + 1) % c.len()]);
                if (a[1] > y) != (q[1] > y) {
                    xs.push(a[0] + (y - a[1]) / (q[1] - a[1]) * (q[0] - a[0]));
                }
            }
        }
        xs.sort_by(f64::total_cmp);
        for [a, b] in xs.as_chunks::<2>().0 {
            let len = b - a;
            if best.is_none_or(|(l, _)| len > l) {
                best = Some((len, [(a + b) / 2.0, y]));
            }
        }
    }
    best.map(|(_, p)| p)
}

/// A path simplified to `tol` (Douglas-Peucker), ends kept. A loop's ends meet, and
/// distances to a chord of length zero are all zero, so a loop is first split at its point
/// farthest from the start and each half simplified on its own.
pub fn simplify(path: &[Pt], tol: f64) -> Vec<Pt> {
    fn dp(p: &[Pt], tol: f64, keep: &mut [bool], lo: usize, hi: usize) {
        if hi <= lo + 1 {
            return;
        }
        let (a, b) = (p[lo], p[hi]);
        let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
        let l = dx.hypot(dy).max(1e-12);
        let (mut worst, mut at) = (0.0, lo);
        for (i, q) in p.iter().enumerate().take(hi).skip(lo + 1) {
            let e = ((q[0] - a[0]) * dy - (q[1] - a[1]) * dx).abs() / l;
            if e > worst {
                worst = e;
                at = i;
            }
        }
        if worst > tol {
            keep[at] = true;
            dp(p, tol, keep, lo, at);
            dp(p, tol, keep, at, hi);
        }
    }
    if path.len() < 3 {
        return path.to_vec();
    }
    let mut keep = vec![false; path.len()];
    keep[0] = true;
    let last = path.len() - 1;
    keep[last] = true;
    let d0 = |q: &Pt| (q[0] - path[0][0]).hypot(q[1] - path[0][1]);
    if d0(&path[last]) <= tol {
        let far = (1..last)
            .max_by(|&i, &j| d0(&path[i]).total_cmp(&d0(&path[j])))
            .expect("at least three points");
        keep[far] = true;
        dp(path, tol, &mut keep, 0, far);
        dp(path, tol, &mut keep, far, last);
    } else {
        dp(path, tol, &mut keep, 0, last);
    }
    path.iter()
        .zip(keep)
        .filter(|(_, k)| *k)
        .map(|(p, _)| *p)
        .collect()
}

/// Order `lines` nearest-neighbour from the origin, reversing an open line when its far
/// end is the nearer: a pen plotter's pen-up travel, greedily shortened.
pub fn order(mut lines: Vec<Line>) -> Vec<Line> {
    let mut out = Vec::with_capacity(lines.len());
    let mut at = [0.0, 0.0];
    let d = |a: Pt, b: Pt| (a[0] - b[0]).hypot(a[1] - b[1]);
    while !lines.is_empty() {
        let (k, flip, _) = lines
            .iter()
            .enumerate()
            .map(|(k, l)| {
                let (s, e) = (l.path[0], l.path[l.path.len() - 1]);
                let (ds, de) = (d(at, s), d(at, e));
                if !l.closed && de < ds {
                    (k, true, de)
                } else {
                    (k, false, ds)
                }
            })
            .min_by(|x, y| x.2.total_cmp(&y.2))
            .expect("not empty");
        let mut l = lines.swap_remove(k);
        if flip {
            l.path.reverse();
        }
        at = if l.closed {
            l.path[0]
        } else {
            l.path[l.path.len() - 1]
        };
        out.push(l);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_drawn_line_becomes_one_centreline_and_a_block_keeps_its_outline() {
        // A 40 mm long, 0.8 mm wide bar and a 10 mm block.
        let bar = geom::rect(0.0, 10.0, 40.0, 10.8);
        let block = geom::rect(0.0, 20.0, 10.0, 30.0);
        let (lines, kept) = split(&geom::union(&bar, &block), 2.0);
        assert_eq!(lines.len(), 1, "one line");
        let l = &lines[0];
        assert!((l.width_mm - 0.8).abs() < 0.15, "width {}", l.width_mm);
        let ys: Vec<f64> = l.path.iter().map(|p| p[1]).collect();
        assert!(
            ys.iter().all(|y| (y - 10.4).abs() < 0.15),
            "along the middle"
        );
        assert_eq!(kept.len(), 1, "the block stays an outline");
        assert!((geom::area(&kept) - 100.0).abs() < 1.0);
    }

    #[test]
    fn a_short_stub_off_a_junction_is_still_a_line() {
        // A 1 mm wide bar, 30 mm long, with a stub 2 mm long hanging off its middle: every
        // edge between the junction and a free end is shorter than three widths but one.
        let bar = geom::rect(0.0, 10.0, 30.0, 11.0);
        let stub = geom::rect(14.5, 11.0, 15.5, 13.0);
        let (lines, kept) = split(&geom::union(&bar, &stub), 0.0);
        assert!(kept.is_empty(), "nothing left as an outline");
        assert!(lines.len() >= 2, "{} lines", lines.len());
        let lowest = lines
            .iter()
            .flat_map(|l| l.path.iter().map(|p| p[1]))
            .fold(f64::MIN, f64::max);
        assert!(lowest > 12.0, "the stub is drawn: reaches y {lowest}");
    }

    #[test]
    fn a_ring_is_one_closed_centreline() {
        let disc = |r: f64| -> Region {
            let n = 256;
            vec![vec![(0..n)
                .map(|i| {
                    let t = i as f64 / n as f64 * std::f64::consts::TAU;
                    [20.0 + r * t.cos(), 20.0 + r * t.sin()]
                })
                .collect()]]
        };
        let ring = geom::difference(&disc(10.0), &disc(8.5));
        let (lines, kept) = split(&ring, 0.0);
        assert!(kept.is_empty());
        assert_eq!(lines.len(), 1);
        let l = &lines[0];
        assert!(
            l.closed && (l.width_mm - 1.5).abs() < 0.1,
            "{} {}",
            l.closed,
            l.width_mm
        );
        assert!(l
            .path
            .iter()
            .all(|p| ((p[0] - 20.0).hypot(p[1] - 20.0) - 9.25).abs() < 0.1));
        // Simplified, it is still a ring and not a dot: its ends meet, and a chord of length
        // zero must not decide what is kept.
        let s = simplify(&l.path, 0.05);
        let extent = s
            .iter()
            .map(|p| (p[0] - s[0][0]).hypot(p[1] - s[0][1]))
            .fold(0.0, f64::max);
        assert!(
            s.len() > 8 && extent > 18.0,
            "{} points, extent {extent}",
            s.len()
        );
    }

    #[test]
    fn lines_are_ordered_to_shorten_travel() {
        let mk = |a: Pt, b: Pt| Line {
            path: vec![a, b],
            closed: false,
            width_mm: 0.5,
        };
        let out = order(vec![
            mk([50.0, 0.0], [60.0, 0.0]),
            mk([12.0, 0.0], [1.0, 0.0]),
        ]);
        assert_eq!(out[0].path[0], [1.0, 0.0], "nearest end first, reversed");
        assert_eq!(out[1].path[0], [50.0, 0.0]);
    }
}
