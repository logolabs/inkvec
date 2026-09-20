//! Writing `d` attributes and primitive elements in the fewest bytes the SVG
//! grammar allows, tracked the way a parser reads them.

use inkvec_core::Point;
use inkvec_fit::curves::Segment;
use inkvec_fit::primitives::PrimitiveKind;

/// `v` at the fewest decimals that still lands within `quantum` of it, never more than
/// `most`, with the leading zero of `0.5` and `-0.5` dropped -- a parser does not need it.
fn short_num(v: f64, quantum: f64, most: usize) -> String {
    let mut best = format!("{v:.most$}");
    for d in 0..most {
        let s = format!("{v:.d$}");
        if s.parse::<f64>().is_ok_and(|r| (r - v).abs() <= quantum) {
            best = s;
            break;
        }
    }
    if best.contains('.') {
        best = best.trim_end_matches('0').trim_end_matches('.').to_string();
    }
    if let Some(rest) = best.strip_prefix("0.") {
        best = format!(".{rest}");
    } else if let Some(rest) = best.strip_prefix("-0.") {
        best = format!("-.{rest}");
    }
    if best == "-0" || best.is_empty() {
        best = "0".to_string();
    }
    best
}
/// Decimals allowed when nothing may be rounded: enough that a coordinate written back
/// is the coordinate that was read, at a scale no renderer resolves.
pub(crate) const LOSSLESS_DECIMALS: usize = 9;
/// How many decimals a coordinate can need at this tolerance.
pub(crate) fn decimals_for(eps: f64) -> usize {
    ((1.0 / (0.25 * eps)).log10().ceil().max(0.0) as usize).min(6)
}
/// What the writer has just put down, which decides whether the next number needs a
/// separator in front of it at all.
#[derive(Clone, Copy, PartialEq)]
enum Tail {
    Empty,
    /// A command letter, or an arc flag: both are exactly one character, so a parser
    /// stops after them whatever comes next.
    Single,
    Number {
        dot: bool,
    },
}
/// Writes `d` in the fewest bytes the SVG grammar allows.
///
/// Every command goes down in whichever of its absolute and relative forms is shorter;
/// the letter is dropped wherever it repeats (and after `M`, where a bare number already
/// means `L`); an axis-aligned line becomes `H` or `V`; a cubic continuing the last one
/// smoothly becomes `S`; leading zeros and separators go wherever a parser does not need
/// them; and each number is written at the fewest decimals that still lands within a
/// quarter of the tolerance.
///
/// The pen is tracked as a *parser* would reconstruct it -- from the numbers actually
/// written, not the ones intended -- so a chain of relative commands cannot accumulate
/// rounding. That is the whole reason relative is safe to use here.
pub(crate) struct Writer {
    pub(crate) out: String,
    tail: Tail,
    /// The last command letter written, or produced by an omission.
    last: u8,
    /// Where a parser would think the pen is.
    at: Point,
    /// Where the current subpath started, as written.
    sub: Point,
    /// The second control point of the last cubic, as written: what an `S` reflects.
    prev_c2: Option<Point>,
    /// Largest rounding allowed in one written number.
    quantum: f64,
    most: usize,
}

impl Writer {
    pub(crate) fn new(quantum: f64, most: usize) -> Self {
        Writer {
            out: String::new(),
            tail: Tail::Empty,
            last: 0,
            at: Point::new(0.0, 0.0),
            sub: Point::new(0.0, 0.0),
            prev_c2: None,
            quantum,
            most,
        }
    }

    fn num(&self, v: f64) -> String {
        short_num(v, self.quantum, self.most)
    }

    fn parse_back(s: &str) -> f64 {
        s.parse().unwrap_or(0.0)
    }

    /// Whether `next` can follow what is already down without a separator between them.
    fn joins(tail: Tail, next: &str) -> bool {
        match tail {
            Tail::Empty | Tail::Single => true,
            // A minus sign always begins a new number. A point does too, but only once
            // the number before it already has one: `1` and `.5` written together read
            // as the single number `1.5`.
            Tail::Number { dot } => match next.as_bytes().first() {
                Some(b'-') => true,
                Some(b'.') => dot,
                _ => false,
            },
        }
    }

