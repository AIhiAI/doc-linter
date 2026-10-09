//! Typed reads / writes for callers outside the ingest code (coverage
//! report, cluster discovery, contradictions check, SCIP cache-hit path).
//! Each function owns its SQL and returns plain Rust data. Written against
//! the neutral [`StoreRead`] / [`Store`] traits.

use std::collections::BTreeMap;

use anyhow::{Context, Result};

use super::traits::{Store, StoreRead, Value};

const NOT_GENERATED: &str = "coalesce(f.generated, 0) = 0";
const HAS_MENTION: &str = "EXISTS (SELECT 1 FROM \"FUNCTION_MENTIONS\" m JOIN Entity e ON e.id = m.dst WHERE m.src = f.symbol)";

fn scalar(conn: &(impl StoreRead + ?Sized), sql: &str, what: &str) -> Result<u64> {
    let rows = conn.query(sql, &[]).context(what.to_string())?;
    Ok(rows
        .first()
        .and_then(|r| r.first())
        .map_or(0, Value::as_u64_or_zero))
}

/// `(key, count)` pairs from a two-column query; empty keys dropped.
fn keyed_counts(
    conn: &(impl StoreRead + ?Sized),
    sql: &str,
    what: &str,
) -> Result<Vec<(String, u64)>> {
    Ok(conn
        .query(sql, &[])
        .context(what.to_string())?
        .iter()
        .map(|r| (r[0].str_or_empty(), r[1].as_u64_or_zero()))
        .collect())
}

// --- coverage -----------------------------------------------------------

/// Hand-written (non-generated) Function rows.
pub fn count_functions(conn: &(impl StoreRead + ?Sized)) -> Result<u64> {
    scalar(
        conn,
        &format!("SELECT count(*) FROM Function f WHERE {NOT_GENERATED}"),
        "count Function",
    )
}

pub fn count_functions_reaching_entity(conn: &(impl StoreRead + ?Sized)) -> Result<u64> {
    scalar(
        conn,
        &format!("SELECT count(*) FROM Function f WHERE {NOT_GENERATED} AND {HAS_MENTION}"),
        "count Function with FUNCTION_MENTIONS",
    )
}

pub fn count_functions_documented(conn: &(impl StoreRead + ?Sized)) -> Result<u64> {
    scalar(
        conn,
        &format!("SELECT count(*) FROM Function f WHERE {NOT_GENERATED} AND f.doc_comment <> ''"),
        "count Function with doc_comment",
    )
}

/// Every Entity id mapped to the number of Docs covering it (0 if none).
pub fn entity_doc_counts(conn: &(impl StoreRead + ?Sized)) -> Result<BTreeMap<String, u64>> {
    let mut out = BTreeMap::new();
    for r in conn
        .query("SELECT id FROM Entity", &[])
        .context("list entities")?
    {
        let id = r[0].str_or_empty();
        if !id.is_empty() {
            out.entry(id).or_insert(0);
        }
    }
    let counted = keyed_counts(
        conn,
        "SELECT e.id, count(DISTINCT d.id) FROM \"COVERS\" c JOIN Doc d ON d.id = c.src \
             JOIN Entity e ON e.id = c.dst GROUP BY e.id",
        "entity doc counts",
    )?;
    out.extend(counted.into_iter().filter(|(id, _)| !id.is_empty()));
    Ok(out)
}

pub fn entity_func_counts(conn: &(impl StoreRead + ?Sized)) -> Result<BTreeMap<String, u64>> {
    let counted = keyed_counts(
        conn,
        &format!(
            "SELECT e.id, count(DISTINCT f.symbol) FROM \"FUNCTION_MENTIONS\" m \
                 JOIN Function f ON f.symbol = m.src JOIN Entity e ON e.id = m.dst \
                 WHERE {NOT_GENERATED} GROUP BY e.id"
        ),
        "entity func counts",
    )?;
    Ok(counted
        .into_iter()
        .filter(|(id, _)| !id.is_empty())
        .collect())
}

pub fn crate_function_totals(conn: &(impl StoreRead + ?Sized)) -> Result<BTreeMap<String, u64>> {
    let counted = keyed_counts(
        conn,
        &format!("SELECT f.crate, count(*) FROM Function f WHERE {NOT_GENERATED} GROUP BY f.crate"),
        "crate function totals",
    )?;
    Ok(counted.into_iter().collect())
}

