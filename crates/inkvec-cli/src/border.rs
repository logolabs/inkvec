//! Art that touches the image border: trace it on a canvas two pixels larger, then move
//! the document back onto the original canvas.
//!
//! # The problem
//!
//! A transparent raster whose artwork reaches its outermost ring of pixels (a tightly
//! cropped logo, a sticker filling its canvas) is traced worse than the same artwork with a
//! little room round it. Where the art meets the canvas edge the planar map has frame
//! edges and junction nodes pinned to the frame, and the boundaries that end there are
//! fitted as open curves between two frame junctions instead of the closed outline the
//! artist drew; a rounded square filling its canvas (`twemoji/1f7eb`) came back with one
//! corner replaced by an arc spanning half a side. Measured by the r2-eval research
//! (2026-10-02, `padtest.py`): embedding the raster in a 2 px transparent margin, tracing,
//! and cropping the render back lowered the 246-icon screen set's dE00 by 2.0 % at 512 px
//! and 5.3 % at 128 px; on the 72 icons that touch the border by 9.7 % and 22.1 %, the rest
//! moving by +0.1 % and +1.7 %. The rest moved only because the padded raster is larger,
//! and the tracer's size-dependent defaults (the fit's lambda `ln(extent / precision)`, the
//! 128 px reference extent of the knob pricing) read the larger size.
//!
//! # What this does
//!
//! 1. [`touches_border`]: the raster has transparency somewhere and a pixel of its outer
//!    ring is not fully transparent. Only then is anything done; every other raster is
//!    traced exactly as before.
//! 2. [`pad`]: the raster embedded at `(PAD, PAD)` in a canvas `PAD` px larger on every
//!    side, the new pixels fully transparent. Transparent is the ground such a raster
//!    already has, so the art now ends inside the canvas, on a closed outline.
//! 3. The caller traces the padded raster with the fit configuration of the *original*
//!    size (`crate::units::fit_config_sized`), and with the knob pricing already done on
//!    the original raster (`crate::intake` runs before this), so the size-dependent
//!    defaults the CLI owns see the size they saw before.
//! 4. [`crop`]: every coordinate of the traced document moved by `(−PAD, −PAD)`, and the
//!    root header written for the original size. A translation is exact, so the drawing
//!    on the original canvas is the padded trace's drawing there; the pad's own pixels are
//!    transparent and the pad holds no ink, so nothing visible is cut off. The document
//!    keeps the emitter's header and coordinates in the original raster's pixels, which is
//!    what `crate::post` (the margin, the background knock-out) and the Studio overlays
//!    read.
//!
//! [`crop`] translates only what the colour emitter writes (listed on the function) and
//! returns `None` on anything else; the caller then traces the unpadded raster, as before.
//!
//! Not from the literature: a boundary condition for our own planar map (zero padding, the
//! usual way image filtering gives a support that reaches past the image a known exterior,
//! applied here to the tracer as a whole), because the published vectorisers we know of do
//! not discuss art cut by the canvas. See also: the metamorphic relation that found the
//! defect (a one-pixel translation of the input must not change the result), after T. Y.
//! Chen, S. C. Cheung, S. M. Yiu (1998), *Metamorphic testing: a new approach for
//! generating next test cases*, Technical Report HKUST-CS98-01, Hong Kong University of
//! Science and Technology, <https://arxiv.org/abs/2002.12543>.

use inkvec_trace::Rgba;

/// Transparent pixels added on every side, the margin the r2-eval measurement used.
pub(crate) const PAD: usize = 2;

/// Whether `img` has art on its border that padding can help: some pixel is not fully
/// opaque (so transparency is the raster's ground, and a transparent pad continues it), and
/// some pixel of the outermost ring has alpha above zero.
///
/// An opaque raster is never padded: its ground is whatever colour fills the border, a
/// transparent pad would turn it into a raster with transparency and send it down the alpha
/// path. The test reads alpha only; `O(w·h)` for the transparency scan, which stops at the
/// first non-opaque pixel, and `O(w + h)` for the ring.
pub(crate) fn touches_border(img: &Rgba) -> bool {
    let (w, h) = (img.width, img.height);
    if w == 0 || h == 0 || img.data.len() < 4 * w * h {
        return false;
    }
    let alpha = |x: usize, y: usize| img.data[(y * w + x) * 4 + 3];
    let transparent_somewhere = img.data[..4 * w * h]
        .as_chunks::<4>()
        .0
        .iter()
        .any(|p| p[3] < 1.0);
    if !transparent_somewhere {
        return false;
    }
    let row = |y: usize| (0..w).any(|x| alpha(x, y) > 0.0);
    let col = |x: usize| (0..h).any(|y| alpha(x, y) > 0.0);
    row(0) || row(h - 1) || col(0) || col(w - 1)
}