    /// The text this command would add, and what it would leave the writer having just
    /// written. Nothing is committed: the caller weighs the forms first.
    fn render(&self, letter: u8, nums: &[String], flag_at: usize) -> (String, u8, Tail) {
        // A bare number after `M` already means `L`, so the letter can go there too.
        let implied = self.last == letter
            || (self.last == b'M' && letter == b'L')
            || (self.last == b'm' && letter == b'l');
        let mut s = String::new();
        let mut tail = self.tail;
        if !implied {
            s.push(char::from(letter));
            tail = Tail::Single;
        }
        for (i, n) in nums.iter().enumerate() {
            if !Self::joins(tail, n) {
                s.push(' ');
            }
            s.push_str(n);
            tail = if (flag_at..flag_at.saturating_add(2)).contains(&i) {
                Tail::Single
            } else {
                Tail::Number {
                    dot: n.contains('.'),
                }
            };
        }
        (s, letter, tail)
    }

    /// Commit whichever rendering is shorter, preferring the one that keeps the previous
    /// letter so the next command can drop its own.
    fn put(&mut self, forms: &[(u8, Vec<String>, usize)]) -> Vec<f64> {
        let mut best: Option<(String, u8, Tail, usize)> = None;
        for (i, (letter, nums, flag_at)) in forms.iter().enumerate() {
            let (s, l, t) = self.render(*letter, nums, *flag_at);
            let better = match &best {
                None => true,
                Some((b, _, _, _)) => {
                    s.len() < b.len() || (s.len() == b.len() && *letter == self.last)
                }
            };
            if better {
                best = Some((s, l, t, i));
            }
        }
        let (s, letter, tail, i) = best.expect("at least one form");
        self.out.push_str(&s);
        self.last = letter;
        self.tail = tail;
        forms[i].1.iter().map(|n| Self::parse_back(n)).collect()
    }

    /// A point written as an absolute pair and as a delta from the pen.
    fn pair(&self, p: Point) -> (Vec<String>, Vec<String>) {
        (
            vec![self.num(p.x), self.num(p.y)],
            vec![self.num(p.x - self.at.x), self.num(p.y - self.at.y)],
        )
    }

    fn move_to(&mut self, p: Point, first: bool) {
        let (abs, rel) = self.pair(p);
        let vals = if first {
            // Nothing precedes the first `M`, so a relative one would mean the same
            // thing and read as a mistake.
            self.put(&[(b'M', abs, usize::MAX)])
        } else {
            self.put(&[(b'M', abs, usize::MAX), (b'm', rel, usize::MAX)])
        };
        self.at = if self.last == b'M' {
            Point::new(vals[0], vals[1])
        } else {
            Point::new(self.at.x + vals[0], self.at.y + vals[1])
        };
        self.sub = self.at;
        self.prev_c2 = None;
    }

    fn line_to(&mut self, p: Point) {
        let (abs, rel) = self.pair(p);
        let mut forms = vec![
            (b'L', abs.clone(), usize::MAX),
            (b'l', rel.clone(), usize::MAX),
        ];
        // An axis-aligned line is one number, but only if it stays axis-aligned once
        // written: the pen must land exactly on the coordinate it keeps.
        if Self::parse_back(&rel[1]) == 0.0 {
            forms.push((b'H', vec![abs[0].clone()], usize::MAX));
            forms.push((b'h', vec![rel[0].clone()], usize::MAX));
        }
        if Self::parse_back(&rel[0]) == 0.0 {
            forms.push((b'V', vec![abs[1].clone()], usize::MAX));
            forms.push((b'v', vec![rel[1].clone()], usize::MAX));
        }
        let vals = self.put(&forms);
        self.at = match self.last {
            b'L' => Point::new(vals[0], vals[1]),
            b'l' => Point::new(self.at.x + vals[0], self.at.y + vals[1]),
            b'H' => Point::new(vals[0], self.at.y),
            b'h' => Point::new(self.at.x + vals[0], self.at.y),
            b'V' => Point::new(self.at.x, vals[0]),
            _ => Point::new(self.at.x, self.at.y + vals[0]),
        };
        self.prev_c2 = None;
    }

