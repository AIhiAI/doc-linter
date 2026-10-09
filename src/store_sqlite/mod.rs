//! SQLite store: the doc graph engine (docs/design/store-trait.md).
//!
//! Implements [`StoreRead`] / [`Store`] for [`SqliteDb`], plus open rw/ro
//! and an atomic `build_and_swap`. Ingest, typed reads and saved queries
//! live in the submodules; statements are plain SQL with `$name` bound
//! parameters.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use rusqlite::{types::ValueRef, Connection, OpenFlags};

use crate::store::{Params, Row, Store, StoreRead, Value};

pub mod history;
pub mod ingest;
pub mod nearest;
pub mod read;
pub mod saved;
pub mod schema;
pub mod vector;
pub mod vectors;

#[cfg(test)]
mod saved_guard_tests;
#[cfg(test)]
mod saved_tests;
#[cfg(test)]
pub(crate) mod testkit;

/// Open SQLite database; the engine's `Db` + `Conn` in one handle. The
/// second field caches the vector index a reader loads on first
/// similarity search (`None` once known absent).
pub struct SqliteDb(Connection, std::cell::OnceCell<Option<vector::Index>>);

/// Register `regexp(pattern, text)`, which backs `text REGEXP pattern`.
/// Like SQL's `=~` it matches the WHOLE string (RE2 full match), so a
/// saved query or `query sql` written from a SQL regex behaves alike.
/// NULL in, NULL out; an invalid pattern is an SQL error.
fn register_regexp(conn: &Connection) -> rusqlite::Result<()> {
    use rusqlite::functions::FunctionFlags;
    conn.create_scalar_function(
        "regexp",
        2,
        FunctionFlags::SQLITE_UTF8 | FunctionFlags::SQLITE_DETERMINISTIC,
        |ctx| {
            let re: std::sync::Arc<regex::Regex> = ctx.get_or_create_aux(0, |v| {
                regex::Regex::new(&format!("^(?:{})$", v.as_str()?))
                    .map_err(|e| rusqlite::Error::UserFunctionError(Box::new(e)))
            })?;
            Ok(match ctx.get_raw(1) {
                ValueRef::Text(t) => Some(re.is_match(&String::from_utf8_lossy(t))),
                _ => None,
            })
        },
    )
}

fn wrap(conn: Connection) -> Result<SqliteDb> {
    register_regexp(&conn)?;
    Ok(SqliteDb(conn, std::cell::OnceCell::new()))
}

pub fn graph_path(root: &Path) -> PathBuf {
    root.join(".doc-lint").join("graph.sqlite")
}

/// One-line notice when a pre-SQLite `graph.the store` sits beside no
/// `graph.sqlite`: the old graph is ignored and `check` rebuilds a fresh
/// one. `None` once the SQLite graph exists, so callers print it once.
pub fn legacy_graph_notice(root: &Path) -> Option<String> {
    let legacy = root.join(".doc-lint").join("graph.kuzu");
    (legacy.exists() && !graph_path(root).exists()).then(|| {
        format!(
            "doc-linter: ignoring legacy {} (Kuzu is no longer used); \
             `doc-linter check` builds the new graph.sqlite, and the old file can be deleted",
            legacy.display()
        )
    })
}

/// Open (or create) read-write in WAL mode.
pub fn open_rw(root: &Path) -> Result<SqliteDb> {
    let parent = root.join(".doc-lint");
    std::fs::create_dir_all(&parent).with_context(|| format!("create {}", parent.display()))?;
    let path = graph_path(root);
    let conn = Connection::open(&path).with_context(|| format!("open {}", path.display()))?;
    conn.busy_timeout(std::time::Duration::from_secs(5))?;
    conn.pragma_update_and_check(None, "journal_mode", "WAL", |_| Ok(()))?;
    conn.pragma_update(None, "synchronous", "NORMAL")?;
    wrap(conn)
}

