//! Everything in an SVG that is not path geometry.
//!
//! The rest of this crate makes the drawing's *description* shorter: fewer segments,
//! spelled in fewer bytes. That leaves the document around it — colours written six digits
//! where three would do, attributes restating a value they already inherit, editor
//! metadata nothing renders, whitespace between tags. On the tracer's own output the path
//! data is most of the file and this is worth a few percent; on a file that came out of a
//! drawing program it can be most of what is there.
//!
//! The rules here are re-implemented from [SVGO](https://github.com/svg/svgo) (MIT), whose
//! plugin set is the reference for what is safe to take out of an SVG and what is not. No
//! SVGO code is used; the behaviour of `convertColors`, `cleanupNumericValues`,
//! `removeUnknownsAndDefaults`, `removeComments`, `removeMetadata`, `removeDesc` and
//! `removeEmptyAttrs`, `cleanupIds`, `collapseGroups` and `moveElemsAttrsToGroup` is
//! reproduced from their documented contracts. Two are deliberately narrower than theirs:
//! a group is collapsed only when it carries no attributes at all, never by pushing a
//! transform or a clip path down into its children, and an attribute is hoisted onto a
//! group only when it is one the children actually inherit. `mergePaths` is not here at
//! all — whether two paths can become one depends on whether their fills overlap, which
//! is a question about geometry, not about markup.
//!
//! `<title>` is never touched and `<desc>` only when it is empty or is the drawing program
//! signing its work, because both are read aloud by a screen reader.
//!
//! Every change is a byte range replaced in the original text, so anything not named here
//! comes through exactly as it went in.

use std::ops::Range;

/// Attributes whose value is a paint, and can therefore be written as a colour.
const PAINT_ATTRS: &[&str] = &[
    "fill",
    "stroke",
    "stop-color",
    "flood-color",
    "lighting-color",
    "color",
    "solid-color",
];

/// Attributes whose value is a number, or a list of them.
const NUMERIC_ATTRS: &[&str] = &[
    "width",
    "height",
    "x",
    "y",
    "x1",
    "y1",
    "x2",
    "y2",
    "cx",
    "cy",
    "r",
    "rx",
    "ry",
    "offset",
    "opacity",
    "fill-opacity",
    "stroke-opacity",
    "stop-opacity",
    "stroke-width",
    "stroke-miterlimit",
    "stroke-dashoffset",
    "viewBox",
    "points",
];

/// Presentation attributes whose value is the one that would apply anyway. Removing these
/// is only safe when no ancestor sets the same property to something else, which is
/// exactly what SVGO's `uselessOverrides` check is for.
const DEFAULTS: &[(&str, &str)] = &[
    ("fill-rule", "nonzero"),
    ("clip-rule", "nonzero"),
    ("fill-opacity", "1"),
    ("stroke-opacity", "1"),
    ("stop-opacity", "1"),
    ("opacity", "1"),
    ("stroke", "none"),
    ("stroke-width", "1"),
    ("stroke-linecap", "butt"),
    ("stroke-linejoin", "miter"),
    ("stroke-miterlimit", "4"),
    ("stroke-dasharray", "none"),
    ("stroke-dashoffset", "0"),
    ("font-style", "normal"),
    ("font-weight", "normal"),
    ("font-stretch", "normal"),
    ("font-variant", "normal"),
    ("text-anchor", "start"),
    ("writing-mode", "lr-tb"),
    ("visibility", "visible"),
    ("display", "inline"),
    ("isolation", "auto"),
    ("mix-blend-mode", "normal"),
    ("paint-order", "normal"),
    ("vector-effect", "none"),
];

/// Elements that draw nothing and that nothing can reference.
const INERT: &[&str] = &["metadata"];

