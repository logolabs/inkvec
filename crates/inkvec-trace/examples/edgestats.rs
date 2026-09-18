//! Reports face and edge counts and the distribution of edge lengths, in
//! particular the share of very short edges, for each image path given on
//! the command line.
fn main() {
    for name in std::env::args().skip(1) {
        let img = inkvec_trace::load_image(std::path::Path::new(&name)).unwrap();
        let t = inkvec_trace::trace_color_full(&img, &inkvec_trace::ColorOptions::default());
        let mut lens: Vec<usize> = t.map.edges.iter().map(|e| e.points.len()).collect();
        lens.sort_unstable();
        let n = lens.len();
        if n == 0 {
            println!("{name}: no edges");
            continue;
        }
        let tiny = lens.iter().filter(|&&l| l <= 4).count();
        let short = lens.iter().filter(|&&l| l <= 12).count();
        println!(
            "{:34} faces {:3}  edges {:4}  len p50 {:3} p90 {:4}  <=4pts {:3} ({:2.0}%)  <=12pts {:3} ({:2.0}%)",
            std::path::Path::new(&name).file_stem().unwrap().to_string_lossy(),
            t.face_color.len(), n, lens[n/2], lens[(n*9)/10],
            tiny, 100.0*tiny as f64/n as f64, short, 100.0*short as f64/n as f64
        );
    }
}
