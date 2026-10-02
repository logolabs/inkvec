//! Tests of the winding pass: exact decimal reflections, the reader's grammar, and the
//! property the pass exists for -- at every point of a grid, the wound path's winding
//! number is non-zero exactly where the original path is filled under even-odd.

use super::*;
use crate::rings::point_in_ring;

/// The winding number of `p` against closed polygons: +1 for each edge crossing the ray
/// towards +x going one way in y, −1 the other way (SVG 1.1 §11.3 `nonzero`).
fn winding_at(p: Point, polys: &[Vec<Point>]) -> i32 {
    let mut w = 0;
    for poly in polys {
        let n = poly.len();
        for k in 0..n {
            let (a, b) = (poly[k], poly[(k + 1) % n]);
            if (a.y > p.y) != (b.y > p.y) {
                let t = (p.y - a.y) / (b.y - a.y);
                if p.x < a.x + t * (b.x - a.x) {
                    w += if b.y > a.y { 1 } else { -1 };
                }
            }
        }
    }
    w
}

/// The polygons of every ring of `d`, as the pass measures them.
fn polys(d: &str) -> Vec<Vec<Point>> {
    parse(d)
        .expect("readable")
        .iter()
        .map(|s| measure(s).pts)
        .collect()
}

/// At every point of a grid over `[-2, 34]²` (steps of 0.37 px, off every integer and
/// half-integer line), `nonzero` on the wound path fills exactly what `evenodd` fills on the
/// original. Returns the wound path.
fn assert_same_fill(d: &str) -> String {
    let (wound, rule) = for_nonzero(d);
    assert_eq!(rule, "", "{d}");
    let (before, after) = (polys(d), polys(&wound));
    let mut checked = 0;
    let mut y = -1.93;
    while y < 34.0 {
        let mut x = -1.91;
        while x < 34.0 {
            let p = Point::new(x, y);
            let evenodd = before.iter().filter(|r| point_in_ring(p, r)).count() % 2 == 1;
            let nonzero = winding_at(p, &after) != 0;
            assert_eq!(evenodd, nonzero, "at {p:?}\n  in  {d}\n  out {wound}");
            checked += 1;
            x += 0.37;
        }
        y += 0.37;
    }
    assert!(checked > 9000);
    wound.into_owned()
}

/// A square ring through the given corners, in the given direction, as `fmt_ring` writes.
fn square(x0: f64, y0: f64, s: f64, anticlockwise: bool) -> String {
    let (x1, y1) = (x0 + s, y0 + s);
    if anticlockwise {
        format!("M{x0:.2},{y0:.2}L{x0:.2},{y1:.2}L{x1:.2},{y1:.2}L{x1:.2},{y0:.2}L{x0:.2},{y0:.2}Z")
    } else {
        format!("M{x0:.2},{y0:.2}L{x1:.2},{y0:.2}L{x1:.2},{y1:.2}L{x0:.2},{y1:.2}L{x0:.2},{y0:.2}Z")
    }
}

/// A circle as `primitive_d` writes it: two half-turn arcs, sweep 0 (anticlockwise on the
/// screen), radius a step short of half the chord.
fn circle(cx: f64, cy: f64, r: f64) -> String {
    format!(
        "M{:.2},{cy:.2}A{:.2},{:.2} 0 1 0 {:.2},{cy:.2}A{:.2},{:.2} 0 1 0 {:.2},{cy:.2}Z",
        cx - r,
        r - 0.01,
        r - 0.01,
        cx + r,
        r - 0.01,
        r - 0.01,
        cx - r
    )
}

#[test]
fn reflections_are_exact_decimals() {
    let n = |t: &'static str| Num {
        text: Cow::Borrowed(t),
        value: t.parse().unwrap(),
    };
    let r = reflect(&n("12.34"), &n("10.21")).unwrap();
    assert_eq!(r.text, "14.47");
    let r = reflect(&n("-0.50"), &n("0.25")).unwrap();
    assert_eq!(r.text, "-1.25");
    // Mixed scales are written at the finer one.
    let r = reflect(&n("3"), &n("1.5")).unwrap();
    assert_eq!(r.text, "4.5");
    let r = reflect(&n("0.10"), &n("0.20")).unwrap();
    assert_eq!(r.text, "0.00");
    assert!(same_decimal(&n("4.50"), &n("4.5")));
    assert!(!same_decimal(&n("4.50"), &n("4.51")));
    assert_eq!(format_decimal(-7, 3), "-0.007");
    assert_eq!(format_decimal(1200, 2), "12.00");
}

#[test]
fn the_reader_takes_the_emitters_grammar_and_nothing_else() {
    for d in [
        "M0.00,0.00L4.00,0.00L4.00,4.00Z",
        "M1.00,1.00C2.00,0.00 3.00,0.00 4.00,1.00S6.00,2.00 7.00,1.00L1.00,1.00Z",
        "M44.38,64.00A19.61,19.61 0 1 0 83.62,64.00A19.61,19.61 0 1 0 44.38,64.00Z",
        "M1.00,1.00A2.00,3.00 12.500 0,1 5.00,1.00L1.00,1.00ZM9.00,9.00L10.00,9.00L10.00,10.00Z",
        "M0,0L4,0L4,4Z",
    ] {
        assert!(parse(d).is_some(), "{d}");
    }
    // An empty path draws nothing under either rule and needs no attribute.
    assert!(parse("").is_none());
    assert_eq!(for_nonzero(""), (Cow::Borrowed(""), ""));
    for d in [
        "m0,0l4,0l0,4z",
        "M0,0L4,0 4,4Z",
        "M0,0H4V4Z",
        "M0,0L4,0L4,4ZL1,1",
        "L0,0L4,4",
        "M0,0Z",
        "M0,0L4e1,0L4,4Z",
        "M0,0L.5,0L4,4Z",
    ] {
        assert!(parse(d).is_none(), "{d:?}");
        assert_eq!(for_nonzero(d).1, EVENODD, "{d:?}");
    }
}

