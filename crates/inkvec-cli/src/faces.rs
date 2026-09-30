//! The vocabulary every writing stage shares: a face's rings, as walks over the shared
//! edges of the planar map.
//!
//! The planar map (`inkvec_trace::planar`) stores every boundary between two colours once,
//! as an *edge*, and `planar::face_edge_order` lists for each face the closed walks over
//! those edges that bound it. Fitting (`inkvec_fit`) then replaces each edge by one fitted
//! curve, indexed like the edges, so a ring here is a list of indices into that fitted
//! array. Nothing in this module computes anything; it exists so that `emit`, `mono`,
//! `rings`, `seams`, `harmonize` and `pathdata` can name these types without importing
//! them from the crate root, which would make every one of them depend on the driver.

/// One ring of a face: the edges it walks, each with whether it is walked backwards.
///
/// Edges are shared — the same edge appears in the two faces either side of it, once
/// forwards and once reversed — which is what keeps the two sides of a boundary on
/// exactly the same curve. The edge index addresses both the planar map's edges and the
/// fitted paths, which are kept in the same order.
pub(crate) type Ring = Vec<(usize, bool)>;

/// The rings of one face: its outer boundary and any holes.
///
/// Nothing downstream relies on the order they are listed in: [`crate::rings::nesting`]
/// works out which ring is the outline and which are holes from the fitted geometry.
pub(crate) type FaceRings = Vec<Ring>;

/// A recovered translucent layer and the face rings it covers.
///
/// The first half is the alpha analysis that found the layers (`inkvec_trace::alpha`); the
/// second holds, per layer and in the same order, the rings of the layer's own outline,
/// taken from a map in which the layer's pieces were merged into one face. See
/// `crate::pipeline` for how that map is built and `crate::emit` for how a layer is
/// painted.
pub(crate) type Layers<'a> = (&'a inkvec_trace::alpha::AlphaAnalysis, &'a [FaceRings]);
