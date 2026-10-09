#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "integration tests panic on broken invariants — that is the point"
)]
//! Phase 0 of roadmap-43: end-to-end test for `query coverage-report`.
//!
//! Builds a tiny tmp-dir corpus (3 docs, 2 entities, 5 functions where
//! 2 carry doc-comments mentioning entities), runs `doc-linter check`
//! to populate the graph, then invokes `query coverage-report`
//! and parses the JSON. Asserts the global percentage matches the
//! expected `2 / 5 = 40.00%`.
//!
//! A second test runs the same corpus through `--write` mode and
//! verifies the regenerated `docs/coverage-report.md` lints clean.

use std::path::Path;

mod common;
use common::{run_doc_linter, unique_tmpdir, write};

/// Lays down the smallest possible vault that exercises the per-entity
/// + per-crate sections:
///
///   * 2 ontology entities (`pricing-rule`, `outlet`) — different
///     reach so the by-entity sort order is verifiable.
///   * 1 crate README (Doc that the SCIP `crate-ref` lookup hits).
///   * 5 SCIP `Function` facts in that crate; only 2 have doc-comments
///     mentioning an entity, so global reach should be exactly 2/5.
fn seed_vault_with_scip(root: &Path) {
    write(
        &root.join(".doc-lint.toml"),
        r#"
exempt = []
required_fields = ["id", "role", "title", "summary", "status", "updated"]
allowed_statuses = ["draft", "stable"]
vale_enabled = false
"#,
    );

    // Ontology axes: role + kind values just sufficient for our docs.
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
    // Roles include `status-report` so the auto-generated
    // `docs/coverage-report.md` (its `role:` field) lints clean
    // against this fixture's ontology.
    for v in &["doc", "index", "status-report"] {
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

    // Entities the auto-generated `coverage-report.md` references in
    // its `covers:` frontmatter (`doc-graph`, `coverage`). Without
    // these the validator flags `unknown-entity`. The Phase 0 brief
    // pins the report's frontmatter shape, so the fixture has to
    // satisfy it.
    for ent in &["doc-graph", "coverage"] {
        write(
            &root.join(format!("docs/ontology/entities/{ent}.md")),
            &format!(
                "---\n\
                 id: entity-{ent}\n\
                 role: ontology-entity\n\
                 title: \"Entity: {ent}\"\n\
                 summary: t\n\
                 status: stable\n\
                 updated: 2026-04-30\n\
                 axis_id: covers\n\
                 value_id: {ent}\n\
                 display: {ent}\n\
                 description: meta\n\
                 ---\n\n# {ent}\n"
            ),
        );
    }

    // Two entities. `pricing-rule` has 1 covering doc + 1 covering func
    // (well-balanced), `outlet` has 0 covering docs + 1 covering func
    // (severely under-documented).
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
         description: an outlet\n\
         ---\n\n# outlet\n",
    );

    // Crate README that COVERS pricing-rule (so by_entity has
    // doc_count = 1 for pricing-rule).
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

    // 5 SCIP functions; 2 with doc-comments referencing an entity.
    write_synthetic_scip(&root.join(".doc-lint").join("code.scip"));
}

/// Writes a SCIP file with 5 functions in `pricing-core`:
///
///   * `compute` — doc-comment mentions `pricing rule` (-> reach).
///   * `validate_outlet` — doc-comment mentions `outlet` (-> reach).
///   * `helper1`, `helper2`, `helper3` — empty doc-comments (dark).
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

    fn make_fn(symbol: &str, doc_lines: &[&str]) -> SymbolInformation {
        let mut sym = SymbolInformation::default();
        sym.symbol = symbol.to_string();
        sym.documentation = doc_lines.iter().map(|s| (*s).to_string()).collect();
        sym.kind = ScipKind::Function.into();
        sym
    }

    let symbols = [
        (
            "rust-analyzer cargo pricing-core 1.0.0 src/lib.rs/compute().",
            vec!["Computes pricing using the configured pricing rule."],
        ),
        (
            "rust-analyzer cargo pricing-core 1.0.0 src/lib.rs/validate_outlet().",
            vec!["Validates an outlet against the registry."],
        ),
        (
            "rust-analyzer cargo pricing-core 1.0.0 src/lib.rs/helper1().",
            vec![],
        ),
        (
            "rust-analyzer cargo pricing-core 1.0.0 src/lib.rs/helper2().",
            vec![],
        ),
        (
            "rust-analyzer cargo pricing-core 1.0.0 src/lib.rs/helper3().",
            vec![],
        ),
    ];

    for (sym, doc_lines) in &symbols {
        doc.symbols.push(make_fn(sym, doc_lines));
        let mut occ = Occurrence::default();
        occ.symbol = (*sym).to_string();
        occ.range = vec![0, 0, 0, 1];
        occ.symbol_roles = 1;
        doc.occurrences.push(occ);
    }

    index.documents.push(doc);
    let bytes = protobuf::Message::write_to_bytes(&index).expect("encode");
    std::fs::write(out, bytes).expect("write scip");
}

