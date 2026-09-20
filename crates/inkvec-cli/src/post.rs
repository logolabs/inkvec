//! What happens to the document after the tracer has finished with it.
//!
//! Each of these rewrites finished SVG text rather than geometry, and each is optional and
//! independent: retarget the viewBox, drop the canvas-filling background face, add a
//! margin, strip the bytes that carry no meaning. They compose in [`post_process`] in the
//! order the flags imply.

use crate::Args;

/// Make an SVG traced at one size render at another, by leaving the geometry alone
/// and changing only the presentation size.
pub(crate) fn retarget(svg: &str, w: usize, h: usize) -> String {
    // `emit_color` and `emit_bilevel` both write the same header shape, and only
    // the first occurrence is the root element's own size.
    if let Some(i) = svg.find("width=\"") {
        if let Some(j) = svg[i..].find('>') {
            let head = &svg[i..i + j];
            if let Some(k) = head.find("height=\"") {
                if let Some(e) = head[k..]
                    .find('"')
                    .and_then(|q| head[k + q + 1..].find('"').map(|r| k + q + 1 + r + 1))
                {
                    return format!("{}width=\"{w}\" height=\"{h}\"{}", &svg[..i], &svg[i + e..]);
                }
            }
        }
    }
    svg.to_string()
}

/// Remove the element that paints the whole canvas, so the artwork sits on
/// transparency. The emitter writes that face either as a `<rect>` spanning the
/// canvas or, when it has holes, as a path whose outer ring visits all four canvas
/// corners. The path carries the corners verbatim and is matched as text; the `<rect>` is a
/// fitted primitive whose edges carry the fit's error, so it is matched by its numbers, to a
/// quarter of a pixel. Children of that face are separate elements and stay. Nothing is
/// removed when no element matches, so an image with no flat background is returned unchanged.
///
/// **The corner strings are built at the emitter's own decimal count, not at a literal
/// two.** This is a string match against text another module wrote, so the two have to
/// agree on how many digits that text carries; until 2026-09-08 the digits here were
/// hard-coded, and setting `INKVEC_EMIT_DECIMALS` to anything else made every match fail
/// silently -- `--no-background` would simply stop removing the background, with no error
/// and no warning. Anything that formats a coordinate for comparison against emitted SVG
/// belongs downstream of `emit_decimals`.
pub(crate) fn knock_out_background(svg: String, w: usize, h: usize) -> String {
    let d = crate::emit::emit_decimals(0.0);
    let (x1, y1) = (w as f64 - 0.5, h as f64 - 0.5);
    // A rectangle fitted to the canvas edges lands a hundredth of a pixel or two off them: a
    // 64 px canvas came back as x=-0.51 y=-0.49 width=64.02 height=63.98, failed a text match
    // against the exact corners, and `--no-background` quietly left the background in.
    const CANVAS_TOL: f64 = 0.25;
    let attr = |el: &str, name: &str| -> Option<f64> {
        let key = format!(" {name}=\"");
        let i = el.find(&key)? + key.len();
        let j = el[i..].find('"')? + i;
        el[i..j].parse().ok()
    };
    let spans_canvas = |el: &str| -> bool {
        match (
            attr(el, "x"),
            attr(el, "y"),
            attr(el, "width"),
            attr(el, "height"),
        ) {
            (Some(x), Some(y), Some(rw), Some(rh)) => {
                (x + 0.5).abs() <= CANVAS_TOL
                    && (y + 0.5).abs() <= CANVAS_TOL
                    && (x + rw - x1).abs() <= CANVAS_TOL
                    && (y + rh - y1).abs() <= CANVAS_TOL
            }
            _ => false,
        }
    };
    let corners = [
        format!("{:.*},{:.*}", d, -0.5, d, -0.5),
        format!("{x1:.*},{:.*}", d, d, -0.5),
        format!("{x1:.*},{y1:.*}", d, d),
        format!("{:.*},{y1:.*}", d, -0.5, d),
    ];
    let mut from = 0usize;
    while let Some(off) = svg[from..].find('<') {
        let start = from + off;
        let rest = &svg[start..];
        let is_rect = rest.starts_with("<rect");
        let is_path = rest.starts_with("<path");
        let end = match rest.find("/>") {
            Some(e) => start + e + 2,
            None => break,
        };
        if is_rect || is_path {
            let el = &svg[start..end];
            let hit = if is_rect {
                spans_canvas(el)
            } else {
                corners.iter().all(|c| el.contains(c.as_str()))
            };
            if hit {
                let mut out = String::with_capacity(svg.len());
                out.push_str(&svg[..start]);
                out.push_str(&svg[end..]);
                return out;
            }
        }
        from = start + 1;
    }
    svg
}

