//! What centreline recovery buys, measured on synthetic line art.
//!
//! Run with `cargo run -p inkvec-trace --example centerline_demo`.
//!
//! Each scene is a small icon drawn the way an icon set actually draws one — a
//! centreline and a stroke width — rasterized with analytic coverage, then recovered two
//! ways:
//!
//! * **stroke**: [`inkvec_trace::centerline::analyse`], one path per stroke plus one
//!   number for the width.
//! * **filled**: the current bilevel front end — sub-pixel contours of the *outline*,
//!   fitted by the same S4 multi-model dynamic program.
//!
//! The two are the same fitter on the same image, so the parameter columns differ only
//! in what was handed to it. That difference is the argument.

use inkvec_core::Point;
use inkvec_fit::multimodel::optimal_multimodel;
use inkvec_fit::FitConfig;
use inkvec_trace::centerline;
use inkvec_trace::coverage::{bilevel_coverage, Rgba};
use inkvec_trace::{trace_bilevel, TraceOptions};

const SS: usize = 16;

fn render(w: usize, h: usize, inside: impl Fn(f64, f64) -> bool) -> Rgba {
    let mut data = vec![0.0f32; w * h * 4];
    for y in 0..h {
        for x in 0..w {
            let mut hits = 0;
            for sy in 0..SS {
                for sx in 0..SS {
                    let px = x as f64 - 0.5 + (sx as f64 + 0.5) / SS as f64;
                    let py = y as f64 - 0.5 + (sy as f64 + 0.5) / SS as f64;
                    if inside(px, py) {
                        hits += 1;
                    }
                }
            }
            let v = 1.0 - hits as f32 / (SS * SS) as f32;
            let i = (y * w + x) * 4;
            data[i] = v;
            data[i + 1] = v;
            data[i + 2] = v;
            data[i + 3] = 1.0;
        }
    }
    Rgba {
        width: w,
        height: h,
        data,
    }
}

fn dist_to_seg(p: Point, a: Point, b: Point) -> f64 {
    let ab = b - a;
    let l2 = ab.dot(ab);
    if l2 <= 1e-18 {
        return p.dist(a);
    }
    let t = ((p - a).dot(ab) / l2).clamp(0.0, 1.0);
    p.dist(Point::new(a.x + ab.x * t, a.y + ab.y * t))
}

/// One drawn stroke of the source icon: a polyline and a width, exactly the two things
/// an SVG `<path stroke stroke-width>` carries.
struct Drawn {
    pts: Vec<Point>,
    width: f64,
    /// A closed polyline repeats no point; the renderer joins last to first.
    closed: bool,
    /// Numbers the source file spends on this stroke, counted the way the MDL objective
    /// counts them: a polyline pays two per vertex, a `<circle>` pays three, and both
    /// pay one for `stroke-width`.
    source_params: f64,
}

impl Drawn {
    fn open(pts: &[(f64, f64)], width: f64) -> Drawn {
        Drawn {
            pts: pts.iter().map(|&(x, y)| Point::new(x, y)).collect(),
            width,
            closed: false,
            source_params: 2.0 * pts.len() as f64 + 1.0,
        }
    }
    fn closed(pts: &[(f64, f64)], width: f64) -> Drawn {
        Drawn {
            pts: pts.iter().map(|&(x, y)| Point::new(x, y)).collect(),
            width,
            closed: true,
            source_params: 2.0 * pts.len() as f64 + 1.0,
        }
    }
    fn circle(cx: f64, cy: f64, r: f64, width: f64) -> Drawn {
        let n = 512;
        let pts = (0..n)
            .map(|k| {
                let t = k as f64 / n as f64 * std::f64::consts::TAU;
                Point::new(cx + r * t.cos(), cy + r * t.sin())
            })
            .collect();
        Drawn {
            pts,
            width,
            closed: true,
            // The source would write `<circle cx cy r stroke-width>`, not 512 vertices.
            source_params: 4.0,
        }
    }

    fn covers(&self, p: Point) -> bool {
        let n = self.pts.len();
        let last = if self.closed { n } else { n - 1 };
        (0..last).any(|k| dist_to_seg(p, self.pts[k], self.pts[(k + 1) % n]) <= self.width * 0.5)
    }
}

struct Scene {
    name: &'static str,
    w: usize,
    h: usize,
    strokes: Vec<Drawn>,
}

impl Scene {
    fn render(&self) -> Rgba {
        render(self.w, self.h, |x, y| {
            let p = Point::new(x, y);
            self.strokes.iter().any(|s| s.covers(p))
        })
    }
}

fn scenes() -> Vec<Scene> {
    vec![
        Scene {
            name: "arrow-right",
            w: 96,
            h: 96,
            strokes: vec![
                Drawn::open(&[(18.0, 48.0), (78.0, 48.0)], 4.0),
                Drawn::open(&[(56.0, 26.0), (78.0, 48.0), (56.0, 70.0)], 4.0),
            ],
        },
        Scene {
            name: "circle-plus",
            w: 96,
            h: 96,
            strokes: vec![
                Drawn::circle(48.0, 48.0, 32.0, 4.0),
                Drawn::open(&[(30.0, 48.0), (66.0, 48.0)], 4.0),
                Drawn::open(&[(48.0, 30.0), (48.0, 66.0)], 4.0),
            ],
        },
        Scene {
            name: "box",
            w: 96,
            h: 96,
            strokes: vec![Drawn::closed(
                &[(20.0, 20.0), (76.0, 20.0), (76.0, 76.0), (20.0, 76.0)],
                4.0,
            )],
        },
        Scene {
            name: "zig-zag",
            w: 128,
            h: 80,
            strokes: vec![Drawn::open(
                &[
                    (14.0, 60.0),
                    (42.0, 20.0),
                    (70.0, 60.0),
                    (98.0, 20.0),
                    (114.0, 42.0),
                ],
                3.0,
            )],
        },
        Scene {
            name: "x-cross",
            w: 96,
            h: 96,
            strokes: vec![
                Drawn::open(&[(18.0, 18.0), (78.0, 78.0)], 3.0),
                Drawn::open(&[(78.0, 18.0), (18.0, 78.0)], 3.0),
            ],
        },
    ]
}