/// A square with a square hole, both written the same way round as the planar map walks
/// them: the hole is reversed, the outline is not, and the outline's text is untouched.
#[test]
fn a_hole_wound_like_its_outline_is_reversed() {
    let outer = square(0.0, 0.0, 30.0, true);
    let hole = square(10.0, 10.0, 10.0, true);
    let d = format!("{outer}{hole}");
    let wound = assert_same_fill(&d);
    assert!(wound.starts_with(&outer), "{wound}");
    let rings: Vec<f64> = parse(&wound)
        .unwrap()
        .iter()
        .map(|s| measure(s).signed)
        .collect();
    assert!(rings[0] < 0.0 && rings[1] > 0.0, "{rings:?}");
    // Already right: borrowed back unchanged.
    assert!(matches!(wind_by_depth(&wound), Some(Cow::Borrowed(_))));
}

/// An island in a hole in a disc, a ring around them, and a sibling beside them, with every
/// ring wound at random: under nonzero the wound path fills what even-odd filled.
#[test]
fn nested_rings_of_every_kind_alternate() {
    let mut seed = 0x9e37_79b9_7f4a_7c15u64;
    let mut coin = || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        seed & 1 == 1
    };
    for _ in 0..24 {
        let mut d = String::new();
        d.push_str(&square(0.0, 0.0, 32.0, coin()));
        d.push_str(&square(2.0, 2.0, 26.0, coin()));
        d.push_str(&circle(15.0, 15.0, 11.0));
        d.push_str(&square(9.0, 9.0, 12.0, coin()));
        d.push_str(&circle(15.0, 15.0, 3.0));
        d.push_str(&square(28.5, 28.5, 3.0, coin()));
        assert_same_fill(&d);
    }
}

/// Rings that share boundary with what they sit in, as a punched hole touching the edge of
/// its face does: the probes are taken inside the ring, so the shared edge does not decide.
#[test]
fn a_hole_touching_its_outline_is_still_a_hole() {
    let outer = square(0.0, 0.0, 30.0, true);
    let notch = "M0.00,10.00L0.00,20.00L10.00,20.00L10.00,10.00L0.00,10.00Z";
    assert_same_fill(&format!("{outer}{notch}"));
}

/// A reversed ring of cubics written with `S` keeps every `S` that is exact in reverse,
/// and reads back as the same curves in the other order.
#[test]
fn smooth_cubics_reverse_exactly() {
    let outer = square(0.0, 0.0, 32.0, true);
    // Anticlockwise on the screen like the outline, so it is reversed.
    let blob = "M8.00,16.00C8.00,20.42 11.58,24.00 16.00,24.00S24.00,20.42 24.00,16.00\
                S20.42,8.00 16.00,8.00S8.00,11.58 8.00,16.00Z";
    let d = format!("{outer}{blob}");
    let wound = assert_same_fill(&d);
    let subs = parse(&wound).unwrap();
    let rev = &subs[1];
    assert!(rev.text.contains('S'), "{}", rev.text);
    let fwd = &parse(blob).unwrap()[0];
    let n = fwd.segs.len();
    for k in 0..n {
        let (Seg::Cubic(a1, a2, _), Seg::Cubic(b1, b2, _)) = (&fwd.segs[k], &rev.segs[n - 1 - k])
        else {
            panic!("not cubics");
        };
        assert!(point(a1).dist(point(b2)) < 1e-12 && point(a2).dist(point(b1)) < 1e-12);
    }
}

/// An arc reversed keeps its radii, rotation and large-arc flag and flips its sweep, and
/// the ring it closes encloses the same region.
#[test]
fn arcs_reverse_by_their_sweep_flag() {
    let outer = circle(16.0, 16.0, 14.0);
    let hole = circle(16.0, 16.0, 6.0);
    let wound = assert_same_fill(&format!("{outer}{hole}"));
    let subs = parse(&wound).unwrap();
    assert!(matches!(
        subs[1].segs[0],
        Seg::Arc {
            sweep: true,
            large: true,
            ..
        }
    ));
    let a0 = measure(&parse(&hole).unwrap()[0]).signed;
    let a1 = measure(&subs[1]).signed;
    assert!((a0 + a1).abs() < 1e-9 * a0.abs(), "{a0} vs {a1}");
}

/// A ring whose last point is not its first closes with `Z`'s implicit line; reversed, it
/// starts where it ended and the implicit line runs the other way.
#[test]
fn an_implicit_closing_line_stays_implicit() {
    let outer = square(0.0, 0.0, 30.0, true);
    let open_hole = "M10.00,10.00L10.00,20.00L20.00,20.00L20.00,10.00Z";
    let wound = assert_same_fill(&format!("{outer}{open_hole}"));
    assert!(
        wound.ends_with("M20.00,10.00L20.00,20.00L10.00,20.00L10.00,10.00Z"),
        "{wound}"
    );
}