/// Strip unreferenced ids, empty groups and trailing zeros in generated SVG.
pub(crate) fn minify_svg(svg: &str) -> String {
    // ids and the groups that exist only to carry them
    let mut s = String::with_capacity(svg.len());
    let mut rest = svg;
    while let Some(i) = rest.find(" id=\"") {
        s.push_str(&rest[..i]);
        let after = &rest[i + 5..];
        match after.find('"') {
            Some(j) => {
                // Gradients and reusable shapes need their ids. Conservatively
                // keep any id appearing after '#'; false positives only cost bytes.
                if svg.contains(&format!("#{}", &after[..j])) {
                    s.push_str(&rest[i..i + 5 + j + 1]);
                }
                rest = &after[j + 1..];
            }
            None => {
                rest = after;
                break;
            }
        }
    }
    s.push_str(rest);
    // Remove a group's closing tag only when its matching opening tag was
    // removed. Transform, opacity and referenced groups must stay balanced.
    let mut flattened = String::with_capacity(s.len());
    let mut groups = Vec::new();
    let mut remaining = s.as_str();
    while let Some(i) = remaining.find('<') {
        flattened.push_str(&remaining[..i]);
        let Some(end) = remaining[i..].find('>') else {
            break;
        };
        let tag = &remaining[i..i + end + 1];
        let keep = if tag == "<g>" {
            groups.push(false);
            false
        } else if tag.starts_with("<g ") && !tag.ends_with("/>") {
            groups.push(true);
            true
        } else if tag == "</g>" {
            groups.pop().unwrap_or(true)
        } else {
            true
        };
        if keep {
            flattened.push_str(tag);
        }
        remaining = &remaining[i + end + 1..];
    }
    flattened.push_str(remaining);
    let s = flattened;
    // numbers: 12.50 -> 12.5, 3.00 -> 3, -0.50 -> -0.5. A token is a decimal only when
    // it has digits on both sides of its dot, so "www.w3.org" and "#1428a0" pass through.
    let b = s.as_bytes();
    let mut out = String::with_capacity(s.len());
    let mut i = 0;
    while i < b.len() {
        let c = b[i];
        let starts =
            c.is_ascii_digit() || (c == b'-' && i + 1 < b.len() && b[i + 1].is_ascii_digit());
        if starts && (i == 0 || b[i - 1] != b'#') {
            let start = i;
            i += 1;
            while i < b.len() && b[i].is_ascii_digit() {
                i += 1;
            }
            if i + 1 < b.len() && b[i] == b'.' && b[i + 1].is_ascii_digit() {
                i += 1;
                while i < b.len() && b[i].is_ascii_digit() {
                    i += 1;
                }
                let t = s[start..i].trim_end_matches('0').trim_end_matches('.');
                out.push_str(if t == "-0" || t.is_empty() { "0" } else { t });
            } else {
                out.push_str(&s[start..i]);
            }
        } else {
            out.push(c as char);
            i += 1;
        }
    }
    out
}

