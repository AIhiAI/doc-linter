//! Typed reads over the doc graph (docs/design/store-trait.md, step 3).
//! Callers hold a `&dyn GraphRead`; the SQLite impl is `store_sqlite::read`.

use std::collections::{HashMap, HashSet};
use std::path::Path;

use anyhow::Result;

use crate::config::LintConfig;
use crate::endpoint_extract::EndpointKind;
use crate::graph::EdgeKind;
use crate::ids::{DocId, EntityId, FunctionSymbol};
use crate::store::query::EntityEgoGraph;
use crate::store::{
    self, AtQueryResult, CoveringDocRow, DarkEndpointRow, DarkFunctionRow, DeadCodeRow, DocRow,
    EndpointRow, EntityGapRow, EntityRow, EntitySubgraph, FunctionContext, FunctionFull,
    FunctionMentionRow, FunctionRow, ImpactResult, LinkRow, PathStep, RankedEntityRow, SiblingRow,
    StoreRead,
};

pub trait GraphRead: StoreRead {
    // docs
    fn list_all_docs(&self) -> Result<Vec<DocRow>>;
    fn get_doc(&self, id: &DocId) -> Result<Option<DocRow>>;
    fn outbound(&self, id: &DocId) -> Result<Vec<LinkRow>>;
    fn inbound(&self, id: &DocId) -> Result<Vec<LinkRow>>;
    fn covers_inbound_for_entity_doc(&self, id: &DocId) -> Result<Vec<LinkRow>>;
    fn shortest_path(
        &self,
        from: &DocId,
        to: &DocId,
        edge_filter: Option<&HashSet<EdgeKind>>,
        max_hops: u32,
    ) -> Result<Vec<PathStep>>;

    // entities
    fn ranked_entities(&self) -> Result<Vec<RankedEntityRow>>;
    fn get_entity(&self, id: &EntityId) -> Result<Option<EntityRow>>;
    fn top_entities_in_crate(&self, crate_name: &str, limit: usize) -> Result<Vec<String>>;
    fn query_entity_subgraph(&self, entity_id: &str, depth: u32) -> Result<Option<EntitySubgraph>>;
    fn query_entity_ego_graph(&self, entity_id: &str, depth: u32)
        -> Result<Option<EntityEgoGraph>>;

    // endpoints
    fn list_endpoints(
        &self,
        kind_filter: Option<&str>,
        dark_only: bool,
    ) -> Result<Vec<EndpointRow>>;
    fn endpoint_reach_by_kind(&self) -> Result<Vec<(EndpointKind, u64, u64)>>;

    // functions
    fn functions_mentioning(&self, entity_id: &EntityId) -> Result<Vec<FunctionRow>>;
    fn function_context(&self, symbol: &FunctionSymbol) -> Result<Option<FunctionContext>>;
    fn search_functions_by_symbol_substring(
        &self,
        needle: &str,
        limit: usize,
    ) -> Result<Vec<FunctionFull>>;
    fn function_mentions(&self, symbol: &FunctionSymbol) -> Result<Vec<FunctionMentionRow>>;
    fn docs_covering_entities(&self, entity_ids: &[EntityId]) -> Result<Vec<CoveringDocRow>>;
    fn function_siblings(&self, target: &FunctionSymbol, limit: usize) -> Result<Vec<SiblingRow>>;

    // location / impact / dead code
    fn query_at(&self, file: &str, line: u32) -> Result<AtQueryResult>;
    fn query_impact(&self, symbol: &str, depth: u32) -> Result<Option<ImpactResult>>;
    fn list_dead_code(&self) -> Result<Vec<DeadCodeRow>>;

    // coverage
    fn list_dark_public_functions(
        &self,
        anchor_required_in: &[String],
        config: &LintConfig,
    ) -> Result<Vec<DarkFunctionRow>>;
    fn list_all_dark_public_functions(&self, config: &LintConfig) -> Result<Vec<DarkFunctionRow>>;
    fn list_dark_endpoints(&self) -> Result<Vec<DarkEndpointRow>>;
    fn list_entity_coverage_gaps(
        &self,
        threshold: f32,
        exempt: &[String],
    ) -> Result<Vec<EntityGapRow>>;
    fn global_function_reach(&self, config: &LintConfig) -> Result<(u64, u64, u64)>;
    fn list_dark_functions_in_crate(&self, crate_name: &str) -> Result<Vec<DarkFunctionRow>>;

    /// Saved query by name (built-in SQL, or a runtime TOML `sql`).
    fn run_saved_query(
        &self,
        root: Option<&Path>,
        name: &str,
        params: &HashMap<String, String>,
    ) -> Result<serde_json::Value>;

