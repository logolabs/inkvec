use super::*;

/// A palette of the given sRGB inks and weights.
fn palette(inks: &[[f32; 3]], weight: &[f32]) -> Palette {
    Palette {
        colors: inks
            .iter()
            .map(|&c| crate::color::rgb_to_oklab(c))
            .collect(),
        rgb: inks.to_vec(),
        weight: weight.to_vec(),
        alpha: vec![1.0; inks.len()],
    }
}

const fn hex(r: u8, g: u8, b: u8) -> [f32; 3] {
    [r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0]
}

#[test]
fn the_codec_tinted_blacks_are_black_and_a_mosaic_neighbour_is_not() {
    // `openmoji/E056` on the web tier: black, three tinted blacks, the blue and the yellow
    // fills; and two neighbouring fills of `synthetic/mosaic_grid6`, which differ in luma.
    let inks = [
        hex(0, 0, 0),
        hex(0x00, 0x01, 0x09),
        hex(0x08, 0x01, 0x00),
        hex(0x03, 0x08, 0x01),
        hex(0x61, 0xb2, 0xe4),
        hex(0xfa, 0xeb, 0x37),
        hex(0xd3, 0x45, 0x45),
        hex(0xd3, 0x5c, 0x45),
    ];
    let weight = [0.05, 0.04, 0.03, 0.01, 0.1, 0.02, 0.2, 0.2];
    let to = variant_map(&palette(&inks, &weight));
    // Black, the heaviest of the four, takes the three tints (2.7 to 5.9 levels from it).
    assert_eq!(&to[..4], &[0, 0, 0, 0]);
    for i in 4..inks.len() {
        assert_eq!(to[i] as usize, i, "ink {i} keeps its own");
    }
}

#[test]
fn a_variant_maps_to_the_heaviest_ink_never_through_another_variant() {
    // b is a variant of a, c of b only (more than the reach from a): c keeps itself, as b
    // is a variant and no target.
    let a = hex(0x40, 0x40, 0x40);
    let b = hex(0x46, 0x46, 0x46);
    let c = hex(0x4c, 0x4c, 0x4c);
    let to = variant_map(&palette(&[a, b, c], &[0.5, 0.3, 0.2]));
    assert_eq!(to, vec![0, 0, 2]);
}

/// Columns of labels: blue, a band of label `band` `width` pixels wide, white.
fn strip_image(width: usize, band: u16) -> (Vec<u16>, usize, usize) {
    let (w, h) = (16 + width, 12);
    let labels = (0..w * h)
        .map(|p| match p % w {
            x if x < 8 => 0,
            x if x < 8 + width => band,
            _ => 1,
        })
        .collect();
    (labels, w, h)
}

#[test]
fn a_halo_goes_and_a_strip_of_ink_between_the_same_two_inks_stays() {
    // Blue and white, and between them a colour on their segment: two pixels wide it is the
    // codec's halo, four wide (an interior at radius one) a strip the artist drew.
    let inks = [
        hex(0x2e, 0x78, 0x89),
        hex(0xff, 0xff, 0xff),
        hex(0x96, 0xbb, 0xc4),
    ];
    let pal = palette(&inks, &[0.4, 0.5, 0.1]);
    for (width, kept) in [(2, false), (4, true)] {
        let (mut labels, w, h) = strip_image(width, 2);
        let rgb: Vec<[f32; 3]> = labels.iter().map(|&l| inks[l as usize]).collect();
        clean_labels(&mut labels, w, h, &rgb, &pal);
        let band = labels.iter().filter(|&&l| l == 2).count();
        assert_eq!(band > 0, kept, "width {width}");
    }
}
