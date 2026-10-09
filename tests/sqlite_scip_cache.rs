#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "integration tests panic on broken invariants — that is the point"
)]
//! SCIP cache-hit ingest on the SQLite store (docs/design/store-trait.md,
//! "SCIP cache-hit ingest on SQLite"): a hit rebuilds a graph identical to a
//! forced full ingest of the same tree, rides doc edits while keeping every
//! Function and Type row, misses on a changed SCIP file, refreshes the
//! snapshot on the miss and falls back to a full ingest when the live graph
//! is gone.
//!
//! Run: `cargo test --test sqlite_scip_cache -- --test-threads=1`

use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::{json, Value};

mod common;
use common::{check, doc_linter_bin, seed_rich, unique_tmpdir};

/// A rich fixture root, checked once (the cache miss).
fn seeded(label: &str) -> PathBuf {
    let root = unique_tmpdir(&format!("scc-{label}")).join("corpus");
    std::fs::create_dir_all(&root).unwrap();
    seed_rich(&root);
    check(&root);
    root
}

/// Run the CLI against `root`; the engine follows from the graph file.
fn cli(root: &Path, args: &[&str]) -> (bool, String) {
    let out = Command::new(doc_linter_bin())
        .arg("--root")
        .arg(root)
        .args(args)
        .output()
        .unwrap();
    let text = String::from_utf8_lossy(&out.stdout).replace(root.to_str().unwrap(), "<ROOT>");
    (out.status.success(), text)
}

fn cli_json(root: &Path, args: &[&str]) -> Value {
    let (ok, text) = cli(root, args);
    assert!(ok, "{args:?} failed on {}: {text}", root.display());
    serde_json::from_str(&text).unwrap_or_else(|e| panic!("{args:?}: {e}\n{text}"))
}

/// Sort the arrays whose order is engine-chosen (`collect(DISTINCT ..)`
/// against `json_group_array(DISTINCT ..)`), and drop what is a documented
/// intentional difference:
/// - `dtype` / `type` spellings (`STRING`/`UINT32` against `TEXT`/`INTEGER`);
/// - `EmbedCache` and `*_embedding` columns, which exist only on Kuzu
///   (SQLite keeps vectors in the usearch file);
/// - schema `examples` (Cypher against SQL text) and the legacy raw
///   `tables` listing of `query_schema`.
fn normalise(v: &mut Value) {
    match v {
        Value::Array(a) => a.iter_mut().for_each(normalise),
        Value::Object(o) => {
            for key in ["shared_entities", "shared_docs"] {
                if let Some(Value::Array(a)) = o.get_mut(key) {
                    a.sort_by_key(ToString::to_string);
                }
            }
            // `context_for` neighbours come in scan order on Kuzu (the
            // Cypher has no ORDER BY) and sorted on SQLite. Past the cap
            // (`top`, default 20) which neighbours survive is engine-chosen
            // too, so a capped list compares by its centre and count only.
            if o.contains_key("neighbor_count") {
                if o.get("neighbor_count") == Some(&json!(20)) {
                    o.remove("neighbors");
                } else if let Some(Value::Array(a)) = o.get_mut("neighbors") {
                    a.sort_by_key(ToString::to_string);
                }
            }
            // Column entries: `{name, type|dtype, default, pk}`.
            if o.contains_key("name") {
                for key in ["type", "dtype", "default"] {
                    o.remove(key);
                }
            }
            // Schema payloads: examples are dialect text, `tables` is Kuzu's raw listing.
            if o.contains_key("nodes") || o.contains_key("node_tables") {
                for key in ["examples", "tables"] {
                    o.remove(key);
                }
            }
            for key in ["nodes", "node_tables", "edges", "rel_tables"] {
                if let Some(Value::Array(a)) = o.get_mut(key) {
                    a.retain(|e| {
                        let n = e.get("label").or_else(|| e.get("name"));
                        n.and_then(Value::as_str) != Some("EmbedCache")
                    });
                }
            }
            for key in ["properties", "columns"] {
                if let Some(Value::Array(a)) = o.get_mut(key) {
                    a.retain(|e| {
                        !e.get("name")
                            .and_then(Value::as_str)
                            .is_some_and(|n| n.ends_with("_embedding"))
                    });
                    a.sort_by_key(|e| e.get("name").map(ToString::to_string));
                }
            }
            o.values_mut().for_each(normalise);
        }
        _ => {}
    }
}

// ---- SCIP cache-hit ingest -------------------------------------------------------

/// `check` on `root` with the SQLite engine; returns stderr.
fn check_stderr(root: &Path, extra: &[&str]) -> String {
    let out = Command::new(doc_linter_bin())
        .arg("--root")
        .arg(root)
        .args(["check", "--no-vale"])
        .args(extra)
        .output()
        .unwrap();
    let err = String::from_utf8_lossy(&out.stderr).to_string();
    assert!(!err.contains("index NOT updated"), "check failed:\n{err}");
    err
}

