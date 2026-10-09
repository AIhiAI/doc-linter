//! Edge-kind registry for the [[entity-doc-graph]] linter: the mapping
//! between `crate::graph::EdgeKind` and the edge-table names, and the
//! [`ResetMode`] the SQLite ingest resets with. The table DDL itself lives
//! in `store_sqlite::schema`.
//!
//! The mapping helpers consult a single [`EDGE_SCHEMA`] registry, and the
//! [`EdgeType`] trait exposes the same mapping to foreign types, so adding
//! a new edge variant is a two-file change: the new `EdgeKind` variant in
//! `graph.rs`, plus a new entry here.

use crate::graph::EdgeKind;

/// Generic mapping between a typed edge variant and its edge-table
/// representation. Implemented on `crate::graph::EdgeKind` via the
/// [`EDGE_SCHEMA`] registry; foreign types (e.g. future `CALLS` /
/// `IMPORTS` enums) can implement it to participate in the same
/// dispatch.
pub trait EdgeType: Sized {
    /// edge-table name, e.g. `"WIKILINK"`.
    fn label(&self) -> &'static str;
    /// Reverse of [`label`] — parses a edge-table name back to the
    /// edge variant, or returns `None` for unknown tables.
    fn from_label(label: &str) -> Option<Self>;
}

/// Static description of one edge variant: enum tag, edge-table
/// name, the `CREATE REL TABLE` DDL string, and whether the edge
/// participates in the Doc→Doc traversal restriction used by
/// `shortest_path`.
struct EdgeSchemaSpec {
    kind: EdgeKind,
    label: &'static str,
}

/// Single source of truth for the `EdgeKind` ↔ edge-table mapping.
/// Adding a new edge variant to [`EdgeKind`] requires adding one row
/// here — no other file in `store/` needs to change.
const EDGE_SCHEMA: &[EdgeSchemaSpec] = &[
    EdgeSchemaSpec {
        kind: EdgeKind::Wikilink,
        label: "WIKILINK",
    },
    EdgeSchemaSpec {
        kind: EdgeKind::MdLink,
        label: "MD_LINK",
    },
    EdgeSchemaSpec {
        kind: EdgeKind::DependsOn,
        label: "DEPENDS_ON",
    },
    EdgeSchemaSpec {
        kind: EdgeKind::InformedBy,
        label: "INFORMED_BY",
    },
    EdgeSchemaSpec {
        kind: EdgeKind::Supersedes,
        label: "SUPERSEDES",
    },
    EdgeSchemaSpec {
        kind: EdgeKind::CrateRef,
        label: "CRATE_REF",
    },
    EdgeSchemaSpec {
        kind: EdgeKind::Covers,
        label: "COVERS",
    },
];

impl EdgeType for EdgeKind {
    fn label(&self) -> &'static str {
        // Linear scan is fine — EDGE_SCHEMA has a handful of entries
        // and lookups are not on the hot path.
        for spec in EDGE_SCHEMA {
            if spec.kind == *self {
                return spec.label;
            }
        }
        // `#[non_exhaustive]` on EdgeKind means we can't statically
        // prove exhaustive coverage; an empty string is the right
        // sentinel for the (impossible) miss because the only consumer
        // (`edge_table`) immediately interpolates the result into a
        // SQL string, which would fail loudly downstream.
        ""
    }

    fn from_label(label: &str) -> Option<EdgeKind> {
        for spec in EDGE_SCHEMA {
            if spec.label == label {
                return Some(spec.kind);
            }
        }
        None
    }
}

/// Maps a edge-table label back to the kebab-case
/// `EdgeKind::as_str()` form so [[entity-doc-graph]] query output keeps
/// the legacy edge-type labels unchanged across the wikilink, md-link,
/// depends-on, informed-by, supersedes, crate-ref, and covers edge
/// types.
pub fn edge_kind_from_table(table: &str) -> Option<EdgeKind> {
    EdgeKind::from_label(table)
}

/// Convert a edge-table label (`WIKILINK`, `MD_LINK`, ...) to the
/// kebab-case wire format the legacy [[entity-doc-graph]] JSON output
/// exposes.
pub fn edge_label_to_kebab(label: &str) -> String {
    edge_kind_from_table(label).map_or_else(
        || label.to_lowercase().replace('_', "-"),
        |k| k.as_str().to_string(),
    )
}

/// How much of the graph an ingest resets before rebuilding.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResetMode {
    /// Drop and recreate every table — vault, cross-bucket, and
    /// code-graph. The shipping default; every `cmd_check` uses this
    /// today because the SCIP ingest pipeline always re-runs.
    Full,
    /// Drop and recreate only vault tables (`Doc`, `Entity`,
    /// `RELATES_TO`, Doc-Doc edges) **and** the cross-bucket edge
    /// tables that point at vault nodes (`FUNCTION_DEFINED_IN`,
    /// `FUNCTION_MENTIONS`, `FUNCTION_BELONGS_TO`, `ENTITY_CALLS`,
    /// `ENDPOINT_TOUCHES_ENTITY`, `DESCRIBED_BY`).
    ///
    /// Code-graph tables (`Function`, `Type`, `File`, `Module`,
    /// `Endpoint`, `Finding`, `Migration`, `RepoMeta`, `CALLS`,
    /// `TEST_FOR`, `IMPORTS`, `IMPORTS_MODULE`, `IN_MODULE`,
    /// `DEFINED_IN_FILE`, `COUPLED_WITH`, `HAS_FINDING`,
    /// `ENDPOINT_HANDLED_BY`) are preserved across the call.
    ///
    /// Used by the cache-hit fast path for roadmap issue #33: when
    /// the SCIP file is unchanged the per-Function ingest can be
    /// skipped, but the vault is always rebuilt fresh from
    /// markdown — cross-bucket edges have to be redrawn against
    /// the freshly-recreated `Doc`/`Entity` rows, which is what
    /// the caller does after this returns.
    VaultAndCrossBucket,
}
