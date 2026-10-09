#![allow(clippy::many_single_char_names)]
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "integration tests panic on broken invariants — that is the point"
)]
//! Read-surface golden (docs/design/store-trait.md "Kuzu-only paths
//! ported"): CLI `query schema|types|similar|graph-summary|sql` and the MCP
//! tools that used to need Cypher (`query_schema`, `query_similar`,
//! `audit_doc_region`, `suggest_tags_for_doc`, `classify_file_coupling`,
//! `context_for`, `list_files|findings|migrations|modules|repos|types`,
//! `query_doc_neighbors`, `query_entity_neighbors`, `sql`) plus the
//! embedding search against the usearch index (fake bag-of-words embedder;
//! the ONNX model is not available in CI), on the fixture and the rich
//! fixture. The goldens in `tests/golden/read_surface_*.tsv` were recorded
//! from the Kuzu engine (the reference before it was removed) with the
//! documented intentional differences removed by [`normalise`];
//! `RECORD_GOLDEN=1` re-records from SQLite: review the diff first.
//!
//! Run: `cargo test --test read_surface_golden -- --test-threads=1`

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

use serde_json::{json, Value};

mod common;
use common::{
    check, doc_linter_bin, read_line_with_timeout, seed_fixture, seed_rich, spawn_line_reader,
    unique_tmpdir, write_line, Snapshot,
};

use doc_linter::graph_read::GraphRead;
use doc_linter::store::StoreRead;
use doc_linter::store_sqlite;