pub fn crate_function_reach(conn: &(impl StoreRead + ?Sized)) -> Result<BTreeMap<String, u64>> {
    let counted = keyed_counts(
        conn,
        &format!(
            "SELECT f.crate, count(*) FROM Function f \
                 WHERE {NOT_GENERATED} AND {HAS_MENTION} GROUP BY f.crate"
        ),
        "crate function reach",
    )?;
    Ok(counted.into_iter().collect())
}

/// `(crate, symbol)` of every hand-written Function with no
/// FUNCTION_MENTIONS edge.
pub fn dark_function_symbols(conn: &(impl StoreRead + ?Sized)) -> Result<Vec<(String, String)>> {
    Ok(conn
        .query(
            &format!(
                "SELECT f.crate, f.symbol FROM Function f \
                     WHERE {NOT_GENERATED} AND NOT {HAS_MENTION}"
            ),
            &[],
        )
        .context("scan dark Functions for exempt classification")?
        .iter()
        .map(|r| (r[0].str_or_empty(), r[1].str_or_empty()))
        .collect())
}

// --- cluster discovery --------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NodeTable {
    Function,
    Doc,
    Entity,
    Type,
    Module,
}

/// `(id, file)` for every node of `table`. A table that fails to query
/// (fresh `init` repo, ingest not run yet) yields nothing.
pub fn cluster_nodes(conn: &(impl StoreRead + ?Sized), table: NodeTable) -> Vec<(String, String)> {
    let sql = match table {
        NodeTable::Function => "SELECT symbol, file FROM Function",
        NodeTable::Doc => "SELECT id, path FROM Doc",
        NodeTable::Entity => "SELECT id, '' FROM Entity",
        NodeTable::Type => "SELECT symbol, file FROM Type",
        NodeTable::Module => "SELECT id, path FROM Module",
    };
    let Ok(rows) = conn.query(sql, &[]) else {
        return Vec::new();
    };
    rows.iter()
        .filter_map(|r| Some((r[0].as_str()?.to_string(), r[1].str_or_empty())))
        .collect()
}

/// Edge tables that connect clustered nodes, as `(from, to, rel table)`.
/// Skipped: `CRATE_REF` (inter-repo dep noise) and
/// `ENDPOINT_TOUCHES_ENTITY` (Endpoint nodes are not clustered).
const CLUSTER_EDGES: &[(NodeTable, NodeTable, &str)] = {
    use NodeTable::{Doc, Entity, Function, Module, Type};
    &[
        (Function, Function, "CALLS"),
        (Function, Doc, "FUNCTION_DEFINED_IN"),
        (Function, Entity, "FUNCTION_BELONGS_TO"),
        (Function, Entity, "FUNCTION_MENTIONS"),
        (Type, Doc, "TYPE_DEFINED_IN"),
        (Type, Entity, "TYPE_BELONGS_TO"),
        (Type, Entity, "TYPE_MENTIONS"),
        (Entity, Entity, "ENTITY_CALLS"),
        (Doc, Entity, "COVERS"),
        (Doc, Doc, "WIKILINK"),
        (Doc, Doc, "MD_LINK"),
        (Doc, Doc, "DEPENDS_ON"),
        (Doc, Doc, "INFORMED_BY"),
        (Doc, Doc, "SUPERSEDES"),
        (Entity, Entity, "RELATES_TO"),
        (Module, Doc, "DESCRIBED_BY"),
    ]
};

/// `(from_table, to_table, from_id, to_id)` for every clustered edge with
/// both ids non-empty. Edge tables that fail to query are skipped.
pub fn cluster_edges(
    conn: &(impl StoreRead + ?Sized),
) -> Vec<(NodeTable, NodeTable, String, String)> {
    let mut out = Vec::new();
    for (from, to, rel) in CLUSTER_EDGES {
        let Ok(rows) = conn.query(&format!("SELECT src, dst FROM \"{rel}\""), &[]) else {
            continue;
        };
        for r in &rows {
            if let (Some(a), Some(b)) = (r[0].as_str(), r[1].as_str()) {
                if !a.is_empty() && !b.is_empty() {
                    out.push((*from, *to, a.to_string(), b.to_string()));
                }
            }
        }
    }
    out
}

// --- concept proposer ---------------------------------------------------

/// One Function or Type row with the fields the concept proposer cites.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConceptSymbol {
    pub symbol: String,
    pub file: String,
    pub line: u64,
    pub signature: String,
    pub doc_comment: String,
    pub is_type: bool,
}