/// Presentation attributes a child inherits from its parent, so that setting one on the
/// group says exactly what setting it on every child said.
///
/// `opacity`, `filter`, `clip-path`, `mask` and `transform` are **not** here and must
/// never be: they apply to the group as a composited whole, and moving one up turns
/// twenty half-transparent shapes into one half-transparent picture of twenty shapes.
const INHERITED: &[&str] = &[
    "fill",
    "fill-rule",
    "fill-opacity",
    "stroke",
    "stroke-width",
    "stroke-linecap",
    "stroke-linejoin",
    "stroke-miterlimit",
    "stroke-dasharray",
    "stroke-dashoffset",
    "stroke-opacity",
    "color",
    "visibility",
    "font-family",
    "font-size",
    "font-weight",
    "font-style",
    "text-anchor",
];

/// Properties that can be written as an attribute instead of inside `style`.
fn is_presentation_property(name: &str) -> bool {
    PAINT_ATTRS.contains(&name)
        || INHERITED.contains(&name)
        || DEFAULTS.iter().any(|(n, _)| *n == name)
        || matches!(
            name,
            "opacity" | "display" | "clip-rule" | "shape-rendering" | "mix-blend-mode"
        )
}

/// `style="fill:#875334;"` written as `fill="#875334"`: SVGO's `convertStyleToAttrs`.
///
/// Only when every declaration is a presentation property, the element does not already
/// set that property as an attribute, and the document has no stylesheet and the element
/// no `class` — `style` outranks an attribute in the cascade, so this is only the same
/// picture when nothing else is competing for the property.
fn style_as_attrs(node: roxmltree::Node, style: &str, has_css: bool) -> Option<String> {
    if has_css || node.attribute("class").is_some() {
        return None;
    }
    let mut out = String::new();
    for decl in style.split(';').filter(|d| !d.trim().is_empty()) {
        let (name, value) = decl.split_once(':')?;
        let (name, value) = (name.trim(), value.trim());
        if !is_presentation_property(name) || value.is_empty() || value.contains('"') {
            return None;
        }
        if node.attribute(name).is_some() {
            return None;
        }
        let value = shorter_colour(value).unwrap_or_else(|| value.to_string());
        out.push_str(&format!("{name}=\"{value}\" "));
    }
    (!out.is_empty()).then(|| out.trim_end().to_string())
}

/// Is this `<desc>` safe to drop? Only when it is empty or is the drawing program
/// signing its work — a `<desc>` someone wrote is read aloud by a screen reader, and
/// that is not a byte worth saving. This is SVGO's `removeDesc` with its default
/// `removeAny: false`, and the same reason `<title>` is never touched here.
fn is_boilerplate_desc(node: roxmltree::Node) -> bool {
    let text = node.text().unwrap_or("").trim().to_ascii_lowercase();
    text.is_empty() || text.starts_with("created with") || text.starts_with("created using")
}

/// The named colours that are shorter than, or as short as, their hex form, and the hex
/// forms short enough to be worth naming. SVGO keeps the whole CSS table; these are the
/// entries where the choice actually changes a byte count.
const NAMED: &[(&str, &str)] = &[
    ("red", "#f00"),
    ("tan", "#d2b48c"),
    ("grey", "#808080"),
    ("gray", "#808080"),
    ("cyan", "#0ff"),
    ("gold", "#ffd700"),
    ("lime", "#0f0"),
    ("navy", "#000080"),
    ("blue", "#00f"),
    ("teal", "#008080"),
    ("aqua", "#0ff"),
    ("snow", "#fffafa"),
    ("pink", "#ffc0cb"),
    ("plum", "#dda0dd"),
    ("peru", "#cd853f"),
    ("linen", "#faf0e6"),
    ("ivory", "#fffff0"),
    ("wheat", "#f5deb3"),
    ("beige", "#f5f5dc"),
    ("white", "#fff"),
    ("black", "#000"),
    ("azure", "#f0ffff"),
    ("coral", "#ff7f50"),
    ("brown", "#a52a2a"),
    ("green", "#008000"),
    ("khaki", "#f0e68c"),
    ("olive", "#808000"),
    ("orange", "#ffa500"),
    ("orchid", "#da70d6"),
    ("purple", "#800080"),
    ("salmon", "#fa8072"),
    ("sienna", "#a0522d"),
    ("silver", "#c0c0c0"),
    ("tomato", "#ff6347"),
    ("violet", "#ee82ee"),
    ("indigo", "#4b0082"),
    ("maroon", "#800000"),
    ("yellow", "#ff0"),
    ("bisque", "#ffe4c4"),
    ("magenta", "#f0f"),
    ("fuchsia", "#f0f"),
    ("crimson", "#dc143c"),
    ("darkred", "#8b0000"),
    ("dimgray", "#696969"),
    ("hotpink", "#ff69b4"),
    ("skyblue", "#87ceeb"),
    ("thistle", "#d8bfd8"),
    ("seagreen", "#2e8b57"),
    ("steelblue", "#4682b4"),
    ("whitesmoke", "#f5f5f5"),
    ("transparent", "#0000"),
];

