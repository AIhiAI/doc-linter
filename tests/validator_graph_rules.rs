#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "integration tests panic on broken invariants — that is the point"
)]
//! Regression tests for the G5 + G7 graph-level linter rules:
//!
//!   - `self-loop` — any outbound edge whose source equals its target
//!     is a copy-paste mistake.
//!   - `superseded-without-successor` — a doc declaring
//!     `lifecycle: superseded` must have at least one inbound
//!     `supersedes:` edge from the successor doc.
//!
//! Both rules ride on the petgraph the lint pass already builds for
//! orphan detection; the seed here is just enough ontology to validate
//! `role: doc` plus the three test docs that exercise the rules.

use std::path::Path;
use std::process::Command;

mod common;
use common::{doc_linter_bin, unique_tmpdir, write};

fn seed_ontology(root: &Path) {
    write(
        &root.join(".doc-lint.toml"),
        r#"
exempt = []
required_fields = ["id", "role", "title", "summary", "status", "updated"]
allowed_statuses = ["draft", "stable"]
vale_enabled = false
"#,
    );
    write(
        &root.join("docs/ontology/axes/role.md"),
        "---\n\
         id: axis-role\n\
         role: ontology-axis\n\
         title: \"Axis: role\"\n\
         summary: t\n\
         status: stable\n\
         updated: 2026-04-30\n\
         axis_id: role\n\
         ---\n\n# axis\n",
    );
    write(
        &root.join("docs/ontology/values/role/doc.md"),
        "---\n\
         id: value-role-doc\n\
         role: ontology-value\n\
         title: \"Role: doc\"\n\
         summary: t\n\
         status: stable\n\
         updated: 2026-04-30\n\
         axis_id: role\n\
         value_id: doc\n\
         display: doc\n\
         description: t\n\
         ---\n\n# doc\n",
    );
    write(
        &root.join("docs/ontology/values/kind/reference.md"),
        "---\n\
         id: value-kind-reference\n\
         role: ontology-value\n\
         title: \"Kind: reference\"\n\
         summary: t\n\
         status: stable\n\
         updated: 2026-04-30\n\
         axis_id: kind\n\
         value_id: reference\n\
         display: reference\n\
         description: t\n\
         ---\n\n# kind\n",
    );
    write(
        &root.join("docs/ontology/values/lifecycle/superseded.md"),
        "---\n\
         id: value-lifecycle-superseded\n\
         role: ontology-value\n\
         title: \"Lifecycle: superseded\"\n\
         summary: t\n\
         status: stable\n\
         updated: 2026-04-30\n\
         axis_id: lifecycle\n\
         value_id: superseded\n\
         display: superseded\n\
         description: t\n\
         ---\n\n# superseded\n",
    );
    write(
        &root.join("docs/ontology/values/lifecycle/stable.md"),
        "---\n\
         id: value-lifecycle-stable\n\
         role: ontology-value\n\
         title: \"Lifecycle: stable\"\n\
         summary: t\n\
         status: stable\n\
         updated: 2026-04-30\n\
         axis_id: lifecycle\n\
         value_id: stable\n\
         display: stable\n\
         description: t\n\
         ---\n\n# stable\n",
    );
}

fn run_check_json(root: &Path) -> serde_json::Value {
    let out = Command::new(doc_linter_bin())
        .arg("--root")
        .arg(root)
        .args(["check", "--no-vale", "--format", "json"])
        .output()
        .expect("spawn doc-linter");
    let stdout = String::from_utf8_lossy(&out.stdout);
    serde_json::from_str(&stdout)
        .unwrap_or_else(|e| panic!("failed to parse JSON: {e}\nstdout:\n{stdout}"))
}

fn codes_for(report: &serde_json::Value, file_suffix: &str) -> Vec<String> {
    report["report"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|e| e["file"].as_str().unwrap().ends_with(file_suffix))
        .flat_map(|e| {
            e["codes"]
                .as_array()
                .unwrap()
                .iter()
                .map(|c| c.as_str().unwrap().to_string())
        })
        .collect()
}

/// Asserts that the [[entity-doc-graph]] validator flags a doc whose
/// own wikilink resolves back to itself.
#[test]
fn self_loop_wikilink_is_flagged() {
    let root = unique_tmpdir("self-loop");
    seed_ontology(&root);

    // A doc whose body wikilinks itself by id. The graph builder
    // resolves the link to this same node — that's the self-loop.
    write(
        &root.join("docs/loop.md"),
        "---\n\
         id: doc-loop\n\
         role: doc\n\
         kind: reference\n\
         lifecycle: stable\n\
         title: Loop\n\
         summary: loops\n\
         status: stable\n\
         updated: 2026-04-30\n\
         ---\n\n# Loop\n\nSee [[doc-loop]] for context.\n",
    );

    let report = run_check_json(&root);
    let codes = codes_for(&report, "docs/loop.md");
    assert!(
        codes.contains(&"self-loop".to_string()),
        "expected self-loop diagnostic, got: {codes:?}"
    );
}

