//! Regions of the plane as spans on horizontal scan lines.
//!
//! The layers stage compares sets: what a face shows, what is painted over it, what a
//! completed shape covers. A region here is its cross-section on the lines `y = y_k`,
//! `y_k = -0.5 + (k + ½)·DY` (pixel centres at integers, as everywhere in the emitter, and
//! four lines per pixel row): per line, a sorted list of disjoint open intervals. Each
//! cross-section is exact for the polygons it is built from (the crossings are computed,
//! not sampled), so union, intersection and difference are exact on every line; only what
//! happens between two lines is not seen, which is why the checks that use these regions
//! add a margin of a pixel and more (see [`super::check`]).

use inkvec_core::Point;

/// Scan-line spacing, px: four lines per pixel row.
pub(crate) const DY: f64 = 0.25;

/// The `y` of scan line `k`.
#[inline]
pub(crate) fn line_y(k: i32) -> f64 {
    -0.5 + (k as f64 + 0.5) * DY
}

/// The first scan line at or above `y` (the smallest `k` with `line_y(k) >= y`).
#[inline]
fn first_line_at_or_after(y: f64) -> i32 {
    ((y + 0.5) / DY - 0.5).ceil() as i32
}

/// A region: per scan line `k0 + i`, its spans `[x0, x1)`, sorted and disjoint (touching
/// spans are merged). Lines outside `k0 .. k0 + rows.len()` are empty.
#[derive(Clone, Debug, Default)]
pub(crate) struct Region {
    k0: i32,
    rows: Vec<Vec<(f64, f64)>>,
}

/// How crossings become spans: even-odd parity, or a non-zero winding number.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Rule {
    EvenOdd,
    #[cfg_attr(not(test), allow(dead_code))]
    NonZero,
}

impl Region {
    /// The empty region.
    pub(crate) fn empty() -> Self {
        Self::default()
    }

    /// True when no line has a span.
    pub(crate) fn is_empty(&self) -> bool {
        self.rows.iter().all(Vec::is_empty)
    }

    /// The spans of line `k`.
    pub(crate) fn line(&self, k: i32) -> &[(f64, f64)] {
        let i = k - self.k0;
        if i < 0 || i as usize >= self.rows.len() {
            return &[];
        }
        &self.rows[i as usize]
    }

    /// The range of lines that may hold spans.
    pub(crate) fn lines(&self) -> std::ops::Range<i32> {
        self.k0..self.k0 + self.rows.len() as i32
    }

    /// The region bounded by closed polygons (each a list of vertices, implicitly closed),
    /// under `rule`. Each edge is entered once per line it crosses, so the cost is the
    /// number of crossings, not lines × edges.
    pub(crate) fn from_polygons(polys: &[Vec<Point>], rule: Rule) -> Self {
        let (mut ymin, mut ymax) = (f64::INFINITY, f64::NEG_INFINITY);
        for p in polys.iter().flatten() {
            if p.y.is_finite() {
                ymin = ymin.min(p.y);
                ymax = ymax.max(p.y);
            }
        }
        if !(ymin <= ymax) {
            return Self::empty();
        }
        let k0 = first_line_at_or_after(ymin);
        let k1 = first_line_at_or_after(ymax); // exclusive: line_y(k1) >= ymax
        if k1 <= k0 {
            return Self::empty();
        }
        let n = (k1 - k0) as usize;
        let mut crossings: Vec<Vec<(f64, i32)>> = vec![Vec::new(); n];
        for poly in polys {
            let m = poly.len();
            if m < 3 {
                continue;
            }
            for i in 0..m {
                let (a, b) = (poly[i], poly[(i + 1) % m]);
                if a.y == b.y || !a.y.is_finite() || !b.y.is_finite() {
                    continue;
                }
                let (lo, hi, dir) = if a.y < b.y { (a, b, 1) } else { (b, a, -1) };
                // Lines with lo.y <= y < hi.y: half-open, so a vertex shared by two edges
                // is counted once.
                let ka = first_line_at_or_after(lo.y).max(k0);
                let kb = first_line_at_or_after(hi.y).min(k1);
                let inv = (hi.x - lo.x) / (hi.y - lo.y);
                for k in ka..kb {
                    let y = line_y(k);
                    if y < lo.y || y >= hi.y {
                        continue;
                    }
                    crossings[(k - k0) as usize].push((lo.x + (y - lo.y) * inv, dir));
                }
            }
        }
        let rows = crossings
            .into_iter()
            .map(|mut c| {
                c.sort_by(|a, b| a.0.total_cmp(&b.0));
                let mut spans = Vec::new();
                match rule {
                    Rule::EvenOdd => {
                        for pair in c.chunks_exact(2) {
                            push_span(&mut spans, pair[0].0, pair[1].0);
                        }
                    }
                    Rule::NonZero => {
                        let mut w = 0;
                        let mut start = 0.0;
                        for &(x, d) in &c {
                            let was = w != 0;
                            w += d;
                            if !was && w != 0 {
                                start = x;
                            } else if was && w == 0 {
                                push_span(&mut spans, start, x);
                            }
                        }
                    }
                }
                spans
            })
            .collect();
        Self { k0, rows }.trimmed()
    }