/// Open an existing graph read-only.
pub fn open_ro(root: &Path) -> Result<SqliteDb> {
    let path = graph_path(root);
    anyhow::ensure!(
        path.exists(),
        "doc graph not found at {} — build it first with `doc-linter check`{}",
        path.display(),
        legacy_graph_notice(root).map_or(String::new(), |n| format!("\n{n}"))
    );
    let conn = Connection::open_with_flags(&path, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .with_context(|| format!("open (read-only) {}", path.display()))?;
    conn.busy_timeout(std::time::Duration::from_secs(5))?;
    check_version(&conn, &path)?;
    wrap(conn)
}

/// Schema version stamped on `conn` (`0` = never stamped).
fn stored_version(conn: &Connection) -> Result<u32> {
    Ok(conn.pragma_query_value(None, "user_version", |r| r.get(0))?)
}

/// Fail with a one-line rebuild hint unless the file carries exactly
/// [`schema::SCHEMA_VERSION`] (missing, older and newer all mismatch).
fn check_version(conn: &Connection, path: &Path) -> Result<()> {
    let found = stored_version(conn)?;
    anyhow::ensure!(
        found == schema::SCHEMA_VERSION,
        "doc graph {} has store schema version {found}, this doc-linter needs {} — \
         run `doc-linter check` to rebuild it",
        path.display(),
        schema::SCHEMA_VERSION
    );
    Ok(())
}

/// Identity of the file at `graph.sqlite`; changes on every
/// [`build_and_swap`] (same scheme as `store::graph_identity`).
pub fn graph_identity(root: &Path) -> Option<(u64, u64)> {
    let meta = std::fs::metadata(graph_path(root)).ok()?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        Some((meta.dev(), meta.ino()))
    }
    #[cfg(not(unix))]
    {
        let t = meta
            .modified()
            .ok()?
            .duration_since(std::time::UNIX_EPOCH)
            .ok()?;
        Some((meta.len(), u64::try_from(t.as_nanos()).unwrap_or(u64::MAX)))
    }
}

/// Rebuild without touching the live file: `VACUUM INTO` a staging copy
/// (or start empty), run `build` on it, rebuild FTS, switch it to
/// rollback-journal mode (one self-contained file), then atomically
/// rename over `graph.sqlite`. The `history` table rides along ([`history`]).
/// Readers keep the old inode and reopen when
/// [`graph_identity`] changes. On error the staging file is removed.
pub fn build_and_swap<T>(root: &Path, build: impl FnOnce(&SqliteDb) -> Result<T>) -> Result<T> {
    let parent = root.join(".doc-lint");
    std::fs::create_dir_all(&parent).with_context(|| format!("create {}", parent.display()))?;
    let live = graph_path(root);
    let staging = parent.join(format!("graph.sqlite.build-{}", std::process::id()));
    let sidecars = |p: &Path| ["-wal", "-shm", "-journal"].map(|s| format!("{}{s}", p.display()));
    let cleanup = |p: &Path| {
        let _ = std::fs::remove_file(p);
        sidecars(p)
            .iter()
            .for_each(|s| drop(std::fs::remove_file(s)));
    };
    cleanup(&staging);
    // A graph of another schema version is not copied: `check` starts empty.
    if let Ok(src) = open_ro(root) {
        src.0
            .execute("VACUUM INTO ?1", [staging.to_string_lossy().as_ref()])
            .context("copy live graph to staging")?;
    }
    // The vector index is a separate file. `build` may leave one at
    // `<staging>.vec`; it gets a unique name recorded in `meta.vec_file`
    // inside the staged db, so the db rename is the single commit point
    // and a reader never pairs a new db with an old index.
    let staged_vec = vectors::staging_path(&staging);
    let vec_name = format!(
        "graph.vec.{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos())
    );
    let built = (|| {
        let conn = Connection::open(&staging)
            .with_context(|| format!("open build db {}", staging.display()))?;
        // ponytail: staging is disposable, so skip fsync; the rename is the commit point.
        conn.pragma_update(None, "synchronous", "OFF")?;
        let db = wrap(conn)?;
        schema::create_schema(&db)?;
        history::carry_over(&db, &live);
        let old_vec = vectors::vec_file(&db)?;
        let out = build(&db)?;
        schema::rebuild_fts(&db)?;
        if staged_vec.exists() {
            vectors::set_vec_file(&db, Some(&vec_name))?;
        } else {
            vectors::set_vec_file(&db, None)?;
        }
        db.0.pragma_update_and_check(None, "journal_mode", "DELETE", |_| Ok(()))?;
        if staged_vec.exists() {
            std::fs::rename(&staged_vec, parent.join(&vec_name))
                .context("move vector index beside the db")?;
        }
        Ok((out, old_vec))
    })();
    let (out, old_vec) = match built {
        Ok(v) => v,
        Err(e) => {
            cleanup(&staging);
            let _ = std::fs::remove_file(&staged_vec);
            let _ = std::fs::remove_file(parent.join(&vec_name));
            return Err(e);
        }
    };
    // Stale WAL/SHM of the replaced file must not be seen by the new one.
    sidecars(&live)
        .iter()
        .for_each(|s| drop(std::fs::remove_file(s)));
    std::fs::rename(&staging, &live)
        .with_context(|| format!("swap {} into {}", staging.display(), live.display()))?;
    // Readers that already loaded the old index hold it in memory.
    if let Some(old) = old_vec {
        let _ = std::fs::remove_file(parent.join(old));
    }
    Ok(out)
}