#[test]
/// Asserts the [[entity-doc-graph]] validator flags a doc with
/// `lifecycle: superseded` but no `supersedes:` successor declared.
fn superseded_without_successor_is_flagged() {
    let root = unique_tmpdir("superseded-bare");
    seed_ontology(&root);

    write(
        &root.join("docs/old.md"),
        "---\n\
         id: doc-old\n\
         role: doc\n\
         kind: reference\n\
         lifecycle: superseded\n\
         title: Old\n\
         summary: old\n\
         status: stable\n\
         updated: 2026-04-30\n\
         ---\n\n# Old\n\nNothing inbound.\n",
    );

    let report = run_check_json(&root);
    let codes = codes_for(&report, "docs/old.md");
    assert!(
        codes.contains(&"superseded-without-successor".to_string()),
        "expected superseded-without-successor, got: {codes:?}"
    );
}

#[test]
/// Asserts the negative case — a `superseded` doc with a declared
/// successor passes [[entity-doc-graph]] validation cleanly.
fn superseded_with_declared_successor_is_clean() {
    let root = unique_tmpdir("superseded-ok");
    seed_ontology(&root);

    write(
        &root.join("docs/old.md"),
        "---\n\
         id: doc-old\n\
         role: doc\n\
         kind: reference\n\
         lifecycle: superseded\n\
         title: Old\n\
         summary: old\n\
         status: stable\n\
         updated: 2026-04-30\n\
         ---\n\n# Old\n",
    );
    write(
        &root.join("docs/new.md"),
        "---\n\
         id: doc-new\n\
         role: doc\n\
         kind: reference\n\
         lifecycle: stable\n\
         title: New\n\
         summary: new\n\
         status: stable\n\
         updated: 2026-04-30\n\
         supersedes:\n\
           - doc-old\n\
         ---\n\n# New\n",
    );

    let report = run_check_json(&root);
    let codes = codes_for(&report, "docs/old.md");
    assert!(
        !codes.contains(&"superseded-without-successor".to_string()),
        "expected no superseded-without-successor on doc-old (successor declared), got: {codes:?}"
    );
}

/// Entities take the `bounded_contexts:` list and docs the
/// `bounded_context:` string. The other spelling used to parse and then be
/// ignored, silently dropping the context.
#[test]
fn wrong_bounded_context_field_spelling_is_flagged() {
    let root = unique_tmpdir("bc-field");
    seed_ontology(&root);
    write(
        &root.join("docs/plural.md"),
        "---\n\
         id: doc-plural\n\
         role: doc\n\
         kind: reference\n\
         lifecycle: stable\n\
         title: Plural\n\
         summary: p\n\
         status: stable\n\
         updated: 2026-04-30\n\
         bounded_contexts: [pricing]\n\
         ---\n\n# Plural\n",
    );
    write(
        &root.join("docs/ontology/entities/singular.md"),
        "---\n\
         id: entity-singular\n\
         role: ontology-entity\n\
         title: \"Entity: Singular\"\n\
         summary: s\n\
         status: stable\n\
         updated: 2026-04-30\n\
         axis_id: covers\n\
         value_id: singular\n\
         display: Singular\n\
         description: s\n\
         bounded_context: pricing\n\
         ---\n\n# Singular\n",
    );
    let report = run_check_json(&root);
    for file in ["docs/plural.md", "entities/singular.md"] {
        let codes = codes_for(&report, file);
        assert!(
            codes.contains(&"wrong-bounded-context-field".to_string()),
            "{file}: expected wrong-bounded-context-field, got: {codes:?}"
        );
    }
}

/// `include` selects the docs: a file outside it isn't linted at all, and
/// `include = []` means no docs (a code-only graph).
#[test]
fn include_globs_select_the_docs() {
    let root = unique_tmpdir("include");
    seed_ontology(&root);
    write(&root.join("notes/scratch.md"), "no frontmatter here\n");
    let cfg = root.join(".doc-lint.toml");
    let base = std::fs::read_to_string(&cfg).unwrap();

    std::fs::write(&cfg, format!("include = [\"docs/**/*.md\"]\n{base}")).unwrap();
    let report = run_check_json(&root);
    assert!(
        codes_for(&report, "notes/scratch.md").is_empty(),
        "file outside include was linted: {report}"
    );

    std::fs::write(&cfg, format!("include = []\n{base}")).unwrap();
    let report = run_check_json(&root);
    assert!(
        report["report"].as_array().unwrap().is_empty(),
        "include = [] still linted docs: {report}"
    );

    std::fs::write(&cfg, &base).unwrap();
    let report = run_check_json(&root);
    assert!(codes_for(&report, "notes/scratch.md").contains(&"missing-frontmatter".to_string()));
}

/// A markdown doc without frontmatter is still flagged, but it now lands
/// in the graph, so search finds it.
#[test]
fn markdown_without_frontmatter_is_searchable() {
    let root = unique_tmpdir("md-no-fm");
    seed_ontology(&root);
    write(
        &root.join("docs/guide/refunds.md"),
        "# Refunds\n\nA refund returns the subtotal plus tax after cancellation.\n",
    );
    let report = run_check_json(&root);
    assert!(
        codes_for(&report, "docs/guide/refunds.md").contains(&"missing-frontmatter".to_string())
    );
    let out = Command::new(doc_linter_bin())
        .arg("--root")
        .arg(&root)
        .args(["query", "similar", "refund cancellation"])
        .output()
        .expect("spawn doc-linter");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("docs-guide-refunds"),
        "frontmatter-less doc not in search:\n{stdout}"
    );
}
