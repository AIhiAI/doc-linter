//! Coverage-history snapshots: one row per `check`, kept in the public
//! `history` table of the SQLite store (docs/reference/store-schema.md).
//!
//! The table is additive (no schema-version bump). It is not rebuilt by
//! `check`: [`super::build_and_swap`] starts from a copy of the live file,
//! and [`carry_over`] also rescues the rows when that copy is skipped
//! because the live file has another schema version. Only the newest
//! [`KEEP`] rows are kept.

use std::path::Path;

use anyhow::{Context, Result};
use serde::Serialize;

use super::SqliteDb;
use crate::config::LintConfig;
use crate::coverage::{self, ReportFilters};
use crate::graph_read::GraphRead;

/// Most rows kept; older ones are deleted on every append.
pub const KEEP: i64 = 1000;

pub(super) const DDL: &str = "CREATE TABLE IF NOT EXISTS history(
  id INTEGER PRIMARY KEY AUTOINCREMENT, ts TEXT NOT NULL, git_rev TEXT,
  total_docs INTEGER NOT NULL, entities INTEGER NOT NULL, functions INTEGER NOT NULL,
  narrative_docs INTEGER NOT NULL, functions_reaching_entity INTEGER NOT NULL,
  entity_reach_pct REAL NOT NULL, documented_functions INTEGER NOT NULL,
  documented_pct REAL NOT NULL, dark_functions INTEGER NOT NULL,
  errors INTEGER NOT NULL, warnings INTEGER NOT NULL);";

const COLS: &str = "ts, git_rev, total_docs, entities, functions, narrative_docs, \
  functions_reaching_entity, entity_reach_pct, documented_functions, documented_pct, \
  dark_functions, errors, warnings";

/// One snapshot. `errors` / `warnings` are the finding counts of that run.
#[derive(Debug, Clone, Serialize)]
pub struct Snapshot {
    pub ts: String,
    pub git_rev: Option<String>,
    pub total_docs: i64,
    pub entities: i64,
    pub functions: i64,
    pub narrative_docs: i64,
    pub functions_reaching_entity: i64,
    pub entity_reach_pct: f64,
    pub documented_functions: i64,
    pub documented_pct: f64,
    pub dark_functions: i64,
    pub errors: i64,
    pub warnings: i64,
}

fn git_rev(root: &Path) -> Option<String> {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["rev-parse", "--short", "HEAD"])
        .output()
        .ok()?;
    let rev = String::from_utf8(out.stdout).ok()?.trim().to_string();
    (out.status.success() && !rev.is_empty()).then_some(rev)
}

/// Measure the graph just swapped in at `root`. Narrative docs are the
/// docs that are not ontology value/axis/entity docs (the same split as
/// the `corpus-shape-classifier` saved query).
pub fn snapshot(
    root: &Path,
    config: &LintConfig,
    errors: usize,
    warnings: usize,
) -> Result<Snapshot> {
    let db = super::open_ro(root)?;
    let count = |sql: &str| -> Result<i64> { Ok(db.0.query_row(sql, [], |r| r.get(0))?) };
    let total_docs = count("SELECT count(*) FROM Doc")?;
    let narrative_docs = count(
        "SELECT count(*) FROM Doc WHERE role NOT IN ('ontology-value','ontology-axis','ontology-entity')",
    )?;
    let entities = count("SELECT count(*) FROM Entity")?;
    let cov = coverage::build_report(&db as &dyn GraphRead, config, &ReportFilters::default())?;
    let g = &cov.global;
    let n = |x: u64| i64::try_from(x).unwrap_or(i64::MAX);
    Ok(Snapshot {
        ts: chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
        git_rev: git_rev(root),
        total_docs,
        entities,
        functions: n(g.total_functions),
        narrative_docs,
        functions_reaching_entity: n(g.functions_reaching_entity),
        entity_reach_pct: f64::from(g.global_pct),
        documented_functions: n(g.documented_functions),
        documented_pct: f64::from(g.documented_pct),
        dark_functions: n(g.dark_functions),
        errors: i64::try_from(errors).unwrap_or(i64::MAX),
        warnings: i64::try_from(warnings).unwrap_or(i64::MAX),
    })
}

