//! Affine Shape Normalization, Equivalence Clustering, and Harmonization.
//!
//! When graphics contain repeated glyphs or recurring icons (such as multiple letters
//! from the same font or repeated UI symbols), rasterization and compression noise
//! introduce subtle, non-deterministic asymmetries between instances. Normal tracing
//! fits each instance independently, producing differing corner fillets and wobbles.
//!
//! This module normalizes closed contours into a canonical, scale- and translation-
//! invariant frame using 0th, 1st, and 2nd-order geometric moments. In canonical space,
//! equivalent shapes are identified via topological matching and contour IoU.
//! A consensus master shape is computed and re-projected back to each instance's
//! target pose via the inverse affine transform, restoring pixel-perfect consistency
//! and enabling compact SVG `<use>` symbol instancing.

use inkvec_core::Point;
use crate::curves::Segment;

/// A 2D Affine Transformation:
///
/// ```text
/// [ x' ]   [ a  c  tx ] [ x ]
/// [ y' ] = [ b  d  ty ] [ y ]
/// [ 1  ]   [ 0  0  1  ] [ 1 ]
/// ```
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AffineTransform {
    /// Horizontal scale / shear component $M_{00}$.
    pub a: f64,
    /// Vertical shear / scale component $M_{10}$.
    pub b: f64,
    /// Horizontal shear / scale component $M_{01}$.
    pub c: f64,
    /// Vertical scale / shear component $M_{11}$.
    pub d: f64,
    /// Horizontal translation component.
    pub tx: f64,
    /// Vertical translation component.
    pub ty: f64,
}

impl AffineTransform {
    /// The identity transformation.
    #[inline]
    pub fn identity() -> Self {
        Self {
            a: 1.0,
            b: 0.0,
            c: 0.0,
            d: 1.0,
            tx: 0.0,
            ty: 0.0,
        }
    }

    /// Translation by `(tx, ty)`.
    #[inline]
    pub fn from_translation(tx: f64, ty: f64) -> Self {
        Self {
            a: 1.0,
            b: 0.0,
            c: 0.0,
            d: 1.0,
            tx,
            ty,
        }
    }

    /// Uniform scale centered at origin.
    #[inline]
    pub fn from_scale(s: f64) -> Self {
        Self {
            a: s,
            b: 0.0,
            c: 0.0,
            d: s,
            tx: 0.0,
            ty: 0.0,
        }
    }

    /// Composes `self` with `other` (evaluates `other` first, then `self`).
    #[inline]
    pub fn compose(&self, o: &AffineTransform) -> Self {
        Self {
            a: self.a * o.a + self.c * o.b,
            b: self.b * o.a + self.d * o.b,
            c: self.a * o.c + self.c * o.d,
            d: self.b * o.c + self.d * o.d,
            tx: self.a * o.tx + self.c * o.ty + self.tx,
            ty: self.b * o.tx + self.d * o.ty + self.ty,
        }
    }

    /// Determinant of the 2x2 linear part.
    #[inline]
    pub fn det(&self) -> f64 {
        self.a * self.d - self.b * self.c
    }

    /// Inverts the affine transformation, returning `None` if degenerate.
    pub fn invert(&self) -> Option<Self> {
        let d = self.det();
        if d.abs() < 1e-12 {
            return None;
        }
        let inv_d = 1.0 / d;
        let ia = self.d * inv_d;
        let ib = -self.b * inv_d;
        let ic = -self.c * inv_d;
        let id = self.a * inv_d;
        let itx = -(ia * self.tx + ic * self.ty);
        let ity = -(ib * self.tx + id * self.ty);
        Some(Self {
            a: ia,
            b: ib,
            c: ic,
            d: id,
            tx: itx,
            ty: ity,
        })
    }

    /// Applies the transform to a 2D point.
    #[inline]
    pub fn apply_point(&self, p: Point) -> Point {
        Point::new(
            self.a * p.x + self.c * p.y + self.tx,
            self.b * p.x + self.d * p.y + self.ty,
        )
    }