impl SqliteDb {
    /// Run several `;`-separated statements (DDL).
    pub fn execute_batch(&self, sql: &str) -> Result<()> {
        self.0.execute_batch(sql).context("execute_batch")
    }
}

fn to_sql(v: &Value) -> rusqlite::types::Value {
    use rusqlite::types::Value as S;
    match v {
        Value::Null => S::Null,
        Value::Bool(b) => S::Integer(i64::from(*b)),
        Value::Int(n) => S::Integer(*n),
        Value::Float(f) => S::Real(*f),
        Value::Str(s) => S::Text(s.clone()),
        // ponytail: lists bind as JSON text; real list data uses side tables.
        Value::List(xs) => S::Text(list_json(xs)),
    }
}

fn list_json(xs: &[Value]) -> String {
    let j: Vec<serde_json::Value> = xs
        .iter()
        .map(|x| match x {
            Value::Null => serde_json::Value::Null,
            Value::Bool(b) => (*b).into(),
            Value::Int(n) => (*n).into(),
            Value::Float(f) => (*f).into(),
            Value::Str(s) => s.clone().into(),
            Value::List(l) => serde_json::from_str(&list_json(l)).unwrap_or_default(),
        })
        .collect();
    serde_json::Value::Array(j).to_string()
}

impl StoreRead for SqliteDb {
    fn query(&self, stmt: &str, params: Params) -> Result<Vec<Row>> {
        let mut st = self
            .0
            .prepare_cached(stmt)
            .with_context(|| stmt.to_string())?;
        for (k, v) in params {
            let name = format!("${k}");
            let i = st
                .parameter_index(&name)?
                .with_context(|| format!("unknown parameter {name} in `{stmt}`"))?;
            st.raw_bind_parameter(i, to_sql(v))?;
        }
        let n = st.column_count();
        let mut rows = st.raw_query();
        let mut out = Vec::new();
        while let Some(r) = rows.next().with_context(|| stmt.to_string())? {
            out.push(
                (0..n)
                    .map(|i| {
                        Ok(match r.get_ref(i)? {
                            ValueRef::Null => Value::Null,
                            ValueRef::Integer(n) => Value::Int(n),
                            ValueRef::Real(f) => Value::Float(f),
                            ValueRef::Text(t) => Value::Str(String::from_utf8_lossy(t).into()),
                            ValueRef::Blob(b) => Value::Str(String::from_utf8_lossy(b).into()),
                        })
                    })
                    .collect::<Result<Row>>()?,
            );
        }
        Ok(out)
    }
}

impl Store for SqliteDb {
    fn exec(&self, stmt: &str, params: Params) -> Result<()> {
        StoreRead::query(self, stmt, params).map(|_| ())
    }
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "tests panic on broken invariants — that is the point of a test"
)]
mod tests {
    use super::vector::{ExactIndex, VectorIndex};
    use super::*;

