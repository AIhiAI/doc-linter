//! Roadmap issue #12 (v0.3.0): entity-level ego graph. The pattern
//! matcher needs local structural context around each entity for
//! bipartite alignment, chain detection, and cross-repo comparison
//! — all of which compare ego graphs, not just node embeddings.
//!
//! Output shape:
//!
//! ```json
//! {
//!   "centre": "auth",
//!   "depth": 2,
//!   "nodes": [
//!     { "id": "auth", "display": "...", "mention_count": 34, "is_god_node": false }
//!   ],
//!   "edges": [
//!     { "source": "backend-api", "target": "auth", "type": "depends_on",
//!       "weight": 1.0, "edge_source": "frontmatter" }
//!   ]
//! }
//! ```
//!
//! BFS traversal at the Rust level so the per-edge metadata
//! (weight / source from roadmap #15) survives the projection.

use serde::Serialize;

/// One entity node in the ego graph. `mention_count` + `is_god_node`
/// read the persisted columns from roadmap #11 so the matcher gets
/// architectural weight without re-querying.
#[derive(Debug, Clone, Serialize)]
pub struct EntitySubgraphNode {
    pub id: String,
    pub display: String,
    pub mention_count: u64,
    pub is_god_node: bool,
}

/// One RELATES_TO edge in the ego graph. Surface every column on
/// the row (#15 added `weight` + `frequency` + `source`) so an
/// agent comparing two subgraphs has all the signal.
#[derive(Debug, Clone, Serialize)]
pub struct EntitySubgraphEdge {
    pub source: String,
    pub target: String,
    #[serde(rename = "type")]
    pub type_: String,
    pub weight: f64,
    pub frequency: i64,
    /// Renamed to avoid the `source` field collision (the edge's
    /// from-side already uses that name). Matches the column on
    /// disk (`r.source` — `"frontmatter"` / `"derived"` /
    /// `"inferred"`).
    pub edge_source: String,
}

/// Full payload for `query subgraph --entity`. Empty `nodes` /
/// `edges` arrays are preserved (rather than omitting fields) so
/// consumers can branch on shape.
#[derive(Debug, Clone, Serialize)]
pub struct EntitySubgraph {
    pub centre: String,
    pub depth: u32,
    pub nodes: Vec<EntitySubgraphNode>,
    pub edges: Vec<EntitySubgraphEdge>,
}
