//! Shared model of the persistent doc graph: neutral value and store
//! traits, row projections, the corpus-walk planners that the SQLite ingest
//! (`store_sqlite::ingest`) turns into rows, and the saved-query catalog.
//! The engine itself is `store_sqlite`; this module holds everything that
//! does not care which SQL database sits underneath.
//!
//! The graph lives at `<root>/.doc-lint/graph.sqlite`; Round 2A's Vale tree
//! lives at `<root>/.doc-lint/vale/` beside it (the shared `.doc-lint/` dir
//! is in `.gitignore`).
//!
//! ## Module layout
//!
//! - [`traits`] — the neutral [`Value`] and the [`StoreRead`] / [`Store`]
//!   traits every typed reader and writer is written against.
//! - [`typed`] — SQL reads and writes for callers outside the ingest code
//!   (coverage report, cluster discovery, contradictions, SCIP cache hit).
//! - [`schema`] — the `EDGE_SCHEMA` registry mapping `EdgeKind` to edge-table
//!   names, the [`EdgeType`] trait, and [`ResetMode`].
//! - [`projections`] — `*Row` / `*Full` / `*Stats` structs every consumer
//!   sees.
//! - [`ingest`], [`code_ingest`], [`endpoint_ingest`], [`file_module_ingest`],
//!   [`finding_ingest`], [`coupling_ingest`], [`imports_ingest`],
//!   [`migration_ingest`], [`repo_ingest`], [`test_for_ingest`],
//!   [`unstubbed_concepts_ingest`] — pure row planning (corpus walk →
//!   `IngestPlan`, SCIP facts → deduped functions and mention pairs, git
//!   coupling, TODO scanner, ...), shared by every writer.
//! - [`symbols`] — symbol tokenization and SCIP descriptor-path helpers.
//! - [`query`] — result types of the read-side helpers and the saved-query
//!   catalog ([`query::saved`]).
//!
//! ## Lifecycle
//!
//! Every `check` builds a fresh graph beside the live one
//! (`store_sqlite::build_and_swap`): docs, code, endpoints, files, coupling,
//! findings and (optionally) embeddings are written into a staging copy
//! which is then renamed over `graph.sqlite`. The graph is fully derivable
//! from the corpus, so there is no migration story. Readers (`query`, `mcp`)
//! open it read-only and reopen when the file identity changes.

pub mod code_ingest;
pub mod coupling_ingest;
pub mod endpoint_ingest;
pub mod file_module_ingest;
pub mod finding_ingest;
pub mod imports_ingest;
pub mod ingest;
pub mod migration_ingest;
pub mod populate_embeddings;
pub mod projections;
pub mod query;
pub mod repo_ingest;
pub mod schema;
pub mod symbols;
pub mod test_for_ingest;
pub mod traits;
pub mod typed;
pub mod unstubbed_concepts_ingest;

// --- Re-exports for the public surface ------------------------------
//
// Consumers (scaffold, explain, query CLI, coverage, endpoint marker
// migrate) import from `crate::store::*`; keeping the surface flat
// means the directory-split doesn't ripple through every call site.

pub use symbols::{scan_doc_comment_for_entities, scan_symbol_for_entities, symbol_tokens};
// `strip_scip_package_prefix` is `pub(crate)` because `scaffold.rs`
// reaches into it for the SCIP-symbol → bare-fn-name conversion.
pub(crate) use symbols::strip_scip_package_prefix;

pub use coupling_ingest::CouplingIngestStats;
pub use endpoint_ingest::{FunctionIndex, FunctionIndexEntry};
pub use file_module_ingest::FileModuleIngestStats;
pub use finding_ingest::FindingIngestStats;
pub use imports_ingest::ImportsIngestStats;
pub use migration_ingest::MigrationIngestStats;
pub use populate_embeddings::EmbeddingStats;
pub use repo_ingest::IngestedRepo;
pub use schema::ResetMode;
pub use test_for_ingest::TestForIngestStats;
pub use unstubbed_concepts_ingest::UnstubbedIngestStats;

pub use projections::{
    ConfidenceTier, CoveringDocRow, DarkEndpointRow, DarkFunctionRow, DocRow, EndpointIngestStats,
    EndpointRow, EntityGapRow, EntityRow, FunctionContext, FunctionFull, FunctionMentionRow,
    FunctionNeighbour, FunctionRow, LinkRow, PathStep, RankedEntityRow, ScipIngestStats,
    SiblingRow,
};

pub use query::{
    list_saved_queries, render_entity_mermaid, AtCoveringDoc, AtEndpoint, AtEntityLink,
    AtQueryResult, DeadCodeRow, EntitySubgraph, EntitySubgraphEdge, EntitySubgraphNode,
    ImpactDepthLevel, ImpactEndpoint, ImpactEntity, ImpactResult, SavedQuery, SavedQueryListing,
};

pub use schema::{edge_kind_from_table, edge_label_to_kebab, EdgeType};

// --- Storage seam (docs/design/store-trait.md) --------------------------
//
// Neutral `Value` and the store traits; see `traits.rs`.

pub use traits::{Params, Row, Store, StoreRead, Value};
