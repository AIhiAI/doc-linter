//! Guards over the whole built-in saved-query catalog.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "tests panic on broken invariants — that is the point of a test"
)]

use std::collections::HashMap;

use super::testkit::{self, run_saved_query};
use crate::store::query::saved::SAVED_QUERIES;

/// Port of the old "every Cypher parses against an empty Kuzu db" guard:
/// each embedded statement must prepare and run on the full empty schema (a
/// bad column or table name fails here, before it reaches users).
#[test]
fn every_builtin_sql_runs_against_an_empty_graph() {
    let dir = std::env::temp_dir().join(format!(
        "doc-linter-saved-sql-syntax-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let db = testkit::open_db(&dir);
    for q in SAVED_QUERIES {
        let params: HashMap<String, String> = q
            .params
            .iter()
            .filter(|p| !p.contains('='))
            .map(|p| ((*p).to_string(), "__test_value__".to_string()))
            .collect();
        run_saved_query(&db, q.name, &params)
            .unwrap_or_else(|e| panic!("saved query `{}` fails on an empty graph: {e:#}", q.name));
    }
    let _ = std::fs::remove_dir_all(&dir);
}
