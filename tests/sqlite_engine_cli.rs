#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "integration tests panic on broken invariants — that is the point"
)]
//! Read-side commands on the SQLite graph (docs/design/store-trait.md,
//! plan item B7 step 4): after `check`, `query`, `explain`,
//! `scaffold-coverage`, `cluster` and `mcp` read the graph file and must
//! print what they printed on the SQLite graph (goldens recorded from the Kuzu
//! engine before it was removed; `RECORD_GOLDEN=1` re-records from SQLite,
//! review the diff first); the post-ingest `check` steps (coverage lint,
//! cluster lint, sitemap) must produce the same diagnostics and
//! `sitemap.json`.
//!
//! Run: `cargo test --test sqlite_engine_cli -- --test-threads=1`

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

mod common;
use common::{
    check, doc_linter_bin, read_line_with_timeout, seed_rich, spawn_line_reader, unique_tmpdir,
    write_line,
};

const SYM: &str = "rust-analyzer cargo b 0.1.0 compute().";

/// A rich fixture root, checked once.
fn seeded(label: &str) -> PathBuf {
    seeded_with(label, "")
}

/// `extra` is appended to `.doc-lint.toml` (tables last).
fn seeded_with(label: &str, extra: &str) -> PathBuf {
    let s = unique_tmpdir(&format!("sec-{label}"));
    seed_rich(&s);
    if !extra.is_empty() {
        let cfg = s.join(".doc-lint.toml");
        let body = std::fs::read_to_string(&cfg).unwrap();
        std::fs::write(&cfg, format!("{body}\n{extra}")).unwrap();
    }
    check(&s);
    assert!(s.join(".doc-lint/graph.sqlite").exists());
    s
}

fn run(root: &Path, args: &[&str]) -> (i32, String) {
    let out = Command::new(doc_linter_bin())
        .arg("--root")
        .arg(root)
        .args(args)
        .output()
        .unwrap();
    let text = String::from_utf8_lossy(&out.stdout).replace(root.to_str().unwrap(), "<ROOT>");
    (out.status.code().unwrap_or(-1), text)
}

#[test]
fn read_commands_match_golden() {
    let s = seeded("read");
    let cases: Vec<Vec<&str>> = vec![
        vec!["query", "list"],
        vec!["query", "backlinks", "doc-a"],
        vec!["query", "neighbors", "doc-a"],
        vec!["query", "context", "doc-a"],
        vec!["query", "path", "doc-a", "r1"],
        vec!["query", "subgraph", "doc-a"],
        vec!["query", "functions-mentioning", "pricing"],
        vec!["query", "function-context", SYM],
        vec!["query", "coverage-report"],
        vec!["query", "endpoints"],
        vec!["query", "at", "crates/b/src/lib.rs:4"],
        vec!["query", "impact", SYM],
        vec!["query", "dead-code"],
        vec!["query", "saved", "orphan-entities"],
        vec![
            "query",
            "saved",
            "entity-callers",
            "--param",
            "entity=checkout",
        ],
        vec!["query", "saved", "--list"],
        vec!["query", "map", "--scope", "entity", "--name", "pricing"],
        vec!["explain", "compute"],
        vec!["explain", "compute", "--json"],
        vec!["scaffold-coverage", "a"],
        vec!["cluster"],
    ];
    let mut report = String::new();
    let mut nonempty = 0;
    for args in &cases {
        let (code, out) = run(&s, args);
        // The catalog listing is ~300 KB of descriptions: keep the names.
        let out = if args.contains(&"--list") {
            let v: serde_json::Value = serde_json::from_str(&out).unwrap();
            let names: Vec<&str> = v["queries"]
                .as_array()
                .unwrap()
                .iter()
                .map(|q| q["name"].as_str().unwrap())
                .collect();
            format!("{} queries: {}", names.len(), names.join(" "))
        } else {
            out
        };
        report += &format!("### {args:?}\nexit {code}\n{out}\n");
        nonempty += usize::from(out.lines().count() > 3);
    }
    golden("read_commands.txt", &report);
    assert!(
        nonempty > 15,
        "vacuous: only {nonempty} commands printed rows"
    );
}

