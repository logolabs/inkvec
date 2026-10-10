use super::*;
use std::collections::BTreeMap;

#[test]
fn test_emit_bilevel() {
    let p1 = vec![
        Point::new(0.0, 0.0),
        Point::new(10.0, 0.0),
        Point::new(10.0, 10.0),
        Point::new(0.0, 10.0),
    ];
    let p_short = vec![Point::new(0.0, 0.0), Point::new(1.0, 1.0)];
    let svg = emit_bilevel(&[p1.clone(), p_short], 20, 20, 0.1, true);
    assert!(svg.contains("<svg"));
    assert!(svg.contains("<rect"));
    assert!(svg.contains("<path"));

    let svg_no_paper = emit_bilevel(&[p1], 20, 20, 0.1, false);
    assert!(!svg_no_paper.contains("<rect"));
}

#[test]
fn test_face_fill_and_opacity() {
    let pal = inkvec_trace::Palette {
        colors: vec![],
        rgb: vec![[1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
        weight: vec![0.5, 0.5],
        alpha: vec![1.0, 1.0],
    };
    let doc = ColorDoc {
        order: &[],
        fitted: &[],
        prims: &[],
        fill_fits: &[],
        pal: &pal,
        face_color: &[0, 1],
        clear: &[],
        opacity: &[],
        alpha_ramps: &[Some(AlphaRamp {
            p0: Point::new(0.0, 0.0),
            p1: Point::new(10.0, 10.0),
            a0: 0.2,
            a1: 0.8,
            color: [0.5, 0.5, 0.5],
        })],
        fades: &[
            None,
            Some((
                gradient::FillModel::Flat([0.5, 0.5, 0.5]),
                gradient::FillModel::Flat([0.2, 0.4, 0.6]),
            )),
        ],
        layers: None,
        matte: [1.0, 1.0, 1.0],
        ribbons: &Default::default(),
        w: 10,
        h: 10,
    };
    let mut defs = String::new();
    assert_eq!(
        face_fill(&doc, 0, 1.0, Some([1.0, 1.0, 1.0]), &mut defs),
        "#ffffff"
    );
    let ramp_fill = face_fill(&doc, 0, 1.0, None, &mut defs);
    assert_eq!(ramp_fill, "url(#a0)");
    assert!(defs.contains("<linearGradient id=\"a0\""));

    let fade_fill = face_fill(&doc, 1, 1.0, None, &mut defs);
    assert!(fade_fill.starts_with('#') || fade_fill.starts_with("url("));

    assert_eq!(opacity_attr(&doc, 0, 0.5), " fill-opacity=\"0.500\"");
    assert_eq!(opacity_attr(&doc, 0, 1.0), "");
}

#[test]
fn test_write_ribbons_and_color_doc() {
    let mut ribbons = crate::ribbons::Ribbons::default();
    ribbons
        .elements
        .insert(0, "<path id=\"ribbon-0\" d=\"M0,0L1,1\"/>".to_string());
    let pal = inkvec_trace::Palette {
        colors: vec![],
        rgb: vec![],
        weight: vec![],
        alpha: vec![],
    };
    let doc = ColorDoc {
        order: &[],
        fitted: &[],
        prims: &[],
        fill_fits: &[],
        pal: &pal,
        face_color: &[],
        clear: &[],
        opacity: &[],
        alpha_ramps: &[],
        fades: &[],
        layers: None,
        matte: [1.0, 1.0, 1.0],
        ribbons: &ribbons,
        w: 10,
        h: 10,
    };
    let doc2 = doc.with_ribbons(&ribbons);
    assert_eq!(doc2.w, 10);
    let mut body = String::new();
    let mut painted = Vec::new();
    write_ribbons(&doc, &mut body, &mut painted);
    assert!(body.contains("ribbon-0"));
    assert_eq!(painted, vec![(0, 0)]);
}

#[test]
fn test_emit_color_single_face() {
    use inkvec_fit::curves::Segment;
    let ring = vec![(0, false)];
    let rings = vec![ring];
    let order = vec![rings];
    let fitted = vec![FittedPath {
        start: Point::new(0.0, 0.0),
        segments: vec![
            Segment::Line(Point::new(10.0, 0.0)),
            Segment::Line(Point::new(10.0, 10.0)),
            Segment::Line(Point::new(0.0, 10.0)),
            Segment::Line(Point::new(0.0, 0.0)),
        ],
        closed: true,
    }];
    let prims = vec![None];
    let pal = inkvec_trace::Palette {
        colors: vec![],
        rgb: vec![[1.0, 0.0, 0.0]],
        weight: vec![1.0],
        alpha: vec![1.0],
    };
    let doc = ColorDoc {
        order: &order,
        fitted: &fitted,
        prims: &prims,
        fill_fits: &[],
        pal: &pal,
        face_color: &[0],
        clear: &[false],
        opacity: &[1.0],
        alpha_ramps: &[None],
        fades: &[None],
        layers: None,
        matte: [1.0, 1.0, 1.0],
        ribbons: &Default::default(),
        w: 20,
        h: 20,
    };
    let opts = EmitOptions {
        precision: 0.1,
        native: true,
        harmonize: true,
        harmonize_threshold: 0.05,
        use_symbols: false,
        cutout: false,
        no_background: false,
    };
    let svg = emit_color(&doc, &opts);
    assert!(svg.contains("<svg"));
    assert!(svg.contains("<path"));
}

#[test]
fn test_annulus_strokes_coverage() {
    use inkvec_core::Point;
    use inkvec_fit::primitives::{PrimitiveFit, PrimitiveKind};
    let order = vec![vec![vec![(0, false)]], vec![vec![(1, false)]]];
    let fitted = vec![];
    let prims = vec![
        Some(PrimitiveFit {
            kind: PrimitiveKind::Circle {
                c: Point::new(10.0, 10.0),
                r: 8.0,
            },
            chi2: 0.0,
            params: 3.0,
        }),
        Some(PrimitiveFit {
            kind: PrimitiveKind::Circle {
                c: Point::new(10.0, 10.0),
                r: 4.0,
            },
            chi2: 0.0,
            params: 3.0,
        }),
    ];
    let pal = inkvec_trace::Palette {
        colors: vec![],
        rgb: vec![[1.0, 0.0, 0.0], [0.0, 0.0, 0.0]],
        weight: vec![1.0, 1.0],
        alpha: vec![1.0, 1.0],
    };
    let doc = ColorDoc {
        order: &order,
        fitted: &fitted,
        prims: &prims,
        fill_fits: &[],
        pal: &pal,
        face_color: &[0, 1],
        clear: &[false, false],
        opacity: &[1.0, 1.0],
        alpha_ramps: &[None, None],
        fades: &[None, None],
        layers: None,
        matte: [1.0, 1.0, 1.0],
        ribbons: &Default::default(),
        w: 20,
        h: 20,
    };
    let drawn = vec![vec![0], vec![0]];
    let stack = Stacking {
        dropped: vec![false, true],
        holes: vec![vec![1], vec![]],
        thin: vec![1.0, 1.0],
    };
    let paint = Paint {
        fills: vec!["#ff0000".to_string(), "#000000".to_string()],
        opac: vec!["".to_string(), "".to_string()],
        base_of: vec![None, None],
    };
    let harmonized = harmonize::Harmonized::default();
    let res = annulus_strokes(&doc, &drawn, &stack, &paint, &harmonized, 2);
    assert_eq!(res.len(), 1);
    assert!(res.contains_key(&0));
    assert!(res[&0].contains("<circle"));
}

#[test]
fn test_emit_color_with_layers() {
    use inkvec_core::Point;
    use inkvec_fit::curves::Segment;
    let order = vec![vec![vec![(0, false)]]];
    let fitted = vec![FittedPath {
        start: Point::new(0.0, 0.0),
        segments: vec![
            Segment::Line(Point::new(10.0, 0.0)),
            Segment::Line(Point::new(10.0, 10.0)),
            Segment::Line(Point::new(0.0, 10.0)),
            Segment::Line(Point::new(0.0, 0.0)),
        ],
        closed: true,
    }];
    let prims = vec![None];
    let pal = inkvec_trace::Palette {
        colors: vec![],
        rgb: vec![[1.0, 0.0, 0.0]],
        weight: vec![1.0],
        alpha: vec![1.0],
    };
    let layer_shapes = vec![vec![vec![(0, false)]]];
    let analysis = inkvec_trace::alpha::AlphaAnalysis {
        layers: vec![inkvec_trace::alpha::Layer {
            color: [0.0, 0.0, 1.0],
            alpha: 0.5,
            faces: vec![0],
            residual: 0.0,
        }],
        opaque_faces: vec![],
        base_rgb: vec![[1.0, 0.0, 0.0]],
    };
    let doc = ColorDoc {
        order: &order,
        fitted: &fitted,
        prims: &prims,
        fill_fits: &[],
        pal: &pal,
        face_color: &[0],
        clear: &[false],
        opacity: &[1.0],
        alpha_ramps: &[None],
        fades: &[None],
        layers: Some((&analysis, &layer_shapes)),
        matte: [1.0, 1.0, 1.0],
        ribbons: &Default::default(),
        w: 20,
        h: 20,
    };
    let opts = EmitOptions {
        precision: 0.1,
        native: true,
        harmonize: true,
        harmonize_threshold: 0.05,
        use_symbols: false,
        cutout: false,
        no_background: false,
    };
    let svg = emit_color(&doc, &opts);
    assert!(svg.contains("<svg"));
    assert!(svg.contains("id=\"layer-0\""));
}

#[test]
fn test_face_ids() {
    let order = vec![vec![vec![(0, false)]], vec![vec![(0, false)]]];
    let fitted = vec![];
    let prims = vec![None];
    let pal = inkvec_trace::Palette {
        colors: vec![],
        rgb: vec![[1.0, 0.0, 0.0]],
        weight: vec![1.0],
        alpha: vec![1.0],
    };
    let doc = ColorDoc {
        order: &order,
        fitted: &fitted,
        prims: &prims,
        fill_fits: &[],
        pal: &pal,
        face_color: &[0, 0],
        clear: &[false, false],
        opacity: &[1.0, 1.0],
        alpha_ramps: &[None, None],
        fades: &[None, None],
        layers: None,
        matte: [1.0, 1.0, 1.0],
        ribbons: &Default::default(),
        w: 20,
        h: 20,
    };
    let ids = face_ids(&doc);
    assert_eq!(ids.len(), 2);
}

#[test]
fn test_surviving_parent() {
    let parent = vec![None, Some(0), Some(1)];
    let dropped = vec![false, true, false];
    assert_eq!(surviving_parent(&parent, &dropped, 2), Some(0));
}