    fn cubic_to(&mut self, c1: Point, c2: Point, p: Point) {
        let (a1, r1) = self.pair(c1);
        let (a2, r2) = self.pair(c2);
        let (ap, rp) = self.pair(p);
        let mut forms = vec![
            (
                b'C',
                [a1.clone(), a2.clone(), ap.clone()].concat(),
                usize::MAX,
            ),
            (b'c', [r1, r2.clone(), rp.clone()].concat(), usize::MAX),
        ];
        // `S` restates the first control point as the reflection of the last one, which
        // a parser computes from what was *written*. Offered only where that reflection
        // is the control point we meant.
        if let Some(pc2) = self.prev_c2 {
            let mirror = Point::new(2.0 * self.at.x - pc2.x, 2.0 * self.at.y - pc2.y);
            if mirror.dist(c1) <= self.quantum {
                forms.push((b'S', [a2, ap].concat(), usize::MAX));
                forms.push((b's', [r2, rp].concat(), usize::MAX));
            }
        }
        let vals = self.put(&forms);
        let (c2_written, end) = match self.last {
            b'C' => (Point::new(vals[2], vals[3]), Point::new(vals[4], vals[5])),
            b'c' => (
                Point::new(self.at.x + vals[2], self.at.y + vals[3]),
                Point::new(self.at.x + vals[4], self.at.y + vals[5]),
            ),
            b'S' => (Point::new(vals[0], vals[1]), Point::new(vals[2], vals[3])),
            _ => (
                Point::new(self.at.x + vals[0], self.at.y + vals[1]),
                Point::new(self.at.x + vals[2], self.at.y + vals[3]),
            ),
        };
        self.at = end;
        self.prev_c2 = Some(c2_written);
    }

    fn arc_to(&mut self, rx: f64, ry: f64, phi: f64, large: bool, sweep: bool, p: Point) {
        let (ap, rp) = self.pair(p);
        let head = vec![
            self.num(rx),
            self.num(ry),
            self.num(phi.to_degrees()),
            u8::from(large).to_string(),
            u8::from(sweep).to_string(),
        ];
        // The two flags sit at 3 and 4, and are one character each: a parser stops after
        // them whatever follows, so nothing needs a separator there.
        let mut abs = head.clone();
        abs.extend(ap);
        let mut rel = head;
        rel.extend(rp);
        let vals = self.put(&[(b'A', abs, 3), (b'a', rel, 3)]);
        self.at = if self.last == b'A' {
            Point::new(vals[5], vals[6])
        } else {
            Point::new(self.at.x + vals[5], self.at.y + vals[6])
        };
        self.prev_c2 = None;
    }

    fn close(&mut self) {
        self.out.push('Z');
        self.last = b'Z';
        self.tail = Tail::Single;
        self.at = self.sub;
        self.prev_c2 = None;
    }