/// Grow the viewBox by `margin` of the larger side on every edge. The geometry does
/// not move: the artwork keeps its coordinates and gains transparent breathing room, so
/// a logo traced from a tight raster no longer sits flush against its own frame.
///
/// `w` x `h` is the viewBox, the size that was traced. The presented `width`/`height` that
/// follow it are read from the document rather than assumed equal: an input reduced for
/// tracing (`--max-dim`, or an undone pixel-block upscale) is presented at the size that
/// arrived, and each presented side grows in proportion to its viewBox side. Matching the
/// presented size against `w` x `h` used to drop the margin silently on every such input.
#[allow(clippy::neg_cmp_op_on_partial_ord)]
pub(crate) fn with_margin(svg: String, w: usize, h: usize, margin: f64) -> String {
    // Negated deliberately: `--margin nan` should leave the document alone, and it would
    // pass `margin <= 0.0`.
    if !(margin > 0.0) {
        return svg;
    }
    let head = format!("viewBox=\"-0.5 -0.5 {w} {h}\" width=\"");
    let Some(start) = svg.find(&head) else {
        return svg;
    };
    // `<dw>" height="<dh>"` follows the head.
    let tail = &svg[start + head.len()..];
    let parsed = tail.split_once('"').and_then(|(dw, rest)| {
        let (dh, _) = rest.strip_prefix(" height=\"")?.split_once('"')?;
        let end = head.len() + dw.len() + " height=\"".len() + 1 + dh.len() + 1;
        Some((dw.parse::<f64>().ok()?, dh.parse::<f64>().ok()?, end))
    });
    let Some((dw, dh, end)) = parsed else {
        return svg;
    };
    let m = margin * w.max(h) as f64;
    let (vw, vh) = (w as f64 + 2.0 * m, h as f64 + 2.0 * m);
    // Unretargeted, the presented size is the viewBox size, written exactly as before.
    let grow = |d: f64, side: usize, v: f64| {
        if d == side as f64 {
            v
        } else {
            d * v / side as f64
        }
    };
    let grown = format!(
        "viewBox=\"{:.2} {:.2} {:.2} {:.2}\" width=\"{:.2}\" height=\"{:.2}\"",
        -0.5 - m,
        -0.5 - m,
        vw,
        vh,
        grow(dw, w, vw),
        grow(dh, h, vh)
    );
    let mut out = String::with_capacity(svg.len() + 32);
    out.push_str(&svg[..start]);
    out.push_str(&grown);
    out.push_str(&svg[start + end..]);
    out
}

/// The output options, in the order they compose: background knock-out, minify, margin.
pub fn post_process(args: &Args, svg: String, w: usize, h: usize) -> String {
    let svg = if args.no_background {
        knock_out_background(svg, w, h)
    } else {
        svg
    };
    let svg = if args.minify {
        compact_paths(minify_svg(&svg))
    } else {
        svg
    };
    with_margin(svg, w, h, args.margin)
}

/// Write the path data in the fewest bytes, without moving anything.
///
/// The emitter chooses its coordinates carefully and then spells them out in full:
/// absolute commands, every letter, every separator. Handing the same numbers to
/// `inkvec-svgmin`'s writer — relative where that is shorter, repeated letters and
/// needless separators dropped, `H`/`V`/`S` where they say the same thing — takes about a
/// twelfth of the file back with nothing moved at all.
///
/// Nothing is rounded, and that is ten points left on the table on purpose. Rounding to
/// two decimals takes 18% instead of 8.5%, but on a gradient-heavy trace it moves a
/// hundred pixels by as much as a fifth of a channel: the emitter writes two decimals for
/// path data and more than that elsewhere, so two decimals is not the precision it chose.
/// `--minify` promises the same geometry, so it keeps the same geometry;
/// `inkvec-svgmin --decimals` is where precision is spent for bytes on purpose.
fn compact_paths(svg: String) -> String {
    match inkvec_svgmin::compact(&svg, &inkvec_svgmin::Options::default()) {
        Ok((out, _)) if out.len() < svg.len() => out,
        _ => svg,
    }
}

#[cfg(test)]
mod hygiene_tests {
    use super::{knock_out_background, minify_svg, with_margin};

    #[test]
    fn margin_grows_the_viewbox_and_the_presented_size() {
        let svg = |dw: u32| {
            format!("<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"-0.5 -0.5 48 48\" width=\"{dw}\" height=\"{dw}\"><path d=\"M1,1L2,2Z\"/></svg>")
        };
        let grown = "viewBox=\"-5.30 -5.30 57.60 57.60\" width=\"57.60\" height=\"57.60\"";
        assert!(with_margin(svg(48), 48, 48, 0.1).contains(grown));
        // Traced at 48 and presented at 96 (a capped input): the margin is kept and the
        // presented size grows by the same proportion.
        let capped = with_margin(svg(96), 48, 48, 0.1);
        assert!(
            capped
                .contains("viewBox=\"-5.30 -5.30 57.60 57.60\" width=\"115.20\" height=\"115.20\""),
            "{capped}"
        );
        assert!(capped.ends_with("<path d=\"M1,1L2,2Z\"/></svg>"));
        assert_eq!(with_margin(svg(96), 48, 48, 0.0), svg(96));
        assert_eq!(with_margin(svg(96), 48, 48, f64::NAN), svg(96));
    }

