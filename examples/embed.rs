//! Toy embed example for #45: demonstrates that downstream code can
//! drive the doc-linter pipeline through the library surface instead of
//! shelling out to the binary.
//!
//! Builds an empty SQLite graph in a tempdir, round-trips a trivial SQL
//! query through `doc_linter::query::run`, and prints the JSON result.
//! No corpus / ontology / SCIP data is loaded — the goal is to prove
//! that the lib + bin split actually exposes the inner functions.
//!
//! Run with: `cargo run --example embed`

use std::path::PathBuf;

use anyhow::Result;
use doc_linter::config::LintConfig;
use doc_linter::query::{run, QueryKind, SqlArgs};
use doc_linter::store_sqlite::{build_and_swap, open_ro};

fn main() -> Result<()> {
    let tmp = std::env::temp_dir().join(format!("doc-linter-embed-{}", std::process::id()));
    let root: PathBuf = tmp.clone();
    std::fs::create_dir_all(&root)?;

    build_and_swap(&root, |_| Ok(()))?;
    let db = open_ro(&root)?;
    let config = LintConfig::default();

    let kind = QueryKind::Sql(SqlArgs {
        query: "SELECT 1 AS one, 'doc-linter' AS source".into(),
        params: Vec::new(),
    });
    let out = run(&db, &root, &config, kind)?;
    println!("{out}");

    std::fs::remove_dir_all(&tmp).ok();
    Ok(())
}
