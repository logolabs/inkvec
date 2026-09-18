//! Research bridge: fit a closed contour from whitespace-separated x y sigma rows.
use inkvec_core::{Point, Polyline};
use inkvec_fit::{curves::Segment, multimodel::optimal_multimodel, FitConfig};
use std::{error::Error, fmt::Write, io::Read};

fn main() -> Result<(), Box<dyn Error>> {
    let mut input = String::new();
    std::io::stdin().read_to_string(&mut input)?;
    let values = input
        .split_whitespace()
        .map(str::parse::<f64>)
        .collect::<Result<Vec<_>, _>>()?;
    if values.len() < 9 || values.len() % 3 != 0 {
        return Err("expected at least three x y sigma rows".into());
    }
    let mut pts = Vec::new();
    let mut sigmas = Vec::new();
    for v in values.as_chunks::<3>().0 {
        if !v.iter().all(|x| x.is_finite()) || v[2] <= 0.0 {
            return Err("coordinates must be finite and sigma positive".into());
        }
        pts.push(Point::new(v[0], v[1]));
        sigmas.push(v[2]);
    }
    let fit = optimal_multimodel(&Polyline::new(pts, sigmas, true), &FitConfig::default());
    let mut d = format!("M{},{}", fit.start.x, fit.start.y);
    for s in &fit.segments {
        match s {
            Segment::Line(p) => write!(d, "L{},{}", p.x, p.y)?,
            Segment::Cubic(a, b, p) => {
                write!(d, "C{},{} {},{} {},{}", a.x, a.y, b.x, b.y, p.x, p.y)?
            }
            Segment::Arc {
                rx,
                ry,
                phi,
                large_arc,
                sweep,
                end,
            } => write!(
                d,
                "A{} {} {} {} {} {} {}",
                rx,
                ry,
                phi.to_degrees(),
                u8::from(*large_arc),
                u8::from(*sweep),
                end.x,
                end.y
            )?,
        }
    }
    d.push('Z');
    eprintln!("segments={} params={}", fit.len(), fit.params());
    println!("<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 256 256\"><path d=\"{d}\" fill=\"black\"/></svg>");
    Ok(())
}
