//! SQLite ingest writers (plan item B3). Each module mirrors one
//! `store/*_ingest.rs`: the the store module's pure row-building logic is
//! reused (made `pub(crate)`), only the write path differs.
//!
//! Translation rules:
//! - `CREATE`/`MERGE`/`UNWIND ... CREATE` -> `INSERT ... ON CONFLICT(pk) DO
//!   UPDATE` in one transaction per ingest, prepared statement reused.
//! - `MATCH (a), (b) CREATE (a)-[:R]->(b)` -> `INSERT ... SELECT ... WHERE
//!   EXISTS(a) AND EXISTS(b)`, so an edge to a missing endpoint is dropped
//!   silently, exactly as the edge insert does.
//! - `STRING[]` columns -> rows in a side table (`doc_tags`, ...).

use anyhow::{Context, Result};

use super::SqliteDb;
use crate::store::{ResetMode, Value};

pub mod code;
pub mod docs;
pub mod misc;
pub mod pipeline;

impl SqliteDb {
    /// Run `f` inside `BEGIN`/`COMMIT`; any error rolls back.
    pub fn transaction<T>(&self, f: impl FnOnce(&Self) -> Result<T>) -> Result<T> {
        self.0.execute_batch("BEGIN").context("begin")?;
        match f(self) {
            Ok(v) => {
                self.0.execute_batch("COMMIT").context("commit")?;
                Ok(v)
            }
            Err(e) => {
                let _ = self.0.execute_batch("ROLLBACK");
                Err(e)
            }
        }
    }

    /// Execute one prepared statement per row with positional (`?`)
    /// parameters; returns the number of rows changed.
    pub fn write_rows(
        &self,
        sql: &str,
        rows: impl IntoIterator<Item = Vec<Value>>,
    ) -> Result<usize> {
        let mut st = self
            .0
            .prepare_cached(sql)
            .with_context(|| sql.to_string())?;
        let mut n = 0;
        for row in rows {
            n += st
                .execute(rusqlite::params_from_iter(row.iter().map(super::to_sql)))
                .with_context(|| sql.to_string())?;
        }
        Ok(n)
    }

    /// Scalar `SELECT` helper: first column of every row as a string.
    pub fn strings(&self, sql: &str) -> Result<Vec<String>> {
        Ok(super::StoreRead::query(self, sql, &[])?
            .into_iter()
            .map(|r| r.first().map(Value::str_or_empty).unwrap_or_default())
            .collect())
    }
}

/// `INSERT INTO t(cols) VALUES (?..) ON CONFLICT(pk) DO UPDATE SET ...`.
/// `cols` must include `pk`. Rows are `cols`-ordered.
pub fn upsert(
    db: &SqliteDb,
    table: &str,
    pk: &str,
    cols: &[&str],
    rows: impl IntoIterator<Item = Vec<Value>>,
) -> Result<usize> {
    let marks = vec!["?"; cols.len()].join(",");
    let sets: Vec<String> = cols
        .iter()
        .filter(|c| **c != pk)
        .map(|c| format!("{c}=excluded.{c}"))
        .collect();
    let conflict = if sets.is_empty() {
        format!("ON CONFLICT({pk}) DO NOTHING")
    } else {
        format!("ON CONFLICT({pk}) DO UPDATE SET {}", sets.join(","))
    };
    db.write_rows(
        &format!(
            "INSERT INTO \"{table}\"({}) VALUES ({marks}) {conflict}",
            cols.join(",")
        ),
        rows,
    )
    .with_context(|| format!("upsert {table}"))
}