/// A corpus root named `corpus` (so repo names are stable); absolute paths
/// are masked as `<ROOT>`. Checked once.
fn seeded(label: &str, seed: impl Fn(&Path)) -> PathBuf {
    let root = unique_tmpdir(&format!("rsg-{label}")).join("corpus");
    std::fs::create_dir_all(&root).unwrap();
    seed(&root);
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

// ---- MCP over stdio ---------------------------------------------------------------

/// One `mcp` server for `root`; calls `(tool, arguments)` in order and
/// returns the parsed tool payload (or `{"error": ...}`), masked and
/// normalised.
fn mcp(root: &Path, calls: &[(String, Value)]) -> Vec<Value> {
    let mut child = Command::new(doc_linter_bin())
        .arg("--root")
        .arg(root)
        .arg("mcp")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    let lines = spawn_line_reader(child.stdout.take().unwrap());
    write_line(
        &mut stdin,
        r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2026-05-19","capabilities":{},"clientInfo":{"name":"t","version":"0"}}}"#,
    );
    read_line_with_timeout(&lines, Duration::from_secs(60));
    let mut out = Vec::new();
    for (i, (tool, args)) in calls.iter().enumerate() {
        let req = json!({"jsonrpc":"2.0","id":i + 2,"method":"tools/call",
            "params":{"name":tool,"arguments":args}});
        write_line(&mut stdin, &req.to_string());
        let reply = read_line_with_timeout(&lines, Duration::from_secs(120))
            .replace(root.to_str().unwrap(), "<ROOT>");
        let v: Value = serde_json::from_str(&reply).unwrap();
        let mut payload = if let Some(e) = v.get("error") {
            json!({"error": e["message"]})
        } else {
            let text = v["result"]["content"][0]["text"].as_str().unwrap_or("null");
            serde_json::from_str(text).unwrap_or(Value::String(text.to_string()))
        };
        normalise(&mut payload);
        out.push(payload);
    }
    drop(stdin);
    let _ = child.wait();
    out
}

/// Ids to probe, sampled from the SQLite graph.
struct Sample {
    docs: Vec<String>,
    doc_paths: Vec<String>,
    ents: Vec<String>,
    funcs: Vec<String>,
    files: Vec<String>,
}

fn sample(s: &Path) -> Sample {
    let db = store_sqlite::open_ro(s).unwrap();
    let col = |sql: &str| -> Vec<String> {
        let mut v: Vec<String> = db
            .query(sql, &[])
            .unwrap()
            .iter()
            .map(|r| r[0].str_or_empty())
            .collect();
        v.sort();
        v
    };
    Sample {
        docs: col("SELECT id FROM Doc"),
        doc_paths: col("SELECT path FROM Doc"),
        ents: col("SELECT id FROM Entity"),
        funcs: col("SELECT symbol FROM Function"),
        files: col("SELECT path FROM File"),
    }
}

fn spread(v: &[String], n: usize) -> Vec<String> {
    if v.len() <= n {
        return v.to_vec();
    }
    (0..n).map(|i| v[i * v.len() / n].clone()).collect()
}

fn call(tool: &str, args: Value) -> (String, Value) {
    (tool.to_string(), args)
}

fn put(snap: &mut Snapshot, label: &str, v: &Value) {
    snap.add(label, &serde_json::to_string(v).unwrap());
}

fn run_corpus(label: &str, seed: impl Fn(&Path)) {
    let s = seeded(label, seed);
    let sm = sample(&s);
    let root = &s;
    let mut snap = Snapshot::new(&format!("read_surface_{label}"));

    // ---- CLI --------------------------------------------------------------------
    for args in [
        vec!["query", "types"],
        vec!["query", "types", "--kind", "struct"],
        vec!["query", "types", "--crate", "a"],
        vec!["query", "types", "--substring", "i"],
        vec!["query", "types", "--top", "1"],
    ] {
        put(
            &mut snap,
            &format!("cli-types {args:?}"),
            &cli_json(root, &args),
        );
    }
    for ty in ["doc", "function", "entity", "section", "type", "all"] {
        for text in [
            "pricing rules",
            "checkout totals compute",
            "cache",
            "doc",
            "xyzzy",
        ] {
            for extra in [vec![], vec!["--min-score", "0.5"], vec!["--top", "2"]] {
                let mut args = vec!["query", "similar", text, "--type", ty];
                args.extend(extra);
                put(
                    &mut snap,
                    &format!("cli-similar {args:?}"),
                    &cli_json(root, &args),
                );
            }
        }
    }
    // Tag filters ride the doc and section axes.
    for args in [
        vec![
            "query",
            "similar",
            "pricing",
            "--type",
            "doc",
            "--with-tag",
            "one",
        ],
        vec![
            "query",
            "similar",
            "pricing",
            "--type",
            "doc",
            "--without-tag",
            "one",
        ],
        vec![
            "query",
            "similar",
            "pricing",
            "--type",
            "section",
            "--with-tag",
            "two",
        ],
        vec![
            "query",
            "similar",
            "pricing",
            "--type",
            "all",
            "--without-tag",
            "two",
        ],
    ] {
        put(
            &mut snap,
            &format!("cli-similar-filter {args:?}"),
            &cli_json(root, &args),
        );
    }
    // graph-summary and schema: same tables and counts, minus the
    // Kuzu-only EmbedCache / *_embedding columns (normalise drops them).
    let mut gs = cli_json(root, &["query", "graph-summary"]);
    normalise(&mut gs);
    put(&mut snap, "cli-graph-summary nodes", &gs["nodes"]);
    put(&mut snap, "cli-graph-summary edges", &gs["edges"]);
    put(&mut snap, "cli-graph-summary totals", &gs["total_edges"]);
    let mut sc = cli_json(root, &["query", "schema"]);
    normalise(&mut sc);
    put(&mut snap, "cli-schema nodes", &sc["nodes"]);
    put(&mut snap, "cli-schema edges", &sc["edges"]);

    // Raw SQL (REGEXP matches the whole string, like the old `=~`).
    let twins: &[&str] = &[
        "SELECT count(*) AS n FROM Doc",
        "SELECT id, role FROM Doc ORDER BY id",
        "SELECT id FROM Doc WHERE id REGEXP 'doc-.*' ORDER BY id",
        "SELECT id FROM Doc WHERE id REGEXP 'doc' ORDER BY id",
        "SELECT id FROM Doc WHERE title REGEXP '(?i)al.*' ORDER BY id",
        "SELECT symbol AS s FROM Function WHERE symbol REGEXP '.*compute.*' ORDER BY s",
        "SELECT src AS doc, dst AS ent FROM COVERS ORDER BY doc, ent",
        "SELECT path AS p FROM File WHERE path REGEXP '.*\\.rs' ORDER BY p",
    ];
    for sq in twins {
        put(
            &mut snap,
            &format!("cli-sql-twin {sq}"),
            &cli_json(root, &["query", "sql", sq]),
        );
    }
    {
        // `query sql` is read-only.
        for bad in [
            "DELETE FROM Doc",
            "INSERT INTO Doc(id) VALUES ('x')",
            "WITH x AS (SELECT 1) DELETE FROM Doc",
            "UPDATE Doc SET title = 'x'",
            "DROP TABLE Doc",
            "PRAGMA journal_mode = DELETE",
            "SELECT 1; DELETE FROM Doc",
            "ATTACH DATABASE ':memory:' AS m",
            "",
        ] {
            let (ok, _) = cli(&s, &["query", "sql", bad]);
            assert!(!ok, "query sql must refuse `{bad}`");
        }
        assert_eq!(
            cli_json(&s, &["query", "sql", "SELECT count(*) AS n FROM Doc"])["row_count"],
            1,
            "the read-only guard must not have damaged the graph"
        );
    }

    // ---- MCP --------------------------------------------------------------------
    let repo_name = "corpus";
    let docs = spread(&sm.docs, 6);
    let ents = spread(&sm.ents, 4);
    let funcs = spread(&sm.funcs, 3);
    let mut calls: Vec<(String, Value)> = vec![
        call("query_schema", json!({})),
        call("list_types", json!({})),
        call("list_types", json!({"kind": "struct"})),
        call("list_types", json!({"substring": "i"})),
        call("list_types", json!({"crate": "a"})),
        call("list_findings", json!({})),
        call("list_findings", json!({"severity": "warning"})),
        call("list_findings", json!({"kind": "todo"})),
        call("list_migrations", json!({})),
        call("list_migrations", json!({"since": 1})),
        call("list_modules", json!({})),
        call("list_modules", json!({"kind": "crate"})),
        call("list_modules", json!({"substring": "a"})),
        call("list_files", json!({})),
        call("list_files", json!({"language": "rust"})),
        call("list_files", json!({"substring": "lib"})),
        call("list_repos", json!({})),
        call("list_files", json!({"top": 2, "offset": 1})),
    ];
    for ty in ["doc", "function", "entity", "section", "type", "all"] {
        for text in ["pricing rules", "checkout totals compute", "cache"] {
            calls.push(call(
                "query_similar",
                json!({"text": text, "type": ty, "top": 4}),
            ));
        }
    }
    calls.push(call(
        "query_similar",
        json!({"text": "pricing", "type": "doc", "source_repo": repo_name}),
    ));
    calls.push(call(
        "query_similar",
        json!({"text": "pricing", "type": "doc", "with_tag": "two"}),
    ));
    for p in spread(&sm.doc_paths, 4) {
        calls.push(call(
            "audit_doc_region",
            json!({"doc_path": p, "against": repo_name}),
        ));
    }
    calls.push(call(
        "audit_doc_region",
        json!({"doc_path": "docs/a.md", "against": "no-such-repo"}),
    ));
    calls.push(call(
        "audit_doc_region",
        json!({"doc_path": "no/such.md", "against": repo_name}),
    ));
    for id in &spread(&sm.docs, 30) {
        calls.push(call("suggest_tags_for_doc", json!({"doc_id": id})));
    }
    calls.push(call(
        "suggest_tags_for_doc",
        json!({"doc_id": "no-such-doc"}),
    ));
    for f in spread(&sm.files, 8) {
        calls.push(call("classify_file_coupling", json!({"path": f})));
    }
    for f in [
        "widget.py",
        "crates/a/src/lib.rs",
        "crates/b/src/lib.rs",
        "no/such.rs",
    ] {
        calls.push(call("classify_file_coupling", json!({"path": f})));
    }
    for id in docs.iter().chain(&ents).chain(&funcs) {
        calls.push(call("context_for", json!({"id": id})));
    }
    calls.push(call("context_for", json!({"id": "no-such-id"})));
    for id in &spread(&sm.docs, 16) {
        for by in ["covers", "wikilink", "all"] {
            calls.push(call("query_doc_neighbors", json!({"id": id, "by": by})));
        }
    }
    for id in &spread(&sm.ents, 16) {
        for by in ["co-cover", "related"] {
            calls.push(call("query_entity_neighbors", json!({"id": id, "by": by})));
        }
    }
    let n_common = calls.len();
    for sq in twins {
        calls.push(call("sql", json!({"query": sq})));
    }
    let res = mcp(root, &calls);
    for (i, v) in res.iter().enumerate() {
        let label = if i < n_common {
            format!("mcp-{} {}", calls[i].0, calls[i].1)
        } else {
            format!("mcp-twin {}", twins[i - n_common])
        };
        put(&mut snap, &label, v);
    }
    {
        // Read-only SQL over MCP.
        let guard = mcp(
            &s,
            &[
                call("sql", json!({"query": "DELETE FROM Doc"})),
                call("sql", json!({"query": "SELECT count(*) AS n FROM Doc"})),
            ],
        );
        assert!(
            guard[0]["error"].as_str().unwrap().contains("read-only"),
            "{guard:?}"
        );
        assert!(guard[1]["row_count"] == 1, "{guard:?}");
    }

    embedding_hits(&mut snap, &s);

    assert!(
        snap.nonempty() > 100,
        "{label}: vacuous snapshot ({} non-empty)",
        snap.nonempty()
    );
    snap.finish();
    let _ = std::fs::remove_dir_all(s.parent().unwrap());
}

// ---- embedding backend against the usearch index ------------------------------------

use doc_linter::embeddings::{Embedder, ONNX_BACKEND_NAME};
use doc_linter::query::{embedding_rank, DocFilters};
use doc_linter::store_sqlite::vector::UsearchIndex;

/// Bag-of-words hashing into 384 slots, L2-normalised: graded cosine
/// scores without a model.
struct BagOfWords;
impl Embedder for BagOfWords {
    fn embed(&self, text: &str) -> anyhow::Result<Vec<f32>> {
        let mut v = vec![0f32; 384];
        for tok in text
            .to_lowercase()
            .split(|c: char| !c.is_alphanumeric())
            .filter(|t| !t.is_empty())
        {
            let h = tok
                .bytes()
                .fold(5381usize, |a, b| a.wrapping_mul(33) ^ usize::from(b));
            v[h % 384] += 1.0;
        }
        let n = v.iter().map(|x| x * x).sum::<f32>().sqrt();
        if n > 0.0 {
            v.iter_mut().for_each(|x| *x /= n);
        }
        Ok(v)
    }
    fn dim(&self) -> usize {
        384
    }
    fn name(&self) -> &'static str {
        ONNX_BACKEND_NAME
    }
}