    /// Builds a region from spans added line by line in any order.
    pub(crate) fn from_line_spans(mut items: Vec<(i32, f64, f64)>) -> Self {
        items.retain(|&(_, a, b)| b > a && a.is_finite() && b.is_finite());
        if items.is_empty() {
            return Self::empty();
        }
        let k0 = items.iter().map(|t| t.0).min().unwrap_or(0);
        let k1 = items.iter().map(|t| t.0).max().unwrap_or(0) + 1;
        let mut rows: Vec<Vec<(f64, f64)>> = vec![Vec::new(); (k1 - k0) as usize];
        for (k, a, b) in items {
            rows[(k - k0) as usize].push((a, b));
        }
        for r in &mut rows {
            *r = normalized(std::mem::take(r));
        }
        Self { k0, rows }.trimmed()
    }

    /// Drops empty lines at either end.
    fn trimmed(mut self) -> Self {
        let first = self.rows.iter().position(|r| !r.is_empty());
        let Some(first) = first else {
            return Self::empty();
        };
        let last = self.rows.iter().rposition(|r| !r.is_empty()).unwrap_or(first);
        self.rows.truncate(last + 1);
        self.rows.drain(..first);
        self.k0 += first as i32;
        self
    }

    /// Applies `f` line by line over the union of both regions' line ranges.
    fn combine(&self, other: &Self, f: impl Fn(&[(f64, f64)], &[(f64, f64)]) -> Vec<(f64, f64)>) -> Self {
        let lo = self.lines().start.min(other.lines().start);
        let hi = self.lines().end.max(other.lines().end);
        if hi <= lo {
            return Self::empty();
        }
        let rows = (lo..hi).map(|k| f(self.line(k), other.line(k))).collect();
        Self { k0: lo, rows }.trimmed()
    }

    /// `self ∪ other`.
    pub(crate) fn union(&self, other: &Self) -> Self {
        if other.is_empty() {
            return self.clone();
        }
        if self.is_empty() {
            return other.clone();
        }
        self.combine(other, |a, b| {
            let mut v: Vec<(f64, f64)> = a.iter().chain(b.iter()).copied().collect();
            v.sort_by(|p, q| p.0.total_cmp(&q.0));
            normalized_sorted(v)
        })
    }

    /// `self ∩ other`.
    pub(crate) fn intersect(&self, other: &Self) -> Self {
        self.combine(other, intersect_spans)
    }

    /// `self \ other`.
    pub(crate) fn minus(&self, other: &Self) -> Self {
        if other.is_empty() {
            return self.clone();
        }
        self.combine(other, subtract_spans)
    }

    /// The region within the box `[x0, x1] × [y0, y1]`.
    pub(crate) fn crop(&self, x0: f64, y0: f64, x1: f64, y1: f64) -> Self {
        let ka = first_line_at_or_after(y0).max(self.lines().start);
        let kb = first_line_at_or_after(y1).min(self.lines().end);
        if kb <= ka {
            return Self::empty();
        }
        let rows = (ka..kb)
            .map(|k| intersect_spans(self.line(k), &[(x0, x1)]))
            .collect();
        Self { k0: ka, rows }.trimmed()
    }