    fn tmp(label: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!(
            "doc-linter-sqlite-{label}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn s(x: &str) -> Value {
        Value::Str(x.into())
    }

    #[test]
    fn roundtrip_insert_read() {
        let root = tmp("rt");
        let db = open_rw(&root).unwrap();
        schema::create_schema(&db).unwrap();
        db.exec(
            "INSERT INTO Function(symbol, crate, line, generated) VALUES ($s, $c, $l, $g)",
            &[
                ("s", s("f::a")),
                ("c", s("k")),
                ("l", Value::Int(7)),
                ("g", Value::Bool(true)),
            ],
        )
        .unwrap();
        db.exec(
            "INSERT INTO doc_tags VALUES ($d, $v)",
            &[("d", s("d1")), ("v", s("t"))],
        )
        .unwrap();
        let rows = db
            .query(
                "SELECT symbol, line, generated FROM Function WHERE crate = $c",
                &[("c", s("k"))],
            )
            .unwrap();
        assert_eq!(rows, vec![vec![s("f::a"), Value::Int(7), Value::Int(1)]]);
        let tags = db
            .query("SELECT value FROM doc_tags WHERE doc_id = 'd1'", &[])
            .unwrap();
        assert_eq!(tags[0][0], s("t"));
        assert!(db.query("SELECT 1", &[("nope", Value::Null)]).is_err());
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn ro_reader_sees_swapped_file_after_reopen() {
        let root = tmp("swap");
        build_and_swap(&root, |db| {
            db.exec("INSERT INTO Repo(id) VALUES ('v1')", &[])
        })
        .unwrap();
        let id1 = graph_identity(&root).unwrap();
        let old = open_ro(&root).unwrap();
        assert_eq!(
            old.query("SELECT id FROM Repo", &[]).unwrap()[0][0],
            s("v1")
        );

        build_and_swap(&root, |db| {
            db.exec("INSERT INTO Repo(id) VALUES ('v2')", &[])
        })
        .unwrap();
        assert_ne!(graph_identity(&root).unwrap(), id1);
        // The old handle still reads the old (unlinked) file...
        assert_eq!(
            old.query("SELECT count(*) FROM Repo", &[]).unwrap()[0][0],
            Value::Int(1)
        );
        // ...a reopen sees both rows (staging started from a copy of live).
        let fresh = open_ro(&root).unwrap();
        assert_eq!(
            fresh.query("SELECT count(*) FROM Repo", &[]).unwrap()[0][0],
            Value::Int(2)
        );

        // A failed build leaves live untouched and no staging file.
        let failed: Result<()> = build_and_swap(&root, |db| {
            db.exec("INSERT INTO Repo(id) VALUES ('v3')", &[])?;
            anyhow::bail!("boom")
        });
        assert!(failed.is_err());
        assert_eq!(
            open_ro(&root)
                .unwrap()
                .query("SELECT count(*) FROM Repo", &[])
                .unwrap()[0][0],
            Value::Int(2)
        );
        let left = std::fs::read_dir(root.join(".doc-lint"))
            .unwrap()
            .flatten()
            .filter(|e| e.file_name().to_string_lossy().contains(".build-"))
            .count();
        assert_eq!(left, 0);
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn fts5_bm25_ranks_the_relevant_doc_first() {
        let root = tmp("fts");
        build_and_swap(&root, |db| {
            for (id, title, summary) in [
                (
                    "a",
                    "Graph storage",
                    "embedded property graph storage engine",
                ),
                ("b", "Linting", "vale prose linting of markdown"),
                ("c", "Retrieval", "bm25 ranking and storage notes"),
            ] {
                db.exec(
                    "INSERT INTO Doc(id, title, summary) VALUES ($i, $t, $s)",
                    &[("i", s(id)), ("t", s(title)), ("s", s(summary))],
                )?;
            }
            Ok(())
        })
        .unwrap();
        let db = open_ro(&root).unwrap();
        let rows = db
            .query(
                "SELECT d.id FROM doc_fts JOIN Doc d ON d.rowid = doc_fts.rowid \
                 WHERE doc_fts MATCH $q ORDER BY bm25(doc_fts) LIMIT 5",
                &[("q", s("storage"))],
            )
            .unwrap();
        let ids: Vec<_> = rows.iter().map(|r| r[0].str_or_empty()).collect();
        assert_eq!(ids.len(), 2);
        assert_eq!(
            ids[0], "a",
            "title+summary hit outranks summary-only: {ids:?}"
        );
        std::fs::remove_dir_all(&root).ok();
    }

    fn random_vectors(n: usize, dim: usize) -> Vec<Vec<f32>> {
        let mut x = 0x9E37_79B9_7F4A_7C15u64;
        let mut next = move || {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            (x >> 40) as f32 / (1u64 << 24) as f32 - 0.5
        };
        (0..n).map(|_| (0..dim).map(|_| next()).collect()).collect()
    }

    #[test]
    fn exact_index_save_load_roundtrip() {
        let dir = tmp("exact");
        let vs = random_vectors(50, 8);
        let mut ix = ExactIndex::new(8);
        vs.iter()
            .enumerate()
            .for_each(|(i, v)| ix.add(i as u64, v).unwrap());
        let p = dir.join("v.bin");
        ix.save(&p).unwrap();
        let back = ExactIndex::load(&p, 8).unwrap();
        assert_eq!(back.search(&vs[3], 1).unwrap()[0].0, 3);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[cfg(feature = "vector-usearch")]
    #[test]
    fn usearch_recall_at_10_vs_exact() {
        use super::vector::UsearchIndex;
        let (n, dim, k) = (5000, 384, 10);
        let vs = random_vectors(n, dim);
        let mut exact = ExactIndex::new(dim);
        let mut hnsw = UsearchIndex::new(dim).unwrap();
        for (i, v) in vs.iter().enumerate() {
            exact.add(i as u64, v).unwrap();
            hnsw.add(i as u64, v).unwrap();
        }
        // Save / load must preserve results.
        let dir = tmp("usearch");
        let p = dir.join("v.usearch");
        hnsw.save(&p).unwrap();
        let hnsw = UsearchIndex::load(&p, dim).unwrap();
        let queries = random_vectors(100, dim);
        let mut hit = 0;
        for q in &queries {
            let truth: std::collections::HashSet<u64> = exact
                .search(q, k)
                .unwrap()
                .into_iter()
                .map(|(i, _)| i)
                .collect();
            hit += hnsw
                .search(q, k)
                .unwrap()
                .iter()
                .filter(|(i, _)| truth.contains(i))
                .count();
        }
        let recall = hit as f64 / (queries.len() * k) as f64;
        eprintln!("recall@{k} = {recall:.3}");
        assert!(recall >= 0.95, "recall@10 {recall}");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn schema_version_stamp_mismatch_rebuild() {
        let root = tmp("ver");
        // Stamp: a built graph carries user_version and the meta row.
        build_and_swap(&root, |_| Ok(())).unwrap();
        let db = open_ro(&root).unwrap();
        assert_eq!(stored_version(&db.0).unwrap(), schema::SCHEMA_VERSION);
        let row = db
            .query("SELECT value FROM meta WHERE key='schema_version'", &[])
            .unwrap();
        assert_eq!(row[0][0].str_or_empty(), schema::SCHEMA_VERSION.to_string());
        drop(db);
        // Mismatch (older, newer, missing): readers error cleanly, no panic.
        for bad in [0, schema::SCHEMA_VERSION + 1] {
            let c = Connection::open(graph_path(&root)).unwrap();
            c.pragma_update(None, "user_version", bad).unwrap();
            drop(c);
            let err = open_ro(&root)
                .err()
                .expect("mismatch must fail")
                .to_string();
            assert!(err.contains("run `doc-linter check`"), "{err}");
            assert!(!err.contains('\n'), "one line: {err}");
            // Rebuild: build_and_swap replaces it and readers work again.
            build_and_swap(&root, |_| Ok(())).unwrap();
            assert!(open_ro(&root).is_ok());
        }
        std::fs::remove_dir_all(&root).ok();
    }
}