/// Per-table row counts via `query graph-summary`.
fn counts(root: &Path) -> Value {
    let mut v = cli_json(root, &["query", "graph-summary"]);
    normalise(&mut v);
    json!({"nodes": v["nodes"], "edges": v["edges"]})
}

fn names(v: &Value, kind: &str) -> Vec<String> {
    v[kind]
        .as_array()
        .unwrap()
        .iter()
        .filter(|e| e["count"].as_u64().unwrap_or(0) > 0)
        .map(|e| e["label"].as_str().unwrap().to_string())
        .collect()
}

#[test]
fn sqlite_scip_cache_hit_reproduces_the_full_ingest() {
    let s = seeded("cache");
    // `pair` already ran one check (the miss). The reference is a forced
    // full ingest of the tree as it stands now: it differs from the miss
    // only by `docs/sitemap.json`, which the first check wrote and the
    // File walker now sees.
    let err = check_stderr(&s, &["--rebuild"]);
    assert!(!err.contains("cache hit"), "--rebuild must not hit:\n{err}");
    let full = counts(&s);
    assert!(
        names(&full, "nodes").contains(&"Function".to_string()),
        "{full}"
    );
    assert!(
        names(&full, "edges").contains(&"CALLS".to_string()),
        "{full}"
    );
    assert!(s.join(".doc-lint/ingest-cache.sqlite.json").exists());

    let err = check_stderr(&s, &[]);
    assert!(
        err.contains("cache hit"),
        "second check must hit the SCIP cache:\n{err}"
    );
    assert!(!err.contains("scip ingest (sqlite) — ") || err.contains("cache hit"));
    assert_eq!(
        counts(&s),
        full,
        "cache-hit ingest must rebuild the same graph"
    );

    // Saved queries over code edges give the same rows too.
    let sample_queries = [
        "hub-functions",
        "orphan-entities",
        "untested-functions",
        "entity-callers",
    ];
    let before: Vec<Value> = sample_queries
        .iter()
        .map(|q| {
            let (_, t) = cli(&s, &["query", "saved", q, "--param", "entity=checkout"]);
            serde_json::from_str(&t).unwrap_or(Value::String(t))
        })
        .collect();
    let _ = check_stderr(&s, &[]);
    let after: Vec<Value> = sample_queries
        .iter()
        .map(|q| {
            let (_, t) = cli(&s, &["query", "saved", q, "--param", "entity=checkout"]);
            serde_json::from_str(&t).unwrap_or(Value::String(t))
        })
        .collect();
    assert_eq!(before, after);

    // A doc edit rides the hit: vault rows follow the edit, code rows stay.
    common::write(
        &s.join("docs/cache-new.md"),
        "---\nid: cache-new\nrole: doc\ntitle: Cache new\nsummary: added after the index\n\
         status: draft\nupdated: 2026-05-03\ntags: [one]\ncovers: [pricing]\n---\n\n# New\n\nSee [[doc-a]].\n",
    );
    let err = check_stderr(&s, &[]);
    assert!(err.contains("cache hit"), "{err}");
    let edited = counts(&s);
    let docs = |c: &Value| {
        c["nodes"]
            .as_array()
            .unwrap()
            .iter()
            .find(|e| e["label"] == "Doc")
            .unwrap()["count"]
            .as_u64()
            .unwrap()
    };
    assert_eq!(docs(&edited), docs(&full) + 1);
    for kind in ["Function", "Type"] {
        let n = |c: &Value| {
            c["nodes"]
                .as_array()
                .unwrap()
                .iter()
                .find(|e| e["label"] == kind)
                .unwrap()["count"]
                .clone()
        };
        assert_eq!(n(&edited), n(&full), "{kind} rows are preserved on a hit");
    }
    // Reference result: a forced full ingest of the same tree.
    let err = check_stderr(&s, &["--rebuild"]);
    assert!(!err.contains("cache hit"), "--rebuild must not hit:\n{err}");
    assert_eq!(
        counts(&s),
        edited,
        "hit and full ingest of the edited tree agree"
    );

    // A changed SCIP file misses.
    let scip = s.join(".doc-lint/code.scip");
    let mut bytes = std::fs::read(&scip).unwrap();
    // An unknown varint field (number 15) parses as a no-op; the cache is
    // keyed on the bytes, so it must miss and re-parse.
    bytes.extend([0x78, 0x01]);
    std::fs::write(&scip, &bytes).unwrap();
    let err = check_stderr(&s, &[]);
    assert!(
        !err.contains("cache hit"),
        "changed SCIP file must miss:\n{err}"
    );
    let err = check_stderr(&s, &[]);
    assert!(
        err.contains("cache hit"),
        "and the miss refreshes the snapshot:\n{err}"
    );

    // Losing the live graph invalidates the snapshot (counts disagree).
    std::fs::remove_file(s.join(".doc-lint/graph.sqlite")).unwrap();
    let err = check_stderr(&s, &[]);
    assert!(
        !err.contains("cache hit"),
        "an empty staging graph must not trust the cache:\n{err}"
    );
    assert_eq!(counts(&s), edited);
}
