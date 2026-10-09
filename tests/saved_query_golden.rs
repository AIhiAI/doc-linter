#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "integration tests panic on broken invariants — that is the point"
)]
//! Saved-query golden (docs/design/saved-query-port.md): ingest a corpus,
//! run EVERY catalog query with sane default params and keep the normalised
//! result sets. The goldens in `tests/golden/saved_queries_*.tsv` were
//! recorded from the Kuzu engine (the reference before it was removed). Eight
//! queries (singleton-tags, entity-to-implementation-files,
//! path-axis-completeness-fingerprint, path-axis-fingerprint-cython-aware,
//! files-by-path-pattern-with-coupling, file-coupling-degree,
//! resource-shape-classifier, coupling-density-stats) hit documented Kuzu
//! quirks (an error, a NULL sum, a wrong count); their goldens were recorded
//! from SQLite. `RECORD_GOLDEN=1` re-records from SQLite: review the diff
//! first.
//!
//! Run: `cargo test --test saved_query_golden -- --test-threads=1`

use std::collections::HashMap;
use std::path::Path;

mod common;
use common::{check, seed_fixture, seed_rich, unique_tmpdir, write, Snapshot};

use doc_linter::store::{query::saved::SAVED_QUERIES, StoreRead};
use doc_linter::store_sqlite::{self, saved::run_saved_query_with_root, SqliteDb};
use serde_json::Value as J;

// --- normalisation -------------------------------------------------------

fn num(f: f64) -> String {
    if (f - f.round()).abs() < 1e-9 {
        format!("{}", f.round() as i64)
    } else {
        format!("{f:.5}")
    }
}

/// Canonical text for a JSON value: bools are 0/1, numbers rounded, list
/// elements sorted, object keys sorted.
fn norm(v: &J) -> String {
    match v {
        J::Null => "null".into(),
        J::Bool(b) => i32::from(*b).to_string(),
        J::Number(n) => num(n.as_f64().unwrap_or(0.0)),
        // Kuzu returns `sum()` over UINT32 (INT128) as a JSON string, SQLite
        // as a number: compare numerically (intentional, see design doc).
        J::String(s) => s
            .parse::<i64>()
            .map_or_else(|_| format!("{s:?}"), |n| n.to_string()),
        J::Array(xs) => {
            // Kuzu `OPTIONAL MATCH` + `collect` of maps yields one all-null
            // map where SQLite yields `[]` (intentional): drop those.
            let mut e: Vec<String> = xs.iter().filter(|x| !null_elem(x)).map(norm).collect();
            e.sort();
            format!("[{}]", e.join(","))
        }
        J::Object(m) => {
            let mut e: Vec<String> = m.iter().map(|(k, v)| format!("{k}={}", norm(v))).collect();
            e.sort();
            format!("{{{}}}", e.join(","))
        }
    }
}

fn null_elem(v: &J) -> bool {
    match v {
        J::Null => true,
        J::Object(m) => m.iter().all(|(k, v)| k == "kind" || v.is_null()),
        _ => false,
    }
}

fn rows_of(v: &J) -> Vec<String> {
    let mut r: Vec<String> = v["rows"].as_array().unwrap().iter().map(norm).collect();
    r.sort();
    r
}

// --- params ----------------------------------------------------------------

#[derive(Default)]
struct Sample {
    entities: Vec<String>,
    symbols: Vec<String>,
    docs: Vec<String>,
    tags: Vec<String>,
    paths: Vec<String>,
}

fn col(db: &SqliteDb, sql: &str) -> Vec<String> {
    db.query(sql, &[])
        .unwrap()
        .iter()
        .map(|r| r[0].str_or_empty())
        .collect()
}

fn sample(db: &SqliteDb) -> Sample {
    Sample {
        entities: col(db, "SELECT id FROM Entity ORDER BY id LIMIT 3"),
        symbols: col(db, "SELECT symbol FROM Function ORDER BY symbol LIMIT 3"),
        docs: col(db, "SELECT id FROM Doc ORDER BY id LIMIT 3"),
        tags: col(
            db,
            "SELECT DISTINCT value FROM doc_tags ORDER BY value LIMIT 3",
        ),
        paths: col(db, "SELECT path FROM File ORDER BY path LIMIT 3"),
    }
}