    /// Write one subpath. `first` says whether it opens the attribute.
    pub(crate) fn subpath(&mut self, start: Point, segs: &[Segment], closed: bool, first: bool) {
        // `Z` already draws the straight line home, so a last segment that is that line
        // is a command saying what the next one says anyway.
        let segs = match segs.split_last() {
            Some((Segment::Line(p), rest)) if closed && p.dist(start) <= self.quantum => rest,
            _ => segs,
        };
        self.move_to(start, first);
        for seg in segs {
            match *seg {
                Segment::Line(p) => self.line_to(p),
                Segment::Cubic(c1, c2, p) => self.cubic_to(c1, c2, p),
                Segment::Arc {
                    rx,
                    ry,
                    phi,
                    large_arc,
                    sweep,
                    end,
                } => self.arc_to(rx, ry, phi, large_arc, sweep, end),
            }
        }
        if closed {
            self.close();
        }
    }
}
/// The element that states a whole-subpath primitive: its tag, its geometry attributes,
/// and what they cost in numbers. A rotated ellipse would need a `transform` that could
/// collide with the element's own, so it stays a path.
pub(crate) fn primitive_element(
    kind: &PrimitiveKind,
    quantum: f64,
    most: usize,
) -> Option<(&'static str, String, f64)> {
    let mut a = String::new();
    let mut attr = |name: &str, v: f64| {
        if !a.is_empty() {
            a.push(' ');
        }
        a.push_str(name);
        a.push_str("=\"");
        a.push_str(&short_num(v, quantum, most));
        a.push('"');
    };
    match *kind {
        PrimitiveKind::Circle { c, r } => {
            attr("cx", c.x);
            attr("cy", c.y);
            attr("r", r);
            Some(("circle", a, 3.0))
        }
        PrimitiveKind::Ellipse { c, rx, ry, angle } => {
            let a0 = angle.rem_euclid(std::f64::consts::PI);
            let (rx, ry) = if a0 < 1e-3 || a0 > std::f64::consts::PI - 1e-3 {
                (rx, ry)
            } else if (a0 - std::f64::consts::FRAC_PI_2).abs() < 1e-3 {
                (ry, rx)
            } else {
                return None;
            };
            attr("cx", c.x);
            attr("cy", c.y);
            attr("rx", rx);
            attr("ry", ry);
            Some(("ellipse", a, 4.0))
        }
        PrimitiveKind::RoundRect { x, y, w, h, rx } => {
            attr("x", x);
            attr("y", y);
            attr("width", w);
            attr("height", h);
            let mut cost = 4.0;
            if rx > 0.0 {
                attr("rx", rx);
                cost += 1.0;
            }
            Some(("rect", a, cost))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn written(start: Point, segs: &[Segment], closed: bool) -> String {
        let mut w = Writer::new(0.01, 2);
        w.subpath(start, segs, closed, true);
        w.out
    }

    /// The form a hand would write: relative `h`/`v` where the line is axis-aligned, `Z`
    /// closing the loop, no separators a parser does not need. The closing line, ending
    /// where `Z` already draws, is absorbed -- this is exactly the path `minify` must
    /// leave byte for byte, so the writer has to produce it too.
    #[test]
    fn a_square_comes_out_in_the_fewest_bytes() {
        let out = written(
            Point::new(10.0, 10.0),
            &[
                Segment::Line(Point::new(100.0, 10.0)),
                Segment::Line(Point::new(100.0, 100.0)),
                Segment::Line(Point::new(10.0, 10.0)),
            ],
            true,
        );
        assert_eq!(out, "M10 10h90v90Z");
        // A last edge that does not end at home cannot be absorbed: it is drawn, and
        // `Z` only closes the gap.
        let out = written(
            Point::new(10.0, 10.0),
            &[
                Segment::Line(Point::new(100.0, 10.0)),
                Segment::Line(Point::new(100.0, 100.0)),
                Segment::Line(Point::new(10.0, 100.0)),
            ],
            true,
        );
        assert_eq!(out, "M10 10h90v90H10Z");
    }

    /// `S` restates a reflected control point in two numbers instead of four -- but only
    /// where the reflection is the control point we meant.
    #[test]
    fn a_continuing_cubic_is_written_as_an_s() {
        let first = Segment::Cubic(
            Point::new(30.0, 10.0),
            Point::new(90.0, 10.0),
            Point::new(110.0, 100.0),
        );
        // The second cubic's first control point is the exact reflection of the first's
        // second: (2·110 − 90, 2·100 − 10) = (130, 190).
        let second = Segment::Cubic(
            Point::new(130.0, 190.0),
            Point::new(150.0, 150.0),
            Point::new(160.0, 110.0),
        );
        let out = written(Point::new(10.0, 100.0), &[first.clone(), second], false);
        // The relative form is shorter here, so the smooth command comes out lowercase.
        assert!(out.contains('s'), "{out}");
        // And the reflection is only offered when it is honest: moved off the mirror, the
        // writer goes back to a full cubic.
        let off = Segment::Cubic(
            Point::new(131.0, 190.0),
            Point::new(150.0, 150.0),
            Point::new(160.0, 110.0),
        );
        let out = written(Point::new(10.0, 100.0), &[first, off], false);
        assert!(!out.contains('s') && !out.contains('S'), "{out}");
    }

    #[test]
    fn numbers_lose_their_leading_and_trailing_zeros() {
        assert_eq!(short_num(0.5, 0.01, 3), ".5");
        assert_eq!(short_num(-0.5, 0.01, 3), "-.5");
        assert_eq!(short_num(1.0, 0.01, 3), "1");
        assert_eq!(short_num(-0.04, 0.01, 3), "-.04");
        assert_eq!(short_num(10.0, 0.01, 1), "10");
        // Nothing rounds past the most decimals allowed.
        assert_eq!(short_num(1.234567, 0.0001, 2), "1.23");
    }
}