    /// Applies the transform to a path segment.
    pub fn apply_segment(&self, seg: &Segment) -> Segment {
        match seg {
            Segment::Line(p) => Segment::Line(self.apply_point(*p)),
            Segment::Cubic(p1, p2, p3) => Segment::Cubic(
                self.apply_point(*p1),
                self.apply_point(*p2),
                self.apply_point(*p3),
            ),
            Segment::Arc {
                rx,
                ry,
                phi,
                large_arc,
                sweep,
                end,
            } => {
                // Approximate scale factor for radii
                let s = self.det().abs().sqrt();
                let flip = self.det() < 0.0;
                Segment::Arc {
                    rx: *rx * s,
                    ry: *ry * s,
                    phi: *phi,
                    large_arc: *large_arc,
                    sweep: if flip { !*sweep } else { *sweep },
                    end: self.apply_point(*end),
                }
            }
        }
    }

    /// Formats as an SVG `transform="matrix(a b c d e f)"` string.
    pub fn svg_matrix(&self) -> String {
        format!(
            "matrix({:.4} {:.4} {:.4} {:.4} {:.4} {:.4})",
            self.a, self.b, self.c, self.d, self.tx, self.ty
        )
    }
}

/// Geometric moments of a 2D closed polygon contour.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PolygonMoments {
    /// Signed area (0th moment $M_{00}$).
    pub area: f64,
    /// Center of mass horizontal coordinate.
    pub cx: f64,
    /// Center of mass vertical coordinate.
    pub cy: f64,
    /// Second central moment $\mu_{20}$ (horizontal variance).
    pub mu20: f64,
    /// Second central moment $\mu_{02}$ (vertical variance).
    pub mu02: f64,
    /// Second central moment $\mu_{11}$ (covariance).
    pub mu11: f64,
}

/// Computes the exact geometric moments of a closed polygon via Green's Theorem.
pub fn compute_polygon_moments(pts: &[Point]) -> Option<PolygonMoments> {
    if pts.len() < 3 {
        return None;
    }

    let n = pts.len();
    let mut area2 = 0.0;
    let mut cx_acc = 0.0;
    let mut cy_acc = 0.0;
    let mut mu20_acc = 0.0;
    let mut mu02_acc = 0.0;
    let mut mu11_acc = 0.0;

    for i in 0..n {
        let next = (i + 1) % n;
        let x0 = pts[i].x;
        let y0 = pts[i].y;
        let x1 = pts[next].x;
        let y1 = pts[next].y;

        let cross = x0 * y1 - x1 * y0;
        area2 += cross;
        cx_acc += (x0 + x1) * cross;
        cy_acc += (y0 + y1) * cross;

        mu20_acc += (x0 * x0 + x0 * x1 + x1 * x1) * cross;
        mu02_acc += (y0 * y0 + y0 * y1 + y1 * y1) * cross;
        mu11_acc += (2.0 * x0 * y0 + x0 * y1 + x1 * y0 + 2.0 * x1 * y1) * cross;
    }

    let area = 0.5 * area2;
    if area.abs() < 1e-6 {
        return None;
    }

    let cx = cx_acc / (6.0 * area);
    let cy = cy_acc / (6.0 * area);

    let mu20 = (mu20_acc / (12.0 * area)) - cx * cx;
    let mu02 = (mu02_acc / (12.0 * area)) - cy * cy;
    let mu11 = (mu11_acc / (24.0 * area)) - cx * cy;

    Some(PolygonMoments {
        area,
        cx,
        cy,
        mu20,
        mu02,
        mu11,
    })
}

/// A closed path or glyph with its geometry and normalization metadata.
#[derive(Debug, Clone)]
pub struct NormalizedContour {
    /// The starting point of the closed path.
    pub start: Point,
    /// The fitted segments composing the contour.
    pub segments: Vec<Segment>,
    /// Dense boundary sample points representing the shape.
    pub sample_points: Vec<Point>,
    /// Geometric moments of the contour.
    pub moments: PolygonMoments,
    /// Affine transform mapping this shape into canonical standard space.
    pub to_canonical: AffineTransform,
    /// Affine transform mapping canonical space back to this shape's target pose.
    pub from_canonical: AffineTransform,
}