/// Parameter value for the `i`-th variant of a case. Variant 0 is a
/// discovered value (hits rows), later ones are generic substrings.
fn param_value(name: &str, i: usize, s: &Sample) -> String {
    let pick = |xs: &[String], fallback: &str| {
        xs.get(i % xs.len().max(1))
            .cloned()
            .unwrap_or_else(|| fallback.to_string())
    };
    match name {
        "entity" | "entity_id" | "entity_a" | "test_entity_id" => pick(&s.entities, "pricing"),
        "entity_b" => pick(&s.entities[s.entities.len().min(1)..], "checkout"),
        "symbol" | "function_symbol" => pick(&s.symbols, "compute"),
        "doc" | "doc_id" => pick(&s.docs, "doc-a"),
        "tag" | "family_tag" | "family_a" | "family_b" => pick(&s.tags, "two"),
        "iter_tag" => "iter-".into(),
        "file_path" | "path" | "caller_file" | "callee_file" | "folder_path" | "path_prefix"
        | "prefix" => pick(&s.paths, "crates"),
        "kind" => ["explanation", "reference", "how-to"][i % 3].into(),
        "min_jaccard" => "0".into(),
        "max_jaccard" => "1".into(),
        "min_calls" => ["5", "0", "1"][i % 3].into(),
        "basename" => ["lib.rs", "a.md", "math.py"][i % 3].into(),
        _ => ["a", "e", "src", "doc"][i % 4].into(),
    }
}

const VARIANTS: usize = 3;

fn run_corpus(label: &str, seed: impl Fn(&Path)) {
    let sroot = unique_tmpdir(&format!("sqg-{label}"));
    seed(&sroot);
    check(&sroot);
    let sdb = store_sqlite::open_ro(&sroot).expect("open sqlite");
    let smp = sample(&sdb);

    let mut snap = Snapshot::new(&format!("saved_queries_{label}"));
    for q in SAVED_QUERIES {
        let declared: Vec<&str> = q
            .params
            .iter()
            .map(|p| p.split_once('=').map_or(*p, |(n, _)| n))
            .collect();
        // Parameterless queries run once; `name=default` params keep their
        // default on variant 0 and are overridden after.
        let variants = if declared.is_empty() { 1 } else { VARIANTS };
        for i in 0..variants {
            let params: HashMap<String, String> = declared
                .iter()
                .filter(|n| !(i == 0 && q.params.iter().any(|p| p.starts_with(&format!("{n}=")))))
                .map(|n| ((*n).to_string(), param_value(n, i, &smp)))
                .collect();
            let case = if declared.is_empty() {
                q.name.to_string()
            } else {
                format!("{}#{i}", q.name)
            };
            let res = run_saved_query_with_root(&sdb, None, q.name, &params);
            let text = match res {
                Ok(v) => format!("{:?}\n{}", v["columns"], rows_of(&v).join("\n")),
                // Error text is engine-specific.
                Err(_) => "ERR".to_string(),
            };
            let text = text.replace(sroot.to_str().unwrap(), "<ROOT>");
            snap.add(&case, &text);
        }
    }

    // Runtime extension: a TOML query with an `sql` form runs on the engine.
    write(
        &sroot.join("saved-queries/doc-count.toml"),
        "name = \"doc-count\"\ndescription = \"d\"\nparams = []\n\
         sql = \"SELECT count(*) AS n FROM Doc\"\n",
    );
    let ext = run_saved_query_with_root(&sdb, Some(&sroot), "doc-count", &HashMap::new()).unwrap();
    assert!(
        ext["rows"][0]["n"].as_i64().unwrap() > 0,
        "runtime sql extension"
    );

    assert!(
        snap.nonempty() > 100,
        "{label}: vacuous run ({} non-empty)",
        snap.nonempty()
    );
    snap.finish();
    let _ = std::fs::remove_dir_all(&sroot);
}

#[test]
fn rich_corpus_saved_queries_match() {
    run_corpus("rich", seed_rich);
}

#[test]
fn fixture_corpus_saved_queries_match() {
    run_corpus("fixture", seed_fixture);
}
