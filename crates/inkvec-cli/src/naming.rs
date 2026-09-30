//! Element names: a basic colour word for each painted face, so the document's ids read
//! `blue-3` in an editor's layer list instead of `path4728`.
//!
//! Used by the colour emitter ([`crate::emit`]) when it numbers its faces. A leaf: it
//! depends on nothing in this crate.

/// A basic colour name for an sRGB colour (0..1 per channel), for use as an element id.
///
/// Not a semantic label — it does not know that a shape is a wing or a wheel — but it is
/// the part of naming that can be done from the geometry alone, and it is the difference
/// between an editor showing `path4728` and showing `blue-3`. Semantic naming needs a
/// model that has seen the picture (DESIGN.md S6); this needs nothing.
///
/// The colour is converted to OKLab (lightness `L` in 0..1, opponent axes `a`, `b`), whose
/// hue and chroma track what a viewer would call the colour far better than sRGB's do:
///
/// * chroma `C = sqrt(a² + b²)` below 0.03 is a neutral, named by `L` alone
///   (white, light-grey, grey, dark-grey, black);
/// * otherwise the hue angle `h = atan2(b, a)` in degrees, 0..360, picks one of eleven
///   bands, and a dark (`L < 0.5`) red or orange is called brown.
///
/// The band edges are hand-set, not fitted; they only have to give a sensible word.
pub(crate) fn colour_name(rgb: [f32; 3]) -> &'static str {
    let c = inkvec_trace::color::rgb_to_oklab(rgb);
    let chroma = (c.a * c.a + c.b * c.b).sqrt();
    if chroma < 0.03 {
        return match c.l {
            l if l > 0.93 => "white",
            l if l > 0.72 => "light-grey",
            l if l > 0.42 => "grey",
            l if l > 0.18 => "dark-grey",
            _ => "black",
        };
    }
    // Hue in degrees, measured the usual way round the OKLab a/b plane.
    let hue = c.b.atan2(c.a).to_degrees().rem_euclid(360.0);
    let base = match hue {
        h if h < 20.0 => "red",
        h if h < 45.0 => "orange",
        h if h < 70.0 => "yellow",
        h if h < 100.0 => "olive",
        h if h < 165.0 => "green",
        h if h < 200.0 => "teal",
        h if h < 240.0 => "cyan",
        h if h < 285.0 => "blue",
        h if h < 320.0 => "purple",
        h if h < 345.0 => "magenta",
        _ => "red",
    };
    // Brown is dark orange, and calling it orange reads wrong in a layer list.
    if (base == "orange" || base == "red") && c.l < 0.5 {
        return "brown";
    }
    base
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_colour_name() {
        assert_eq!(colour_name([1.0, 1.0, 1.0]), "white");
        assert_eq!(colour_name([0.0, 0.0, 0.0]), "black");
        assert_eq!(colour_name([1.0, 0.0, 0.0]), "orange");
        assert_eq!(colour_name([0.0, 0.0, 1.0]), "blue");
    }
}