/// Hand-written Function and Type rows, sorted by symbol. A table that
/// fails to query (ingest not run) yields nothing.
pub fn concept_symbols(conn: &(impl StoreRead + ?Sized)) -> Vec<ConceptSymbol> {
    let mut out = Vec::new();
    for (is_type, sql) in [
        (
            false,
            "SELECT symbol, file, line, signature, doc_comment FROM Function f \
             WHERE coalesce(f.generated, 0) = 0",
        ),
        (
            true,
            "SELECT symbol, file, line, signature, doc_comment FROM Type",
        ),
    ] {
        let Ok(rows) = conn.query(sql, &[]) else {
            continue;
        };
        for r in &rows {
            let Some(symbol) = r[0].as_str().filter(|s| !s.is_empty()) else {
                continue;
            };
            out.push(ConceptSymbol {
                symbol: symbol.to_string(),
                file: r[1].str_or_empty(),
                line: r[2].as_u64_or_zero(),
                signature: r[3].str_or_empty(),
                doc_comment: r[4].str_or_empty(),
                is_type,
            });
        }
    }
    out.sort_by(|a, b| a.symbol.cmp(&b.symbol));
    out
}

/// `(caller, callee)` for every CALLS edge between Functions.
pub fn concept_calls(conn: &(impl StoreRead + ?Sized)) -> Vec<(String, String)> {
    cluster_edges(conn)
        .into_iter()
        .filter(|(f, t, _, _)| *f == NodeTable::Function && *t == NodeTable::Function)
        .map(|(_, _, a, b)| (a, b))
        .collect()
}

/// Stored doc-comment vectors as `(symbol, vector)`. The SQLite store holds
/// them in its vector index, not in a readable column, so this is empty:
/// `ontology propose --embeddings` reports "no readable stored vectors".
pub fn concept_embeddings(_conn: &(impl StoreRead + ?Sized)) -> Vec<(String, Vec<f32>)> {
    Vec::new()
}

// --- contradictions check -----------------------------------------------

/// `(id, summary)` of every Doc tagged `design`, blanks dropped.
pub fn design_doc_summaries(conn: &(impl StoreRead + ?Sized)) -> Result<Vec<(String, String)>> {
    Ok(conn
        .query(
            "SELECT d.id, d.summary FROM Doc d WHERE EXISTS \
                 (SELECT 1 FROM doc_tags t WHERE t.doc_id = d.id AND t.value = 'design')",
            &[],
        )
        .context("scan design-tagged Docs for contradictions check")?
        .iter()
        .map(|r| (r[0].str_or_empty(), r[1].str_or_empty()))
        .filter(|(id, summary)| !id.is_empty() && !summary.is_empty())
        .collect())
}

pub fn insert_contradiction_finding(
    conn: &(impl Store + ?Sized),
    id: &str,
    file: &str,
    message: &str,
) -> Result<()> {
    conn.exec(
        "INSERT INTO Finding(id, kind, file, line, message, severity) \
             VALUES ($id, 'contradiction', $file, 0, $message, 'warning')",
        &[
            ("id", Value::Str(id.to_string())),
            ("file", Value::Str(file.to_string())),
            ("message", Value::Str(message.to_string())),
        ],
    )
}

// --- SCIP cache-hit path ------------------------------------------------

/// `(importer_path, imported_path, import_count)` for every IMPORTS edge.
pub fn file_import_counts(conn: &(impl StoreRead + ?Sized)) -> Result<Vec<(String, String, i64)>> {
    Ok(conn
        .query("SELECT src, dst, import_count FROM \"IMPORTS\"", &[])
        .context("read IMPORTS")?
        .iter()
        .filter_map(|r| {
            Some((
                r[0].as_str()?.to_string(),
                r[1].as_str()?.to_string(),
                r[2].as_i64()?,
            ))
        })
        .collect())
}

/// Delete the File / Module / Finding rows so the source-derived passes
/// rebuild them from the current tree. SQL has no DETACH: drop the rows,
/// then every edge that touched them.
pub fn clear_source_derived_rows(conn: &(impl Store + ?Sized)) -> Result<()> {
    const SQL: &[&str] = &[
        "DELETE FROM \"HAS_FINDING\"",
        "DELETE FROM Finding",
        "DELETE FROM \"IN_MODULE\"",
        "DELETE FROM \"IMPORTS_MODULE\"",
        "DELETE FROM \"DESCRIBED_BY\"",
        "DELETE FROM Module",
        "DELETE FROM \"DEFINED_IN_FILE\"",
        "DELETE FROM \"COUPLED_WITH\"",
        "DELETE FROM \"IMPORTS\"",
        "DELETE FROM File",
    ];
    for q in SQL {
        conn.exec(q, &[]).with_context(|| (*q).to_string())?;
    }
    Ok(())
}
