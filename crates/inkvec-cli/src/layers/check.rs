//! The checks a completion must pass before it is used.
//!
//! The rule of `docs/theory/verification.md`: a solver may propose anything, and a result a
//! theorem speaks about is used only when a checker accepts it, on a certificate the solver
//! emits. Here the theorem is the painter's interval (`docs/theory/chain-representation.md`,
//! R4.1; Lean: `Inkvec.Design.painter_interval_iff`): with the paint order fixed, a shape `E`
//! written for a face that shows `V`, under the regions `U` painted after it, leaves every
//! visible point of the picture unchanged exactly when `V ⊆ E ⊆ V ∪ U`.
//!
//! The check runs on the scan lines of [`super::region`]. On each line, each span of the
//! inner region must lie in one span of the outer region, widened by [`SPAN_SLACK`] at each
//! end. The solver ([`propose_cover`]) names, for every inner span, the outer span it claims
//! contains it: the certificate. The checker ([`check_cover`]) walks every inner span
//! itself, takes the claimed outer span, and accepts the pair only when both outputs of the
//! generated kernel `inkvec_verified::generated::design::span_within_iv` are non-negative
//! (Lean: `Inkvec.Gen.spanWithinK`, its meaning `spanWithin_meaning`; the fold over a line
//! is `line_cover`). The enclosure is outward rounded, so an accepted pair holds in exact
//! arithmetic; a refusal (`None`) rejects the completion and the face keeps its shape.
//!
//! What is trusted rather than checked: the scan conversion of the shapes into spans
//! ([`super::region::Region::from_polygons`], flattened within [`super::shape::FLAT_TOL`]) and
//! the region algebra that builds `V` and `U`. The containment of the spans is checked.

use inkvec_verified::generated::design::span_within_iv;
use inkvec_verified::iv::Iv;

use super::region::Region;

/// The most a scan line may show of `V` outside the completed shape, or of the shape
/// outside the allowed region, px (summed over the line's crossings): the solver's own
/// test while it searches ([`Interval::holds`]). A tenth of a pixel: below what a
/// renderer's anti-aliasing can show at an edge, and above the flattening error of
/// [`super::shape::FLAT_TOL`] at both ends of a line.
pub(crate) const LINE_SLACK: f64 = 0.1;

/// The checker's slack at each end of a span, px: half of [`LINE_SLACK`], so a span
/// accepted at both ends is out by at most [`LINE_SLACK`] on its line.
pub(crate) const SPAN_SLACK: f64 = 0.5 * LINE_SLACK;

/// The solver's measure of how far a shape is from the interval. Not a verdict: it steers
/// the search and the diagnostics; [`certify_completion`] decides.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Interval {
    /// The widest scan line of `V \ E`, px: what the face shows that the shape would lose.
    pub(crate) lost: f64,
    /// The widest scan line of `E \ H`, px: what the shape would paint where it may not.
    pub(crate) spilled: f64,
}

impl Interval {
    /// Both within [`LINE_SLACK`].
    pub(crate) fn holds(&self) -> bool {
        self.lost <= LINE_SLACK && self.spilled <= LINE_SLACK
    }
}

/// The solver's measure of `lower ⊆ e ⊆ upper` (see [`Interval`]).
pub(crate) fn measure(lower: &Region, e: &Region, upper: &Region) -> Interval {
    Interval {
        lost: lower.minus(e).widest_line(),
        spilled: e.minus(upper).widest_line(),
    }
}

/// A certificate that `inner ⊆ outer` on every scan line: for line `k0 + i`, `rows[i][n]`
/// is the index of the span of `outer` claimed to contain the `n`-th span of `inner`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Cover {
    k0: i32,
    rows: Vec<Vec<usize>>,
}

impl Cover {
    fn row(&self, k: i32) -> Option<&[usize]> {
        let i = usize::try_from(k - self.k0).ok()?;
        self.rows.get(i).map(Vec::as_slice)
    }
}