/// The shortest way to write the same colour, or `None` when it is already shortest or is
/// not a colour this understands. `url(#…)`, `currentColor`, `none` and `inherit` are
/// paints but not colours, and come through untouched.
fn shorter_colour(v: &str) -> Option<String> {
    let t = v.trim();
    if t.is_empty() || t.starts_with("url(") || t.contains("var(") {
        return None;
    }
    let lower = t.to_ascii_lowercase();
    // What the colour is, as a hex string, whatever it was written as.
    let hex = if let Some(body) = lower.strip_prefix("rgb(").and_then(|s| s.strip_suffix(')')) {
        let parts: Vec<&str> = body.split([',', ' ']).filter(|s| !s.is_empty()).collect();
        if parts.len() != 3 {
            return None;
        }
        let mut c = [0u8; 3];
        for (i, p) in parts.iter().enumerate() {
            // A percentage is a colour too, but rounding it changes the colour; SVGO
            // converts these and so does this, at the same eighth-of-a-percent the byte
            // grid allows.
            let n = if let Some(pc) = p.strip_suffix('%') {
                let f: f64 = pc.parse().ok()?;
                (f / 100.0 * 255.0).round()
            } else {
                p.parse::<f64>().ok()?.round()
            };
            if !(0.0..=255.0).contains(&n) {
                return None;
            }
            c[i] = n as u8;
        }
        format!("#{:02x}{:02x}{:02x}", c[0], c[1], c[2])
    } else if lower.starts_with('#') {
        lower.clone()
    } else if let Some((_, h)) = NAMED.iter().find(|(n, _)| *n == lower) {
        (*h).to_string()
    } else {
        return None;
    };
    // `#aabbcc` is `#abc` when every pair repeats. `#aabbccdd` likewise, where the fourth
    // pair is the alpha.
    let body = hex.strip_prefix('#')?;
    if !body.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    let mut best = hex.clone();
    if body.len() == 6 || body.len() == 8 {
        let b = body.as_bytes();
        if b.chunks(2).all(|p| p[0] == p[1]) {
            let short: String = b.chunks(2).map(|p| char::from(p[0])).collect();
            best = format!("#{short}");
        }
    }
    // And a name, where one is shorter still.
    if let Some((name, _)) = NAMED
        .iter()
        .filter(|(_, h)| **h == best)
        .min_by_key(|(n, _)| n.len())
    {
        if name.len() < best.len() {
            best = (*name).to_string();
        }
    }
    (best != t).then_some(best)
}

/// The shortest way to write the same number: no `+`, no leading zero before the point, no
/// trailing zeros after it. This never changes a value, only its spelling, so unlike
/// SVGO's `cleanupNumericValues` there is no precision to choose.
fn shorter_number(v: &str) -> String {
    let t = v.trim();
    let (sign, rest) = match t.strip_prefix('-') {
        Some(r) => ("-", r),
        None => ("", t.strip_prefix('+').unwrap_or(t)),
    };
    if rest.is_empty() || !rest.bytes().all(|b| b.is_ascii_digit() || b == b'.') {
        return t.to_string();
    }
    if rest.matches('.').count() > 1 {
        return t.to_string();
    }
    let mut body = rest.to_string();
    if body.contains('.') {
        body = body.trim_end_matches('0').trim_end_matches('.').to_string();
    }
    let body = body
        .strip_prefix("0.")
        .map_or(body.clone(), |f| format!(".{f}"));
    let body = body.trim_start_matches('0');
    let body = if body.is_empty() || body == "." {
        "0"
    } else {
        body
    };
    let out = format!("{sign}{body}");
    if out == "-0" || out.is_empty() {
        "0".to_string()
    } else {
        out
    }
}