/// Embedding search over every corpus kind crossed with the repo, tag
/// filters: the hits above the zero-similarity floor (scores to 3 decimals)
/// and the corpus size. Hits tied at the floor have an engine-chosen order.
fn embedding_hits(snap: &mut Snapshot, s: &Path) {
    store_sqlite::build_and_swap(s, |db| {
        store_sqlite::vectors::populate(db, Some(&BagOfWords), UsearchIndex::new)?;
        Ok(())
    })
    .unwrap();
    let sdb = store_sqlite::open_ro(s).unwrap();
    let g: &dyn GraphRead = &sdb;
    let repo = sdb
        .query("SELECT id FROM Repo", &[])
        .unwrap()
        .first()
        .map(|r| r[0].str_or_empty())
        .unwrap_or_default();
    // Repo ids are absolute paths; mask them like everything else.
    let mask = |t: String| {
        t.replace(&repo, "<REPO>")
            .replace(s.to_str().unwrap(), "<ROOT>")
    };
    let filters = [
        DocFilters::default(),
        DocFilters {
            source_repo: Some("nope"),
            ..Default::default()
        },
        DocFilters {
            with_tag: Some("two"),
            ..Default::default()
        },
        DocFilters {
            without_tag: Some("one"),
            ..Default::default()
        },
    ];
    for kind in ["doc", "function", "entity", "section", "type", "all"] {
        for text in [
            "pricing rules overview",
            "checkout flow",
            "compute totals",
            "alpha",
            "zzz",
        ] {
            let q = BagOfWords.embed(text).unwrap();
            for (fi, f) in filters.iter().enumerate() {
                for repo_filter in [false, true] {
                    let mut f = *f;
                    if repo_filter {
                        f.source_repo = Some(&repo);
                    }
                    if kind == "type" && f.source_repo.is_some() {
                        continue;
                    }
                    let label = format!("embedding {kind} {text:?} filter#{fi} repo={repo_filter}");
                    let text = match embedding_rank(g, kind, &q, 5, f) {
                        Ok((hits, n)) => {
                            let mut strong: Vec<_> = hits
                                .iter()
                                .filter(|(_, sc)| *sc > 0.05)
                                .map(|(i, sc)| format!("{i}={}", (sc * 1000.0).round() as i64))
                                .collect();
                            strong.sort();
                            format!("{n} {}", strong.join(" "))
                        }
                        Err(_) => "ERR".to_string(),
                    };
                    snap.add(&label, &mask(text));
                }
            }
        }
    }
}

#[test]
fn fixture_read_surface_matches() {
    run_corpus("fixture", seed_fixture);
}

#[test]
fn rich_read_surface_matches() {
    run_corpus("rich", seed_rich);
}
