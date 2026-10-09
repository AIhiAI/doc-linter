#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "integration tests panic on broken invariants — that is the point"
)]
//! Phase 1 of roadmap-43: structural-bridge integration test.
//!
//! Builds a synthetic SCIP index with two functions:
//!   1. `compute()` — its doc-comment mentions the `pricing-rule`
//!      entity by synonym. Should produce a `high` confidence
//!      `FUNCTION_MENTIONS` edge (the Round-3B path).
//!   2. `find_outlet()` — its doc-comment is empty BUT its symbol
//!      path tokenizes to `find` + `outlet`, which resolves to
//!      the `outlet` entity. Should produce a `low` confidence
//!      `FUNCTION_MENTIONS` edge (the Phase 1 symbol-path path).
//!
//! Then runs `doc-linter check` and queries the SQLite graph via the
//! `query sql` subcommand to assert both edges landed with the correct
//! `confidence` values. This is the single load-bearing test for
//! the Phase 1 promise: no doc-comment, no problem — the bridge
//! still spans the gap.

use std::path::Path;

mod common;
use common::{run_doc_linter, unique_tmpdir, write};

/// Lays down a minimal ontology covering pricing-rule and outlet,
/// the matching crate README, and a synthetic SCIP index.
fn seed_vault(root: &Path) {
    write(
        &root.join(".doc-lint.toml"),
        r#"
exempt = []
required_fields = ["id", "role", "title", "summary", "status", "updated"]
allowed_statuses = ["draft", "stable"]
vale_enabled = false
"#,
    );

    // Ontology axes / values.
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

    // Two entities the test will reach.
    write(
        &root.join("docs/ontology/entities/pricing-rule.md"),
        "---\n\
         id: entity-pricing-rule\n\
         role: ontology-entity\n\
         title: \"Entity: Pricing Rule\"\n\
         summary: t\n\
         status: stable\n\
         updated: 2026-04-30\n\
         axis_id: covers\n\
         value_id: pricing-rule\n\
         display: Pricing Rule\n\
         description: a pricing rule\n\
         synonyms: [rule]\n\
         ---\n\n# pricing-rule\n",
    );
    write(
        &root.join("docs/ontology/entities/outlet.md"),
        "---\n\
         id: entity-outlet\n\
         role: ontology-entity\n\
         title: \"Entity: Outlet\"\n\
         summary: t\n\
         status: stable\n\
         updated: 2026-04-30\n\
         axis_id: covers\n\
         value_id: outlet\n\
         display: Outlet\n\
         description: a retail outlet\n\
         synonyms: [store]\n\
         ---\n\n# outlet\n",
    );

    // Crate README so FUNCTION_DEFINED_IN can resolve.
    write(
        &root.join("crates/pricing-core/README.md"),
        "---\n\
         id: crate-pricing-core\n\
         role: doc\n\
         kind: reference\n\
         title: pricing-core\n\
         summary: pricing core crate\n\
         status: stable\n\
         updated: 2026-04-30\n\
         covers: [pricing-rule]\n\
         ---\n\n# pricing-core\n\nLinks to [[entity-pricing-rule]].\n",
    );

    write_synthetic_scip(&root.join(".doc-lint").join("code.scip"));
}

/// Encodes a SCIP `Index` with two Function symbols:
///   `compute()`     — has a doc-comment mentioning `pricing rule`.
///   `find_outlet()` — has NO doc-comment; the symbol path is the
///                     only signal.
fn write_synthetic_scip(out: &Path) {
    use scip::types::symbol_information::Kind as ScipKind;
    use scip::types::{Document, Index, Occurrence, SymbolInformation};

    if let Some(p) = out.parent() {
        std::fs::create_dir_all(p).unwrap();
    }

    let mut index = Index::default();
    let mut doc = Document::default();
    doc.relative_path = "crates/pricing-core/src/lib.rs".to_string();
    doc.language = "rust".to_string();

    // High-confidence path: doc-comment names the entity.
    let high_sym = "rust-analyzer cargo pricing-core 1.0.0 src/lib.rs/compute().";
    let mut high = SymbolInformation::default();
    high.symbol = high_sym.to_string();
    high.documentation =
        vec!["Computes pricing for an order using the configured pricing rule.".to_string()];
    high.kind = ScipKind::Function.into();
    doc.symbols.push(high);

    let mut occ_high = Occurrence::default();
    occ_high.symbol = high_sym.to_string();
    occ_high.range = vec![0, 4, 0, 11];
    occ_high.symbol_roles = 1;
    doc.occurrences.push(occ_high);

    // Low-confidence path: empty doc-comment, name reaches `outlet`.
    let low_sym = "rust-analyzer cargo pricing-core 1.0.0 src/lib.rs/find_outlet().";
    let mut low = SymbolInformation::default();
    low.symbol = low_sym.to_string();
    low.documentation = Vec::new();
    low.kind = ScipKind::Function.into();
    doc.symbols.push(low);

    let mut occ_low = Occurrence::default();
    occ_low.symbol = low_sym.to_string();
    occ_low.range = vec![10, 4, 10, 14];
    occ_low.symbol_roles = 1;
    doc.occurrences.push(occ_low);

    index.documents.push(doc);
    let bytes = protobuf::Message::write_to_bytes(&index).expect("encode");
    std::fs::write(out, bytes).expect("write scip");
}