    /// The region grown by `m` px: every span widened by `m` on each side, and every line
    /// joined by the lines within `m` of it. A square structuring element, which contains
    /// the disc of radius `m`.
    pub(crate) fn dilate(&self, m: f64) -> Self {
        if self.is_empty() || m <= 0.0 {
            return self.clone();
        }
        let r = (m / DY).round() as i32;
        let (lo, hi) = (self.lines().start - r, self.lines().end + r);
        let rows = (lo..hi)
            .map(|k| {
                let mut v: Vec<(f64, f64)> = ((k - r)..=(k + r))
                    .flat_map(|j| self.line(j).iter().map(|&(a, b)| (a - m, b + m)))
                    .collect();
                v.sort_by(|p, q| p.0.total_cmp(&q.0));
                normalized_sorted(v)
            })
            .collect();
        Self { k0: lo, rows }.trimmed()
    }

    /// The region shrunk by `m` px: a point stays when the square of half-side `m` about it
    /// lies inside (on every line within `m`, the span shrunk by `m` on each side).
    pub(crate) fn erode(&self, m: f64) -> Self {
        if self.is_empty() || m <= 0.0 {
            return self.clone();
        }
        let r = (m / DY).round() as i32;
        let (lo, hi) = (self.lines().start, self.lines().end);
        let shrunk = |k: i32| -> Vec<(f64, f64)> {
            self.line(k)
                .iter()
                .filter_map(|&(a, b)| (b - a > 2.0 * m).then_some((a + m, b - m)))
                .collect()
        };
        let rows = (lo..hi)
            .map(|k| {
                let mut acc = shrunk(k);
                for j in (k - r)..=(k + r) {
                    if j == k || acc.is_empty() {
                        continue;
                    }
                    acc = intersect_spans(&acc, &shrunk(j));
                }
                acc
            })
            .collect();
        Self { k0: lo, rows }.trimmed()
    }

    /// Area, px²: each line's span length times the line spacing.
    pub(crate) fn area(&self) -> f64 {
        self.rows
            .iter()
            .flatten()
            .map(|&(a, b)| b - a)
            .sum::<f64>()
            * DY
    }

    /// The longest total span length on any one line, px: how far a boundary is out at
    /// worst, summed over its crossings of that line.
    pub(crate) fn widest_line(&self) -> f64 {
        self.rows
            .iter()
            .map(|r| r.iter().map(|&(a, b)| b - a).sum::<f64>())
            .fold(0.0, f64::max)
    }

    /// Whether `p` is inside, read on the nearest scan line.
    pub(crate) fn contains(&self, p: Point) -> bool {
        let k = ((p.y + 0.5) / DY - 0.5).round() as i32;
        self.line(k).iter().any(|&(a, b)| a <= p.x && p.x < b)
    }

    /// The bounding box `(x0, y0, x1, y1)` of the spans, or `None` when empty. `y` is the
    /// first and last line's `y`, widened by half a line.
    pub(crate) fn bbox(&self) -> Option<(f64, f64, f64, f64)> {
        let (mut x0, mut x1) = (f64::INFINITY, f64::NEG_INFINITY);
        let (mut y0, mut y1) = (f64::INFINITY, f64::NEG_INFINITY);
        for (i, r) in self.rows.iter().enumerate() {
            if let (Some(f), Some(l)) = (r.first(), r.last()) {
                x0 = x0.min(f.0);
                x1 = x1.max(l.1);
                let y = line_y(self.k0 + i as i32);
                y0 = y0.min(y - 0.5 * DY);
                y1 = y1.max(y + 0.5 * DY);
            }
        }
        (x0 <= x1).then_some((x0, y0, x1, y1))
    }
}

/// Appends `[a, b)` to sorted, disjoint spans, merging with the last when they touch.
fn push_span(v: &mut Vec<(f64, f64)>, a: f64, b: f64) {
    if b <= a {
        return;
    }
    if let Some(last) = v.last_mut() {
        if a <= last.1 {
            last.1 = last.1.max(b);
            return;
        }
    }
    v.push((a, b));
}

/// Spans sorted by start, merged where they overlap or touch.
fn normalized_sorted(v: Vec<(f64, f64)>) -> Vec<(f64, f64)> {
    let mut out = Vec::with_capacity(v.len());
    for (a, b) in v {
        push_span(&mut out, a, b);
    }
    out
}

