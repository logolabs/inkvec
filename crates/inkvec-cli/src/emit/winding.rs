//! Winding every ring of a compound path by its nesting depth, so the path fills the same
//! under the `nonzero` rule as under `evenodd`, and needs no `fill-rule` at all.
//!
//! # The problem
//!
//! The emitters write a face, its same-coloured siblings and the holes punched in them as
//! one compound path, and until 2026-10 relied on `fill-rule="evenodd"` to make the holes
//! holes: every ring was written in whatever direction the planar map's walk gave it, and
//! a hole usually ran the same way as the outline around it. Under `nonzero` -- SVG's
//! default, the only rule of TrueType and OpenType glyph outlines, of Fontello and the
//! icon-font chain, of many cutter and CAD importers, and of Android vector drawables before
//! API 24 -- such a hole fills back in. On the 246-icon screen set, 73 files (30%) rendered
//! differently under `nonzero` (r2-output, 2026-10-02); the artist's own files, 1.
//!
//! # The method
//!
//! SVG 1.1 §11.3 defines both rules on one ray cast from a point: `nonzero` counts +1 for
//! every crossing of a ring running one way and −1 for the other way, `evenodd` counts the
//! crossings. For rings that do not cross each other -- the rings of a planar partition,
//! which is what every emitter here writes -- a point inside `k` nested rings is crossed
//! once by each of them, so if the ring at depth `d` runs the way `(−1)^d` says, the
//! winding number is `1 − 1 + 1 − …` over `k` terms: 1 for odd `k`, 0 for even. That is
//! the even-odd answer at every point, so the two rules agree everywhere and the attribute
//! says nothing. (Where two rings of one path overlap, which a partition does not do but a
//! pair of fitted curves can by a hair, no direction makes the rules agree: see below.)
//! This is the convention of glyph outlines -- outer contours one way, the counters inside
//! them the other (Apple, "TrueType Reference Manual", ch. 1: "TrueType uses the non-zero
//! winding number rule", and the contour direction determines what is filled).
//!
//! Here, in the SVG's own y-down coordinates, a ring at even depth runs with a negative
//! shoelace sum `Σ (x_k·y_(k+1) − x_(k+1)·y_k)` (anticlockwise on the screen) and one at
//! odd depth with a positive sum. Even depth is negative because that is the way the
//! planar map already walks a face's outline: on the screen set 2,107 of 2,123 outer rings
//! ran that way, so this convention reverses 154 rings where the opposite would reverse
//! 2,152.
//!
//! # The passes, per compound path
//!
//! 1. **Read** the `d` as the emitters write it ([`parse`]): absolute `M`, `L`, `C`, `S`,
//!    `A`, `Z`, numbers kept as their own text. Anything else -- a relative command, an
//!    implicit repeat, a subpath that does not start with `M` -- and the path is returned
//!    unread, so its element keeps `fill-rule="evenodd"`: the fallback is the old output,
//!    never a wrong picture.
//! 2. **Measure** each ring on the polygon that follows its curves
//!    ([`crate::rings::ring_points`]): its signed area, and, when there are other rings,
//!    up to five points strictly inside it ([`crate::rings::interior_probes`]).
//! 3. **Depth parity.** A ring's depth is the number of rings of the same path that
//!    contain it. Only its parity matters, and a larger ring contains a probe exactly when
//!    a ray from the probe crosses it an odd number of times, so the parity is the parity
//!    of all crossings with the larger rings' edges together ([`DepthIndex`]). The
//!    majority over the probes decides, the rule [`crate::rings::ring_inside`] uses.
//! 4. **Write** each ring that runs the wrong way reversed ([`write_reversed`]) and every
//!    other ring as its original text, byte for byte.
//!
//! # Why the picture does not change
//!
//! Reversing a ring changes the order and direction of its segments and nothing else: every
//! coordinate is the text the emitter wrote, a cubic's control points swap, and an arc keeps
//! its radii, rotation and large-arc flag with the sweep flag flipped, which by SVG 1.1 F.6.5
//! selects the same centre and so the same arc (swapping the ends negates the half-chord
//! and flipping the sweep negates the sign that picks the centre). An `S` the emitter wrote
//! is expanded with its reflected control point computed in exact decimal arithmetic, and
//! the reversed ring writes `S` only where that reflection holds exactly, so the curve a
//! reader rebuilds is the curve that was written.
//!
//! Measured on the 246-icon screen set at 512 px (2026-10-02), against v0.2.4 rendered under
//! `evenodd`:
//!
//! * the wound files with `evenodd` forced back on render identically in resvg, 246 of 246:
//!   the reversal moves nothing;
//! * under the default rule, 237 of 246 are identical in resvg. The other 9 differ in 1 to
//!   100 pixels of 262,144, all where two rings of one path overlap by a sliver -- two
//!   same-coloured siblings whose fitted outlines cross where they touch. Even-odd cut the
//!   overlap out and showed the ground through it; the wound path paints it, which is
//!   closer to the artist's file in 6 of the 9 (the gate: 3 icons better, 1 worse by
//!   0.001 dE00). No ring direction can make two overlapping rings agree under both rules;
//! * Chromium (Skia) agrees on 215 of 246; its anti-aliasing depends on edge direction, so
//!   12 files differ by at most 10 pixels even with `evenodd` forced, and the rest by at most
//!   8/255 outside the same 9 overlaps;
//! * read under `nonzero`, v0.2.4's files differed from themselves in 85 of 246 (2.95
//!   million pixels; every hole filled); the wound files in those 9 slivers only.
//!
//! Method from: W3C, "Scalable Vector Graphics (SVG) 1.1 (Second Edition)", 2011, §11.3
//! (`fill-rule`) and Appendix F.6 (arc parameterisation), <https://www.w3.org/TR/SVG11/>.
//! Inspired by: the font formats' contour-direction rule (Apple, "TrueType Reference
//! Manual", ch. 1, <https://developer.apple.com/fonts/TrueType-Reference-Manual/RM01/Chap1.html>)
//! and Fontello's advice for icon fonts ("fill is defined by contour direction",
//! <https://github.com/fontello/fontello/wiki/How-to-use-custom-images>). The depth-parity
//! ray cast is the crossing-number test of [`crate::rings::point_in_ring`] summed over
//! rings. See also: `inkvec-fab`'s `region_d`, which keeps the opposite windings its
//! polygon clipper produces for the same reason.