/// Every number in a list rewritten shorter, the separators kept as they were.
fn shorter_numbers(v: &str) -> String {
    let mut out = String::with_capacity(v.len());
    let mut tok = String::new();
    for ch in v.chars() {
        if ch.is_ascii_whitespace() || ch == ',' {
            if !tok.is_empty() {
                out.push_str(&shorter_number(&tok));
                tok.clear();
            }
            out.push(ch);
        } else {
            tok.push(ch);
        }
    }
    if !tok.is_empty() {
        out.push_str(&shorter_number(&tok));
    }
    out
}

/// Attributes every child of a group sets to the same value, moved onto the group and
/// taken off the children: SVGO's `moveElemsAttrsToGroup`, restricted to the properties
/// that are actually inherited.
///
/// Every element child must set it. A child that did not would start inheriting the value
/// instead of using the initial one, and `stroke="none"` becoming a stroke is not a
/// smaller file, it is a different picture.
///
/// The edits, the start of each group written into — one that has just been given an
/// attribute is no longer an empty group to collapse — and the attribute ranges the hoist
/// has already claimed.
type Hoists = (Vec<(Range<usize>, String)>, Vec<usize>, Vec<Range<usize>>);

fn hoist_shared_attrs(svg: &str, doc: &roxmltree::Document, taken: &[Range<usize>]) -> Hoists {
    let clear = |r: &Range<usize>| !taken.iter().any(|t| r.start < t.end && t.start < r.end);
    let mut edits = Vec::new();
    let mut written_into = Vec::new();
    let mut consumed = Vec::new();
    for group in doc.descendants().filter(|n| n.tag_name().name() == "g") {
        let kids: Vec<roxmltree::Node> = group
            .children()
            .filter(roxmltree::Node::is_element)
            .collect();
        if kids.len() < 2 {
            continue;
        }
        let Some((open, _)) = tag_ranges(svg, group.range()) else {
            continue;
        };
        if !clear(&open) {
            continue;
        }
        let mut add = String::new();
        let mut removals: Vec<Range<usize>> = Vec::new();
        for &name in INHERITED {
            if group.attribute(name).is_some() {
                continue; // the group already says something here
            }
            let Some(value) = kids[0].attribute(name) else {
                continue;
            };
            if !kids.iter().all(|k| k.attribute(name) == Some(value)) {
                continue;
            }
            let ranges: Vec<Range<usize>> = kids
                .iter()
                .filter_map(|k| k.attributes().find(|a| a.name() == name))
                .map(|a| with_leading_space(svg, a.range()))
                .collect();
            if ranges.len() != kids.len() || !ranges.iter().all(&clear) {
                continue;
            }
            add.push_str(&format!(" {name}=\"{value}\""));
            removals.extend(ranges);
        }
        if add.is_empty() {
            continue;
        }
        // Keeping a group that would otherwise vanish costs its two tags; the hoist has
        // to be worth more than that.
        let saved: usize = removals.iter().map(Range::len).sum();
        let keeps_a_group = group.attributes().len() == 0;
        let tag_cost = if keeps_a_group { open.len() + 4 } else { 0 };
        if saved <= add.len() + tag_cost {
            continue;
        }
        let at = open.end - 1;
        edits.push((at..at, add));
        consumed.extend(removals.iter().cloned());
        edits.extend(removals.into_iter().map(|r| (r, String::new())));
        written_into.push(group.range().start);
    }
    (edits, written_into, consumed)
}

