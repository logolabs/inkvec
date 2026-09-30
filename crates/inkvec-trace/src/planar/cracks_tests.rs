//! Tests of `cracks` against the forms it replaced (2026-09-30): the run-based crack
//! extraction against the pixel scan, element for element, and the radix-sorted incidence
//! against the comparison sort, on empty images, one pixel, one row, one column, one
//! label, a checkerboard, noise, repeated rows, and the outside label inside the image.

use super::cracks::{dual_segments, radix_sort_by_node, Incidence, Seg};
use super::*;

/// Every pixel side separating two labels, pixel by pixel: the form [`dual_segments`]
/// replaced (`cracks::dual_segments`), kept as its test reference.
///
/// Vertical sides first (row by row), then horizontal ones. Each is oriented so that the
/// label it records as `left` is on the left when walking from `a` to `b` in image
/// coordinates (y down).
fn dual_segments_scan(labels: &[u16], w: usize, h: usize) -> Vec<Seg> {
    let mut segs: Vec<Seg> = Vec::new();

    // Vertical dual edges: node (i, j) -> (i, j+1) separates pixel (i-1, j) from (i, j).
    for j in 0..h {
        for i in 0..=w {
            let l = label_at(labels, w, h, i as isize - 1, j as isize);
            let r = label_at(labels, w, h, i as isize, j as isize);
            if l != r {
                // Walking downward (+y), the pixel on the left in screen terms is (i, j).
                segs.push(Seg {
                    a: node_id(i, j, w),
                    b: node_id(i, j + 1, w),
                    left: r,
                    right: l,
                });
            }
        }
    }
    // Horizontal dual edges: node (i, j) -> (i+1, j) separates pixel (i, j-1) from (i, j).
    for j in 0..=h {
        for i in 0..w {
            let u = label_at(labels, w, h, i as isize, j as isize - 1);
            let d = label_at(labels, w, h, i as isize, j as isize);
            if u != d {
                // Walking rightward (+x), the pixel above is on the left.
                segs.push(Seg {
                    a: node_id(i, j, w),
                    b: node_id(i + 1, j, w),
                    left: u,
                    right: d,
                });
            }
        }
    }
    segs
}

/// xorshift64, so the tests need no dependency.
struct Rng(u64);
impl Rng {
    fn below(&mut self, n: u64) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0 % n.max(1)
    }
}

/// Label maps covering the cases the run form must get right: empty images, one pixel,
/// one row, one column, one label everywhere, a checkerboard (a run per pixel), noise,
/// blocks with repeated rows, and the outside label `u16::MAX` inside the image (at
/// row starts and ends, and everywhere).
fn maps() -> Vec<(Vec<u16>, usize, usize)> {
    let mut rng = Rng(0x5851_F42D_4C95_7F2D);
    let mut out = Vec::new();
    let shapes = [
        (0usize, 0usize),
        (0, 4),
        (4, 0),
        (1, 1),
        (1, 17),
        (17, 1),
        (2, 2),
        (23, 19),
        (70, 45),
    ];
    for (w, h) in shapes {
        let n = w * h;
        out.push((vec![3u16; n], w, h));
        out.push((vec![u16::MAX; n], w, h));
        out.push((
            (0..n)
                .map(|p| ((p % w.max(1) + p / w.max(1)) % 2) as u16)
                .collect(),
            w,
            h,
        ));
        out.push(((0..n).map(|_| rng.below(4) as u16).collect(), w, h));
        let with_max: Vec<u16> = (0..n)
            .map(|_| match rng.below(4) {
                0 => u16::MAX,
                1 => u16::MAX - 1,
                k => k as u16,
            })
            .collect();
        out.push((with_max, w, h));
        // Blocks 5 px tall, so most rows repeat the row above.
        let bw = w.div_ceil(3).max(1);
        let blocks: Vec<u16> = (0..bw * h.div_ceil(5).max(1))
            .map(|_| rng.below(3) as u16)
            .collect();
        out.push((
            (0..n)
                .map(|p| blocks[(p / w / 5) * bw + (p % w) / 3])
                .collect(),
            w,
            h,
        ));
    }
    out
}

#[test]
fn segments_from_runs_equal_the_pixel_scan() {
    for (labels, w, h) in maps() {
        assert_eq!(
            dual_segments(&labels, w, h),
            dual_segments_scan(&labels, w, h),
            "{w}x{h}"
        );
    }
}

#[test]
fn incidence_from_the_radix_sort_equals_the_comparison_sort() {
    for (labels, w, h) in maps() {
        let mut segs = dual_segments(&labels, w, h);
        for round in 0..2 {
            let inc = Incidence::new(&segs);
            let (nodes, start, list) = Incidence::new_sorted(&segs);
            assert_eq!((&inc.nodes, &inc.start, &inc.segs), (&nodes, &start, &list));
            // The O(1) far-end lookup gives the list the binary search gives.
            for (k, s) in segs.iter().enumerate() {
                assert_eq!(Some(inc.end(k, false)), inc.get(s.a));
                assert_eq!(Some(inc.end(k, true)), inc.get(s.b));
            }
            if round == 0 {
                // Again after the saddle split, whose copies sit a whole grid further on.
                split_saddle_corners(&mut segs, &labels, w, h);
            }
        }
    }
}

#[test]
fn the_radix_sort_is_a_stable_sort_by_node() {
    let mut rng = Rng(0x2545_F491_4F6C_DD1D);
    for (m, range) in [
        (0usize, 1u64),
        (1, 1),
        (500, 7),
        (3000, 1 << 20),
        (3000, 1 << 32),
    ] {
        let v: Vec<(u32, usize)> = (0..m).map(|i| (rng.below(range) as u32, i)).collect();
        let mut want = v.clone();
        want.sort_by_key(|p| p.0);
        assert_eq!(radix_sort_by_node(v), want);
    }
    let top = vec![(u32::MAX, 0usize), (0, 1), (u32::MAX, 2), (1 << 31, 3)];
    assert_eq!(
        radix_sort_by_node(top),
        vec![(0, 1), (1 << 31, 3), (u32::MAX, 0), (u32::MAX, 2)]
    );
}
