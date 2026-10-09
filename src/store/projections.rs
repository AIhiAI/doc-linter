//! Row-shape projections and `Value` coercion helpers shared by
//! every read path in the [[entity-doc-graph]] linter.
//!
//! All of the `*Row` / `*Full` / `*Stats` structs that consumers
//! (scaffold, explain, query, coverage) see live here. The `val_*`
//! helpers, [`doc_row_from_columns`], [`value_to_json`],
//! [`node_prop_string`] and [`rel_property_u32`] are visible to the
//! sibling submodules but private to the rest of the crate.

use crate::endpoint_extract::EndpointKind;

/// Per-edge confidence tier on the [[entity-doc-graph]]
/// `FUNCTION_MENTIONS` rel. Two values today:
///
///   - [`Self::High`] — match found in the function's `///` doc-comment
///     text. Authored prose explicitly names the entity; strong signal.
///   - [`Self::Low`] — match found only in the SCIP symbol path (the
///     function's module / file / symbol name itself contains an
///     entity token). Useful as a coverage backstop but noisier.
///
/// Serializes / writes to the store as the bare lowercase token
/// (`"high"` / `"low"`) so the on-disk schema and JSON output stay
/// byte-identical to the pre-#51 stringly-typed form. Roadmap #40
/// may extend this to graphify's three-tier `EXTRACTED / INFERRED /
/// AMBIGUOUS` taxonomy — `#[non_exhaustive]` keeps that addition
/// non-breaking for downstream matches.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize)]
#[serde(rename_all = "lowercase")]
#[non_exhaustive]
pub enum ConfidenceTier {
    High,
    Low,
}

impl ConfidenceTier {
    /// Stable string form (`"high"` / `"low"`) used by the
    /// [[entity-doc-graph]] ingest when writing the `r.confidence`
    /// column on FUNCTION_MENTIONS rels.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::High => "high",
            Self::Low => "low",
        }
    }

    /// Parses the lowercase token read back from the store's
    /// `r.confidence` column. Returns `None` on an unknown value;
    /// callers at the row-projection boundary fall back to
    /// [`ConfidenceTier::High`] (the safe default — matches the
    /// pre-#51 behaviour where an empty/absent confidence defaulted
    /// to "high").
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "high" => Some(Self::High),
            "low" => Some(Self::Low),
            _ => None,
        }
    }
}

impl std::fmt::Display for ConfidenceTier {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

#[cfg(test)]
mod confidence_tier_proptests {
    use super::ConfidenceTier;
    use proptest::prelude::*;

    fn arb_confidence_tier() -> impl Strategy<Value = ConfidenceTier> {
        prop_oneof![Just(ConfidenceTier::High), Just(ConfidenceTier::Low),]
    }