use std::borrow::Cow;

use inkvec_core::Point;
use inkvec_fit::{curves::Segment, FittedPath};

use crate::rings::{interior_probes, ring_points};

/// The attribute an element needs after [`for_nonzero`]: nothing when the rings were
/// wound, `evenodd` when the path could not be read and keeps its old meaning.
pub(crate) const EVENODD: &str = " fill-rule=\"evenodd\"";

/// `d` with every ring wound by its nesting depth, and the `fill-rule` attribute the element
/// still needs: empty when the rings were wound (the default `nonzero` then fills what
/// `evenodd` did), [`EVENODD`] when `d` holds something [`parse`] does not read, in which
/// case `d` comes back unchanged.
///
/// Rings already running the right way are copied byte for byte, so a path that needed no
/// change is returned borrowed. An empty `d` draws nothing under either rule and needs no
/// attribute.
pub(crate) fn for_nonzero(d: &str) -> (Cow<'_, str>, &'static str) {
    if d.trim().is_empty() {
        return (Cow::Borrowed(d), "");
    }
    match wind_by_depth(d) {
        Some(w) => (w, ""),
        None => (Cow::Borrowed(d), EVENODD),
    }
}

/// [`for_nonzero`]'s work: `None` when `d` cannot be read.
pub(crate) fn wind_by_depth(d: &str) -> Option<Cow<'_, str>> {
    let subs = parse(d)?;
    let rings: Vec<Measured> = subs.iter().map(measure).collect();
    let odd = depth_parity(&rings);
    // A ring at even depth runs with a negative shoelace sum, one at odd depth positive.
    // A ring enclosing nothing has no direction worth keeping and is left as it is.
    let flip: Vec<bool> = rings
        .iter()
        .zip(&odd)
        .map(|(r, &o)| r.signed != 0.0 && (r.signed > 0.0) != o)
        .collect();
    if !flip.iter().any(|&f| f) {
        return Some(Cow::Borrowed(d));
    }
    let mut out = String::with_capacity(d.len() + 8);
    for (s, &f) in subs.iter().zip(&flip) {
        if f {
            write_reversed(s, &mut out);
        } else {
            out.push_str(s.text);
        }
    }
    Some(Cow::Owned(out))
}