/// Compare `got` with `tests/golden/<name>`, or rewrite it under `RECORD_GOLDEN`.
fn golden(name: &str, got: &str) {
    // File mtimes differ per run; everything else is deterministic.
    let re = regex::Regex::new(r#""last_touched": "[^"]*""#).unwrap();
    let got = re.replace_all(got, r#""last_touched": "<T>""#);
    let got = got.as_ref();
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/golden")
        .join(name);
    if std::env::var_os("RECORD_GOLDEN").is_some() {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, got).unwrap();
        return;
    }
    let want = std::fs::read_to_string(&path)
        .unwrap_or_else(|_| panic!("missing golden {name}; run with RECORD_GOLDEN=1"));
    if got != want {
        let got_path = std::env::temp_dir().join(format!("{name}.got"));
        std::fs::write(&got_path, got).unwrap();
        panic!(
            "{name} differs from the golden; diff {} {}",
            path.display(),
            got_path.display()
        );
    }
}

#[test]
fn check_steps_match_golden() {
    // Make the coverage and cluster lints fire on the fixture.
    let s = seeded_with(
        "check",
        "anchor_required_in = [\"crates/a\", \"crates/b\"]\n\
         coverage_min_per_entity_doc_ratio = 5.0\n\
         [cluster_lint]\nmin_members = 2\nmin_density = 0.0\n",
    );
    // A second full `check` prints the lint report (human format lists the
    // coverage-lint diagnostics under the repo root).
    let (code, report) = run(&s, &["check", "--no-vale", "--format", "json"]);
    assert!(
        report.contains("dark-public-function") || report.contains("entity-coverage-gap"),
        "no coverage diagnostics in the report; the comparison is vacuous:\n{report}"
    );
    golden("check_report.json", &format!("exit {code}\n{report}"));
    golden(
        "sitemap.json",
        &std::fs::read_to_string(s.join("docs/sitemap.json")).unwrap(),
    );
}

#[cfg(unix)]
#[test]
fn mcp_serves_and_reingests_a_sqlite_graph() {
    let s = seeded("mcp");
    let id_before = std::fs::metadata(s.join(".doc-lint/graph.sqlite"))
        .map(|m| std::os::unix::fs::MetadataExt::ino(&m))
        .unwrap();
    let mut child = Command::new(doc_linter_bin())
        .arg("--root")
        .arg(&s)
        .arg("mcp")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    let lines = spawn_line_reader(child.stdout.take().unwrap());
    let mut call = |id: u32, body: &str| {
        write_line(
            &mut stdin,
            &format!(r#"{{"jsonrpc":"2.0","id":{id},{body}}}"#),
        );
        read_line_with_timeout(&lines, Duration::from_secs(30))
    };
    call(
        1,
        r#""method":"initialize","params":{"protocolVersion":"2026-05-19","capabilities":{},"clientInfo":{"name":"t","version":"0"}}"#,
    );
    let tool = |name: &str, args: &str| {
        format!(r#""method":"tools/call","params":{{"name":"{name}","arguments":{args}}}"#)
    };
    let docs = call(2, &tool("list_docs", "{}"));
    assert!(docs.contains("doc-a"), "{docs}");
    let saved = call(
        3,
        &tool(
            "query_saved",
            r#"{"name":"entity-callers","params":{"entity":"checkout"}}"#,
        ),
    );
    assert!(saved.contains("row_count"), "{saved}");
    let rank = call(4, &tool("list_entities", "{}"));
    assert!(rank.contains("pricing"), "{rank}");
    let cy = call(
        5,
        &tool("cypher", r#"{"query":"MATCH (n:Doc) RETURN n.id"}"#),
    );
    assert!(
        cy.contains("unknown tool"),
        "the cypher tool is gone with Kuzu: {cy}"
    );
    let re = call(6, &tool("reingest", r#"{"no_vale":true}"#));
    assert!(
        re.contains("files_scanned") || re.contains("result"),
        "{re}"
    );
    let id_after = std::fs::metadata(s.join(".doc-lint/graph.sqlite"))
        .map(|m| std::os::unix::fs::MetadataExt::ino(&m))
        .unwrap();
    assert_ne!(id_before, id_after, "reingest swaps in a new graph file");
    let docs = call(7, &tool("list_docs", "{}"));
    assert!(docs.contains("doc-a"), "after reingest: {docs}");
    drop(stdin);
    let _ = child.wait();
}
