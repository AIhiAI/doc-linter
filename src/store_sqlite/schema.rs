//! SQLite translation of `store::schema` (docs/design/store-trait.md).
//!
//! Node tables keep their the store columns minus the `FLOAT[384]` embeddings
//! (those live in a [`super::vector::VectorIndex`] keyed by the row's
//! `rowid`). `UINT32` becomes plain `INTEGER`, so no unsigned-arithmetic
//! quirks. List columns become side tables (`doc_tags(doc_id, value)`,
//! ...) indexed on `value`. Each rel table becomes an edge table
//! `(src, dst, props...)` keyed by the endpoint primary keys, indexed on
//! both ends. FTS5 external-content tables cover Doc / Section / Function
//! text; they are filled by [`rebuild_fts`] (done by `build_and_swap`).

use anyhow::Result;

use super::SqliteDb;

const NODES: &str = "
CREATE TABLE IF NOT EXISTS Doc(
  id TEXT PRIMARY KEY, path TEXT, role TEXT, kind TEXT, lifecycle TEXT,
  bounded_context TEXT, title TEXT, summary TEXT, status TEXT, updated TEXT,
  summary_embed_hash TEXT, repo_id TEXT, phase TEXT);
CREATE TABLE IF NOT EXISTS Entity(
  id TEXT PRIMARY KEY, display TEXT, description TEXT, mention_count INTEGER,
  is_god_node INTEGER, entity_class TEXT, display_embed_hash TEXT, repo_id TEXT);
CREATE TABLE IF NOT EXISTS Section(
  id TEXT PRIMARY KEY, doc_id TEXT, heading TEXT, anchor TEXT, level INTEGER,
  line INTEGER, text TEXT, text_embed_hash TEXT);
CREATE TABLE IF NOT EXISTS Function(
  symbol TEXT PRIMARY KEY, crate TEXT, file TEXT, line INTEGER, doc_comment TEXT,
  language TEXT, signature TEXT, body_excerpt TEXT, last_touched TEXT,
  doc_comment_embed_hash TEXT, repo_id TEXT, generated INTEGER);
CREATE TABLE IF NOT EXISTS Type(
  symbol TEXT PRIMARY KEY, kind TEXT, crate TEXT, file TEXT, line INTEGER,
  doc_comment TEXT, language TEXT, doc_comment_embed_hash TEXT, signature TEXT,
  body_excerpt TEXT, last_touched TEXT, repo_id TEXT);
CREATE TABLE IF NOT EXISTS Field(
  symbol TEXT PRIMARY KEY, name TEXT, owner TEXT, file TEXT, line INTEGER,
  doc_comment TEXT, language TEXT);
CREATE TABLE IF NOT EXISTS File(
  path TEXT PRIMARY KEY, language TEXT, loc INTEGER, last_touched TEXT);
CREATE TABLE IF NOT EXISTS Module(
  id TEXT PRIMARY KEY, kind TEXT, path TEXT, name TEXT);
CREATE TABLE IF NOT EXISTS Finding(
  id TEXT PRIMARY KEY, kind TEXT, file TEXT, line INTEGER, message TEXT, severity TEXT);
CREATE TABLE IF NOT EXISTS Migration(
  id TEXT PRIMARY KEY, from_version INTEGER, to_version INTEGER, applied INTEGER,
  applied_at TEXT, title TEXT, summary TEXT);
CREATE TABLE IF NOT EXISTS RepoMeta(
  id TEXT PRIMARY KEY, domain TEXT, problem_statement TEXT);
CREATE TABLE IF NOT EXISTS Repo(
  id TEXT PRIMARY KEY, root_path TEXT, name TEXT);
CREATE TABLE IF NOT EXISTS Endpoint(
  id TEXT PRIMARY KEY, kind TEXT, method TEXT, path TEXT, handler_symbol TEXT,
  source_file TEXT, source_line INTEGER);