/// An attribute's range, with the whitespace in front of it: removing one without the
/// other leaves a wasted byte where it used to be.
fn with_leading_space(svg: &str, r: Range<usize>) -> Range<usize> {
    svg[..r.start]
        .rfind(|c: char| !c.is_whitespace())
        .map_or(r.clone(), |i| i + 1..r.end)
}

/// The element's own two tags, `<g …>` and `</g>`, as ranges inside its full extent.
/// `None` when it is written self-closing, which has no closing tag to remove.
fn tag_ranges(svg: &str, whole: Range<usize>) -> Option<(Range<usize>, Range<usize>)> {
    let text = svg.get(whole.clone())?;
    let open_end = whole.start + text.find('>')? + 1;
    if text[..open_end - whole.start].ends_with("/>") {
        return None;
    }
    let close_start = whole.start + text.rfind('<')?;
    (close_start >= open_end).then_some((whole.start..open_end, close_start..whole.end))
}

/// Does an ancestor set this property to something else? Then the value here is not a
/// default being restated, it is an override, and it has to stay.
fn overrides_an_ancestor(node: roxmltree::Node, name: &str, value: &str) -> bool {
    node.ancestors()
        .skip(1)
        .filter_map(|a| a.attribute(name))
        .next()
        .is_some_and(|inherited| inherited.trim() != value)
}

/// The edits this module wants to make to `svg`, as byte ranges to replace.
///
/// `taken` are ranges another pass has already claimed (a rewritten path, an element
/// replaced by a primitive); anything overlapping one of those is left alone, because the
/// text it refers to is about to be replaced wholesale.
pub(crate) fn edits(
    svg: &str,
    doc: &roxmltree::Document,
    taken: &[Range<usize>],
) -> Vec<(Range<usize>, String)> {
    let (mut out, hoisted, consumed) = hoist_shared_attrs(svg, doc, taken);
    let mut taken: Vec<Range<usize>> = taken.to_vec();
    taken.extend(consumed);
    let clear = |r: &Range<usize>| !taken.iter().any(|t| r.start < t.end && t.start < r.end);
    let has_css = doc
        .descendants()
        .any(|n| n.tag_name().name() == "style" && n.is_element());

    // Nodes removed whole: nothing inside one is edited, since its text is going anyway and
    // an edit inside a removed range shifts every byte the removal was measured against.
    let dropped = |n: roxmltree::Node| {
        n.is_element()
            && (INERT.contains(&n.tag_name().name())
                || (n.tag_name().name() == "desc" && is_boilerplate_desc(n)))
    };
    for node in doc.descendants() {
        if node.ancestors().skip(1).any(dropped) {
            continue;
        }
        if node.is_comment() || node.is_pi() {
            let r = node.range();
            if clear(&r) {
                out.push((r, String::new()));
            }
            continue;
        }
        if !node.is_element() {
            continue;
        }
        // Editor metadata and descriptions render nothing. `<title>` stays: a screen
        // reader reads it, which is not a byte anyone should be saving.
        let name = node.tag_name().name();
        if INERT.contains(&name) || (name == "desc" && is_boilerplate_desc(node)) {
            let r = node.range();
            if clear(&r) {
                out.push((r, String::new()));
            }
            continue;
        }
        // A `<g>` that says nothing groups nothing: its children inherit the same
        // everything with it gone. Only a group with no attributes at all qualifies —
        // this is SVGO's `collapseGroups` without the part that moves a transform or a
        // clip path down into the children, which is where that plugin gets interesting
        // and where it gets dangerous.
        if name == "g"
            && node.attributes().len() == 0
            && node.has_children()
            && !hoisted.contains(&node.range().start)
        {
            if let Some((open, close)) = tag_ranges(svg, node.range()) {
                if clear(&open) && clear(&close) {
                    out.push((open, String::new()));
                    out.push((close, String::new()));
                }
            }
            continue;
        }
        for attr in node.attributes() {
            let name = attr.name();
            let value = attr.value();
            let r = attr.range();
            if !clear(&r) {
                continue;
            }
            // Removing `stroke="none"` from `<path fill="#fff" stroke="none" d="…"/>` and
            // leaving its space behind trades one attribute for one wasted byte.
            let with_space = with_leading_space(svg, r.clone());
            // `attr=""` says nothing at all.
            if value.is_empty() && !PAINT_ATTRS.contains(&name) {
                out.push((with_space, String::new()));
                continue;
            }
            // An `id` nothing points at is a name no one uses.
            if name == "id" && !svg.contains(&format!("#{value}")) {
                out.push((with_space, String::new()));
                continue;
            }
            // A namespace nothing is written in, and a version number nothing reads.
            if (name.starts_with("xmlns:")
                && !svg.contains(&format!("{}:", name.trim_start_matches("xmlns:"))))
                || (name == "version" && node.tag_name().name() == "svg")
            {
                out.push((with_space, String::new()));
                continue;
            }
            if name == "style" {
                if let Some(attrs) = style_as_attrs(node, value, has_css) {
                    if attrs.len() < svg[r.clone()].len() {
                        out.push((r, attrs));
                    }
                }
                continue;
            }
            if let Some((_, def)) = DEFAULTS.iter().find(|(n, _)| *n == name) {
                if value.trim() == *def && !overrides_an_ancestor(node, name, value.trim()) {
                    out.push((with_space, String::new()));
                    continue;
                }
            }
            let replacement = if PAINT_ATTRS.contains(&name) {
                shorter_colour(value)
            } else if NUMERIC_ATTRS.contains(&name) {
                let s = shorter_numbers(value);
                (s != value).then_some(s)
            } else {
                None
            };
            if let Some(v) = replacement {
                // The attribute's range covers `name="value"`; the quote is whatever the
                // source used.
                let text = &svg[r.clone()];
                let quote = text.chars().next_back().unwrap_or('"');
                // Equal length still counts: `#FCEA2B` and `#fcea2b` are the same size,
                // and the one that matches its neighbours compresses with them.
                let new = format!("{name}={quote}{v}{quote}");
                if new.len() <= text.len() && new != text {
                    out.push((r, new));
                }
            }
        }
    }
    out
}

