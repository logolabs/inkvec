use super::*;

#[test]
fn structural_transaction_rejects_repair_inflation_and_accepts_exact_merges() {
    use inkvec_core::{Point, Polyline};
    use inkvec_fit::curves::Segment;
    let a = Point::new(0.0, 0.0);
    let b = Point::new(10.0, 0.0);
    let c = Point::new(20.0, 0.0);
    let poly = Polyline::new(vec![a, b, c], vec![0.1; 3], false);
    let original = FittedPath {
        start: a,
        segments: vec![Segment::Line(b), Segment::Line(c)],
        closed: false,
    };
    let compact = FittedPath {
        start: a,
        segments: vec![Segment::Line(c)],
        closed: false,
    };
    let inflated = FittedPath {
        start: a,
        segments: vec![Segment::Cubic(b, b, c)],
        closed: false,
    };
    let cfg = FitConfig::default();
    assert!(structural_trial_improves(
        std::slice::from_ref(&poly),
        &[compact],
        std::slice::from_ref(&original),
        &[1.0],
        &cfg
    ));
    assert!(!structural_trial_improves(
        std::slice::from_ref(&poly),
        &[inflated],
        std::slice::from_ref(&original),
        &[1.0],
        &cfg
    ));
    assert!(!structural_trial_improves(
        &[poly],
        &[],
        &[original],
        &[1.0],
        &cfg
    ));
}

#[test]
fn test_path_cost_computation() {
    use inkvec_core::{Point, Polyline};
    use inkvec_fit::curves::Segment;
    let p = FittedPath {
        start: Point::new(0.0, 0.0),
        segments: vec![
            Segment::Line(Point::new(10.0, 10.0)),
            Segment::Line(Point::new(20.0, 10.0)),
        ],
        closed: false,
    };
    let poly = Polyline::new(
        vec![
            Point::new(0.0, 0.0),
            Point::new(10.0, 10.0),
            Point::new(20.0, 10.0),
        ],
        vec![0.5, 0.5, 0.5],
        false,
    );
    let cfg = FitConfig::from_precision(256.0, 0.1, 2.0);
    let cost = path_cost(&poly, &p, &cfg);
    assert!(cost > 0.0);
}

#[test]
fn test_gradient_count() {
    use inkvec_trace::gradient::{FillFit, FillModel, Interp};
    let fills = vec![
        FillFit {
            model: FillModel::Flat([1.0, 0.0, 0.0]),
            chi2: 0.0,
            params: 3.0,
            cost: 0.0,
        },
        FillFit {
            model: FillModel::Linear {
                p0: (0.0, 0.0),
                p1: (10.0, 10.0),
                c0: [1.0, 0.0, 0.0],
                c1: [0.0, 1.0, 0.0],
                interp: Interp::Srgb,
                mids: vec![],
            },
            chi2: 0.1,
            params: 10.0,
            cost: 0.5,
        },
    ];
    assert_eq!(gradient_count(&fills), 1);
}

#[test]
fn test_scaled_and_ring_times() {
    let cfg = FitConfig::default();
    let s = scaled(&cfg, 2.5);
    assert_eq!(s.lambda, cfg.lambda * 2.5);

    report_ring_times(None);
    let rt = vec![(10.0, 5), (25.0, 12), (5.0, 2)];
    report_ring_times(Some(&rt));
}

#[test]
fn test_segment_counts() {
    let fitted = vec![FittedPath {
        start: Point::new(0.0, 0.0),
        segments: vec![
            Segment::Line(Point::new(1.0, 1.0)),
            Segment::Cubic(
                Point::new(2.0, 2.0),
                Point::new(3.0, 3.0),
                Point::new(4.0, 4.0),
            ),
        ],
        closed: false,
    }];
    assert_eq!(segment_counts(&fitted), (1, 1));
}

#[test]
fn test_final_fills() {
    let pal = inkvec_trace::Palette {
        colors: vec![],
        rgb: vec![[1.0, 0.0, 0.0]],
        weight: vec![1.0],
        alpha: vec![1.0],
    };
    let fills = vec![gradient::FillFit {
        model: gradient::FillModel::Flat([0.5, 0.5, 0.5]),
        chi2: 0.0,
        params: 3.0,
        cost: 0.0,
    }];
    let res_no_grad = final_fills(fills.clone(), &[0], &pal, true);
    assert_eq!(res_no_grad.len(), 1);
    let res_grad = final_fills(fills, &[0], &pal, false);
    assert_eq!(res_grad.len(), 1);
}

#[test]
fn test_prefer_primitive() {
    let mut circle_pts = Vec::new();
    for i in 0..16 {
        let angle = (i as f64) * 2.0 * std::f64::consts::PI / 16.0;
        circle_pts.push(Point::new(
            10.0 + 5.0 * angle.cos(),
            10.0 + 5.0 * angle.sin(),
        ));
    }
    let poly = inkvec_core::Polyline::new(circle_pts.clone(), vec![0.1; 16], true);
    let curve = FittedPath {
        start: circle_pts[0],
        segments: circle_pts[1..].iter().map(|&p| Segment::Line(p)).collect(),
        closed: true,
    };
    let cfg = FitConfig::default();
    let (_fit, prim) = prefer_primitive(&poly, curve, &cfg);
    assert!(prim.is_some());
}