-- Not touched by ingest resets: vectors survive rebuilds by text fingerprint
-- (the old engine's EmbedCache), and `meta.vec_file` names the usearch index file.
CREATE TABLE IF NOT EXISTS embed_cache(fp TEXT PRIMARY KEY, emb BLOB NOT NULL);
CREATE TABLE IF NOT EXISTS meta(key TEXT PRIMARY KEY, value TEXT NOT NULL);
";

/// Side table, owner key column, for the former `STRING[]` columns
/// (`Doc.tags` -> `doc_tags`, ...). Each has `(<owner col>, value)`.
const LIST_TABLES: &[(&str, &str)] = &[
    ("doc_tags", "doc_id"),
    ("doc_covers", "doc_id"),
    ("doc_attributes", "doc_id"),
    ("entity_synonyms", "entity_id"),
    ("entity_bounded_contexts", "entity_id"),
    ("entity_scanner_coverage", "entity_id"),
    ("entity_source_modules", "entity_id"),
    ("entity_attributes", "entity_id"),
];

/// Edge table name and extra property columns; the the store `FROM -> TO` pair
/// is noted per row. `src` / `dst` hold the endpoint primary keys.
const EDGES: &[(&str, &str)] = &[
    ("WIKILINK", "line INTEGER"),                 // Doc -> Doc
    ("MD_LINK", "line INTEGER"),                  // Doc -> Doc
    ("DEPENDS_ON", "line INTEGER"),               // Doc -> Doc
    ("INFORMED_BY", "line INTEGER"),              // Doc -> Doc
    ("SUPERSEDES", "line INTEGER"),               // Doc -> Doc
    ("CRATE_REF", "line INTEGER"),                // Doc -> Doc
    ("COVERS", "line INTEGER, inferred INTEGER"), // Doc -> Entity
    ("SECTION_OF", ""),                           // Section -> Doc
    (
        "RELATES_TO",
        "type TEXT, weight REAL, frequency INTEGER, source TEXT",
    ), // Entity -> Entity
    ("FUNCTION_DEFINED_IN", ""),                  // Function -> Doc
    ("FUNCTION_BELONGS_TO", ""),                  // Function -> Entity
    ("FUNCTION_MENTIONS", "confidence TEXT"),     // Function -> Entity
    ("TYPE_DEFINED_IN", ""),                      // Type -> Doc
    ("TYPE_BELONGS_TO", ""),                      // Type -> Entity
    ("TYPE_MENTIONS", "confidence TEXT"),         // Type -> Entity
    (
        "CALLS",
        "source_file TEXT, source_line INTEGER, dispatch INTEGER",
    ), // Function -> Function
    ("TEST_FOR", "confidence TEXT"),              // Function -> Function
    (
        "ENTITY_CALLS",
        "frequency INTEGER, weight REAL, source TEXT",
    ), // Entity -> Entity
    ("ENDPOINT_HANDLED_BY", ""),                  // Endpoint -> Function
    ("ENDPOINT_TOUCHES_ENTITY", ""),              // Endpoint -> Entity
    ("DEFINED_IN_FILE", ""),                      // Function -> File
    ("IN_MODULE", ""),                            // File -> Module
    ("IMPORTS_MODULE", ""),                       // Module -> Module
    ("DESCRIBED_BY", ""),                         // Module -> Doc
    (
        "COUPLED_WITH",
        "commits INTEGER, last_co_change_at TEXT, jaccard REAL",
    ), // File -> File
    ("IMPORTS", "import_count INTEGER, source TEXT"), // File -> File
    ("HAS_FINDING", ""),                          // File -> Finding
    ("METHOD_OF", ""),                            // Function -> Type
    ("USES_TYPE", ""),                            // Function -> Type
    ("REFERENCES", "source_file TEXT, source_line INTEGER"), // Function -> Field
    ("IMPLEMENTS", ""),                           // Type -> Type
    ("EXTENDS", ""),                              // Type -> Type
];

/// (fts table, content table, indexed columns).
const FTS: &[(&str, &str, &str)] = &[
    ("doc_fts", "Doc", "title, summary"),
    ("section_fts", "Section", "heading, text"),
    ("function_fts", "Function", "symbol, doc_comment"),
];

fn ddl() -> String {
    let mut s = String::from(NODES);
    for (t, owner_col) in LIST_TABLES {
        s += &format!(
            "CREATE TABLE IF NOT EXISTS {t}({owner_col} TEXT NOT NULL, value TEXT NOT NULL);\
             CREATE INDEX IF NOT EXISTS {t}_value ON {t}(value);\
             CREATE INDEX IF NOT EXISTS {t}_owner ON {t}({owner_col});"
        );
    }
    for (t, props) in EDGES {
        let props = if props.is_empty() {
            String::new()
        } else {
            format!(", {props}")
        };
        s += &format!(
            "CREATE TABLE IF NOT EXISTS \"{t}\"(src TEXT NOT NULL, dst TEXT NOT NULL{props});\
             CREATE INDEX IF NOT EXISTS {t}_src ON \"{t}\"(src);\
             CREATE INDEX IF NOT EXISTS {t}_dst ON \"{t}\"(dst);"
        );
    }
    for (f, content, cols) in FTS {
        s += &format!(
            "CREATE VIRTUAL TABLE IF NOT EXISTS {f} USING fts5({cols}, \
             content='{content}', content_rowid='rowid', tokenize='porter unicode61');"
        );
    }
    s += super::history::DDL;
    s
}

/// Store schema version (docs/reference/store-schema.md). Bump on any
/// change to the public surface; stamped by [`create_schema`] into
/// `PRAGMA user_version` and `meta.schema_version`.
pub const SCHEMA_VERSION: u32 = 1;

/// Create every table, index and FTS table (idempotent) and stamp the
/// schema version.
pub fn create_schema(db: &SqliteDb) -> Result<()> {
    db.execute_batch(&ddl())?;
    db.execute_batch(&format!(
        "PRAGMA user_version = {SCHEMA_VERSION};\
         INSERT OR REPLACE INTO meta(key, value) VALUES('schema_version', '{SCHEMA_VERSION}');"
    ))
}

/// Re-sync the external-content FTS tables with their content tables.
/// Call after bulk writes; `build_and_swap` does it before the swap.
pub fn rebuild_fts(db: &SqliteDb) -> Result<()> {
    for (f, _, _) in FTS {
        db.execute_batch(&format!("INSERT INTO {f}({f}) VALUES('rebuild');"))?;
    }
    Ok(())
}

/// Endpoint node tables of each rel table, `(rel, from, to)`: what the store's
/// `show_connection` reports, for `query schema`.
const EDGE_ENDPOINTS: &[(&str, &str, &str)] = &[
    ("WIKILINK", "Doc", "Doc"),
    ("MD_LINK", "Doc", "Doc"),
    ("DEPENDS_ON", "Doc", "Doc"),
    ("INFORMED_BY", "Doc", "Doc"),
    ("SUPERSEDES", "Doc", "Doc"),
    ("CRATE_REF", "Doc", "Doc"),
    ("COVERS", "Doc", "Entity"),
    ("SECTION_OF", "Section", "Doc"),
    ("RELATES_TO", "Entity", "Entity"),
    ("FUNCTION_DEFINED_IN", "Function", "Doc"),
    ("FUNCTION_BELONGS_TO", "Function", "Entity"),
    ("FUNCTION_MENTIONS", "Function", "Entity"),
    ("TYPE_DEFINED_IN", "Type", "Doc"),
    ("TYPE_BELONGS_TO", "Type", "Entity"),
    ("TYPE_MENTIONS", "Type", "Entity"),
    ("CALLS", "Function", "Function"),
    ("TEST_FOR", "Function", "Function"),
    ("ENTITY_CALLS", "Entity", "Entity"),
    ("ENDPOINT_HANDLED_BY", "Endpoint", "Function"),
    ("ENDPOINT_TOUCHES_ENTITY", "Endpoint", "Entity"),
    ("DEFINED_IN_FILE", "Function", "File"),
    ("IN_MODULE", "File", "Module"),
    ("IMPORTS_MODULE", "Module", "Module"),
    ("DESCRIBED_BY", "Module", "Doc"),
    ("COUPLED_WITH", "File", "File"),
    ("IMPORTS", "File", "File"),
    ("HAS_FINDING", "File", "Finding"),
    ("METHOD_OF", "Function", "Type"),
    ("USES_TYPE", "Function", "Type"),
    ("REFERENCES", "Function", "Field"),
    ("IMPLEMENTS", "Type", "Type"),
    ("EXTENDS", "Type", "Type"),
];

/// Node tables a user sees (the others are side tables, FTS shadows,
/// `meta` and `embed_cache`).
const NODE_TABLES: &[&str] = &[
    "Doc",
    "Entity",
    "Section",
    "Function",
    "Type",
    "Field",
    "File",
    "Module",
    "Finding",
    "Migration",
    "RepoMeta",
    "Repo",
    "Endpoint",
];

/// Owner node table of each list side table, for folding them back into
/// the owner's columns as `STRING[]`.
fn list_owner(table: &str) -> &'static str {
    if table.starts_with("doc_") {
        "Doc"
    } else {
        "Entity"
    }
}

