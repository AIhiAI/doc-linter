//! Roadmap issue #205: heterogeneous ego graph used by the MCP
//! `query_entity` tool. The Entity-only walker in [`super::entity_subgraph`]
//! served the CLI's `query subgraph --entity` (Entity → Entity along
//! RELATES_TO only); the MCP tool description advertised a richer
//! shape that included Doc / Function / Endpoint reach via COVERS /
//! FUNCTION_BELONGS_TO / ENDPOINT_TOUCHES_ENTITY / ENTITY_CALLS /
//! RELATES_TO. This module is the implementation of that description.
//!
//! Output shape:
//!
//! ```json
//! {
//!   "centre": "pricing-rule",
//!   "depth": 2,
//!   "nodes": [
//!     { "kind": "entity", "id": "pricing-rule", "display": "...",
//!       "mention_count": 34, "is_god_node": false },
//!     { "kind": "doc",      "id": "...", "title": "...", "path": "..." },
//!     { "kind": "function", "symbol": "...", "file": "...", "line": 12 },
//!     { "kind": "endpoint", "id": "...", "kind_": "axum", "method": "GET",
//!       "path": "..." }
//!   ],
//!   "edges": [
//!     { "source": "pricing-rule", "target": "billing", "type": "relates_to" },
//!     { "source": "f-1",          "target": "pricing-rule", "type": "function_belongs_to" },
//!     ...
//!   ]
//! }
//! ```
//!
//! BFS over Entity nodes (Entity-Entity hops propagate the frontier);
//! Doc / Function / Endpoint neighbours are collected at each visited
//! entity but are sinks — we do not walk outward from them.

use serde::Serialize;

/// A heterogeneous node in the ego graph. The `kind` discriminator
/// tells agents how to read the variant-specific fields.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum EgoNode {
    Entity {
        id: String,
        display: String,
        mention_count: u64,
        is_god_node: bool,
    },
    Doc {
        id: String,
        title: String,
        path: String,
    },
    Function {
        symbol: String,
        file: String,
        line: u32,
        language: String,
    },
    Endpoint {
        id: String,
        #[serde(rename = "kind_")]
        endpoint_kind: String,
        method: String,
        path: String,
    },
}

/// One edge in the ego graph. `type_` is the kebab-cased rel-table
/// label (`relates_to`, `entity_calls`, `covers`,
/// `function_belongs_to`, `endpoint_touches_entity`).
#[derive(Debug, Clone, Serialize)]
pub struct EgoEdge {
    pub source: String,
    pub target: String,
    #[serde(rename = "type")]
    pub type_: String,
}

/// Full payload returned by [`query_entity_ego_graph`].
#[derive(Debug, Clone, Serialize)]
pub struct EntityEgoGraph {
    pub centre: String,
    pub depth: u32,
    pub nodes: Vec<EgoNode>,
    pub edges: Vec<EgoEdge>,
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "tests panic on broken invariants — that is the point of a test"
)]
mod tests {}