/// Phase 0 of roadmap-43: a 5-function fixture where exactly 2 mention
/// an entity should produce a global reach of 40.00%. Phase 1's
/// structural bridge (symbol-name indexing) may add edges from the
/// other 3 functions if their symbol tokens hit an entity — the test
/// names (`helper1`/`helper2`/`helper3`) are deliberately
/// non-resolving so the symbol-path indexer doesn't bridge them.
#[test]
fn coverage_report_global_pct_matches_doc_comment_reach() {
    let root = unique_tmpdir("global-pct");
    seed_vault_with_scip(&root);

    // Run check so the graph is built + scip ingested. We
    // tolerate non-zero exit (the small ontology fixture may flag
    // orphan-doc warnings that don't matter for this assertion).
    let _check = run_doc_linter(&root, &["check", "--no-vale"]);

    let out = run_doc_linter(&root, &["query", "coverage-report"]);
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    let stderr = String::from_utf8_lossy(&out.stderr).to_string();
    assert!(
        out.status.success(),
        "coverage-report failed:\n--- stdout ---\n{stdout}\n--- stderr ---\n{stderr}"
    );
    let parsed: serde_json::Value =
        serde_json::from_str(&stdout).expect("coverage-report emits JSON");

    let global = &parsed["global"];
    assert_eq!(
        global["total_functions"].as_u64(),
        Some(5),
        "exactly 5 functions ingested: {parsed}"
    );
    assert_eq!(
        global["functions_reaching_entity"].as_u64(),
        Some(2),
        "2 of 5 functions reach an entity: {parsed}"
    );
    let pct = global["global_pct"].as_f64().expect("global_pct present");
    assert!(
        (pct - 40.0).abs() < 0.01,
        "global_pct should be 40.00, got {pct}: {parsed}"
    );
    assert_eq!(global["dark_functions"].as_u64(), Some(3));
    assert_eq!(global["documented_functions"].as_u64(), Some(2));

    // Per-entity: pricing-rule has 1 doc + 1 func (1:1 → balanced).
    let entities = parsed["by_entity"]
        .as_array()
        .expect("by_entity is an array");
    let pr = entities
        .iter()
        .find(|e| e["entity"] == "pricing-rule")
        .expect("pricing-rule in by_entity");
    assert_eq!(pr["doc_count"].as_u64(), Some(1));
    assert!(pr["func_count"].as_u64().unwrap() >= 1);

    // Per-crate: pricing-core dominates the section.
    let crates = parsed["by_crate"].as_array().expect("by_crate is an array");
    let pc = crates
        .iter()
        .find(|c| c["crate"] == "pricing-core")
        .expect("pricing-core in by_crate");
    assert_eq!(pc["total_functions"].as_u64(), Some(5));
}

#[test]
fn coverage_report_write_emits_lint_clean_markdown() {
    let root = unique_tmpdir("write-markdown");
    seed_vault_with_scip(&root);

    let _check = run_doc_linter(&root, &["check", "--no-vale"]);

    // First write — should create the file.
    let out = run_doc_linter(&root, &["query", "coverage-report", "--write"]);
    assert!(
        out.status.success(),
        "coverage-report --write failed: stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );
    let report_path = root.join("docs/coverage-report.md");
    assert!(
        report_path.exists(),
        "docs/coverage-report.md was not written"
    );

    // The generated doc must lint clean on its own.
    let lint = run_doc_linter(
        &root,
        &["check", "--no-vale", "--file", "docs/coverage-report.md"],
    );
    let lint_stdout = String::from_utf8_lossy(&lint.stdout).to_string();
    let lint_stderr = String::from_utf8_lossy(&lint.stderr).to_string();
    assert!(
        lint.status.success(),
        "coverage-report.md should lint clean: stdout={lint_stdout} stderr={lint_stderr}"
    );

    // Sanity: frontmatter has the expected role + covers.
    let body = std::fs::read_to_string(&report_path).expect("read report");
    assert!(body.contains("role: status-report"));
    assert!(body.contains("covers: [doc-graph, coverage]"));
    assert!(body.contains("## Global"));
    assert!(body.contains("## By entity"));
    assert!(body.contains("## By crate"));
}

#[test]
fn coverage_report_filter_arguments_narrow_output() {
    let root = unique_tmpdir("filters");
    seed_vault_with_scip(&root);
    let _check = run_doc_linter(&root, &["check", "--no-vale"]);

    // Crate filter — only `pricing-core` should appear in by_crate.
    let out = run_doc_linter(
        &root,
        &["query", "coverage-report", "--crate-filter", "pricing-core"],
    );
    assert!(out.status.success());
    let parsed: serde_json::Value =
        serde_json::from_slice(&out.stdout).expect("JSON from --crate-filter");
    let crates = parsed["by_crate"].as_array().expect("by_crate array");
    assert_eq!(crates.len(), 1, "only pricing-core: {parsed}");
    assert_eq!(crates[0]["crate"], "pricing-core");

    // Entity filter — only `pricing-rule` should appear in by_entity.
    let out = run_doc_linter(
        &root,
        &[
            "query",
            "coverage-report",
            "--entity-filter",
            "pricing-rule",
        ],
    );
    assert!(out.status.success());
    let parsed: serde_json::Value =
        serde_json::from_slice(&out.stdout).expect("JSON from --entity-filter");
    let entities = parsed["by_entity"].as_array().expect("by_entity array");
    assert_eq!(entities.len(), 1, "only pricing-rule: {parsed}");
    assert_eq!(entities[0]["entity"], "pricing-rule");
}