fn main() {
    let cfg = FitConfig::default();

    println!("Centreline recovery on synthetic line art (analytic coverage, 16x supersampled).");
    println!("Stroke parameters = fitted path + 1 for stroke-width.");
    println!("Filled parameters  = the same S4 fitter on the sub-pixel *outline* contours.\n");

    let mut tot_stroke = 0.0f64;
    let mut tot_filled = 0.0f64;
    let mut tot_source = 0.0f64;
    let mut rows: Vec<(String, f64, f64, f64, f64)> = Vec::new();

    for sc in scenes() {
        let img = sc.render();
        let field = bilevel_coverage(&img);
        let labels = centerline::bilevel_labels(&field);
        let res = centerline::analyse(&field, &labels, img.width, img.height);

        println!("=== {} ({}x{}) ===", sc.name, sc.w, sc.h);
        println!(
            "  drawn: {} stroke(s), widths {:?}",
            sc.strokes.len(),
            sc.strokes.iter().map(|s| s.width).collect::<Vec<_>>()
        );
        println!(
            "  stroke_fraction {:.3}   residual regions {}",
            res.stroke_fraction,
            res.residual_regions.len()
        );
        println!(
            "  {:>3}  {:>8}  {:>16}  {:>6}  {:>4}  {:>6}",
            "#", "length", "width +- sigma", "closed", "segs", "params"
        );

        let mut stroke_params = 0.0;
        for (k, s) in res.strokes.iter().enumerate() {
            let path = s.fit(&cfg);
            let p = path.params() + 1.0;
            stroke_params += p;
            println!(
                "  {:>3}  {:>8.2}  {:>9.3} +- {:.3}  {:>6}  {:>4}  {:>6.0}",
                k,
                s.length(),
                s.width,
                s.width_sigma,
                s.closed,
                path.segments.len(),
                p
            );
        }

        // The competing description: outline contours through the same fitter.
        let (polys, _) = trace_bilevel(&img, &TraceOptions::default());
        let mut filled_params = 0.0;
        let mut filled_segs = 0usize;
        for poly in &polys {
            let path = optimal_multimodel(poly, &cfg);
            filled_params += path.params();
            filled_segs += path.segments.len();
        }
        let source_params: f64 = sc.strokes.iter().map(|s| s.source_params).sum();

        println!(
            "  filled outline: {} contour(s), {} segs, {:.0} params",
            polys.len(),
            filled_segs,
            filled_params
        );
        let saving = if filled_params > 0.0 {
            100.0 * (1.0 - stroke_params / filled_params)
        } else {
            0.0
        };
        println!(
            "  stroke total {:.0} params   vs filled {:.0}   saving {:.0}%\n",
            stroke_params, filled_params, saving
        );

        rows.push((
            sc.name.to_string(),
            stroke_params,
            filled_params,
            saving,
            source_params,
        ));
        tot_stroke += stroke_params;
        tot_filled += filled_params;
        tot_source += source_params;
    }

    println!("{:-<62}", "");
    println!(
        "{:<14} {:>10} {:>10} {:>9} {:>12}",
        "scene", "stroke", "filled", "saving", "source-drawn"
    );
    println!("{:-<62}", "");
    for (name, s, f, pct, src) in &rows {
        println!(
            "{:<14} {:>10.0} {:>10.0} {:>8.0}% {:>12.0}",
            name, s, f, pct, src
        );
    }
    println!("{:-<62}", "");
    println!(
        "{:<14} {:>10.0} {:>10.0} {:>8.0}% {:>12.0}",
        "total",
        tot_stroke,
        tot_filled,
        100.0 * (1.0 - tot_stroke / tot_filled),
        tot_source
    );
    println!(
        "\n`source-drawn` is what the artist's own file spends, for scale. The stroke\n\
         column is the same order of magnitude as the drawing; the filled column is not.\n\
         \n\
         Where the stroke column still loses to the source the reason is nameable, and\n\
         in neither case is it the centreline:\n\
         \n\
         * circle-plus - the ring comes back as three cubics (21 params) where the\n\
           source writes <circle> (4). That is S4's primitive alphabet not being reached\n\
           from here, not a cost of stroking: inkvec_fit::primitives already fits\n\
           circles, and a closed stroke is exactly the input it wants.\n\
         * x-cross - four graph arms each pay a 2-param start and a width, where the\n\
           source drew two polylines straight through the crossing. Pairing collinear\n\
           arms across a junction recovers it, and the junction is in the graph to do it\n\
           with. The filled description cannot: it has one blob and no junction at all.\n\
         \n\
         The saving that is not in any column: stroke-width is one number. In the filled\n\
         description the line weight is implicit in every one of those coordinates, and\n\
         there is no edit that changes it."
    );
}