// ---------------------------------------------------------------------------------------
// Reading

/// One number as the emitter wrote it: its text, which is copied when the ring is written
/// again, and its value, for the geometry.
#[derive(Clone, Debug, PartialEq)]
struct Num<'a> {
    /// The digits, verbatim, or the exact decimal text of a computed reflection.
    text: Cow<'a, str>,
    /// The value of `text`.
    value: f64,
}

/// A point as two written numbers.
type Pt<'a> = [Num<'a>; 2];

/// One segment of a ring, absolute, without its start (the previous segment's end).
#[derive(Clone, Debug)]
enum Seg<'a> {
    /// A straight line to the point.
    Line(Pt<'a>),
    /// A cubic: first control, second control, end. An `S` is stored with its first
    /// control computed ([`reflect`]).
    Cubic(Pt<'a>, Pt<'a>, Pt<'a>),
    /// An elliptical arc, SVG 1.1 §8.3.8.
    Arc {
        /// x radius.
        rx: Num<'a>,
        /// y radius.
        ry: Num<'a>,
        /// x-axis rotation, degrees.
        rot: Num<'a>,
        /// Large-arc flag.
        large: bool,
        /// Sweep flag.
        sweep: bool,
        /// End point.
        end: Pt<'a>,
    },
}

impl<'a> Seg<'a> {
    /// Where the segment ends.
    fn end(&self) -> &Pt<'a> {
        match self {
            Seg::Line(p) | Seg::Cubic(_, _, p) | Seg::Arc { end: p, .. } => p,
        }
    }
}

/// One ring as written: its text in `d`, its start, its segments, and whether a `Z` closes
/// it.
#[derive(Debug)]
struct Sub<'a> {
    /// The ring's own text, from its `M` up to the next ring's `M` (or the end).
    text: &'a str,
    /// The `M` point.
    start: Pt<'a>,
    /// The segments, each starting where the one before ended. Never empty.
    segs: Vec<Seg<'a>>,
    /// Ended by `Z`.
    closed: bool,
}

/// A cursor over path data that hands out command letters, numbers and flags, skipping the
/// separators (spaces and commas) between them.
struct Lexer<'a> {
    /// The whole `d`.
    s: &'a str,
    /// The next byte to read.
    i: usize,
}

impl<'a> Lexer<'a> {
    /// Skip spaces and commas.
    fn skip(&mut self) {
        let b = self.s.as_bytes();
        while self.i < b.len() && matches!(b[self.i], b' ' | b',' | b'\n' | b'\t' | b'\r') {
            self.i += 1;
        }
    }

    /// The next command letter, without consuming it; `None` at the end of the text or
    /// before anything that is not a letter.
    fn peek_command(&mut self) -> Option<u8> {
        self.skip();
        self.s
            .as_bytes()
            .get(self.i)
            .copied()
            .filter(u8::is_ascii_alphabetic)
    }

