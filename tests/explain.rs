#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "integration tests panic on broken invariants — that is the point"
)]
//! Phase 4 of roadmap-43: end-to-end tests for the
//! `doc-linter explain <SYMBOL>` subcommand.
//!
//! Each test seeds a tmp corpus (markdown ontology + a synthesized
//! SCIP file with named functions and doc-comments mentioning known
//! entities), then invokes `explain` via the public binary and
//! asserts on the output shape.

use std::path::Path;

mod common;
use common::{run_doc_linter, unique_tmpdir, write};

fn seed_ontology(root: &Path, extra_entities: &[&str]) {
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
    for v in &["doc", "index"] {
        write(
            &root.join(format!("docs/ontology/values/role/{v}.md")),
            &format!(
                "---\n\
                 id: value-role-{v}\n\
                 role: ontology-value\n\
                 title: \"Role: {v}\"\n\
                 summary: t\n\
                 status: stable\n\
                 updated: 2026-04-30\n\
                 axis_id: role\n\
                 value_id: {v}\n\
                 display: {v}\n\
                 description: t\n\
                 ---\n\n# {v}\n"
            ),
        );
    }
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
    for ent in extra_entities {
        write(
            &root.join(format!("docs/ontology/entities/{ent}.md")),
            &format!(
                "---\n\
                 id: entity-{ent}\n\
                 role: ontology-entity\n\
                 title: \"Entity: {ent}\"\n\
                 summary: \"summary of {ent}\"\n\
                 status: stable\n\
                 updated: 2026-04-30\n\
                 axis_id: covers\n\
                 value_id: {ent}\n\
                 display: {ent}\n\
                 description: \"the {ent} entity, central to the domain\"\n\
                 ---\n\n# {ent}\n"
            ),
        );
    }
}

fn seed_corpus(root: &Path, extra_entities: &[&str]) {
    let config =
        "required_fields = [\"id\", \"role\", \"title\", \"summary\", \"status\", \"updated\"]\n\
                  allowed_statuses = [\"draft\", \"stable\"]\n\
                  vale_enabled = false\n";
    write(&root.join(".doc-lint.toml"), config);
    seed_ontology(root, extra_entities);
}

/// Seed a how-to doc that COVERS the given [[entity-doc-graph]] entities.
/// Used to ensure the narrative-doc section of the explain output is
/// populated.
fn seed_howto_doc(root: &Path, slug: &str, covers: &[&str]) {
    let covers_yaml = covers
        .iter()
        .map(|c| format!("  - {c}"))
        .collect::<Vec<_>>()
        .join("\n");
    write(
        &root.join(format!("docs/how-to/{slug}.md")),
        &format!(
            "---\n\
             id: {slug}\n\
             role: doc\n\
             kind: how-to\n\
             title: \"How to {slug}\"\n\
             summary: a short summary\n\
             status: stable\n\
             updated: 2026-04-30\n\
             covers:\n{covers_yaml}\n\
             ---\n\n# {slug}\n"
        ),
    );
}

fn write_synthetic_scip(out: &Path, funcs: &[(&str, Vec<&str>)]) {
    use scip::types::symbol_information::Kind as ScipKind;
    use scip::types::{Document, Index, Occurrence, SymbolInformation};

    if let Some(p) = out.parent() {
        std::fs::create_dir_all(p).unwrap();
    }
    let mut index = Index::default();
    let mut doc = Document::default();
    doc.relative_path = "crates/my-crate/src/lib.rs".to_string();
    doc.language = "rust".to_string();
    for (symbol, doc_lines) in funcs {
        let mut sym = SymbolInformation::default();
        sym.symbol = (*symbol).to_string();
        sym.documentation = doc_lines.iter().map(|s| (*s).to_string()).collect();
        sym.kind = ScipKind::Function.into();
        doc.symbols.push(sym);
        let mut occ = Occurrence::default();
        occ.symbol = (*symbol).to_string();
        occ.range = vec![0, 0, 0, 1];
        occ.symbol_roles = 1;
        doc.occurrences.push(occ);
    }
    index.documents.push(doc);
    let bytes = protobuf::Message::write_to_bytes(&index).expect("encode");
    std::fs::write(out, bytes).expect("write scip");
}

// ---------------- Tests ---------------------------------------------