impl NormalizedContour {
    /// Builds a normalized contour from fitted segments and boundary points.
    pub fn new(start: Point, segments: Vec<Segment>, sample_points: Vec<Point>) -> Option<Self> {
        let moments = compute_polygon_moments(&sample_points)?;
        let abs_area = moments.area.abs();
        if abs_area < 1.0 {
            return None;
        }

        // Scale factor: normalize area to target canonical area (e.g. 100x100 = 10000.0)
        let target_area = 10000.0;
        let scale = (target_area / abs_area).sqrt();

        // T(p) = scale * (p - centroid) + (canonical_center)
        let canon_center = 100.0;
        let to_canonical = AffineTransform {
            a: scale,
            b: 0.0,
            c: 0.0,
            d: scale,
            tx: canon_center - scale * moments.cx,
            ty: canon_center - scale * moments.cy,
        };

        let from_canonical = to_canonical.invert()?;

        Some(Self {
            start,
            segments,
            sample_points,
            moments,
            to_canonical,
            from_canonical,
        })
    }

    /// Rasterizes the canonical shape into a fixed-size binary occupancy bitmask for fast IoU.
    pub fn rasterize_canonical(&self, grid_size: usize) -> Vec<bool> {
        let mut mask = vec![false; grid_size * grid_size];
        let canon_pts: Vec<Point> = self
            .sample_points
            .iter()
            .map(|&p| self.to_canonical.apply_point(p))
            .collect();

        if canon_pts.len() < 3 {
            return mask;
        }

        let n = canon_pts.len();
        for y in 0..grid_size {
            let py = (y as f64 + 0.5) * (200.0 / grid_size as f64);
            for x in 0..grid_size {
                let px = (x as f64 + 0.5) * (200.0 / grid_size as f64);
                // Ray casting winding check
                let mut inside = false;
                let mut j = n - 1;
                for i in 0..n {
                    let pi = canon_pts[i];
                    let pj = canon_pts[j];
                    if ((pi.y > py) != (pj.y > py))
                        && (px < (pj.x - pi.x) * (py - pi.y) / (pj.y - pi.y) + pi.x)
                    {
                        inside = !inside;
                    }
                    j = i;
                }
                if inside {
                    mask[y * grid_size + x] = true;
                }
            }
        }
        mask
    }
}

/// Measures the Intersection-over-Union (IoU) of two canonical occupancy grids.
pub fn canonical_iou(mask1: &[bool], mask2: &[bool]) -> f64 {
    assert_eq!(mask1.len(), mask2.len());
    let mut intersection = 0usize;
    let mut union = 0usize;

    for i in 0..mask1.len() {
        let a = mask1[i];
        let b = mask2[i];
        if a && b {
            intersection += 1;
        }
        if a || b {
            union += 1;
        }
    }

    if union == 0 {
        0.0
    } else {
        intersection as f64 / union as f64
    }
}

/// A cluster of equivalent recurring shapes.
#[derive(Debug, Clone)]
pub struct EquivalenceClass {
    /// Indices of shapes in this cluster.
    pub members: Vec<usize>,
    /// The canonical consensus starting point.
    pub canonical_start: Point,
    /// The canonical consensus segments (in canonical frame).
    pub canonical_segments: Vec<Segment>,
}