    /// A number written as the emitters write them, `-?digits(.digits)?`, or `None`. No
    /// exponent, no leading `+`, no bare `.5`: none of the emitters produces one, and
    /// refusing them makes the whole path fall back rather than be misread.
    fn number(&mut self) -> Option<Num<'a>> {
        self.skip();
        let b = self.s.as_bytes();
        let start = self.i;
        let mut j = start;
        if b.get(j) == Some(&b'-') {
            j += 1;
        }
        let int_start = j;
        while b.get(j).is_some_and(u8::is_ascii_digit) {
            j += 1;
        }
        if j == int_start {
            return None;
        }
        if b.get(j) == Some(&b'.') {
            let frac = j + 1;
            j = frac;
            while b.get(j).is_some_and(u8::is_ascii_digit) {
                j += 1;
            }
            if j == frac {
                return None;
            }
        }
        let text = &self.s[start..j];
        let value: f64 = text.parse().ok()?;
        self.i = j;
        Some(Num {
            text: Cow::Borrowed(text),
            value,
        })
    }

    /// A point: two numbers.
    fn point(&mut self) -> Option<Pt<'a>> {
        Some([self.number()?, self.number()?])
    }

    /// An arc flag: one `0` or `1`.
    fn flag(&mut self) -> Option<bool> {
        self.skip();
        let f = match self.s.as_bytes().get(self.i)? {
            b'0' => false,
            b'1' => true,
            _ => return None,
        };
        self.i += 1;
        Some(f)
    }
}

/// Read the rings of `d`, as the emitters write them: every ring starts with an absolute
/// `M` and continues with absolute `L`, `C`, `S` and `A`, each with its own letter, and
/// may end with `Z`. `None` for anything else, and for a ring with no segments.
///
/// An `S` becomes the cubic it stands for: its first control point is the reflection of the
/// previous cubic's second control point about the current point, or the current point
/// itself after anything but a cubic (SVG 1.1 §8.3.6), computed exactly ([`reflect`]).
fn parse(d: &str) -> Option<Vec<Sub<'_>>> {
    let mut lx = Lexer { s: d, i: 0 };
    let mut subs = Vec::new();
    while let Some(c) = lx.peek_command() {
        if c != b'M' {
            return None;
        }
        let begin = lx.i;
        lx.i += 1;
        let start = lx.point()?;
        let mut segs: Vec<Seg> = Vec::new();
        let mut closed = false;
        while let Some(c) = lx.peek_command() {
            if c == b'M' {
                break;
            }
            lx.i += 1;
            let at = segs.last().map_or(&start, Seg::end).clone();
            let seg = match c {
                b'L' => Seg::Line(lx.point()?),
                b'C' => Seg::Cubic(lx.point()?, lx.point()?, lx.point()?),
                b'S' => {
                    let c1 = match segs.last() {
                        Some(Seg::Cubic(_, c2, _)) => {
                            [reflect(&at[0], &c2[0])?, reflect(&at[1], &c2[1])?]
                        }
                        _ => at,
                    };
                    Seg::Cubic(c1, lx.point()?, lx.point()?)
                }
                b'A' => Seg::Arc {
                    rx: lx.number()?,
                    ry: lx.number()?,
                    rot: lx.number()?,
                    large: lx.flag()?,
                    sweep: lx.flag()?,
                    end: lx.point()?,
                },
                b'Z' => {
                    closed = true;
                    break;
                }
                _ => return None,
            };
            segs.push(seg);
        }
        if segs.is_empty() {
            return None;
        }
        // After a `Z` only another ring may follow.
        if closed && lx.peek_command().is_some_and(|c| c != b'M') {
            return None;
        }
        lx.skip();
        subs.push(Sub {
            text: &d[begin..lx.i],
            start,
            segs,
            closed,
        });
    }
    lx.skip();
    (lx.i == d.len() && !subs.is_empty()).then_some(subs)
}

/// A written decimal as an integer and a power of ten: `-12.34` is `(-1234, 2)`. `None` for
/// more digits than an `i128` holds safely.
fn decimal(text: &str) -> Option<(i128, u32)> {
    let (neg, body) = match text.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, text),
    };
    let (int, frac) = body.split_once('.').unwrap_or((body, ""));
    if int.len() + frac.len() > 30 {
        return None;
    }
    let digits: String = [int, frac].concat();
    let m: i128 = digits.parse().ok()?;
    Some((if neg { -m } else { m }, frac.len() as u32))
}