/// Append `s` to the live graph and trim to [`KEEP`] rows. Uses a plain
/// connection: the live file is in rollback-journal mode and stays so.
pub fn append(root: &Path, s: &Snapshot) -> Result<()> {
    let path = super::graph_path(root);
    let conn =
        rusqlite::Connection::open(&path).with_context(|| format!("open {}", path.display()))?;
    conn.busy_timeout(std::time::Duration::from_secs(5))?;
    conn.execute_batch(DDL)?; // a graph from before this table existed
    conn.execute(
        &format!("INSERT INTO history({COLS}) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13)"),
        rusqlite::params![
            s.ts,
            s.git_rev,
            s.total_docs,
            s.entities,
            s.functions,
            s.narrative_docs,
            s.functions_reaching_entity,
            s.entity_reach_pct,
            s.documented_functions,
            s.documented_pct,
            s.dark_functions,
            s.errors,
            s.warnings
        ],
    )?;
    conn.execute(
        "DELETE FROM history WHERE id <= (SELECT max(id) FROM history) - ?1",
        [KEEP],
    )?;
    Ok(())
}

/// Measure and append in one step (what `check` calls after the swap).
pub fn record(root: &Path, config: &LintConfig, errors: usize, warnings: usize) -> Result<()> {
    append(root, &snapshot(root, config, errors, warnings)?)
}

/// Copy history rows from the previous file at `old` into the staging
/// database when staging has none (the live file was of another schema
/// version, so `VACUUM INTO` was skipped). Best effort: a missing table,
/// an unreadable file or a column mismatch just leaves history empty.
pub(super) fn carry_over(staging: &SqliteDb, old: &Path) {
    let empty = staging
        .0
        .query_row("SELECT count(*) FROM history", [], |r| r.get::<_, i64>(0))
        .is_ok_and(|n| n == 0);
    if !empty || !old.exists() {
        return;
    }
    let copy = || -> rusqlite::Result<()> {
        staging.0.execute(
            "ATTACH DATABASE ?1 AS old",
            [old.to_string_lossy().as_ref()],
        )?;
        let r = staging.0.execute_batch(&format!(
            "INSERT INTO main.history({COLS}) SELECT {COLS} FROM old.history ORDER BY id;"
        ));
        let _ = staging.0.execute_batch("DETACH DATABASE old;");
        r
    };
    let _ = copy();
}

/// Newest-first rows (`limit` at most) as the `{columns,row_count,rows}`
/// JSON of `run_sql`; empty when the graph predates the table.
pub fn read(db: &dyn GraphRead, limit: usize) -> Result<serde_json::Value> {
    let sql = format!("SELECT id, {COLS} FROM history ORDER BY id DESC LIMIT {limit}");
    match db.run_sql(&sql, Vec::new()) {
        Ok(v) => Ok(v),
        Err(e) if format!("{e:#}").contains("no such table") => {
            Ok(serde_json::json!({"columns": [], "row_count": 0, "rows": []}))
        }
        Err(e) => Err(e),
    }
}

/// Human table for `query history`, oldest first so the trend reads down.
pub fn render_table(v: &serde_json::Value) -> String {
    let rows = crate::graph_read::rows_of(v);
    if rows.is_empty() {
        return "No history yet: run `doc-linter check`.".to_string();
    }
    let mut out = format!(
        "{:<20} {:<8} {:>5} {:>8} {:>9} {:>7} {:>8} {:>5} {:>5}\n",
        "WHEN (UTC)", "REV", "DOCS", "ENTITIES", "FUNCTIONS", "REACH%", "DOCUM%", "ERR", "WARN"
    );
    for r in rows.iter().rev() {
        let i = |k: &str| r.get(k).and_then(serde_json::Value::as_i64).unwrap_or(0);
        let f = |k: &str| r.get(k).and_then(serde_json::Value::as_f64).unwrap_or(0.0);
        let s = |k: &str| r.get(k).and_then(serde_json::Value::as_str).unwrap_or("-");
        out += &format!(
            "{:<20} {:<8} {:>5} {:>8} {:>9} {:>7.1} {:>8.1} {:>5} {:>5}\n",
            s("ts").trim_end_matches('Z').replace('T', " "),
            s("git_rev"),
            i("total_docs"),
            i("entities"),
            i("functions"),
            f("entity_reach_pct"),
            f("documented_pct"),
            i("errors"),
            i("warnings")
        );
    }
    out
}