/// Groups candidate contours into equivalence classes based on canonical IoU.
pub fn cluster_equivalent_shapes(
    contours: &[NormalizedContour],
    iou_threshold: f64,
) -> Vec<EquivalenceClass> {
    let n = contours.len();
    if n == 0 {
        return Vec::new();
    }

    let grid_size = 48;
    let masks: Vec<Vec<bool>> = contours
        .iter()
        .map(|c| c.rasterize_canonical(grid_size))
        .collect();

    let mut visited = vec![false; n];
    let mut clusters = Vec::new();

    for i in 0..n {
        if visited[i] {
            continue;
        }
        visited[i] = true;
        let mut members = vec![i];

        for j in i + 1..n {
            if visited[j] {
                continue;
            }

            // Quick moment ratio check: moments must be reasonably similar
            let ar_i = contours[i].moments.mu20 / (contours[i].moments.mu02.max(1e-4));
            let ar_j = contours[j].moments.mu20 / (contours[j].moments.mu02.max(1e-4));
            if (ar_i - ar_j).abs() > 0.35 * ar_i.max(ar_j) {
                continue;
            }

            let iou = canonical_iou(&masks[i], &masks[j]);
            if iou >= iou_threshold {
                visited[j] = true;
                members.push(j);
            }
        }

        // Pick exemplar with cleanest representation (e.g. fewest segments / best arcs)
        let exemplar_idx = *members
            .iter()
            .min_by_key(|&&m| contours[m].segments.len())
            .unwrap_or(&i);

        let exemplar = &contours[exemplar_idx];
        let canon_start = exemplar.to_canonical.apply_point(exemplar.start);
        let canon_segments: Vec<Segment> = exemplar
            .segments
            .iter()
            .map(|seg| exemplar.to_canonical.apply_segment(seg))
            .collect();

        clusters.push(EquivalenceClass {
            members,
            canonical_start: canon_start,
            canonical_segments: canon_segments,
        });
    }

    clusters
}

/// A compound shape contour: an outer boundary loop with zero or more inner hole loops.
#[derive(Debug, Clone)]
pub struct CompoundShape {
    /// Outer boundary start point.
    pub outer_start: Point,
    /// Outer boundary segments.
    pub outer_segments: Vec<Segment>,
    /// Outer boundary sample points.
    pub outer_points: Vec<Point>,
    /// Inner holes (each with start, segments, and sample points).
    pub holes: Vec<(Point, Vec<Segment>, Vec<Point>)>,
    /// Geometric moments of the outer boundary.
    pub moments: PolygonMoments,
    /// Forward transform mapping this shape into canonical standard space.
    pub to_canonical: AffineTransform,
    /// Inverse transform mapping canonical space back to this shape's target pose.
    pub from_canonical: AffineTransform,
}

impl CompoundShape {
    /// Builds a new compound shape.
    pub fn new(
        outer_start: Point,
        outer_segments: Vec<Segment>,
        outer_points: Vec<Point>,
        holes: Vec<(Point, Vec<Segment>, Vec<Point>)>,
    ) -> Option<Self> {
        let moments = compute_polygon_moments(&outer_points)?;
        let abs_area = moments.area.abs();
        if abs_area < 1.0 {
            return None;
        }

        let target_area = 10000.0;
        let scale = (target_area / abs_area).sqrt();
        let canon_center = 100.0;

        let to_canonical = AffineTransform {
            a: scale,
            b: 0.0,
            c: 0.0,
            d: scale,
            tx: canon_center - scale * moments.cx,
            ty: canon_center - scale * moments.cy,
        };

        let from_canonical = to_canonical.invert()?;

        Some(Self {
            outer_start,
            outer_segments,
            outer_points,
            holes,
            moments,
            to_canonical,
            from_canonical,
        })
    }

