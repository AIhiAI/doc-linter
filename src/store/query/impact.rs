//! Roadmap issue #36 (v0.3.0): `query impact <symbol>` — transitive
//! blast-radius view. Walks CALLS edges backwards from a target
//! Function to enumerate every caller / transitive caller, grouped
//! by depth. Also surfaces touched entities (anything the chain
//! belongs-to or mentions) and touched endpoints (any caller that's
//! an Endpoint handler).
//!
//! Scope (v1 — matches the issue's primary acceptance criterion):
//!   - `--kind function` only. Entity / endpoint starting points
//!     are a follow-up; the issue lists them as optional knobs.
//!   - Bounded-context "crossing" warnings are deferred. They need
//!     a cleanly-resolved bounded-context column on Function, which
//!     today is only populated for Doc rows. Tracked separately so
//!     this PR stays a clean BFS.
//!   - TEST_FOR edges (#24) aren't implemented yet, so the
//!     `tests:` field stays an empty array — the JSON shape is in
//!     place for when #24 lands.
//!
//! ## Algorithm
//!
//! Pure Rust BFS over CALLS, one SQL query per depth level.
//! `*1..N` path-projection in a single query would be faster but
//! complicates depth tagging — and the per-level pattern lines up
//! with the issue's "grouped by depth" output shape, so iterative
//! BFS is what the JSON wants anyway.

use crate::store::FunctionFull;

/// One depth level in the impact graph — the callers `n` hops away
/// from the target.
#[derive(Debug, Clone, serde::Serialize)]
pub struct ImpactDepthLevel {
    pub depth: u32,
    pub callers: Vec<FunctionFull>,
}

/// One touched entity surfaced by walking the impact subgraph's
/// FUNCTION_BELONGS_TO / FUNCTION_MENTIONS edges. `god_node` reads
/// the persisted Entity column from roadmap #11 so the severity
/// classifier doesn't have to re-query.
#[derive(Debug, Clone, serde::Serialize)]
pub struct ImpactEntity {
    pub id: String,
    pub display: String,
    pub god_node: bool,
}

/// One touched endpoint — Endpoint nodes whose handler appears
/// anywhere in the impact subgraph (target + callers).
#[derive(Debug, Clone, serde::Serialize)]
pub struct ImpactEndpoint {
    pub id: String,
    pub kind: String,
    pub method: String,
    pub path: String,
}

/// Full payload returned by `query_impact`. Empty arrays are
/// preserved (`tests` always today; `entities` / `endpoints` /
/// `depths` when the target has no callers) so consumers can
/// branch on the shape without optional-chaining.
#[derive(Debug, Clone, serde::Serialize)]
pub struct ImpactResult {
    pub target: FunctionFull,
    pub depths: Vec<ImpactDepthLevel>,
    pub entities: Vec<ImpactEntity>,
    pub endpoints: Vec<ImpactEndpoint>,
    /// Roadmap #24 (TEST_FOR edges) — schema not yet in place, so
    /// always `[]` today. Field exists so the JSON shape is stable
    /// across the #24 follow-up.
    pub tests: Vec<serde_json::Value>,
}