/// The whitespace *inside* one tag: a run of it between two attributes is a single space,
/// and any before the closing bracket is nothing. A file written one attribute to a line
/// carries a newline and an indent between each of them.
///
/// Whitespace inside a quoted value is left exactly as it is — that is somebody's `d`, or
/// a transform list, or a font name.
fn squeeze_tag(tag: &str) -> String {
    let mut out = String::with_capacity(tag.len());
    let mut quote: Option<char> = None;
    let mut pending = false;
    for ch in tag.chars() {
        if let Some(q) = quote {
            out.push(ch);
            if ch == q {
                quote = None;
            }
            continue;
        }
        if ch == '"' || ch == '\'' {
            if pending {
                out.push(' ');
                pending = false;
            }
            quote = Some(ch);
            out.push(ch);
        } else if ch.is_whitespace() {
            pending = !out.is_empty();
        } else {
            if pending && ch != '>' && ch != '/' {
                out.push(' ');
            }
            pending = false;
            out.push(ch);
        }
    }
    out
}

/// Whitespace between tags that no text node needs.
///
/// Only whitespace that sits directly between two tags is touched, and only outside the
/// elements where whitespace is content (`<text>` and friends), where a single space is
/// the difference between two words and one.
pub(crate) fn squeeze_whitespace(svg: &str) -> String {
    const TEXTUAL: &[&str] = &["text", "tspan", "textPath", "title", "style", "pre"];
    let mut out = String::with_capacity(svg.len());
    let mut rest = svg;
    let mut depth_in_text = 0usize;
    while let Some(i) = rest.find('<') {
        let chunk = &rest[..i];
        if depth_in_text > 0 || !chunk.chars().all(char::is_whitespace) {
            out.push_str(chunk);
        }
        let Some(end) = rest[i..].find('>') else {
            out.push_str(&rest[i..]);
            return out;
        };
        let tag = &rest[i..i + end + 1];
        let name: String = tag
            .trim_start_matches('<')
            .trim_start_matches('/')
            .chars()
            .take_while(|c| c.is_alphanumeric() || *c == '-' || *c == ':')
            .collect();
        if TEXTUAL.contains(&name.as_str()) {
            if tag.starts_with("</") {
                depth_in_text = depth_in_text.saturating_sub(1);
            } else if !tag.ends_with("/>") {
                depth_in_text += 1;
            }
        }
        out.push_str(&squeeze_tag(tag));
        rest = &rest[i + end + 1..];
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn colours_go_to_their_shortest_spelling() {
        assert_eq!(shorter_colour("#AABBCC").as_deref(), Some("#abc"));
        assert_eq!(shorter_colour("#ff0000").as_deref(), Some("red"));
        assert_eq!(shorter_colour("rgb(255, 0, 0)").as_deref(), Some("red"));
        assert_eq!(shorter_colour("#ffffff").as_deref(), Some("#fff"));
        assert_eq!(shorter_colour("white").as_deref(), Some("#fff"));
        assert_eq!(shorter_colour("#c9754a"), None); // already shortest
        assert_eq!(shorter_colour("url(#grad)"), None);
        assert_eq!(shorter_colour("none"), None);
        assert_eq!(shorter_colour("currentColor"), None);
    }

    #[test]
    fn numbers_lose_what_says_nothing() {
        assert_eq!(shorter_number("0.500"), ".5");
        assert_eq!(shorter_number("1.0"), "1");
        assert_eq!(shorter_number("+3"), "3");
        assert_eq!(shorter_number("-0.25"), "-.25");
        assert_eq!(shorter_number("010"), "10");
        assert_eq!(shorter_number("0"), "0");
        assert_eq!(shorter_number("-0.0"), "0");
        assert_eq!(shorter_numbers("0 0 24.0 24.0"), "0 0 24 24");
        // Not a number: left exactly as it is.
        assert_eq!(shorter_number("1e3"), "1e3");
        assert_eq!(shorter_number("auto"), "auto");
    }

    #[test]
    fn whitespace_inside_a_tag_becomes_one_space_or_none() {
        assert_eq!(
            squeeze_tag("<svg\n  width=\"24\"\n  height=\"24\"\n>"),
            "<svg width=\"24\" height=\"24\">"
        );
        assert_eq!(
            squeeze_tag("<path  d=\"M0 0 L1 1\" />"),
            "<path d=\"M0 0 L1 1\"/>"
        );
        // A value's own whitespace is somebody's data.
        assert_eq!(
            squeeze_tag("<path d=\"M0,0  L1,1\"/>"),
            "<path d=\"M0,0  L1,1\"/>"
        );
    }

    #[test]
    fn a_style_of_presentation_properties_becomes_attributes() {
        let doc = roxmltree::Document::parse(
            "<svg xmlns=\"http://www.w3.org/2000/svg\"><path style=\"fill:#875334;\"/></svg>",
        )
        .unwrap();
        let path = doc.descendants().find(|n| n.has_tag_name("path")).unwrap();
        assert_eq!(
            style_as_attrs(path, "fill:#875334;", false).as_deref(),
            Some("fill=\"#875334\"")
        );
        // A stylesheet in the document outranks an attribute, so nothing moves.
        assert_eq!(style_as_attrs(path, "fill:#875334;", true), None);
        // Something that is not a presentation property has to stay in `style`.
        assert_eq!(style_as_attrs(path, "enable-background:new;", false), None);
    }

    #[test]
    fn whitespace_between_tags_goes_but_inside_text_stays() {
        assert_eq!(
            squeeze_whitespace("<svg>\n  <path d=\"M0,0\"/>\n</svg>"),
            "<svg><path d=\"M0,0\"/></svg>"
        );
        assert_eq!(
            squeeze_whitespace("<svg><text>a <tspan>b</tspan></text></svg>"),
            "<svg><text>a <tspan>b</tspan></text></svg>"
        );
    }
}