    /// Rasterizes the compound shape (outer minus holes) into a canonical binary bitmask.
    pub fn rasterize_canonical(&self, grid_size: usize) -> Vec<bool> {
        let mut mask = vec![false; grid_size * grid_size];
        let canon_outer: Vec<Point> = self
            .outer_points
            .iter()
            .map(|&p| self.to_canonical.apply_point(p))
            .collect();

        if canon_outer.len() < 3 {
            return mask;
        }

        let canon_holes: Vec<Vec<Point>> = self
            .holes
            .iter()
            .map(|(_, _, pts)| {
                pts.iter()
                    .map(|&p| self.to_canonical.apply_point(p))
                    .collect()
            })
            .collect();

        let n_outer = canon_outer.len();
        for y in 0..grid_size {
            let py = (y as f64 + 0.5) * (200.0 / grid_size as f64);
            for x in 0..grid_size {
                let px = (x as f64 + 0.5) * (200.0 / grid_size as f64);

                let mut inside_outer = false;
                let mut j = n_outer - 1;
                for i in 0..n_outer {
                    let pi = canon_outer[i];
                    let pj = canon_outer[j];
                    if ((pi.y > py) != (pj.y > py))
                        && (px < (pj.x - pi.x) * (py - pi.y) / (pj.y - pi.y) + pi.x)
                    {
                        inside_outer = !inside_outer;
                    }
                    j = i;
                }

                if !inside_outer {
                    continue;
                }

                let mut inside_hole = false;
                for hole_pts in &canon_holes {
                    let n_h = hole_pts.len();
                    if n_h < 3 {
                        continue;
                    }
                    let mut hj = n_h - 1;
                    let mut in_this_hole = false;
                    for hi in 0..n_h {
                        let pi = hole_pts[hi];
                        let pj = hole_pts[hj];
                        if ((pi.y > py) != (pj.y > py))
                            && (px < (pj.x - pi.x) * (py - pi.y) / (pj.y - pi.y) + pi.x)
                        {
                            in_this_hole = !in_this_hole;
                        }
                        hj = hi;
                    }
                    if in_this_hole {
                        inside_hole = true;
                        break;
                    }
                }

                if !inside_hole {
                    mask[y * grid_size + x] = true;
                }
            }
        }
        mask
    }
}

/// A cluster of equivalent recurring compound shapes (with holes).
#[derive(Debug, Clone)]
pub struct CompoundEquivalenceClass {
    /// Indices of shapes in this cluster.
    pub members: Vec<usize>,
    /// The index of the exemplar shape.
    pub exemplar: usize,
    /// The canonical consensus starting point of the outer boundary.
    pub canonical_outer_start: Point,
    /// The canonical consensus segments of the outer boundary.
    pub canonical_outer_segments: Vec<Segment>,
    /// The canonical consensus holes (each with start point and segments).
    pub canonical_holes: Vec<(Point, Vec<Segment>)>,
}

