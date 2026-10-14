//! The pruned minimum against the full scan it replaces: the same bits, every time.

use super::*;

/// A small deterministic generator (PCG-style LCG), so failures reproduce.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> f64 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        (self.0 >> 11) as f64 / (1u64 << 53) as f64
    }
    fn pt(&mut self, scale: f64) -> Point {
        Point::new(scale * (self.next() - 0.5), scale * (self.next() - 0.5))
    }
}

/// A random closed-ish chain of lines, cubics and circular arcs, the way a ring is drawn:
/// each segment starts where the last one ended.
fn random_path(rng: &mut Rng, n: usize) -> FittedPath {
    let start = rng.pt(200.0);
    let mut cur = start;
    let mut segments = Vec::with_capacity(n);
    for _ in 0..n {
        let end = Point::new(
            cur.x + 40.0 * (rng.next() - 0.5),
            cur.y + 40.0 * (rng.next() - 0.5),
        );
        let seg = match (rng.next() * 3.0) as u32 {
            0 => Segment::Line(end),
            1 => Segment::Cubic(
                Point::new(
                    cur.x + 30.0 * (rng.next() - 0.5),
                    cur.y + 30.0 * (rng.next() - 0.5),
                ),
                Point::new(
                    end.x + 30.0 * (rng.next() - 0.5),
                    end.y + 30.0 * (rng.next() - 0.5),
                ),
                end,
            ),
            _ => {
                let r = 0.5 * cur.dist(end) + 1.0 + 20.0 * rng.next();
                Segment::Arc {
                    rx: r,
                    ry: r,
                    phi: 0.0,
                    large_arc: rng.next() < 0.3,
                    sweep: rng.next() < 0.5,
                    end,
                }
            }
        };
        segments.push(seg);
        cur = end;
    }
    FittedPath {
        start,
        segments,
        closed: false,
    }
}

/// Points everywhere that matters: far off, near the chain, and exactly on it (a node, and
/// a point the arc sampler itself produced), where the bound and the value are both zero.
#[test]
fn the_pruned_distance_is_the_full_scans_to_the_bit() {
    let mut rng = Rng(7);
    for case in 0..40 {
        let path = random_path(&mut rng, 1 + case % 9);
        let pieces = PathPieces::new(&path).expect("a path with segments");
        let mut probes: Vec<Point> = (0..60).map(|_| rng.pt(320.0)).collect();
        probes.push(path.start);
        probes.extend(path.segments.iter().map(Segment::end));
        let along = crate::curves::sample_run(path.start, &path.segments, 0.7);
        probes.extend(
            along
                .iter()
                .map(|p| Point::new(p.x + 0.01 * (rng.next() - 0.5), p.y)),
        );
        probes.extend(along);
        for p in probes {
            let (fast, scan) = (pieces.dist2(p), pieces.dist2_scan(p));
            assert_eq!(
                fast.to_bits(),
                scan.to_bits(),
                "case {case}, {p:?}: {fast} vs {scan}"
            );
        }
    }
}

/// A weighted minimum (a squared distance over an interpolated sigma, as the editability
/// pricing's curve-to-points term charges it) prunes by the weight and still returns the
/// full scan's bits.
#[test]
fn a_weighted_minimum_is_the_full_scans_to_the_bit() {
    let mut rng = Rng(11);
    let mut pts = vec![rng.pt(100.0)];
    for _ in 0..400 {
        let l = *pts.last().unwrap();
        pts.push(Point::new(
            l.x + 2.0 * (rng.next() - 0.5),
            l.y + 2.0 * (rng.next() - 0.3),
        ));
    }
    let sigma: Vec<f64> = (0..pts.len()).map(|_| 0.001 + 0.3 * rng.next()).collect();
    let value = |s: Point, j: usize| {
        let (a, b) = (pts[j], pts[j + 1]);
        let (dx, dy) = (b.x - a.x, b.y - a.y);
        let l2 = dx * dx + dy * dy;
        let t = if l2 > 0.0 {
            (((s.x - a.x) * dx + (s.y - a.y) * dy) / l2).clamp(0.0, 1.0)
        } else {
            0.0
        };
        let q = Point::new(a.x + t * dx, a.y + t * dy);
        let sig = sigma[j] * (1.0 - t) + sigma[j + 1] * t;
        (s.dist(q) / sig).powi(2)
    };
    let tree = BoxTree::new(
        pts.windows(2)
            .zip(sigma.windows(2))
            .map(|(w, s)| Piece::around(w, 1.0 / s[0].max(s[1]).powi(2)))
            .collect(),
    );
    assert_eq!(tree.len(), pts.len() - 1);
    for k in 0..500 {
        let s = if k % 2 == 0 {
            rng.pt(150.0)
        } else {
            let p = pts[k % pts.len()];
            Point::new(
                p.x + 0.2 * (rng.next() - 0.5),
                p.y + 0.2 * (rng.next() - 0.5),
            )
        };
        let fast = tree.min_by(s, |j| value(s, j));
        let scan = (0..pts.len() - 1)
            .map(|j| value(s, j))
            .fold(f64::INFINITY, f64::min);
        assert_eq!(fast.to_bits(), scan.to_bits(), "{s:?}: {fast} vs {scan}");
    }
}

/// The walk skips most of a long chain: on a circle in the most chords the arc sampler
/// gives, a point near it is measured against a few dozen chords, not thousands.
#[test]
fn a_query_near_a_long_chain_visits_few_pieces() {
    let path = FittedPath {
        start: Point::new(100.0, 0.0),
        segments: vec![
            Segment::Arc {
                rx: 100.0,
                ry: 100.0,
                phi: 0.0,
                large_arc: true,
                sweep: true,
                end: Point::new(-100.0, 0.0),
            },
            Segment::Arc {
                rx: 100.0,
                ry: 100.0,
                phi: 0.0,
                large_arc: true,
                sweep: true,
                end: Point::new(100.0, 0.0),
            },
        ],
        closed: false,
    };
    let pieces = PathPieces::new(&path).unwrap();
    assert!(pieces.tree.len() >= 8_000, "{} chords", pieces.tree.len());
    let mut visited = 0;
    let p = Point::new(0.0, 100.3);
    let d = pieces.tree.min_by(p, |i| {
        visited += 1;
        match &pieces.segs[i] {
            KSeg::L(l) => l.nearest(KPoint::new(p.x, p.y), 1e-9).distance_sq,
            KSeg::C(c) => c.nearest(KPoint::new(p.x, p.y), 1e-6).distance_sq,
        }
    });
    assert!((d.sqrt() - 0.3).abs() < 1e-3, "{}", d.sqrt());
    assert!(visited < 100, "{visited} chords measured");
}

/// No pieces, nothing near: infinitely far, as the scan's fold starts.
#[test]
fn an_empty_tree_is_infinitely_far() {
    let tree = BoxTree::new(Vec::new());
    assert!(tree.is_empty());
    assert_eq!(tree.min_by(Point::new(0.0, 0.0), |_| 0.0), f64::INFINITY);
    let empty = FittedPath {
        start: Point::new(0.0, 0.0),
        segments: Vec::new(),
        closed: false,
    };
    assert!(PathPieces::new(&empty).is_none());
}