/// Edge insert. Rows are `[src, dst, props...]`; rows whose endpoint is
/// missing from its node table are dropped. Returns edges created.
pub fn insert_edges(
    db: &SqliteDb,
    rel: &str,
    from: (&str, &str),
    to: (&str, &str),
    props: &[&str],
    rows: impl IntoIterator<Item = Vec<Value>>,
) -> Result<usize> {
    let n = 2 + props.len();
    let cols = ["src", "dst"]
        .into_iter()
        .chain(props.iter().copied())
        .collect::<Vec<_>>()
        .join(",");
    let sel = (1..=n)
        .map(|i| format!("?{i}"))
        .collect::<Vec<_>>()
        .join(",");
    let sql = format!(
        "INSERT INTO \"{rel}\"({cols}) SELECT {sel} \
         WHERE EXISTS(SELECT 1 FROM \"{}\" WHERE {}=?1) \
         AND EXISTS(SELECT 1 FROM \"{}\" WHERE {}=?2)",
        from.0, from.1, to.0, to.1
    );
    db.write_rows(&sql, rows)
        .with_context(|| format!("insert {rel} edges"))
}

/// [`insert_edges`] for a rel with no properties.
pub fn insert_pairs(
    db: &SqliteDb,
    rel: &str,
    from: (&str, &str),
    to: (&str, &str),
    pairs: impl IntoIterator<Item = (String, String)>,
) -> Result<usize> {
    insert_edges(
        db,
        rel,
        from,
        to,
        &[],
        pairs
            .into_iter()
            .map(|(a, b)| vec![Value::Str(a), Value::Str(b)]),
    )
}

/// Replace the side-table rows of the given owners.
pub fn write_list(
    db: &SqliteDb,
    table: &str,
    owner_col: &str,
    owners: impl IntoIterator<Item = (String, Vec<String>)>,
) -> Result<()> {
    let mut rows = Vec::new();
    for (owner, values) in owners {
        for v in values {
            rows.push(vec![Value::Str(owner.clone()), Value::Str(v)]);
        }
    }
    db.write_rows(
        &format!("INSERT INTO {table}({owner_col}, value) VALUES (?, ?)"),
        rows,
    )
    .map(|_| ())
    .with_context(|| format!("write {table}"))
}

pub fn s(x: impl Into<String>) -> Value {
    Value::Str(x.into())
}

pub fn int(n: impl TryInto<i64>) -> Value {
    Value::Int(n.try_into().unwrap_or(i64::MAX))
}

const VAULT_TABLES: &[&str] = &[
    "Doc",
    "Entity",
    "Section",
    "doc_tags",
    "doc_covers",
    "doc_attributes",
    "entity_synonyms",
    "entity_bounded_contexts",
    "entity_scanner_coverage",
    "entity_source_modules",
    "entity_attributes",
    "SECTION_OF",
    "COVERS",
    "WIKILINK",
    "MD_LINK",
    "DEPENDS_ON",
    "INFORMED_BY",
    "SUPERSEDES",
    "CRATE_REF",
    "RELATES_TO",
    // Cross-bucket edges that point at vault nodes.
    "FUNCTION_BELONGS_TO",
    "FUNCTION_DEFINED_IN",
    "FUNCTION_MENTIONS",
    "TYPE_BELONGS_TO",
    "TYPE_DEFINED_IN",
    "TYPE_MENTIONS",
    "ENTITY_CALLS",
    "ENDPOINT_TOUCHES_ENTITY",
    "DESCRIBED_BY",
];

const CODE_TABLES: &[&str] = &[
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
    "CALLS",
    "TEST_FOR",
    "ENDPOINT_HANDLED_BY",
    "DEFINED_IN_FILE",
    "IN_MODULE",
    "IMPORTS_MODULE",
    "COUPLED_WITH",
    "IMPORTS",
    "HAS_FINDING",
    "METHOD_OF",
    "USES_TYPE",
    "REFERENCES",
    "IMPLEMENTS",
    "EXTENDS",
];

/// SQLite twin of `store::schema::reset_schema_with_mode`: the
/// staging file starts as a copy of the live graph, so "drop and
/// recreate" is "delete the rows of the affected tables".
pub fn reset(db: &SqliteDb, mode: ResetMode) -> Result<()> {
    let tables: Vec<&str> = match mode {
        ResetMode::Full => VAULT_TABLES.iter().chain(CODE_TABLES).copied().collect(),
        ResetMode::VaultAndCrossBucket => VAULT_TABLES.to_vec(),
    };
    for t in tables {
        db.execute_batch(&format!("DELETE FROM \"{t}\";"))?;
    }
    Ok(())
}
