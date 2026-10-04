//! Constraints on the multimodel program: a cap on how many measured points a segment may
//! span, and vertices every solution must keep.
//!
//! Normal fitting has neither ([`Limits::is_free`]). Stage 12's crossing repair
//! (`inkvec-cli`'s `rings::repair_ring_crossings`) refits a guilty boundary under one or both:
//! a halved cap (`optimal_multimodel_capped_full`) or pins beside each crossing
//! ([`optimal_multimodel_forced`]). The scan reads them span by span through
//! [`Limits::allows`] (`scan::SpanScorer::fill`), the closed-loop solver cuts a pinned loop at
//! a pin (`solve_closed`), and the post-fit passes run on a pinned, uncapped fit with the pins
//! kept (`post_fit_passes`).

use super::*;

/// Which spans `i..j` the dynamic program may use: the constraints the crossing repair puts
/// on a refit, and nothing at all in normal fitting ([`Limits::is_free`]).
///
/// A span is admissible when it covers at most `max_span` measured points and no forced
/// vertex lies strictly inside it, so every solution has each forced vertex as one of its
/// vertices. Both conditions are monotone in `j` for a fixed start `i` (once a span from
/// `i` is refused, every longer one from `i` is refused too), which is what lets the scan
/// drop a start for good the first time it is refused (`scan::SpanScorer::fill`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Limits {
    /// The most measured points one segment may span; `usize::MAX` for no cap.
    pub(crate) max_span: usize,
    /// Indices of the polyline being solved, ascending, that every solution must take as
    /// vertices. Empty in normal fitting.
    pub(crate) forced: Vec<usize>,
}

impl Limits {
    /// Spans of at most `max_span` points and no forced vertex.
    pub(crate) fn capped(max_span: usize) -> Self {
        Limits {
            max_span,
            forced: Vec::new(),
        }
    }

    /// No constraint at all: the normal fit, which may decimate and runs the post-fit passes.
    pub(crate) fn is_free(&self) -> bool {
        self.max_span == usize::MAX && self.forced.is_empty()
    }

    /// Whether the span `i..j` (`i < j`) is admissible: at most `max_span` points, and no
    /// forced vertex `f` with `i < f < j`. O(number of forced vertices), which is a handful.
    #[inline]
    pub(crate) fn allows(&self, i: usize, j: usize) -> bool {
        j - i <= self.max_span && self.forced.iter().all(|&f| f <= i || f >= j)
    }

    /// The last index a span from `i` may reach under the forced vertices: the first one
    /// after `i`, or `usize::MAX` when there is none.
    #[inline]
    pub(crate) fn wall_after(&self, i: usize) -> usize {
        self.forced
            .iter()
            .copied()
            .find(|&f| f > i)
            .unwrap_or(usize::MAX)
    }
}

/// As [`optimal_multimodel_capped_full`], with the vertices in `forced` (indices into
/// `poly`, any order, duplicates ignored) imposed on every solution: the local crossing
/// repair's refit.
///
/// The program is the same one, restricted to the segmentations that break at every forced
/// vertex: no span may pass over one (`Limits::allows`). Breaking there costs what any
/// vertex costs (its [`vertex_cost`]), so among the segmentations that keep the forced
/// vertices this is still the optimum of `½χ² + λ·params + breaks`. Like the capped fit it
/// runs on every measured point (no decimation, so the forced indices mean what the caller
/// meant). Uncapped (`max_span` at least the point count), it then runs the usual post-fit
/// passes with the forced vertices kept (`crate::merge::merge_free_cubics_keeping`): a merge
/// that absorbed one would reopen the crossing it was placed to close, and without the
/// merge the refit is the raw program's answer, more segments than the merged fit it
/// replaces. Under a cap, like the capped fit, it skips them.
///
/// On a closed boundary the loop is cut at a forced vertex, which is exact rather than the
/// two-cut heuristic of `solve_closed`: a vertex every admissible solution has is a cut
/// that loses nothing. With `forced` empty it is [`optimal_multimodel_capped_full`] under a
/// cap below the point count, and [`optimal_multimodel_full`] (decimation, merge and all)
/// without one; the repair calls the capped program itself when it has nothing to pin.
///
/// Inspired by: the topology-preserving subdivision simplification literature, which adds
/// vertices back only where a simplified chain would cross another, rather than tightening
/// the tolerance of the whole chain — de Berg, van Kreveld & Schirra (1998), "Topologically
/// correct subdivision simplification using the bandwidth criterion", *Cartography and
/// Geographic Information Systems* 25(4):243-257, doi:10.1559/152304098782383007; Saalfeld
/// (1999), "Topologically consistent line simplification with the Douglas-Peucker
/// algorithm", *Cartography and Geographic Information Science* 26(1):7-18,
/// doi:10.1559/152304099782424901. Ours keeps the description-length program and only
/// constrains where it may break.
pub fn optimal_multimodel_forced(
    poly: &Polyline,
    cfg: &FitConfig,
    max_span: usize,
    forced: &[usize],
) -> MultimodelFit {
    let n = poly.len();
    let mut f: Vec<usize> = forced.iter().copied().filter(|&k| k < n).collect();
    f.sort_unstable();
    f.dedup();
    if !poly.closed {
        // The ends of an open boundary are vertices of every solution already.
        f.retain(|&k| k > 0 && k + 1 < n);
    }
    // No span can cover more than `n` points (an opened loop has `n + 1`), so a cap of `n` or
    // more is no cap: say so, which is what lets the pinned fit run the post-fit passes.
    let lim = Limits {
        max_span: if max_span >= n {
            usize::MAX
        } else {
            max_span.max(1)
        },
        forced: f,
    };
    optimal_multimodel_impl(poly, cfg, &lim, research::structural())
}