/// One line comparing the oldest and newest of the last `limit` rows, for
/// `doc-linter report`. `None` with fewer than two rows.
pub fn trend_line(v: &serde_json::Value) -> Option<String> {
    let rows = crate::graph_read::rows_of(v);
    let (new, old) = (rows.first()?, rows.last()?);
    if rows.len() < 2 {
        return None;
    }
    let i =
        |r: &serde_json::Value, k: &str| r.get(k).and_then(serde_json::Value::as_i64).unwrap_or(0);
    let f = |r: &serde_json::Value, k: &str| {
        r.get(k).and_then(serde_json::Value::as_f64).unwrap_or(0.0)
    };
    Some(format!(
        "last {} runs: docs {} -> {}, entity reach {:.1}% -> {:.1}%, documented {:.1}% -> {:.1}%, errors {} -> {}",
        rows.len(),
        i(old, "total_docs"),
        i(new, "total_docs"),
        f(old, "entity_reach_pct"),
        f(new, "entity_reach_pct"),
        f(old, "documented_pct"),
        f(new, "documented_pct"),
        i(old, "errors"),
        i(new, "errors")
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store_sqlite::{build_and_swap, open_ro};

    fn tmp(label: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!(
            "doc-linter-history-{label}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn snap(docs: i64) -> Snapshot {
        Snapshot {
            ts: format!("2026-01-01T00:00:{docs:02}Z"),
            git_rev: Some("abc1234".into()),
            total_docs: docs,
            entities: 2,
            functions: 10,
            narrative_docs: 1,
            functions_reaching_entity: 5,
            entity_reach_pct: 50.0,
            documented_functions: 4,
            documented_pct: 40.0,
            dark_functions: 5,
            errors: 1,
            warnings: 2,
        }
    }

    fn count(root: &Path) -> i64 {
        let db = open_ro(root).unwrap();
        db.0.query_row("SELECT count(*) FROM history", [], |r| r.get(0))
            .unwrap()
    }

    #[test]
    fn append_read_newest_first_with_limit() {
        let root = tmp("read");
        build_and_swap(&root, |_| Ok(())).unwrap();
        for d in 1..=3 {
            append(&root, &snap(d)).unwrap();
        }
        let db = open_ro(&root).unwrap();
        let v = read(&db, 2).unwrap();
        let rows = crate::graph_read::rows_of(&v);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0]["total_docs"], 3);
        assert_eq!(rows[1]["total_docs"], 2);
        assert!(trend_line(&v).unwrap().contains("docs 2 -> 3"));
        assert!(render_table(&v).contains("abc1234"));
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn capped_at_keep() {
        let root = tmp("cap");
        build_and_swap(&root, |_| Ok(())).unwrap();
        append(&root, &snap(1)).unwrap();
        let conn = rusqlite::Connection::open(super::super::graph_path(&root)).unwrap();
        conn.execute_batch(&format!(
            "WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i+1 FROM n WHERE i < {})
             INSERT INTO history({COLS})
             SELECT ts, git_rev, total_docs, entities, functions, narrative_docs,
               functions_reaching_entity, entity_reach_pct, documented_functions,
               documented_pct, dark_functions, errors, warnings FROM history, n;",
            KEEP + 4
        ))
        .unwrap();
        drop(conn);
        assert!(count(&root) > KEEP);
        append(&root, &snap(2)).unwrap();
        assert_eq!(count(&root), KEEP);
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn survives_swap() {
        let root = tmp("swap");
        build_and_swap(&root, |_| Ok(())).unwrap();
        append(&root, &snap(1)).unwrap();
        build_and_swap(&root, |_| Ok(())).unwrap();
        assert_eq!(count(&root), 1);
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn survives_schema_version_rebuild() {
        let root = tmp("schema");
        build_and_swap(&root, |_| Ok(())).unwrap();
        append(&root, &snap(1)).unwrap();
        append(&root, &snap(2)).unwrap();
        // Pretend the live file was written by an older doc-linter.
        let conn = rusqlite::Connection::open(super::super::graph_path(&root)).unwrap();
        conn.pragma_update(None, "user_version", 0).unwrap();
        drop(conn);
        assert!(open_ro(&root).is_err());
        build_and_swap(&root, |_| Ok(())).unwrap();
        assert_eq!(count(&root), 2);
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn old_graph_without_table_reads_empty_and_gets_one_on_append() {
        let root = tmp("old");
        build_and_swap(&root, |_| Ok(())).unwrap();
        let conn = rusqlite::Connection::open(super::super::graph_path(&root)).unwrap();
        conn.execute_batch("DROP TABLE history;").unwrap();
        drop(conn);
        let db = open_ro(&root).unwrap();
        assert!(crate::graph_read::rows_of(&read(&db, 5).unwrap()).is_empty());
        drop(db);
        append(&root, &snap(1)).unwrap();
        assert_eq!(count(&root), 1);
        std::fs::remove_dir_all(&root).ok();
    }
}