/// `img` embedded at `(PAD, PAD)` in a `(w + 2·PAD) × (h + 2·PAD)` raster whose new pixels
/// are `[0, 0, 0, 0]` (fully transparent, straight alpha; the colour of a transparent pixel
/// is never read once it is matted, and zero is what a decoder hands back for one).
pub(crate) fn pad(img: &Rgba) -> Rgba {
    let (w, h) = (img.width, img.height);
    let (pw, ph) = (w + 2 * PAD, h + 2 * PAD);
    let mut data = vec![0.0f32; pw * ph * 4];
    for y in 0..h {
        let src = &img.data[y * w * 4..(y + 1) * w * 4];
        let at = ((y + PAD) * pw + PAD) * 4;
        data[at..at + w * 4].copy_from_slice(src);
    }
    Rgba {
        width: pw,
        height: ph,
        data,
    }
}

/// The document traced on the padded canvas, moved back onto the original `w × h` one:
/// every coordinate translated by `(−PAD, −PAD)` and the root's
/// `viewBox="-0.5 -0.5 {w + 2·PAD} {h + 2·PAD}" width=… height=…` written for `w × h`.
///
/// What is translated, element by element (everything the colour emitter writes):
///
/// * `<path d>`: the coordinates of every absolute command (`M L T` pairs, `H` x, `V` y,
///   `C S Q` control and end points, `A`'s end point but not its radii, rotation or flags);
///   relative commands move nothing, except a path's first `m`, which is absolute;
/// * `<rect x y>`, `<circle cx cy>`, `<ellipse cx cy>`, `<line x1 y1 x2 y2>`, and an
///   ellipse's `transform="rotate(a cx cy)"` centre;
/// * `<linearGradient x1 y1 x2 y2>` and `<radialGradient cx cy fx fy>` in
///   `userSpaceOnUse` units; when either carries a `gradientTransform`, the translation is
///   put in front of it instead (`translate(−PAD −PAD) …`, folded into a leading
///   `translate`), which is the same map. `objectBoundingBox` gradients are relative to
///   the shape and need nothing.
///
/// Lengths (`width`, `height`, `r`, `rx`, `ry`, stroke widths) and colours are untouched.
/// Each number keeps the number of decimals it was written with; the shift is a whole
/// number of pixels, so the result is the exact decimal.
///
/// `None`, leaving the caller to trace the unpadded raster, when the document holds
/// anything else that carries coordinates: `<use>`, `<symbol>`, `<image>`, `<text>`,
/// `<pattern>`, `<mask>`, `<clipPath>`, `<polygon>`, `<polyline>`, `<filter>`, any other
/// `transform`, a shape inside `<defs>` (symbol geometry, in its own coordinates), a header
/// that is not the emitter's, or path data that does not parse.
pub(crate) fn crop(svg: &str, w: usize, h: usize) -> Option<String> {
    let p = PAD as f64;
    let mut out = String::with_capacity(svg.len());
    let mut rest = svg;
    let mut root_done = false;
    let mut in_defs = 0usize;
    while let Some(lt) = rest.find('<') {
        out.push_str(&rest[..lt]);
        rest = &rest[lt..];
        // Comments, and the XML declaration or processing instructions, pass through.
        if rest.starts_with("<!--") {
            let end = rest.find("-->")? + 3;
            out.push_str(&rest[..end]);
            rest = &rest[end..];
            continue;
        }
        let end = rest.find('>')? + 1;
        let tag = &rest[..end];
        rest = &rest[end..];
        if tag.starts_with("<?") || tag.starts_with("</") {
            if tag == "</defs>" {
                in_defs = in_defs.checked_sub(1)?;
            }
            out.push_str(tag);
            continue;
        }
        let name_end = tag[1..]
            .find(|c: char| c.is_whitespace() || c == '>' || c == '/')
            .map(|i| i + 1)?;
        let name = &tag[1..name_end];
        let self_closing = tag.ends_with("/>");
        if !root_done {
            if name != "svg" {
                return None;
            }
            out.push_str(&root_header(tag, w, h)?);
            root_done = true;
            continue;
        }
        let moved = match name {
            "defs" => {
                if !self_closing {
                    in_defs += 1;
                }
                Some(tag.to_string())
            }
            "g" | "stop" | "title" | "desc" | "metadata" => {
                if attr(tag, "transform").is_some() {
                    None
                } else {
                    Some(tag.to_string())
                }
            }
            "path" | "rect" | "circle" | "ellipse" | "line" if in_defs > 0 => None,
            "path" => shift_path(tag, p),
            "rect" => no_transform(tag).and_then(|t| shift_attrs(&t, &["x", "y"], p)),
            "circle" => no_transform(tag).and_then(|t| shift_attrs(&t, &["cx", "cy"], p)),
            "line" => no_transform(tag).and_then(|t| shift_attrs(&t, &["x1", "y1", "x2", "y2"], p)),
            "ellipse" => shift_ellipse(tag, p),
            "linearGradient" => shift_gradient(tag, &["x1", "y1", "x2", "y2"], p),
            "radialGradient" => shift_gradient(tag, &["cx", "cy", "fx", "fy"], p),
            // Elements of the RDF metadata block, should one already be present.
            n if n.starts_with("rdf:") || n.starts_with("dc:") => Some(tag.to_string()),
            _ => None,
        }?;
        out.push_str(&moved);
    }
    out.push_str(rest);
    root_done.then_some(out)
}