/// Clusters compound shapes based on matching hole topology and canonical IoU.
pub fn cluster_compound_shapes(
    shapes: &[CompoundShape],
    iou_threshold: f64,
) -> Vec<CompoundEquivalenceClass> {
    let n = shapes.len();
    if n == 0 {
        return Vec::new();
    }

    let grid_size = 48;
    let masks: Vec<Vec<bool>> = shapes
        .iter()
        .map(|s| s.rasterize_canonical(grid_size))
        .collect();

    let mut visited = vec![false; n];
    let mut clusters = Vec::new();

    for i in 0..n {
        if visited[i] {
            continue;
        }
        visited[i] = true;
        let mut members = vec![i];

        for j in i + 1..n {
            if visited[j] {
                continue;
            }

            if shapes[i].holes.len() != shapes[j].holes.len() {
                continue;
            }

            let ar_i = shapes[i].moments.mu20 / (shapes[i].moments.mu02.max(1e-4));
            let ar_j = shapes[j].moments.mu20 / (shapes[j].moments.mu02.max(1e-4));
            if (ar_i - ar_j).abs() > 0.35 * ar_i.max(ar_j) {
                continue;
            }

            let iou = canonical_iou(&masks[i], &masks[j]);
            if iou >= iou_threshold {
                visited[j] = true;
                members.push(j);
            }
        }

        let exemplar_idx = *members
            .iter()
            .min_by_key(|&&m| {
                shapes[m].outer_segments.len()
                    + shapes[m].holes.iter().map(|(_, segs, _)| segs.len()).sum::<usize>()
            })
            .unwrap_or(&i);

        let exemplar = &shapes[exemplar_idx];
        let mut canon_outer_start = exemplar.to_canonical.apply_point(exemplar.outer_start);
        let mut canon_outer_segments: Vec<Segment> = exemplar
            .outer_segments
            .iter()
            .map(|seg| exemplar.to_canonical.apply_segment(seg))
            .collect();

        let mut canon_holes: Vec<(Point, Vec<Segment>)> = exemplar
            .holes
            .iter()
            .map(|(start, segs, _)| {
                (
                    exemplar.to_canonical.apply_point(*start),
                    segs.iter()
                        .map(|s| exemplar.to_canonical.apply_segment(s))
                        .collect(),
                )
            })
            .collect();

        // When multiple equivalent instances exist, compute the robust spatial median consensus
        // across all members in canonical space. This cancels out raster-phase quantization noise
        // and rejects single-glyph outliers / defects.
        if members.len() > 1 {
            let member_outer_polys: Vec<Vec<Point>> = members
                .iter()
                .map(|&m| {
                    shapes[m]
                        .outer_points
                        .iter()
                        .map(|&p| shapes[m].to_canonical.apply_point(p))
                        .collect()
                })
                .collect();

            let candidates: Vec<Point> = member_outer_polys
                .iter()
                .map(|poly| closest_point_on_closed_polyline(canon_outer_start, poly))
                .collect();
            let new_start = median_point(&candidates);
            let mut cur_delta = Point::new(new_start.x - canon_outer_start.x, new_start.y - canon_outer_start.y);
            canon_outer_start = new_start;

            for seg in canon_outer_segments.iter_mut() {
                let end = seg.end();
                let end_candidates: Vec<Point> = member_outer_polys
                    .iter()
                    .map(|poly| closest_point_on_closed_polyline(end, poly))
                    .collect();
                let new_end = median_point(&end_candidates);
                let next_delta = Point::new(new_end.x - end.x, new_end.y - end.y);

                match seg {
                    Segment::Line(ref mut p) => {
                        *p = new_end;
                    }
                    Segment::Cubic(ref mut c1, ref mut c2, ref mut p) => {
                        c1.x += cur_delta.x;
                        c1.y += cur_delta.y;
                        c2.x += next_delta.x;
                        c2.y += next_delta.y;
                        *p = new_end;
                    }
                    Segment::Arc { ref mut end, .. } => {
                        *end = new_end;
                    }
                }
                cur_delta = next_delta;
            }

            for (hole_idx, (h_start, h_segs)) in canon_holes.iter_mut().enumerate() {
                let member_hole_polys: Vec<Vec<Point>> = members
                    .iter()
                    .filter_map(|&m| {
                        shapes[m].holes.get(hole_idx).map(|(_, _, pts)| {
                            pts.iter()
                                .map(|&p| shapes[m].to_canonical.apply_point(p))
                                .collect()
                        })
                    })
                    .collect();

                if member_hole_polys.len() == members.len() {
                    let h_start_candidates: Vec<Point> = member_hole_polys
                        .iter()
                        .map(|poly| closest_point_on_closed_polyline(*h_start, poly))
                        .collect();
                    let new_h_start = median_point(&h_start_candidates);
                    let mut h_cur_delta = Point::new(new_h_start.x - h_start.x, new_h_start.y - h_start.y);
                    *h_start = new_h_start;

                    for seg in h_segs.iter_mut() {
                        let end = seg.end();
                        let end_candidates: Vec<Point> = member_hole_polys
                            .iter()
                            .map(|poly| closest_point_on_closed_polyline(end, poly))
                            .collect();
                        let new_end = median_point(&end_candidates);
                        let next_delta = Point::new(new_end.x - end.x, new_end.y - end.y);

                        match seg {
                            Segment::Line(ref mut p) => {
                                *p = new_end;
                            }
                            Segment::Cubic(ref mut c1, ref mut c2, ref mut p) => {
                                c1.x += h_cur_delta.x;
                                c1.y += h_cur_delta.y;
                                c2.x += next_delta.x;
                                c2.y += next_delta.y;
                                *p = new_end;
                            }
                            Segment::Arc { ref mut end, .. } => {
                                *end = new_end;
                            }
                        }
                        h_cur_delta = next_delta;
                    }
                }
            }
        }

        clusters.push(CompoundEquivalenceClass {
            members,
            exemplar: exemplar_idx,
            canonical_outer_start: canon_outer_start,
            canonical_outer_segments: canon_outer_segments,
            canonical_holes: canon_holes,
        });
    }

    clusters
}