/// Spans in any order, sorted and merged.
fn normalized(mut v: Vec<(f64, f64)>) -> Vec<(f64, f64)> {
    v.sort_by(|p, q| p.0.total_cmp(&q.0));
    normalized_sorted(v)
}

/// The intersection of two sorted, disjoint span lists.
fn intersect_spans(a: &[(f64, f64)], b: &[(f64, f64)]) -> Vec<(f64, f64)> {
    let (mut i, mut j) = (0, 0);
    let mut out = Vec::new();
    while i < a.len() && j < b.len() {
        let lo = a[i].0.max(b[j].0);
        let hi = a[i].1.min(b[j].1);
        if hi > lo {
            out.push((lo, hi));
        }
        if a[i].1 < b[j].1 {
            i += 1;
        } else {
            j += 1;
        }
    }
    out
}

/// `a \ b` for two sorted, disjoint span lists.
fn subtract_spans(a: &[(f64, f64)], b: &[(f64, f64)]) -> Vec<(f64, f64)> {
    let mut out = Vec::new();
    let mut j = 0;
    for &(mut lo, hi) in a {
        while j < b.len() && b[j].1 <= lo {
            j += 1;
        }
        let mut jj = j;
        while jj < b.len() && b[jj].0 < hi {
            if b[jj].0 > lo {
                out.push((lo, b[jj].0));
            }
            lo = lo.max(b[jj].1);
            if lo >= hi {
                break;
            }
            jj += 1;
        }
        if lo < hi {
            out.push((lo, hi));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn square(x0: f64, y0: f64, x1: f64, y1: f64) -> Vec<Point> {
        vec![
            Point::new(x0, y0),
            Point::new(x1, y0),
            Point::new(x1, y1),
            Point::new(x0, y1),
        ]
    }

    #[test]
    fn a_square_has_its_area() {
        let r = Region::from_polygons(&[square(0.0, 0.0, 10.0, 4.0)], Rule::EvenOdd);
        assert!((r.area() - 40.0).abs() < 1e-9, "{}", r.area());
    }

    #[test]
    fn set_operations_are_exact_on_each_line() {
        let a = Region::from_polygons(&[square(0.0, 0.0, 10.0, 10.0)], Rule::EvenOdd);
        let b = Region::from_polygons(&[square(5.0, 5.0, 15.0, 15.0)], Rule::EvenOdd);
        assert!((a.union(&b).area() - 175.0).abs() < 1e-9);
        assert!((a.intersect(&b).area() - 25.0).abs() < 1e-9);
        assert!((a.minus(&b).area() - 75.0).abs() < 1e-9);
    }

    #[test]
    fn even_odd_makes_a_hole_and_non_zero_does_not_when_wound_alike() {
        let outer = square(0.0, 0.0, 10.0, 10.0);
        let inner = square(2.0, 2.0, 8.0, 8.0);
        let eo = Region::from_polygons(&[outer.clone(), inner.clone()], Rule::EvenOdd);
        assert!((eo.area() - 64.0).abs() < 1e-9);
        let nz = Region::from_polygons(&[outer, inner], Rule::NonZero);
        assert!((nz.area() - 100.0).abs() < 1e-9);
    }

    #[test]
    fn erosion_and_dilation_move_the_boundary_by_the_margin() {
        let a = Region::from_polygons(&[square(0.0, 0.0, 10.0, 10.0)], Rule::EvenOdd);
        let e = a.erode(1.0);
        assert!((e.area() - 64.0).abs() < 1.0, "{}", e.area());
        let d = a.dilate(1.0);
        assert!((d.area() - 144.0).abs() < 1.5, "{}", d.area());
        assert!(e.minus(&a).is_empty());
        assert!(a.minus(&d).is_empty());
    }

    #[test]
    fn contains_reads_the_nearest_line() {
        let a = Region::from_polygons(&[square(0.0, 0.0, 10.0, 10.0)], Rule::EvenOdd);
        assert!(a.contains(Point::new(5.0, 5.0)));
        assert!(!a.contains(Point::new(11.0, 5.0)));
    }
}