/// `2·p − c` in exact decimal arithmetic, written at the larger of the two scales: the
/// reflection of control point `c` about `p`, as an `S` asks the reader to compute it.
/// `-0.50` style: a sign, the integer part, and exactly `scale` fractional digits.
fn reflect<'a>(p: &Num<'_>, c: &Num<'_>) -> Option<Num<'a>> {
    let ((mp, sp), (mc, sc)) = (decimal(&p.text)?, decimal(&c.text)?);
    let scale = sp.max(sc);
    let up = |m: i128, s: u32| m.checked_mul(10i128.checked_pow(scale - s)?);
    let m = up(mp, sp)?.checked_mul(2)?.checked_sub(up(mc, sc)?)?;
    let text = format_decimal(m, scale);
    let value = text.parse().ok()?;
    Some(Num {
        text: Cow::Owned(text),
        value,
    })
}

/// The decimal `m · 10^(−scale)` as text with exactly `scale` fractional digits.
fn format_decimal(m: i128, scale: u32) -> String {
    let sign = if m < 0 { "-" } else { "" };
    let a = m.unsigned_abs();
    if scale == 0 {
        return format!("{sign}{a}");
    }
    let p = 10u128.pow(scale);
    format!("{sign}{}.{:0w$}", a / p, a % p, w = scale as usize)
}

/// Whether two written numbers are the same decimal, whatever their trailing zeros.
fn same_decimal(a: &Num<'_>, b: &Num<'_>) -> bool {
    match (decimal(&a.text), decimal(&b.text)) {
        (Some((ma, sa)), Some((mb, sb))) => {
            let s = sa.max(sb);
            let up = |m: i128, k: u32| m.checked_mul(10i128.checked_pow(s - k)?);
            matches!((up(ma, sa), up(mb, sb)), (Some(x), Some(y)) if x == y)
        }
        _ => false,
    }
}

// ---------------------------------------------------------------------------------------
// Measuring

/// What the depth test reads from one ring.
struct Measured {
    /// The ring as a polygon following its curves, open (the last point is not repeated).
    pts: Vec<Point>,
    /// Half the shoelace sum, px²: negative anticlockwise on the screen (y down).
    signed: f64,
}

/// The value of a written point.
fn point(p: &Pt<'_>) -> Point {
    Point::new(p[0].value, p[1].value)
}

/// The polygon of a ring and its signed area. The polygon is
/// [`crate::rings::ring_points`]'s, the one the emitter's nesting reads: every segment end,
/// plus four points along each cubic and arc, so a ring of two half-circle arcs encloses
/// its disc rather than the zero area of its two end points.
fn measure(s: &Sub<'_>) -> Measured {
    let path = FittedPath {
        start: point(&s.start),
        segments: s
            .segs
            .iter()
            .map(|g| match g {
                Seg::Line(p) => Segment::Line(point(p)),
                Seg::Cubic(a, b, p) => Segment::Cubic(point(a), point(b), point(p)),
                Seg::Arc {
                    rx,
                    ry,
                    rot,
                    large,
                    sweep,
                    end,
                } => Segment::Arc {
                    rx: rx.value,
                    ry: ry.value,
                    phi: rot.value.to_radians(),
                    large_arc: *large,
                    sweep: *sweep,
                    end: point(end),
                },
            })
            .collect(),
        closed: true,
    };
    let mut pts = ring_points(&vec![(0, false)], std::slice::from_ref(&path));
    // `ring_points` repeats the start at the end of a closed ring; the shoelace sum and
    // the crossing test both treat the polygon as closed already.
    if pts.len() > 1 && pts.first() == pts.last() {
        pts.pop();
    }
    let n = pts.len();
    let mut twice = 0.0;
    for k in 0..n {
        let (a, b) = (pts[k], pts[if k + 1 == n { 0 } else { k + 1 }]);
        twice += a.x * b.y - b.x * a.y;
    }
    Measured {
        pts,
        signed: 0.5 * twice,
    }
}