/// **The solver's certificate**: for each span of `inner`, the span of `outer` on the same
/// line that overlaps it most (the first, when none does; the checker then refuses).
pub(crate) fn propose_cover(inner: &Region, outer: &Region) -> Cover {
    let lines = inner.lines();
    let rows = lines
        .clone()
        .map(|k| {
            let outs = outer.line(k);
            inner
                .line(k)
                .iter()
                .map(|&(a0, a1)| {
                    let mut best = (0usize, f64::NEG_INFINITY);
                    for (j, &(b0, b1)) in outs.iter().enumerate() {
                        let overlap = a1.min(b1) - a0.max(b0);
                        if overlap > best.1 {
                            best = (j, overlap);
                        }
                    }
                    best.0
                })
                .collect()
        })
        .collect();
    Cover {
        k0: lines.start,
        rows,
    }
}

/// **The checker**: every span of `inner`, on every line, lies in the span of `outer` the
/// certificate names, widened by `slack` at each end, as decided by the generated kernel
/// `span_within_iv` on outward-rounded intervals. Any refusal, missing entry or index out of
/// range is a rejection.
pub(crate) fn check_cover(inner: &Region, outer: &Region, slack: f64, cert: &Cover) -> bool {
    let Some(s) = Iv::point(slack) else {
        return false;
    };
    for k in inner.lines() {
        let spans = inner.line(k);
        if spans.is_empty() {
            continue;
        }
        let Some(row) = cert.row(k) else {
            return false;
        };
        if row.len() != spans.len() {
            return false;
        }
        let outs = outer.line(k);
        for (&(a0, a1), &j) in spans.iter().zip(row) {
            let Some(&(b0, b1)) = outs.get(j) else {
                return false;
            };
            let (Some(a0), Some(a1), Some(b0), Some(b1)) =
                (Iv::point(a0), Iv::point(a1), Iv::point(b0), Iv::point(b1))
            else {
                return false;
            };
            match span_within_iv(&[a0, a1, b0, b1, s]) {
                Some([lo, hi]) if lo.is_nonneg() && hi.is_nonneg() => {}
                _ => return false,
            }
        }
    }
    true
}

/// **The painter-interval check**: `lower ⊆ e ⊆ upper` on every scan line, each certified by
/// [`check_cover`] on the solver's [`propose_cover`]. The shape `e` contains the lower end
/// (what the face shows, with the half pixel of its cover it must reach under;
/// [`super::required_region`]) and stays within the upper end (what the face shows and what
/// is painted over it; [`super::allowed_region`]).
pub(crate) fn certify_completion(lower: &Region, e: &Region, upper: &Region) -> bool {
    check_cover(lower, e, SPAN_SLACK, &propose_cover(lower, e))
        && check_cover(e, upper, SPAN_SLACK, &propose_cover(e, upper))
}

/// The gate's parameter count of path data, as `bench/inkvec_bench/svgmodel.py` counts it:
/// a line 2 (`L`, `H`, `V`), a quadratic 4 (`Q`, `T`), a cubic 6 (`C`, `S`), an arc 7; a
/// move-to and a close-path nothing. Repeated arguments after one command letter count as
/// repeated segments; a move-to's extra pairs are lines.
pub(crate) fn gate_count(d: &str) -> f64 {
    let mut total = 0.0;
    let bytes = d.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        let c = bytes[i] as char;
        if c.is_ascii_alphabetic() {
            let mut j = i + 1;
            while j < bytes.len() && !(bytes[j] as char).is_ascii_alphabetic() {
                j += 1;
            }
            let args = count_numbers(&d[i + 1..j], c.to_ascii_uppercase());
            let (per, each) = match c.to_ascii_uppercase() {
                'L' => (2, 2.0),
                'T' => (2, 4.0),
                'H' | 'V' => (1, 2.0),
                'Q' | 'S' => (4, if c.to_ascii_uppercase() == 'Q' { 4.0 } else { 6.0 }),
                'C' => (6, 6.0),
                'A' => (7, 7.0),
                'M' => (2, 2.0),
                _ => (0, 0.0),
            };
            if per > 0 {
                let n = args / per;
                total += if c.to_ascii_uppercase() == 'M' {
                    each * n.saturating_sub(1) as f64
                } else {
                    each * n as f64
                };
            }
            i = j;
        } else {
            i += 1;
        }
    }
    total
}