/// The root tag with the padded canvas's `viewBox`, `width` and `height` replaced by the
/// original size's, or `None` when the tag is not exactly the emitter's header for a
/// padded canvas (`viewBox="-0.5 -0.5 {w+2P} {h+2P}" width="{w+2P}" height="{h+2P}"`).
fn root_header(tag: &str, w: usize, h: usize) -> Option<String> {
    let (pw, ph) = (w + 2 * PAD, h + 2 * PAD);
    let old = format!("viewBox=\"-0.5 -0.5 {pw} {ph}\" width=\"{pw}\" height=\"{ph}\"");
    let new = format!("viewBox=\"-0.5 -0.5 {w} {h}\" width=\"{w}\" height=\"{h}\"");
    tag.contains(&old).then(|| tag.replacen(&old, &new, 1))
}

/// The value of attribute `name` in `tag` (double-quoted, as the emitter writes), with the
/// byte range of the value.
fn attr<'a>(tag: &'a str, name: &str) -> Option<(&'a str, std::ops::Range<usize>)> {
    let key = format!(" {name}=\"");
    let at = tag.find(&key)? + key.len();
    let len = tag[at..].find('"')?;
    Some((&tag[at..at + len], at..at + len))
}

/// `tag` itself when it has no `transform` attribute.
fn no_transform(tag: &str) -> Option<String> {
    attr(tag, "transform").is_none().then(|| tag.to_string())
}

/// A decimal number moved by `−p`, written with the decimals it had (at least none).
fn shift_number(text: &str, p: f64) -> Option<String> {
    let v: f64 = text.trim().parse().ok()?;
    if !v.is_finite() {
        return None;
    }
    let decimals = text.split_once('.').map_or(0, |(_, f)| f.len());
    Some(format!("{:.*}", decimals, v - p))
}

/// `tag` with each of `names` that is present moved by `−p`.
fn shift_attrs(tag: &str, names: &[&str], p: f64) -> Option<String> {
    let mut t = tag.to_string();
    for n in names {
        if let Some((v, range)) = attr(&t, n) {
            let s = shift_number(v, p)?;
            t.replace_range(range, &s);
        }
    }
    Some(t)
}

/// An ellipse: its centre, and the centre of a `rotate(a cx cy)` transform, the only
/// transform the emitter puts on one.
fn shift_ellipse(tag: &str, p: f64) -> Option<String> {
    let mut t = shift_attrs(tag, &["cx", "cy"], p)?;
    if let Some((v, range)) = attr(&t, "transform") {
        let inner = v.trim().strip_prefix("rotate(")?.strip_suffix(')')?;
        let parts: Vec<&str> = inner
            .split(|c: char| c == ',' || c.is_whitespace())
            .filter(|s| !s.is_empty())
            .collect();
        let new = match parts.as_slice() {
            [a] => format!("rotate({a})"),
            [a, cx, cy] => format!(
                "rotate({a} {} {})",
                shift_number(cx, p)?,
                shift_number(cy, p)?
            ),
            _ => return None,
        };
        // A bare rotate(a) turns about the origin, which moves with the translation: it is
        // no longer the same map once the centre is shifted. Refuse it.
        if parts.len() == 1 {
            return None;
        }
        t.replace_range(range, &new);
    }
    Some(t)
}

/// A gradient in `userSpaceOnUse` units: its points moved, or, when it carries a
/// `gradientTransform`, the translation put in front of that transform.
fn shift_gradient(tag: &str, names: &[&str], p: f64) -> Option<String> {
    let user = attr(tag, "gradientUnits").is_some_and(|(v, _)| v == "userSpaceOnUse");
    if !user {
        return Some(tag.to_string());
    }
    if attr(tag, "transform").is_some() {
        return None;
    }
    if let Some((v, range)) = attr(tag, "gradientTransform") {
        let mut t = tag.to_string();
        t.replace_range(range, &prepend_translate(v, p)?);
        return Some(t);
    }
    shift_attrs(tag, names, p)
}