/// Per ring, whether it lies inside an odd number of the path's other rings.
///
/// A ring alone in its path is at depth 0. Otherwise each ring is probed at up to five
/// points strictly inside it ([`crate::rings::interior_probes`], which keeps them off the
/// boundaries a hole shares with what it is punched from), and a probe's parity is that of
/// its ray crossings with the edges of every ring of larger area: a ring with less area
/// cannot contain this one, and a ring nested *inside* this one must not count even if a
/// probe happens to fall in it. The majority of the probes decides.
fn depth_parity(rings: &[Measured]) -> Vec<bool> {
    if rings.len() < 2 {
        return vec![false; rings.len()];
    }
    let index = DepthIndex::new(rings);
    rings
        .iter()
        .enumerate()
        .map(|(i, r)| {
            let probes = interior_probes(&r.pts);
            if probes.is_empty() {
                return false;
            }
            let odd = probes.iter().filter(|&&p| index.odd(p, i)).count();
            odd * 2 > probes.len()
        })
        .collect()
}

/// Every edge of every ring of one path, bucketed by height so a horizontal ray meets only
/// the edges that can cross it.
///
/// The rows split the path's vertical extent evenly, about `√E` of them for `E` edges (at
/// least one, at most 4096); each edge is listed in every row its `y` range touches, so an
/// edge that straddles a ray's height is always in the ray's row. Building is linear in the
/// edges plus their total height in rows; a query reads one row.
struct DepthIndex<'r> {
    /// The rings.
    rings: &'r [Measured],
    /// Absolute area of each ring, px².
    area: Vec<f64>,
    /// Top of the first row, px.
    y0: f64,
    /// Row height, px (positive).
    h: f64,
    /// Row `r`'s entries are `entries[row_start[r]..row_start[r + 1]]`.
    row_start: Vec<usize>,
    /// (ring, index of the edge's first point in that ring).
    entries: Vec<(u32, u32)>,
}

impl<'r> DepthIndex<'r> {
    /// Bucket the edges of `rings`.
    fn new(rings: &'r [Measured]) -> Self {
        let (mut lo, mut hi, mut edges) = (f64::INFINITY, f64::NEG_INFINITY, 0usize);
        for r in rings {
            for p in &r.pts {
                lo = lo.min(p.y);
                hi = hi.max(p.y);
            }
            edges += r.pts.len();
        }
        let rows = ((edges as f64).sqrt().ceil() as usize).clamp(1, 4096);
        let h = ((hi - lo) / rows as f64).max(1e-9);
        let mut idx = DepthIndex {
            rings,
            area: rings.iter().map(|r| r.signed.abs()).collect(),
            y0: lo,
            h,
            row_start: vec![0; rows + 1],
            entries: Vec::new(),
        };
        // Two passes, counting then filling, so the rows sit in one array.
        let spans: Vec<(u32, u32, usize, usize)> = rings
            .iter()
            .enumerate()
            .flat_map(|(ri, r)| {
                let n = r.pts.len();
                let idx = &idx;
                (0..n).map(move |k| {
                    let (a, b) = (r.pts[k], r.pts[if k + 1 == n { 0 } else { k + 1 }]);
                    let (r0, r1) = (idx.row(a.y.min(b.y)), idx.row(a.y.max(b.y)));
                    (ri as u32, k as u32, r0, r1)
                })
            })
            .collect();
        for &(_, _, r0, r1) in &spans {
            for row in r0..=r1 {
                idx.row_start[row + 1] += 1;
            }
        }
        for row in 0..rows {
            idx.row_start[row + 1] += idx.row_start[row];
        }
        let mut fill = idx.row_start.clone();
        idx.entries = vec![(0, 0); idx.row_start[rows]];
        for &(ri, k, r0, r1) in &spans {
            for row in r0..=r1 {
                idx.entries[fill[row]] = (ri, k);
                fill[row] += 1;
            }
        }
        idx
    }