#[test]
fn test_merge_map() {
    use inkvec_trace::planar::{Edge, PlanarMap};
    let map = PlanarMap {
        edges: vec![
            Edge {
                points: vec![Point::new(0.0, 0.0), Point::new(10.0, 0.0)],
                sigma: vec![0.1, 0.1],
                left: 0,
                right: 1,
                start_node: 0,
                end_node: 1,
                closed: false,
                lambda_scale: 1.0,
            },
            Edge {
                points: vec![Point::new(10.0, 0.0), Point::new(10.0, 10.0)],
                sigma: vec![0.1, 0.1],
                left: 1,
                right: 2,
                start_node: 1,
                end_node: 2,
                closed: false,
                lambda_scale: 1.0,
            },
        ],
        width: 20,
        height: 20,
        n_labels: 3,
    };
    let remapped = merge_map(&map, &[0, 0, 2]);
    assert_eq!(remapped.edges[0].left, u16::MAX);
    assert_eq!(remapped.edges[0].right, u16::MAX);
    assert_eq!(remapped.edges[1].left, 0);
    assert_eq!(remapped.edges[1].right, 2);
}

#[test]
fn test_write_uncertainty() {
    use inkvec_trace::planar::{Edge, PlanarMap};
    let uncert_file = std::env::temp_dir().join(format!("bands_{}.svg", std::process::id()));
    let args = Args {
        uncertainty: Some(uncert_file.clone()),
        uncertainty_k: 2.0,
        ..Args::default()
    };
    let map = PlanarMap {
        edges: vec![Edge {
            points: vec![Point::new(5.0, 5.0), Point::new(15.0, 15.0)],
            sigma: vec![0.5, 0.5],
            left: 0,
            right: 1,
            start_node: 0,
            end_node: 1,
            closed: false,
            lambda_scale: 1.0,
        }],
        width: 20,
        height: 20,
        n_labels: 2,
    };
    write_uncertainty(&args, "<svg viewBox=\"0 0 20 20\"></svg>", &map);
    assert!(uncert_file.exists());
    let content = std::fs::read_to_string(&uncert_file).unwrap();
    let _ = std::fs::remove_file(&uncert_file);
    assert!(content.contains("<svg"));
    assert!(content.contains("<path"));
}

#[test]
fn test_write_colour() {
    use inkvec_trace::planar::{Edge, PlanarMap};
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
        layers: false,
        cutout: false,
        no_background: false,
    };
    let map = PlanarMap {
        edges: vec![Edge {
            points: vec![
                Point::new(0.0, 0.0),
                Point::new(10.0, 0.0),
                Point::new(10.0, 10.0),
                Point::new(0.0, 10.0),
                Point::new(0.0, 0.0),
            ],
            sigma: vec![0.1; 5],
            left: 0,
            right: 1,
            start_node: 0,
            end_node: 0,
            closed: true,
            lambda_scale: 1.0,
        }],
        width: 20,
        height: 20,
        n_labels: 1,
    };
    let svg = write_colour(&doc, &opts, &map, None, true);
    assert!(svg.contains("<svg"));

    let layers = inkvec_trace::alpha::AlphaAnalysis {
        layers: vec![inkvec_trace::alpha::Layer {
            color: [0.0, 1.0, 0.0],
            alpha: 0.5,
            faces: vec![0],
            residual: 0.0,
        }],
        opaque_faces: vec![],
        base_rgb: vec![[1.0, 0.0, 0.0]],
    };
    let svg_layered = write_colour(&doc, &opts, &map, Some(layers), true);
    assert!(svg_layered.contains("<svg"));
}

#[test]
fn test_face_transparency() {
    use inkvec_trace::gradient::FillModel;
    let img = inkvec_trace::Rgba {
        data: vec![1.0, 0.0, 0.0, 0.5, 0.0, 1.0, 0.0, 1.0],
        width: 2,
        height: 1,
    };
    let args = Args {
        native_alpha: true,
        ..Args::default()
    };
    let pal = inkvec_trace::Palette {
        colors: vec![],
        rgb: vec![[1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
        weight: vec![1.0, 1.0],
        alpha: vec![0.02, 0.5],
    };
    let alpha_src = AlphaSource {
        flat: img.clone(),
        alpha: vec![0.5, 1.0],
        matte: [1.0, 1.0, 1.0],
        cutout: false,
    };
    let trans = face_transparency(
        &img,
        &args,
        Some(&alpha_src),
        &[0, 1],
        &pal,
        &[0, 1],
        &[
            None,
            Some(inkvec_trace::native::Fade {
                alpha: FillModel::Flat([0.5, 0.5, 0.5]),
                color: FillModel::Flat([0.0, 1.0, 0.0]),
            }),
        ],
    );
    assert_eq!(trans.clear.len(), 2);
    assert_eq!(trans.opacity.len(), 2);
    assert_eq!(trans.fades.len(), 2);
}

#[test]
fn test_apply_mirrors() {
    use inkvec_trace::planar::PlanarMap;
    let map = PlanarMap {
        edges: vec![],
        width: 10,
        height: 10,
        n_labels: 1,
    };
    let sym = inkvec_trace::symmetry::detect(&map, &[], &[0]);
    let mut fitted = vec![];
    let mut prims = vec![];
    let mirrored = apply_mirrors(&sym, &mut fitted, &mut prims);
    assert_eq!(mirrored, 0);
}