/// Asserts that the [[entity-doc-graph]] structural bridge emits both
/// high-confidence (doc-comment scan) and low-confidence (symbol scan)
/// `FUNCTION_MENTIONS` edges into the SQLite store.
#[test]
fn structural_bridge_emits_high_and_low_confidence_edges() {
    let root = unique_tmpdir("high-low");
    seed_vault(&root);

    // Run check so the SQLite graph is built and SCIP is ingested.
    let _check = run_doc_linter(&root, &["check", "--no-vale"]);
    assert!(root.join(".doc-lint/graph.sqlite").exists());

    // Query the SQLite graph for both edges with their confidence
    // values via the raw `query sql` subcommand.
    let cypher = "SELECT src AS symbol, dst AS entity, confidence AS conf \
                  FROM FUNCTION_MENTIONS ORDER BY symbol, entity";
    let out = run_doc_linter(&root, &["query", "sql", cypher]);
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    let stderr = String::from_utf8_lossy(&out.stderr).to_string();
    assert!(
        out.status.success(),
        "sql query failed:\n--- stdout ---\n{stdout}\n--- stderr ---\n{stderr}"
    );

    let parsed: serde_json::Value = serde_json::from_str(&stdout).expect("sql emits JSON");
    let rows = parsed["rows"].as_array().expect("rows array");

    let high_sym = "rust-analyzer cargo pricing-core 1.0.0 src/lib.rs/compute().";
    let low_sym = "rust-analyzer cargo pricing-core 1.0.0 src/lib.rs/find_outlet().";

    // High-confidence: compute() -> pricing-rule, confidence=high.
    let high = rows
        .iter()
        .find(|r| r["symbol"] == high_sym && r["entity"] == "pricing-rule");
    assert!(
        high.is_some(),
        "missing high-confidence compute()->pricing-rule edge: rows={rows:#?}"
    );
    assert_eq!(
        high.unwrap()["conf"].as_str(),
        Some("high"),
        "doc-comment hit should be high confidence: {high:?}"
    );

    // Low-confidence: find_outlet() -> outlet, confidence=low.
    let low = rows
        .iter()
        .find(|r| r["symbol"] == low_sym && r["entity"] == "outlet");
    assert!(
        low.is_some(),
        "missing low-confidence find_outlet()->outlet edge: rows={rows:#?}"
    );
    assert_eq!(
        low.unwrap()["conf"].as_str(),
        Some("low"),
        "symbol-path hit should be low confidence: {low:?}"
    );
}

#[test]
/// Asserts that when doc-comment and symbol scanners both surface the
/// same entity, the [[entity-doc-graph]] structural bridge keeps the
/// high-confidence row in dedup.
fn structural_bridge_high_wins_when_both_signals_fire() {
    // A function whose doc-comment AND symbol path both name the
    // same entity should produce ONE edge with high confidence (not
    // two, not low). Verifies the dedup contract.
    let root = unique_tmpdir("dedup");
    seed_vault(&root);

    // Replace the SCIP file with one where compute() also has
    // `compute_pricing_rule()` in its symbol — both signals fire on
    // pricing-rule. We piggyback on the existing high_sym test by
    // renaming the function symbol to include the entity tokens.
    {
        use scip::types::symbol_information::Kind as ScipKind;
        use scip::types::{Document, Index, Occurrence, SymbolInformation};
        let mut index = Index::default();
        let mut doc = Document::default();
        doc.relative_path = "crates/pricing-core/src/lib.rs".to_string();
        doc.language = "rust".to_string();
        let sym = "rust-analyzer cargo pricing-core 1.0.0 src/lib.rs/compute_pricing_rule().";
        let mut s = SymbolInformation::default();
        s.symbol = sym.to_string();
        s.documentation = vec!["Wraps the pricing rule resolver.".to_string()];
        s.kind = ScipKind::Function.into();
        doc.symbols.push(s);
        let mut occ = Occurrence::default();
        occ.symbol = sym.to_string();
        occ.range = vec![0, 4, 0, 11];
        occ.symbol_roles = 1;
        doc.occurrences.push(occ);
        index.documents.push(doc);
        let bytes = protobuf::Message::write_to_bytes(&index).expect("encode");
        std::fs::write(root.join(".doc-lint/code.scip"), bytes).unwrap();
    }

    let _check = run_doc_linter(&root, &["check", "--no-vale"]);

    let sym = "rust-analyzer cargo pricing-core 1.0.0 src/lib.rs/compute_pricing_rule().";
    let cypher = format!(
        "SELECT confidence AS conf, count(*) AS n FROM FUNCTION_MENTIONS \
         WHERE src = '{sym}' AND dst = 'pricing-rule' GROUP BY confidence"
    );
    let out = run_doc_linter(&root, &["query", "sql", &cypher]);
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    let parsed: serde_json::Value = serde_json::from_str(&stdout).expect("sql emits JSON");
    let rows = parsed["rows"].as_array().expect("rows array");
    assert_eq!(rows.len(), 1, "exactly one row: {rows:?}");
    // Exactly one edge — high wins.
    assert_eq!(rows[0]["n"].as_i64(), Some(1));
    assert_eq!(rows[0]["conf"].as_str(), Some("high"));
}
