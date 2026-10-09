//! Read-side query helpers over the [[entity-doc-graph]] SQLite store.
//!
//! Split by node-type so adding a new query lands in the obvious file:
//!
//! - [`docs`] — Doc enumeration, neighbor walks, shortest-path, raw
//!   `sql` passthrough.
//! - [`entities`] — Entity ranking, single lookup, per-crate top-K.
//! - [`functions`] — Function symbol search, full-context fetch,
//!   mention/sibling walks, doc-coverage join.
//! - [`endpoints`] — Endpoint listing + per-kind reach metrics.
//! - [`coverage`] — Phase 3/4 of roadmap-43: dark-function /
//!   dark-endpoint / coverage-gap diagnostics + scaffolding inputs.

pub mod at;
pub mod coverage;
pub mod dead_code;
pub mod docs;
pub mod endpoints;
pub mod entities;
pub mod entity_ego;
pub mod entity_subgraph;
pub mod functions;
pub mod impact;
pub mod map_render;
pub mod saved;

pub use at::{AtCoveringDoc, AtEndpoint, AtEntityLink, AtQueryResult, FileRow, ModuleRow};
pub use dead_code::DeadCodeRow;
pub use entity_ego::{EgoEdge, EgoNode, EntityEgoGraph};
pub use entity_subgraph::{EntitySubgraph, EntitySubgraphEdge, EntitySubgraphNode};
pub use impact::{ImpactDepthLevel, ImpactEndpoint, ImpactEntity, ImpactResult};
pub use map_render::render_entity_mermaid;
pub use saved::{
    fill_params, list_saved_queries, list_saved_queries_with_root, load_runtime_queries,
    merged_catalog, resolve_query, RuntimeSavedQuery, SavedQuery, SavedQueryListing,
};