    proptest! {
        /// `ConfidenceTier::parse(t.as_str()) == Some(t)` for every
        /// variant — round-trip invariant on the lowercase projection
        /// written into the edge `confidence` column.
        #[test]
        fn confidence_tier_as_str_parse_roundtrip(t in arb_confidence_tier()) {
            let s = t.as_str();
            prop_assert_eq!(ConfidenceTier::parse(s), Some(t));
        }
    }
}

// ---------------------------------------------------------------------
// Doc-side rows
// ---------------------------------------------------------------------

/// A doc-node row from the [[entity-doc-graph]] SQLite graph — mirror of the
/// legacy `NodeMeta` JSON shape so AI agents reading sitemap.json / query
/// output don't break.
#[derive(Debug, Clone, serde::Serialize)]
pub struct DocRow {
    pub id: String,
    pub path: String,
    pub role: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lifecycle: Option<String>,
    pub title: String,
    pub summary: String,
    pub status: String,
    pub updated: String,
    pub tags: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub covers: Vec<String>,
    /// Non-standard frontmatter as `key=value` strings.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub attributes: Vec<String>,
}

/// An edge row from the [[entity-doc-graph]] SQLite graph for query output.
/// `line == 0` is suppressed via a custom serializer so frontmatter-typed
/// edges (which never have a line number) match the legacy JSON shape.
#[derive(Debug, Clone, serde::Serialize)]
pub struct LinkRow {
    pub id: String,
    pub title: String,
    pub role: String,
    pub line: usize,
    pub edge_type: String,
}

/// One node in a [[entity-doc-graph]] shortest-path result. The `via_*`
/// fields describe the edge that delivered us *to* this node from the
/// previous one; the starting node has both set to `None`. Mirrors the
/// `PathHop` JSON shape produced by `query.rs::path`.
#[derive(Debug, Clone, serde::Serialize)]
pub struct PathStep {
    pub id: String,
    pub title: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub via_line: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub via_edge_type: Option<String>,
}

// ---------------------------------------------------------------------
// Entity-side rows
// ---------------------------------------------------------------------

/// One ranked entity row produced by `ranked_entities`. The
/// graphify-style god_node flag marks the top-K most-mentioned
/// entities (K = 3) so consumers can pivot on the same field name they
/// already use against `graphify-out/graph.json`.
///
/// Bug #150 follow-up: `entity_class` is the ontology-declared
/// classification (well-known values: `language`, `infrastructure`,
/// `domain`). Surfaced so an agent can tell at a glance which
/// entities are excluded from god-node ranking — a saturated
/// `python` row with `god_node=false` is correct when its
/// `entity_class=language` is visible, vs. surprising without
/// it. `None` (serialized as `null`) is the default for the bulk
/// of architectural entities.
#[derive(Debug, Clone, serde::Serialize)]
pub struct RankedEntityRow {
    pub rank: u32,
    pub id: String,
    pub display: String,
    pub mentions: u64,
    pub god_node: bool,
    pub scanner_coverage: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub entity_class: Option<String>,
}

/// Phase 4 of roadmap-43: a single [[entity-doc-graph]] Entity node's
/// display + description, fetched directly off the Entity table (the
/// ontology is the authoritative source — but the graph already has it
/// loaded). Returns `None` when the entity id isn't in the graph, e.g.
/// when the scaffolder fell back to a heuristic that points at an
/// unknown id.
#[derive(Debug, Clone)]
// `id` / `display` are returned to callers of `get_entity` (and JSON
// consumers via future helpers) even though the in-tree explain
// renderer only reads `description`. Struct-level allow keeps the
// shape stable across the pub query API.
#[allow(dead_code)]
pub struct EntityRow {
    pub id: String,
    pub display: String,
    pub description: String,
}

// ---------------------------------------------------------------------
// Function-side rows
// ---------------------------------------------------------------------

/// One row from `query::functions_mentioning` over the [[entity-doc-graph]]
/// code-graph — the JSON projection returned to AI agents. `crate` is
/// renamed via serde because `crate` is a Rust reserved word (field name
/// has to be `crate_name` in code but the JSON consumers see `crate`).
#[derive(Debug, Clone, serde::Serialize)]
pub struct FunctionRow {
    pub symbol: String,
    #[serde(rename = "crate")]
    pub crate_name: String,
    pub file: String,
    pub line: u32,
}

/// One Doc/Entity neighbour of a Function in the [[entity-doc-graph]] —
/// used by `function_context` to enumerate everything one Function node
/// connects to (its source-file Doc + every mentioned Entity).
#[derive(Debug, Clone, serde::Serialize)]
pub struct FunctionNeighbour {
    pub kind: String, // "doc" or "entity"
    pub id: String,
    pub title: String,
    pub edge: String, // "function-defined-in" / "function-mentions" / "method-of" / "uses-type"
}

/// Full [[entity-doc-graph]] Function node + every Doc/Entity it links to,
/// returned by `query function-context`.
#[derive(Debug, Clone, serde::Serialize)]
pub struct FunctionContext {
    pub function: FunctionFull,
    pub links: Vec<FunctionNeighbour>,
}

#[derive(Debug, Clone, serde::Serialize)]
/// Full function-row payload — symbol, crate, file, line, kind, doc
/// excerpt — returned by [[entity-doc-graph]] queries that surface a
/// single function in detail (e.g. `query function-context`).
pub struct FunctionFull {
    pub symbol: String,
    #[serde(rename = "crate")]
    pub crate_name: String,
    pub file: String,
    pub line: u32,
    pub doc_comment: String,
}

/// Phase 4 of roadmap-43: fetch the [[entity-doc-graph]] Entity ids a
/// single Function mentions, paired with their confidence ("high" /
/// "low") and display name. Same data that drives `function_context`,
/// but flat — the explain renderer doesn't need the doc-defined-in side.
#[derive(Debug, Clone)]
pub struct FunctionMentionRow {
    pub entity_id: String,
    pub display: String,
    pub confidence: ConfidenceTier,
}

/// Phase 4 of roadmap-43: every [[entity-doc-graph]] Doc that COVERS any
/// of the given entity ids. Used by the explain renderer to surface the
/// narrative docs (`how-to`, `explanation`, `reference`) that ground the
/// function in prose. Sorted by doc id; deduped across the input set.
#[derive(Debug, Clone)]
pub struct CoveringDocRow {
    pub id: String,
    pub title: String,
    pub kind: Option<String>,
}

/// Phase 4 of roadmap-43: sibling functions of `target_symbol` in the
/// [[entity-doc-graph]] code-graph — other Functions in the same crate that
/// share at least one FUNCTION_MENTIONS entity. Returned as
/// `(symbol, shared_entity_ids)` pairs, sorted by overlap descending then
/// symbol.
#[derive(Debug, Clone)]
pub struct SiblingRow {
    pub symbol: String,
    pub shared_entities: Vec<String>,
}

// ---------------------------------------------------------------------
// Coverage-diagnostic rows
// ---------------------------------------------------------------------

/// One dark-public-function row from the [[entity-doc-graph]] coverage
/// diagnostic — a Function with no `FUNCTION_MENTIONS` edge of any
/// confidence and an empty doc-comment, surfaced by
/// `list_dark_public_functions`.
#[derive(Debug, Clone)]
pub struct DarkFunctionRow {
    pub crate_name: String,
    pub symbol: String,
    pub file: String,
    pub line: u32,
}

/// One dark-endpoint row from the [[entity-doc-graph]] coverage diagnostic
/// — an Endpoint with no `ENDPOINT_TOUCHES_ENTITY` edge, surfaced by
/// `list_dark_endpoints`.
#[derive(Debug, Clone)]
pub struct DarkEndpointRow {
    pub kind: EndpointKind,
    pub method: String,
    pub path: String,
    pub file: String,
    pub line: u32,
}

/// One entity-coverage-gap row from the [[entity-doc-graph]] diagnostic —
/// reports per-entity narrative-doc count and reaching-function count, used
/// by `list_entity_coverage_gaps` to surface mega-entities and orphans.
#[derive(Debug, Clone)]
pub struct EntityGapRow {
    pub entity: String,
    pub doc_count: u64,
    pub func_count: u64,
    pub ratio: f32,
}

// ---------------------------------------------------------------------
// Endpoint rows
// ---------------------------------------------------------------------

/// Output row for `query endpoints` over the [[entity-doc-graph]] —
/// one Endpoint plus the entities it reaches (via `ENDPOINT_TOUCHES_ENTITY`).
/// Sorted ascending by entity id so re-queries produce deterministic JSON.
#[derive(Debug, Clone, serde::Serialize)]
pub struct EndpointRow {
    pub id: String,
    pub kind: EndpointKind,
    pub method: String,
    pub path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub handler_symbol: Option<String>,
    pub entities: Vec<String>,
    pub source_file: String,
    pub source_line: u32,
}

// ---------------------------------------------------------------------
// Ingest stat counters
// ---------------------------------------------------------------------

/// Counts of what `ingest_scip` produced. Returned to `cmd_check` so the
/// CLI can echo a one-liner to stderr.
///
/// `mentions_edges` is the total edge count across both confidence
/// tiers; `mentions_edges_high` and `mentions_edges_low` break it
/// down. Phase 1 of roadmap-43 adds the low tier (symbol-path
/// indexing) while leaving the high tier (doc-comment scan)
/// unchanged.
#[derive(Debug, Default, Clone, Copy)]
pub struct ScipIngestStats {
    pub functions: usize,
    /// Roadmap issue #32 (v0.3.0): Type node count — dual-written from
    /// struct/enum/trait/module/type_alias FunctionFacts into the new
    /// `Type` node table. Non-zero on every Rust/Python ingest; lets
    /// agents subset by kind without scanning Function.
    pub types: usize,
    pub defined_in_edges: usize,
    pub mentions_edges: usize,
    pub mentions_edges_high: usize,
    pub mentions_edges_low: usize,
    /// Phase 5c of roadmap-43: count of `FunctionFact`s skipped by
    /// the `coverage_codegen_exclude` glob set. These never reach the
    /// graph — no Function node, no FUNCTION_DEFINED_IN, no
    /// FUNCTION_MENTIONS — so the reach % denominator excludes
    /// codegen noise.
    pub codegen_excluded: usize,
    /// FUNCTION_BELONGS_TO edges produced by matching each Function's
    /// file path against the `source_modules:` globs declared on every
    /// entity. Zero when no entity declares globs (the default for
    /// repos that haven't opted in yet); rises as entity authors map
    /// their concepts to filesystem regions.
    pub belongs_to_edges: usize,
    /// Roadmap issue #9 (v0.3.0): CALLS edges created. The SCIP
    /// reference scan emits more (caller, callee) pairs than this —
    /// the edge insert drops edges whose callee isn't in the
    /// Function table (external symbols from another crate / a stdlib
    /// call), and only the created ones are counted.
    pub calls_edges: usize,
    /// Roadmap issue #10 (v0.3.0): ENTITY_CALLS rows produced by
    /// folding CALLS + FUNCTION_BELONGS_TO into Entity-level
    /// structural edges. Zero when no entity has opted into the
    /// `source_modules:` glob system or the SCIP index doesn't
    /// resolve cross-entity calls; rises as authors map their
    /// concepts to filesystem regions.
    pub entity_calls_edges: usize,
    /// Roadmap issue #11 (v0.3.0): number of Entity rows that had
    /// their `mention_count` + `is_god_node` columns refreshed
    /// during the post-FUNCTION_MENTIONS sweep. Equals the size of
    /// the Entity table on a successful run.
    pub entities_updated: usize,
    /// Roadmap issue #11 (v0.3.0): how many of those Entities ended
    /// up flagged `is_god_node = true` (mention_count above
    /// `mean + 2*stddev`).
    pub god_nodes: usize,
    /// Roadmap issue #33 v3: count of `ENDPOINT_TOUCHES_ENTITY`
    /// edges rebuilt by the cache-hit rederive path. Zero on the
    /// full-ingest path (those edges are counted on
    /// `EndpointIngestStats.touches_entity_edges` instead).
    pub touches_entity_rederived: usize,
    /// Roadmap issue #32 v4 (v0.4.0): `METHOD_OF` edges from
    /// `Function` to `Type`, emitted when a method's SCIP descriptor
    /// prefix (minus its leaf segment) matches a Type row's
    /// descriptor prefix. Falls out of the SCIP symbol shape — no
    /// `Relationship` walk required.
    pub method_of_edges: usize,
    /// Roadmap issue #32 v5 (v0.4.0): `USES_TYPE` edges created from
    /// `Function` to `Type`, one per (function, type) pair whose
    /// reference occurrences resolve to a SCIP type-suffix (`#`)
    /// symbol. Pairs whose target Type isn't in the graph
    /// (cross-crate / stdlib refs) are dropped and not counted.
    pub uses_type_edges: usize,
    /// Legacy gap 7: `Field` nodes inserted (deduped, file exclusions
    /// applied).
    pub fields: usize,
    /// Legacy gap 7: `REFERENCES` edges created (Function → Field);
    /// references to fields outside the graph are not counted.
    pub references_edges: usize,
    /// Roadmap issue #32 v6 (v0.4.0): `IMPLEMENTS` edges created
    /// from `Type` to `Type`, one per
    /// `SymbolInformation.relationships` entry with
    /// `is_implementation: true` whose (FROM, TO) kind pair is NOT
    /// (Trait, Trait). Cross-crate trait impls (where the trait lives
    /// in another crate's SCIP index) drop because the TO-side Type
    /// isn't in the graph, and are not counted.
    pub implements_edges: usize,
    /// Roadmap issue #32 v8 (v0.4.0): `EXTENDS` edges created from
    /// `Type` to `Type` — the (Trait, Trait) subset of the same
    /// `is_implementation` walk, split out into its own rel table
    /// so consumers can ask "trait X inherits from?" without also
    /// matching every `impl Trait for Struct`.
    pub extends_edges: usize,
    /// Roadmap issue #32 v9 (v0.4.0): `TYPE_DEFINED_IN` Type → Doc
    /// edge count — symmetric to `defined_in_edges` but for the
    /// type-side rows that no longer dual-write into Function.
    pub type_defined_in_edges: usize,
    /// Roadmap issue #32 v9 (v0.4.0): `TYPE_MENTIONS` Type → Entity
    /// edge count (all confidence tiers).
    pub type_mentions_edges: usize,
    /// Roadmap issue #32 v9: high-confidence subset of
    /// `type_mentions_edges` (doc-comment hit).
    pub type_mentions_edges_high: usize,
    /// Roadmap issue #32 v9: low-confidence subset of
    /// `type_mentions_edges` (symbol-path hit, type propagation,
    /// or self-entity demote).
    pub type_mentions_edges_low: usize,
    /// Roadmap issue #32 v9 (v0.4.0): `TYPE_BELONGS_TO` Type →
    /// Entity edge count — symmetric to `belongs_to_edges`,
    /// emitted when a type's source file matches an entity's
    /// declared `source_modules:` globs.
    pub type_belongs_to_edges: usize,
}

/// Counts of what `ingest_endpoints` produced. Returned to `cmd_check`
/// for the stderr one-liner.
#[derive(Debug, Default, Clone, Copy)]
pub struct EndpointIngestStats {
    pub axum: usize,
    pub clap: usize,
    pub mcp: usize,
    /// Roadmap issue #19 (v0.3.0): FastAPI endpoint count.
    /// Zero on non-Python repos / when no `@app.<method>` or
    /// `@router.<method>` decorators were found.
    pub fastapi: usize,
    /// Roadmap issue #19 (v0.3.0): Flask endpoint count.
    /// One per (method, path) pair — `@app.route("/x",
    /// methods=["GET", "POST"])` emits two facts → two counter
    /// bumps.
    pub flask: usize,
    /// Roadmap issue #19 (v0.3.0): Express endpoint count.
    /// One per (method, path) pair on `.js` / `.ts` files.
    pub express: usize,
    /// JAX-RS endpoint count — one per (method, path) resource method.
    pub jaxrs: usize,
    pub handled_by_edges: usize,
    pub touches_entity_edges: usize,
}