/// `query schema` on SQLite: node and rel tables with columns (list side
/// tables folded back in as `STRING[]`), endpoints and row counts. Differs
/// from the store in column type spelling (`TEXT` / `INTEGER` / `REAL`), the
/// absent `*_embedding` columns and the absent `EmbedCache` table
/// (vectors live in the usearch file).
pub fn schema_tables(db: &SqliteDb) -> Result<Vec<crate::graph_read::TableInfo>> {
    use crate::graph_read::{ColumnInfo, TableInfo};
    use crate::store::StoreRead;
    let count = |t: &str| -> Result<u64> {
        Ok(db.query(&format!("SELECT count(*) FROM \"{t}\""), &[])?[0][0].as_u64_or_zero())
    };
    let columns = |t: &str, skip: &[&str]| -> Result<Vec<ColumnInfo>> {
        Ok(db
            .query(
                &format!("SELECT name, type, pk FROM pragma_table_info('{t}')"),
                &[],
            )?
            .iter()
            .filter(|r| !skip.contains(&r[0].str_or_empty().as_str()))
            .map(|r| ColumnInfo {
                name: r[0].str_or_empty(),
                ty: r[1].str_or_empty(),
                pk: r[2].as_i64().unwrap_or(0) > 0,
                default: String::new(),
            })
            .collect())
    };
    let mut out = Vec::new();
    for t in NODE_TABLES {
        let mut cols = columns(t, &[])?;
        for (list, _) in LIST_TABLES {
            if list_owner(list) == *t {
                let name = list.split_once('_').map_or(*list, |(_, n)| n);
                cols.push(ColumnInfo {
                    name: name.to_string(),
                    ty: "STRING[]".to_string(),
                    pk: false,
                    default: String::new(),
                });
            }
        }
        out.push(TableInfo {
            name: (*t).to_string(),
            is_rel: false,
            columns: cols,
            from: String::new(),
            to: String::new(),
            row_count: count(t)?,
        });
    }
    for (t, from, to) in EDGE_ENDPOINTS {
        out.push(TableInfo {
            name: (*t).to_string(),
            is_rel: true,
            columns: columns(t, &["src", "dst"])?,
            from: (*from).to_string(),
            to: (*to).to_string(),
            row_count: count(t)?,
        });
    }
    Ok(out)
}