    /// Read-only raw SQL (`$name` parameters). Non-SELECT / non-WITH
    /// statements are refused.
    fn run_sql(&self, sql: &str, params: Vec<(&str, store::Value)>) -> Result<serde_json::Value>;

    /// Every node and rel table with its columns and row count
    /// (`query schema`, `query graph-summary`, MCP `query_schema`).
    fn schema_tables(&self) -> Result<Vec<TableInfo>>;

    /// Nearest embedded rows of one corpus by cosine similarity, from the
    /// vector index (empty when no embeddings pass has run).
    fn embedding_nearest(&self, req: &NearestRequest<'_>) -> Result<NearestResult>;
}

/// One column of a table, as `query schema` reports it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ColumnInfo {
    pub name: String,
    pub ty: String,
    pub pk: bool,
    pub default: String,
}

/// One node or rel table. `from` / `to` name the endpoint node tables of a
/// rel table (empty for node tables).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TableInfo {
    pub name: String,
    pub is_rel: bool,
    pub columns: Vec<ColumnInfo>,
    pub from: String,
    pub to: String,
    pub row_count: u64,
}

/// A similarity search over one corpus (`doc`, `function`, `entity`,
/// `section`, `type`). `source_repo` / tag filters apply to docs and, via
/// the parent doc, sections.
#[derive(Debug, Clone, Copy)]
pub struct NearestRequest<'a> {
    pub kind: &'a str,
    pub query: &'a [f32],
    pub top: usize,
    pub source_repo: Option<&'a str>,
    pub with_tag: Option<&'a str>,
    pub without_tag: Option<&'a str>,
}

/// Result of [`GraphRead::embedding_nearest`]: `(id, body, cosine)` best
/// first, and how many rows of the corpus carry a vector.
#[derive(Debug, Clone, Default)]
pub struct NearestResult {
    pub hits: Vec<(String, String, f64)>,
    pub corpus_size: usize,
}

/// Run `sql` on the open graph; the `{columns,row_count,rows}` shape. List
/// columns come back as JSON arrays (SQL builds them with `json_group_array`).
pub fn query_sql(
    db: &dyn GraphRead,
    sql: &str,
    params: Vec<(&str, store::Value)>,
) -> Result<serde_json::Value> {
    db.run_sql(sql, params)
}

/// SQL `WHERE` body (and `$name` bindings) for the `source_repo` /
/// `with_tag` / `without_tag` filters on `Doc` rows aliased `alias`. Empty
/// when no filter is set.
pub fn doc_filter_sql(
    alias: &str,
    source_repo: Option<&str>,
    with_tag: Option<&str>,
    without_tag: Option<&str>,
) -> (String, Vec<(&'static str, store::Value)>) {
    let mut clauses: Vec<String> = Vec::new();
    let mut params: Vec<(&'static str, store::Value)> = Vec::new();
    if let Some(id) = source_repo {
        clauses.push(format!("{alias}.repo_id = $repo_id"));
        params.push(("repo_id", store::Value::Str(id.to_string())));
    }
    for (tag, name, neg) in [
        (with_tag, "with_tag", ""),
        (without_tag, "without_tag", "NOT "),
    ] {
        if let Some(tag) = tag {
            clauses.push(format!(
                "{neg}EXISTS(SELECT 1 FROM doc_tags g WHERE g.doc_id = {alias}.id AND g.value = ${name})"
            ));
            params.push((name, store::Value::Str(tag.to_string())));
        }
    }
    (clauses.join(" AND "), params)
}

/// Rows of a `{columns,row_count,rows}` result (empty when absent).
pub fn rows_of(v: &serde_json::Value) -> Vec<serde_json::Value> {
    v.get("rows")
        .and_then(|r| r.as_array())
        .cloned()
        .unwrap_or_default()
}

/// A read-only handle on the SQLite graph of a root.
pub struct ReadGraph(crate::store_sqlite::SqliteDb);

impl ReadGraph {
    /// Open `<root>/.doc-lint/graph.sqlite` read-only.
    pub fn open(root: &Path) -> Result<Self> {
        crate::store_sqlite::open_ro(root).map(Self)
    }

    /// Identity of the graph file this handle was opened on; changes when
    /// a rebuild is swapped in.
    pub fn identity(&self, root: &Path) -> Option<(u64, u64)> {
        crate::store_sqlite::graph_identity(root)
    }
}

impl From<crate::store_sqlite::SqliteDb> for ReadGraph {
    fn from(db: crate::store_sqlite::SqliteDb) -> Self {
        Self(db)
    }
}

impl std::ops::Deref for ReadGraph {
    type Target = dyn GraphRead;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}