    #[test]
    fn knock_out_matches_a_fitted_canvas_rect_to_a_quarter_pixel() {
        let fitted = "<svg viewBox=\"-0.5 -0.5 64 64\"><rect id=\"white-1\" x=\"-0.51\" y=\"-0.49\" width=\"64.02\" height=\"63.98\" fill=\"#ffffff\"/><rect id=\"black-1\" x=\"15.50\" y=\"15.50\" width=\"32.00\" height=\"32.00\" fill=\"#000000\"/></svg>".to_string();
        let out = knock_out_background(fitted, 64, 64);
        assert!(!out.contains("white-1"), "{out}");
        assert!(out.contains("black-1"), "{out}");
        // A shape that stops a pixel short of the canvas is artwork, not background.
        let inset = "<svg viewBox=\"-0.5 -0.5 64 64\"><rect id=\"panel\" x=\"0.50\" y=\"-0.50\" width=\"63.00\" height=\"64.00\" fill=\"#ffffff\"/></svg>".to_string();
        assert!(knock_out_background(inset, 64, 64).contains("panel"));
    }

    #[test]
    fn knock_out_removes_only_the_canvas_face() {
        let svg = "<svg viewBox=\"-0.5 -0.5 4 4\"><g id=\"white-1-group\"><rect id=\"white-1\" x=\"-0.50\" y=\"-0.50\" width=\"4.00\" height=\"4.00\" fill=\"#ffffff\"/><path id=\"red-2\" d=\"M1.00,1.00L2.00,1.00L2.00,2.00Z\" fill=\"#ff0000\"/></g></svg>".to_string();
        let out = knock_out_background(svg, 4, 4);
        assert!(!out.contains("<rect"));
        assert!(out.contains("red-2"));
        let holed = "<svg><path id=\"bg-1\" d=\"M-0.50,-0.50L3.50,-0.50L3.50,3.50L-0.50,3.50ZM1.00,1.00L2.00,1.00L2.00,2.00Z\" fill=\"#fff\"/><path id=\"a-2\" d=\"M1.00,1.00L2.00,2.00Z\"/></svg>".to_string();
        let out = knock_out_background(holed, 4, 4);
        assert!(!out.contains("bg-1") && out.contains("a-2"));
        let none = "<svg><path id=\"a-1\" d=\"M1.00,1.00L2.00,2.00Z\"/></svg>".to_string();
        assert_eq!(knock_out_background(none.clone(), 4, 4), none);
    }

    #[test]
    fn minify_strips_ids_groups_and_zeros() {
        let svg = "<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"-0.5 -0.5 128 128\" width=\"128\" height=\"128\"><g id=\"white-1-group\"><rect id=\"white-1\" x=\"-0.50\" y=\"-0.50\" width=\"128.00\" height=\"128.00\" fill=\"#1428a0\"/><path id=\"p-2\" d=\"M12.50,3.00C1.10,2.00 0.00,-0.50 7.25,7.20Z\" fill=\"#ff00a0\"/></g></svg>";
        let out = minify_svg(svg);
        assert_eq!(out, "<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"-0.5 -0.5 128 128\" width=\"128\" height=\"128\"><rect x=\"-0.5\" y=\"-0.5\" width=\"128\" height=\"128\" fill=\"#1428a0\"/><path d=\"M12.5,3C1.1,2 0,-0.5 7.25,7.2Z\" fill=\"#ff00a0\"/></svg>");
    }

    #[test]
    fn minify_preserves_gradient_and_shape_references() {
        let svg = "<svg><defs><linearGradient id=\"shade\"><stop offset=\"0\" stop-color=\"red\"/></linearGradient><path id=\"shape\" d=\"M0,0L2,2\"/></defs><g id=\"unused\"><use href=\"#shape\" fill=\"url(#shade)\"/></g></svg>";
        let out = minify_svg(svg);
        assert!(out.contains("id=\"shade\""));
        assert!(out.contains("id=\"shape\""));
        assert!(out.contains("href=\"#shape\""));
        assert!(out.contains("url(#shade)"));
        assert!(!out.contains("unused"));
    }

    #[test]
    fn minify_keeps_attribute_groups_balanced() {
        let svg = "<svg><g transform=\"translate(2,3)\"><g id=\"unused\"><path d=\"M0,0L1,1\"/></g></g></svg>";
        assert_eq!(
            minify_svg(svg),
            "<svg><g transform=\"translate(2,3)\"><path d=\"M0,0L1,1\"/></g></svg>"
        );
        let referenced =
            "<svg><g id=\"repeat\"><path d=\"M0,0L1,1\"/></g><use href=\"#repeat\"/></svg>";
        assert_eq!(minify_svg(referenced), referenced);
    }
}
