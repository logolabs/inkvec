//! The checks a completion must pass before it is used.
//!
//! The rule of `docs/theory/verification.md`: a solver may propose anything, and a result a
//! theorem speaks about is used only when a checker accepts it. Here the theorem is the
//! painter's interval (`docs/theory/chain-representation.md`, R4.1; Lean:
//! `InkvecTheory.Design.Layers.painter_interval_iff`): with the paint order fixed, a shape
//! `E` written for a face that shows `V`, under the regions `U` painted after it, leaves
//! every visible point of the picture unchanged exactly when `V ⊆ E ⊆ V ∪ U`.
//!
//! **To be replaced by the generated checker.** `certify_completion` and `gate_count` are
//! the call sites `crates/inkvec-verified` will take over: the same predicates, generated
//! from the Lean definitions `sampledIntervalCheck` and `gateCount`, in exact arithmetic.
//! Until then they are written here as plain functions on the scan-line regions of
//! [`super::region`], which are exact on every scan line for the flattened shapes.

use super::region::Region;

/// The most a scan line may show of `V` outside the completed shape, or of the shape
/// outside the allowed region, px (summed over the line's crossings). A tenth of a pixel:
/// below what a renderer's anti-aliasing can show at an edge, and above the flattening
/// error of [`super::shape::FLAT_TOL`] at both ends of a line.
pub(crate) const LINE_SLACK: f64 = 0.1;

/// The verdict on one proposed completion: how far it is from satisfying the interval.
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

/// **The painter-interval check**, sampled on scan lines: `lower ⊆ e ⊆ upper`. The shape
/// `e` contains the lower end (what the face shows, with the half pixel of its cover it
/// must reach under; [`super::required_region`]) and stays within the upper end (what the
/// face shows and what is painted over it; [`super::allowed_region`]). Accepts when
/// [`Interval::holds`].
pub(crate) fn certify_completion(lower: &Region, e: &Region, upper: &Region) -> Interval {
    Interval {
        lost: lower.minus(e).widest_line(),
        spilled: e.minus(upper).widest_line(),
    }
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
}