    /// The row holding height `y`: monotone in `y`, clamped to the rows that exist.
    fn row(&self, y: f64) -> usize {
        let rows = self.row_start.len() - 1;
        (((y - self.y0) / self.h).floor().max(0.0) as usize).min(rows - 1)
    }

    /// Whether `p` lies inside an odd number of the rings larger than ring `own`: the
    /// parity of the crossings of the ray from `p` towards +x with their edges, counted by
    /// the rule of [`crate::rings::point_in_ring`] (an edge counts when exactly one end is
    /// above `p`, and its crossing lies to the right of `p`).
    fn odd(&self, p: Point, own: usize) -> bool {
        let row = self.row(p.y);
        let a_own = self.area[own];
        let mut odd = false;
        for &(ri, k) in &self.entries[self.row_start[row]..self.row_start[row + 1]] {
            let ri = ri as usize;
            if ri == own || self.area[ri] <= a_own {
                continue;
            }
            let pts = &self.rings[ri].pts;
            let k = k as usize;
            let (a, b) = (pts[k], pts[if k + 1 == pts.len() { 0 } else { k + 1 }]);
            if (a.y > p.y) != (b.y > p.y) {
                let t = (p.y - a.y) / (b.y - a.y);
                if p.x < a.x + t * (b.x - a.x) {
                    odd = !odd;
                }
            }
        }
        odd
    }
}

// ---------------------------------------------------------------------------------------
// Writing

/// Append `s` written in the opposite direction.
///
/// With the ring's points `P_0 = start, …, P_n` (segment `k` runs from `P_(k−1)` to `P_k`),
/// the reversed ring starts at `P_n` and walks the segments last to first, each from its end
/// to its start: a line to `P_(k−1)`; a cubic `(c1, c2)` as `(c2, c1)`, written `S` when its
/// new first control point is exactly the reflection of the previous reversed cubic's
/// second ([`reflect`], [`same_decimal`]); an arc with the same radii, rotation and
/// large-arc flag and the sweep flag flipped. A `Z` closes it as it closed the original;
/// any implicit closing line `P_n → P_0` of the original is the reversed ring's own
/// closing line `P_0 → P_n`. Numbers are their written text throughout.
fn write_reversed(s: &Sub<'_>, out: &mut String) {
    let pt = |p: &Pt<'_>| format!("{},{}", p[0].text, p[1].text);
    out.push('M');
    out.push_str(&pt(s.segs[s.segs.len() - 1].end()));
    // The second control point of the last cubic written, for the `S` test.
    let mut prev_c2: Option<&Pt<'_>> = None;
    for k in (0..s.segs.len()).rev() {
        let from = s.segs[k].end();
        let to = if k == 0 {
            &s.start
        } else {
            s.segs[k - 1].end()
        };
        match &s.segs[k] {
            Seg::Line(_) => {
                out.push('L');
                out.push_str(&pt(to));
                prev_c2 = None;
            }
            Seg::Cubic(c1, c2, _) => {
                // The reversed cubic is (from, c2, c1, to).
                let smooth = prev_c2.is_some_and(|q| {
                    matches!(
                        (reflect(&from[0], &q[0]), reflect(&from[1], &q[1])),
                        (Some(x), Some(y)) if same_decimal(&x, &c2[0]) && same_decimal(&y, &c2[1])
                    )
                });
                if smooth {
                    out.push_str(&format!("S{} {}", pt(c1), pt(to)));
                } else {
                    out.push_str(&format!("C{} {} {}", pt(c2), pt(c1), pt(to)));
                }
                prev_c2 = Some(c1);
            }
            Seg::Arc {
                rx,
                ry,
                rot,
                large,
                sweep,
                ..
            } => {
                out.push_str(&format!(
                    "A{},{} {} {},{} {}",
                    rx.text,
                    ry.text,
                    rot.text,
                    u8::from(*large),
                    u8::from(!*sweep),
                    pt(to)
                ));
                prev_c2 = None;
            }
        }
    }
    if s.closed {
        out.push('Z');
    }
}

#[cfg(test)]
mod tests;
