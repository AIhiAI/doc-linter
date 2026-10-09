//! Roadmap issue #25 (v0.3.0): `query at <file>:<line>` — inverse
//! lookup from a source location to the surrounding graph context.
//!
//! What it returns:
//!   - `function` — the Function whose body covers `<line>` in
//!     `<file>`. Resolved by the start-line heuristic: the Function
//!     with `f.file = <file>` and the largest `f.line <= <line>`.
//!     Approximate by design — Function rows only store the start
//!     line (#22 promotes the body span to a stored column, at
//!     which point this helper can pick the smallest containing
//!     span instead).
//!   - `file` — the File node for `<file>`.
//!   - `module` — the File's enclosing Module (rust_crate or
//!     python_package), if any.
//!   - `entities` — every Entity the function `FUNCTION_BELONGS_TO`
//!     or `FUNCTION_MENTIONS`.
//!   - `covering_docs` — every Doc whose `covers:` includes one of
//!     those entities.
//!   - `nearby_endpoints` — every Endpoint whose handler is this
//!     function.
//!
//! Empty fields when nothing matches — never an error. An agent
//! calling `query at backend/server.py:142` on a freshly-cloned
//! repo (no SCIP indexed yet) gets `{"function": null, "file":
//! null, ...}` rather than a non-zero exit.

use crate::store::FunctionFull;

/// One File row's worth of payload — `path`, `language`, `loc`,
/// `last_touched`. Mirrors the File table from roadmap #17.
#[derive(Debug, Clone, serde::Serialize)]
pub struct FileRow {
    pub path: String,
    pub language: String,
    pub loc: u32,
    pub last_touched: String,
}

/// One Module row's worth of payload — `id`, `kind`, `path`, `name`.
/// Mirrors the Module table from roadmap #17.
#[derive(Debug, Clone, serde::Serialize)]
pub struct ModuleRow {
    pub id: String,
    pub kind: String,
    pub path: String,
    pub name: String,
}

/// One entity link in the `at` output — id + display + which edge
/// surfaced it (`function-belongs-to` or `function-mentions`).
#[derive(Debug, Clone, serde::Serialize)]
pub struct AtEntityLink {
    pub id: String,
    pub display: String,
    pub edge: String,
}

/// One covering-doc entry — id + title. Returned in id order so
/// re-queries are deterministic.
#[derive(Debug, Clone, serde::Serialize)]
pub struct AtCoveringDoc {
    pub id: String,
    pub title: String,
}

/// One endpoint handled by the function — id + kind + path. Lighter
/// than the full `EndpointRow` because the `at` query is about
/// pointing-to-context, not fanning out into each endpoint's
/// entities (callers can re-query `query endpoints` if they need
/// that).
#[derive(Debug, Clone, serde::Serialize)]
pub struct AtEndpoint {
    pub id: String,
    pub kind: String,
    pub method: String,
    pub path: String,
}

/// Full payload returned by `query_at`. Every field is optional /
/// possibly-empty so the JSON shape is stable when nothing matches.
#[derive(Debug, Clone, serde::Serialize)]
pub struct AtQueryResult {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub function: Option<FunctionFull>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file: Option<FileRow>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub module: Option<ModuleRow>,
    pub entities: Vec<AtEntityLink>,
    pub covering_docs: Vec<AtCoveringDoc>,
    pub nearby_endpoints: Vec<AtEndpoint>,
}