/// Finds the closest point on a closed polyline to point `p`.
pub fn closest_point_on_closed_polyline(p: Point, poly: &[Point]) -> Point {
    if poly.is_empty() {
        return p;
    }
    if poly.len() == 1 {
        return poly[0];
    }
    let mut best_pt = poly[0];
    let mut best_dist_sq = f64::INFINITY;

    let n = poly.len();
    for i in 0..n {
        let a = poly[i];
        let b = poly[(i + 1) % n];
        let ab_x = b.x - a.x;
        let ab_y = b.y - a.y;
        let len_sq = ab_x * ab_x + ab_y * ab_y;
        let proj = if len_sq < 1e-12 {
            a
        } else {
            let t = (((p.x - a.x) * ab_x + (p.y - a.y) * ab_y) / len_sq).clamp(0.0, 1.0);
            Point::new(a.x + t * ab_x, a.y + t * ab_y)
        };
        let d_sq = (p.x - proj.x).powi(2) + (p.y - proj.y).powi(2);
        if d_sq < best_dist_sq {
            best_dist_sq = d_sq;
            best_pt = proj;
        }
    }
    best_pt
}

/// Computes the coordinate-wise median of a list of points.
pub fn median_point(pts: &[Point]) -> Point {
    if pts.is_empty() {
        return Point::new(0.0, 0.0);
    }
    if pts.len() == 1 {
        return pts[0];
    }
    if pts.len() == 2 {
        return Point::new(0.5 * (pts[0].x + pts[1].x), 0.5 * (pts[0].y + pts[1].y));
    }
    let mut xs: Vec<f64> = pts.iter().map(|p| p.x).collect();
    let mut ys: Vec<f64> = pts.iter().map(|p| p.y).collect();
    xs.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    ys.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let mid = xs.len() / 2;
    let med_x = if xs.len() % 2 == 1 {
        xs[mid]
    } else {
        0.5 * (xs[mid - 1] + xs[mid])
    };
    let med_y = if ys.len() % 2 == 1 {
        ys[mid]
    } else {
        0.5 * (ys[mid - 1] + ys[mid])
    };
    Point::new(med_x, med_y)
}

#[cfg(test)]
mod tests {
    use super::*;
    use inkvec_core::Point;

    #[test]
    fn test_affine_identity_and_inversion() {
        let t = AffineTransform::from_translation(10.0, -20.0);
        let s = AffineTransform::from_scale(2.5);
        let comp = t.compose(&s);

        let p = Point::new(4.0, 6.0);
        let pt = comp.apply_point(p);
        assert!((pt.x - 20.0).abs() < 1e-9);
        assert!((pt.y - (-5.0)).abs() < 1e-9);

        let inv = comp.invert().expect("invertible");
        let orig = inv.apply_point(pt);
        assert!((orig.x - p.x).abs() < 1e-9);
        assert!((orig.y - p.y).abs() < 1e-9);
    }

    #[test]
    fn test_polygon_moments_square() {
        // 10x10 square at (10, 20)
        let pts = vec![
            Point::new(10.0, 20.0),
            Point::new(20.0, 20.0),
            Point::new(20.0, 30.0),
            Point::new(10.0, 30.0),
        ];

        let m = compute_polygon_moments(&pts).expect("valid moments");
        assert!((m.area.abs() - 100.0).abs() < 1e-6);
        assert!((m.cx - 15.0).abs() < 1e-6);
        assert!((m.cy - 25.0).abs() < 1e-6);
    }