/// Asserts that the [[entity-doc-graph]] `explain` command renders the
/// full symbol-to-prose walk when the supplied symbol resolves uniquely.
#[test]
fn explain_unique_symbol_emits_full_walk() {
    let root = unique_tmpdir("unique-walk");
    seed_corpus(&root, &["pricing-rule", "outlet"]);
    seed_howto_doc(&root, "how-to-resolve-prices", &["pricing-rule"]);
    write_synthetic_scip(
        &root.join(".doc-lint/code.scip"),
        &[
            (
                "rust-analyzer cargo my-crate 0.1.0 src/lib.rs/resolve_price().",
                vec!["Resolve a pricing-rule for an outlet."],
            ),
            (
                "rust-analyzer cargo my-crate 0.1.0 src/lib.rs/apply_rules().",
                vec!["Walk all pricing-rule entries and apply them to the outlet."],
            ),
        ],
    );
    let _ = run_doc_linter(&root, &["check", "--no-vale"]);

    let out = run_doc_linter(&root, &["explain", "resolve_price"]);
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    let stderr = String::from_utf8_lossy(&out.stderr).to_string();
    assert!(
        out.status.success(),
        "exit nonzero. stderr={stderr}\nstdout={stdout}"
    );
    assert!(stdout.contains("== Function =="));
    assert!(stdout.contains("== Entities (from FUNCTION_MENTIONS) =="));
    assert!(stdout.contains("== Narrative docs covering these entities =="));
    assert!(stdout.contains("== Sibling functions in my-crate"));
    assert!(
        stdout.contains("entity-pricing-rule"),
        "expected entity-pricing-rule; got\n{stdout}"
    );
    assert!(
        stdout.contains("how-to-resolve-prices"),
        "expected the how-to doc; got\n{stdout}"
    );
    assert!(
        stdout.contains("apply_rules"),
        "expected sibling apply_rules; got\n{stdout}"
    );
}

/// Asserts that the [[entity-doc-graph]] `explain` command lists every
/// matching candidate when the supplied symbol is ambiguous instead of
/// falling back to a guess.
#[test]
fn explain_ambiguous_symbol_lists_candidates() {
    let root = unique_tmpdir("ambiguous");
    seed_corpus(&root, &["outlet"]);
    write_synthetic_scip(
        &root.join(".doc-lint/code.scip"),
        &[
            (
                "rust-analyzer cargo my-crate 0.1.0 src/lib.rs/handle_one().",
                vec![],
            ),
            (
                "rust-analyzer cargo my-crate 0.1.0 src/lib.rs/handle_two().",
                vec![],
            ),
            (
                "rust-analyzer cargo my-crate 0.1.0 src/lib.rs/handle_three().",
                vec![],
            ),
        ],
    );
    let _ = run_doc_linter(&root, &["check", "--no-vale"]);

    // Without --list: should hint about ambiguity, list a preview,
    // and exit 1.
    let out = run_doc_linter(&root, &["explain", "handle_"]);
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    assert_eq!(
        out.status.code(),
        Some(1),
        "ambiguous → exit 1, got {:?}",
        out.status.code()
    );
    assert!(
        stdout.contains("candidates match") && stdout.contains("--list"),
        "expected ambiguity hint with --list pointer; got {stdout}"
    );

    // With --list: full list, exit 0.
    let out = run_doc_linter(&root, &["explain", "handle_", "--list"]);
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    assert!(out.status.success(), "with --list → exit 0");
    assert!(
        stdout.contains("3 candidates"),
        "expected 3-count header; got {stdout}"
    );
    for fn_name in &["handle_one", "handle_two", "handle_three"] {
        assert!(
            stdout.contains(fn_name),
            "expected {fn_name} in --list output; got {stdout}"
        );
    }
}

/// Asserts that the [[entity-doc-graph]] `explain` command emits a
/// helpful error (not a generic miss) when no symbol matches the input.
#[test]
fn explain_missing_symbol_emits_helpful_error() {
    let root = unique_tmpdir("missing");
    seed_corpus(&root, &["outlet"]);
    write_synthetic_scip(
        &root.join(".doc-lint/code.scip"),
        &[(
            "rust-analyzer cargo my-crate 0.1.0 src/lib.rs/something().",
            vec![],
        )],
    );
    let _ = run_doc_linter(&root, &["check", "--no-vale"]);

    let out = run_doc_linter(&root, &["explain", "definitely_not_there"]);
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    // Per the brief: exit 0 (or 1 — pick one; not 2). We chose 0.
    assert_eq!(
        out.status.code(),
        Some(0),
        "missing symbol → exit 0; got {:?}",
        out.status.code()
    );
    assert!(
        stdout.contains("no function matches"),
        "expected 'no function matches' message; got {stdout}"
    );
    assert!(
        stdout.contains("query functions-mentioning"),
        "expected hint about query subcommand; got {stdout}"
    );
}

/// Asserts that the [[entity-doc-graph]] `explain --json` mode emits a
/// well-formed structured object (not a free-form string).
#[test]
fn explain_json_emits_structured_object() {
    let root = unique_tmpdir("json");
    seed_corpus(&root, &["outlet"]);
    write_synthetic_scip(
        &root.join(".doc-lint/code.scip"),
        &[(
            "rust-analyzer cargo my-crate 0.1.0 src/lib.rs/list_outlets().",
            vec!["List every outlet in the system."],
        )],
    );
    let _ = run_doc_linter(&root, &["check", "--no-vale"]);

    let out = run_doc_linter(&root, &["explain", "list_outlets", "--json"]);
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    assert!(out.status.success(), "exit nonzero. stdout={stdout}");
    let parsed: serde_json::Value =
        serde_json::from_str(stdout.trim()).expect("output is valid JSON");
    for key in [
        "symbol",
        "crate",
        "file",
        "line",
        "doc_comment",
        "entities",
        "narrative_docs",
        "siblings",
    ] {
        assert!(
            parsed.get(key).is_some(),
            "expected key `{key}` in JSON output: {stdout}"
        );
    }
}