/// How many numbers an argument list holds. Arc flags may be written without separators
/// (`0 01` or `011`), so for an arc command the flags are counted one character each.
fn count_numbers(s: &str, cmd: char) -> usize {
    let b = s.as_bytes();
    let mut n = 0;
    let mut i = 0;
    let mut in_arc = 0; // position within the current arc's 7 arguments
    while i < b.len() {
        let c = b[i] as char;
        if c == ' ' || c == ',' || c == '\n' || c == '\t' || c == '\r' {
            i += 1;
            continue;
        }
        if cmd == 'A' && (in_arc == 3 || in_arc == 4) && (c == '0' || c == '1') {
            n += 1;
            in_arc = (in_arc + 1) % 7;
            i += 1;
            continue;
        }
        // A number: optional sign, digits, one dot, more digits, an exponent.
        let start = i;
        if c == '-' || c == '+' {
            i += 1;
        }
        let mut seen_dot = false;
        while i < b.len() {
            let ch = b[i] as char;
            if ch.is_ascii_digit() {
                i += 1;
            } else if ch == '.' && !seen_dot {
                seen_dot = true;
                i += 1;
            } else if (ch == 'e' || ch == 'E') && i > start {
                i += 1;
                if i < b.len() && (b[i] == b'-' || b[i] == b'+') {
                    i += 1;
                }
            } else {
                break;
            }
        }
        if i == start {
            i += 1;
            continue;
        }
        n += 1;
        if cmd == 'A' {
            in_arc = (in_arc + 1) % 7;
        }
    }
    n
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_as_the_gate_does() {
        assert_eq!(gate_count("M0,0L10,0L10,10Z"), 4.0);
        assert_eq!(gate_count("M0 0H10V10Z"), 4.0);
        assert_eq!(gate_count("M0,0C1,1 2,2 3,3S5,5 6,6Z"), 12.0);
        assert_eq!(gate_count("M0,0A5,5 0 1,0 10,0A5,5 0 1 0 0,0Z"), 14.0);
        assert_eq!(gate_count("M0,0 10,0 10,10Z"), 4.0);
        assert_eq!(gate_count("M1 2a2 2 0 012-2"), 7.0);
    }

    fn spans(items: &[(i32, f64, f64)]) -> Region {
        Region::from_line_spans(items.to_vec())
    }

    #[test]
    fn cover_accepts_containment_and_refuses_a_forged_certificate() {
        let inner = spans(&[(0, 1.0, 2.0), (0, 5.0, 6.0), (1, 1.0, 6.0)]);
        let outer = spans(&[(0, 0.0, 3.0), (0, 4.98, 7.0), (1, 0.0, 7.0)]);
        let cert = propose_cover(&inner, &outer);
        assert!(check_cover(&inner, &outer, SPAN_SLACK, &cert));
        // Naming the wrong span is refused, as is a missing line.
        let mut forged = cert.clone();
        forged.rows[0][1] = 0;
        assert!(!check_cover(&inner, &outer, SPAN_SLACK, &forged));
        let mut short = cert.clone();
        short.rows.pop();
        assert!(!check_cover(&inner, &outer, SPAN_SLACK, &short));
        // Out by more than the slack at one end.
        let wide = spans(&[(0, 1.0, 3.06)]);
        assert!(!check_cover(&wide, &outer, SPAN_SLACK, &propose_cover(&wide, &outer)));
        let near = spans(&[(0, 1.0, 3.04)]);
        assert!(check_cover(&near, &outer, SPAN_SLACK, &propose_cover(&near, &outer)));
        // A span bridging a gap of the outer region is refused: no one span contains it.
        let bridge = spans(&[(0, 2.0, 5.5)]);
        assert!(!check_cover(&bridge, &outer, SPAN_SLACK, &propose_cover(&bridge, &outer)));
    }
}