/// `translate(−p −p)` composed in front of the transform list `list`: folded into a leading
/// `translate(tx ty)` when there is one (`translate(tx − p ty − p) …`), else written before
/// it. The same map either way: `T(−p)·T(t) = T(t − p)`.
fn prepend_translate(list: &str, p: f64) -> Option<String> {
    let s = list.trim();
    if let Some(after) = s.strip_prefix("translate(") {
        let close = after.find(')')?;
        let parts: Vec<&str> = after[..close]
            .split(|c: char| c == ',' || c.is_whitespace())
            .filter(|s| !s.is_empty())
            .collect();
        if let [tx, ty] = parts.as_slice() {
            return Some(format!(
                "translate({} {}){}",
                shift_number(tx, p)?,
                shift_number(ty, p)?,
                &after[close + 1..]
            ));
        }
    }
    Some(format!("translate({} {}) {s}", -p, -p))
}

/// A `<path>` with every absolute coordinate of its `d` moved by `−p` (see [`crop`]), or
/// `None` when it has a `transform` or its data does not parse.
fn shift_path(tag: &str, p: f64) -> Option<String> {
    if attr(tag, "transform").is_some() {
        return None;
    }
    let Some((d, range)) = attr(tag, "d") else {
        return Some(tag.to_string());
    };
    let moved = shift_path_data(d, p)?;
    let mut t = tag.to_string();
    t.replace_range(range, &moved);
    Some(t)
}

/// Path data with every absolute coordinate moved by `−p`; separators and the numbers that
/// do not move are copied as they were.
///
/// A token is a command letter or a number (`-?digits[.digits][e±digits]`); a command's
/// numbers are taken in groups of its argument count (a command letter may be followed by
/// several groups), and within a group the positions holding an x or a y are moved. A
/// number that does not fit its command's grammar (an incomplete group, a number before
/// any command) makes the whole data refused.
fn shift_path_data(d: &str, p: f64) -> Option<String> {
    // Per command: the argument count, and for each argument 0 (not a coordinate), 1 (x)
    // or 2 (y).
    fn shape(c: char) -> Option<&'static [u8]> {
        Some(match c.to_ascii_uppercase() {
            'M' | 'L' | 'T' => &[1, 2],
            'H' => &[1],
            'V' => &[2],
            'C' => &[1, 2, 1, 2, 1, 2],
            'S' | 'Q' => &[1, 2, 1, 2],
            'A' => &[0, 0, 0, 0, 0, 1, 2],
            'Z' => &[],
            _ => return None,
        })
    }
    let b = d.as_bytes();
    let mut out = String::with_capacity(d.len() + 8);
    let mut i = 0usize;
    let mut cmd: Option<char> = None;
    // The position within the current group of arguments, and the groups completed since
    // the last command letter. A counter that wraps by comparison rather than `%`: wazero's
    // arm64 compiler miscompiled an `i32.rem_u` in a loop once (see `band.rs`).
    let (mut slot, mut group) = (0usize, 0usize);
    // Whether the current command is a relative `m` that opens the path (its first pair is
    // absolute by the grammar).
    let mut first = true;
    let mut first_m_abs = false;
    while i < b.len() {
        let c = b[i] as char;
        if c.is_ascii_alphabetic() && c != 'e' && c != 'E' {
            if slot != 0 {
                return None;
            }
            shape(c)?;
            first_m_abs = first && c == 'm';
            first = false;
            cmd = Some(c);
            group = 0;
            out.push(c);
            i += 1;
            continue;
        }
        if c == ',' || c.is_ascii_whitespace() {
            out.push(c);
            i += 1;
            continue;
        }
        // A number.
        let start = i;
        if b[i] == b'-' || b[i] == b'+' {
            i += 1;
        }
        let mut seen_dot = false;
        while i < b.len() && (b[i].is_ascii_digit() || (b[i] == b'.' && !seen_dot)) {
            seen_dot |= b[i] == b'.';
            i += 1;
        }
        if i < b.len() && (b[i] == b'e' || b[i] == b'E') {
            i += 1;
            if i < b.len() && (b[i] == b'-' || b[i] == b'+') {
                i += 1;
            }
            while i < b.len() && b[i].is_ascii_digit() {
                i += 1;
            }
        }
        if i == start {
            return None;
        }
        let tok = &d[start..i];
        let k = cmd?;
        let s = shape(k)?;
        if s.is_empty() {
            return None;
        }
        let role = s[slot];
        // A relative command moves nothing, except the coordinates of the path's first `m`
        // in its first group (absolute by definition).
        let absolute = k.is_ascii_uppercase() || (first_m_abs && group == 0);
        if role != 0 && absolute {
            out.push_str(&shift_number(tok, p)?);
        } else {
            out.push_str(tok);
        }
        slot += 1;
        if slot == s.len() {
            slot = 0;
            group += 1;
        }
    }
    // An incomplete last group.
    if slot != 0 {
        return None;
    }
    Some(out)
}

#[cfg(test)]
#[path = "border_tests.rs"]
mod tests;