    #[test]
    fn test_canonical_iou_between_shifted_squares() {
        let square1 = vec![
            Point::new(10.0, 20.0),
            Point::new(50.0, 20.0),
            Point::new(50.0, 60.0),
            Point::new(10.0, 60.0),
        ];
        let square2 = vec![
            Point::new(500.0, 700.0),
            Point::new(580.0, 700.0),
            Point::new(580.0, 780.0),
            Point::new(500.0, 780.0),
        ];

        let c1 = NormalizedContour::new(
            square1[0],
            vec![
                Segment::Line(square1[1]),
                Segment::Line(square1[2]),
                Segment::Line(square1[3]),
                Segment::Line(square1[0]),
            ],
            square1,
        )
        .expect("contour 1");

        let c2 = NormalizedContour::new(
            square2[0],
            vec![
                Segment::Line(square2[1]),
                Segment::Line(square2[2]),
                Segment::Line(square2[3]),
                Segment::Line(square2[0]),
            ],
            square2,
        )
        .expect("contour 2");

        let mask1 = c1.rasterize_canonical(32);
        let mask2 = c2.rasterize_canonical(32);
        let iou = canonical_iou(&mask1, &mask2);
        assert!(
            iou > 0.95,
            "Shifted and scaled identical shapes must have IoU > 0.95, got {iou}"
        );
    }

    #[test]
    fn test_compound_shape_with_holes() {
        // Outer 100x100 square with an inner 20x20 hole (like letter 'O')
        let outer1 = vec![
            Point::new(0.0, 0.0),
            Point::new(100.0, 0.0),
            Point::new(100.0, 100.0),
            Point::new(0.0, 100.0),
        ];
        let hole1 = vec![
            Point::new(40.0, 40.0),
            Point::new(60.0, 40.0),
            Point::new(60.0, 60.0),
            Point::new(40.0, 60.0),
        ];

        // Second instance translated and scaled
        let outer2 = vec![
            Point::new(300.0, 400.0),
            Point::new(500.0, 400.0),
            Point::new(500.0, 600.0),
            Point::new(300.0, 600.0),
        ];
        let hole2 = vec![
            Point::new(380.0, 480.0),
            Point::new(420.0, 480.0),
            Point::new(420.0, 520.0),
            Point::new(380.0, 520.0),
        ];

        let s1 = CompoundShape::new(
            outer1[0],
            vec![
                Segment::Line(outer1[1]),
                Segment::Line(outer1[2]),
                Segment::Line(outer1[3]),
                Segment::Line(outer1[0]),
            ],
            outer1,
            vec![(
                hole1[0],
                vec![
                    Segment::Line(hole1[1]),
                    Segment::Line(hole1[2]),
                    Segment::Line(hole1[3]),
                    Segment::Line(hole1[0]),
                ],
                hole1,
            )],
        )
        .expect("compound shape 1");

        let s2 = CompoundShape::new(
            outer2[0],
            vec![
                Segment::Line(outer2[1]),
                Segment::Line(outer2[2]),
                Segment::Line(outer2[3]),
                Segment::Line(outer2[0]),
            ],
            outer2,
            vec![(
                hole2[0],
                vec![
                    Segment::Line(hole2[1]),
                    Segment::Line(hole2[2]),
                    Segment::Line(hole2[3]),
                    Segment::Line(hole2[0]),
                ],
                hole2,
            )],
        )
        .expect("compound shape 2");

        let clusters = cluster_compound_shapes(&[s1, s2], 0.90);
        assert_eq!(clusters.len(), 1, "Both shapes with holes should cluster together");
        assert_eq!(clusters[0].members.len(), 2);
    }

    #[test]
    fn test_median_point_and_closest_point() {
        let p1 = Point::new(10.0, 20.0);
        let p2 = Point::new(12.0, 24.0);
        let mid = median_point(&[p1, p2]);
        assert_eq!(mid, Point::new(11.0, 22.0));

        let p3 = Point::new(100.0, 200.0); // outlier
        let med3 = median_point(&[p1, p2, p3]);
        assert_eq!(med3, Point::new(12.0, 24.0)); // median rejects outlier!

        let poly = vec![
            Point::new(0.0, 0.0),
            Point::new(10.0, 0.0),
            Point::new(10.0, 10.0),
            Point::new(0.0, 10.0),
        ];
        let q = Point::new(5.0, -2.0);
        let closest = closest_point_on_closed_polyline(q, &poly);
        assert_eq!(closest, Point::new(5.0, 0.0));
    }
}

