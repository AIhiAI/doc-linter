#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "integration tests panic on broken invariants — that is the point"
)]
//! Phase 5c of roadmap-43: end-to-end tests for the
//! `coverage_codegen_exclude` and `coverage_function_exempt`
//! configuration knobs.
//!
//! The first knob filters Function nodes out of the SCIP ingest
//! entirely — codegen sentinels (`frb_generated.rs`, *_generated.rs,
//! build.rs) never reach the graph, so authored doc-comments would
//! not be wiped on the next codegen.
//!
//! The second knob is applied at COUNT TIME (not ingest): the
//! Function node still exists and keeps its `FUNCTION_MENTIONS` edges,
//! but its bare name (matched against a list of regexes from the
//! config) flips it to `exempt: true` in scaffold output and
//! subtracts it from the dark count in coverage-report.

use std::path::Path;

mod common;
use common::{run_doc_linter, unique_tmpdir, write};

/// Minimal ontology that satisfies the linter without bringing in
/// vocabulary closure. Only `outlet` is registered as an entity so
/// the symbol-token bridge stays narrow.
fn seed_minimal_corpus(root: &Path, extra_config: &str) {
    let mut config = String::from(
        "required_fields = [\"id\", \"role\", \"title\", \"summary\", \"status\", \"updated\"]\n\
         allowed_statuses = [\"draft\", \"stable\"]\n\
         vale_enabled = false\n",
    );
    config.push_str(extra_config);
    write(&root.join(".doc-lint.toml"), &config);

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
    write(
        &root.join("docs/ontology/entities/outlet.md"),
        "---\n\
         id: entity-outlet\n\
         role: ontology-entity\n\
         title: \"Entity: outlet\"\n\
         summary: t\n\
         status: stable\n\
         updated: 2026-04-30\n\
         axis_id: covers\n\
         value_id: outlet\n\
         display: outlet\n\
         description: e\n\
         ---\n\n# outlet\n",
    );
}

/// Writes a SCIP file with `funcs` Function entries for the
/// [[entity-doc-graph]] coverage tests, spread across one or more
/// `Document` nodes (one per distinct file path). Each entry is
/// `(file_path, symbol, doc_lines)`. Empty `doc_lines` → dark.
fn write_synthetic_scip_multifile(out: &Path, funcs: &[(&str, &str, Vec<&str>)]) {
    use scip::types::symbol_information::Kind as ScipKind;
    use scip::types::{Document, Index, Occurrence, SymbolInformation};
    use std::collections::BTreeMap;

    if let Some(p) = out.parent() {
        std::fs::create_dir_all(p).unwrap();
    }
    let mut by_file: BTreeMap<&str, Vec<(&str, &Vec<&str>)>> = BTreeMap::new();
    for (file, sym, docs) in funcs {
        by_file.entry(*file).or_default().push((sym, docs));
    }

    let mut index = Index::default();
    for (file, entries) in by_file {
        let mut doc = Document::default();
        doc.relative_path = file.to_string();
        doc.language = "rust".to_string();
        for (symbol, doc_lines) in entries {
            let mut sym = SymbolInformation::default();
            sym.symbol = symbol.to_string();
            sym.documentation = doc_lines.iter().map(|s| (*s).to_string()).collect();
            sym.kind = ScipKind::Function.into();
            doc.symbols.push(sym);
            let mut occ = Occurrence::default();
            occ.symbol = symbol.to_string();
            occ.range = vec![0, 0, 0, 1];
            occ.symbol_roles = 1;
            doc.occurrences.push(occ);
        }
        index.documents.push(doc);
    }
    let bytes = protobuf::Message::write_to_bytes(&index).expect("encode");
    std::fs::write(out, bytes).expect("write scip");
}

// ---------------- Tests ---------------------------------------------

/// Phase 5c — codegen-excluded files don't produce Function nodes.
/// Seed one normal-file fact and one whose path matches
/// `**/frb_generated.rs`; ingest; assert the coverage-report only
/// counts the normal-file Function.
#[test]
fn codegen_excluded_files_dont_produce_function_nodes() {
    let root = unique_tmpdir("codegen-exclude");
    seed_minimal_corpus(&root, "");
    write_synthetic_scip_multifile(
        &root.join(".doc-lint/code.scip"),
        &[
            (
                "crates/my-crate/src/lib.rs",
                "rust-analyzer cargo my-crate 0.1.0 src/lib.rs/normal_fn().",
                vec![],
            ),
            (
                "crates/my-crate/src/frb_generated.rs",
                "rust-analyzer cargo my-crate 0.1.0 src/frb_generated.rs/codegen_fn().",
                vec![],
            ),
        ],
    );

    let _ = run_doc_linter(&root, &["check", "--no-vale"]);

    let out = run_doc_linter(&root, &["query", "coverage-report"]);
    assert!(
        out.status.success(),
        "coverage-report failed: stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );
    let parsed: serde_json::Value = serde_json::from_slice(&out.stdout).expect("JSON output");
    assert_eq!(
        parsed["global"]["total_functions"].as_u64(),
        Some(1),
        "only the non-codegen function should be in the graph: {parsed}"
    );
}

/// Phase 5c [[entity-doc-graph]] — `^parse_iso_` matches a bare fn name;
/// the dark count drops by the matched count and `exempt_functions` rises.
///
/// Seed 5 dark functions; two whose names start with `parse_iso_`.
/// Expected: dark = 3, exempt = 2.
#[test]
fn function_exempt_pattern_drops_dark_count() {
    let root = unique_tmpdir("exempt-drops-dark");
    seed_minimal_corpus(&root, "");
    write_synthetic_scip_multifile(
        &root.join(".doc-lint/code.scip"),
        &[
            (
                "crates/my-crate/src/lib.rs",
                "rust-analyzer cargo my-crate 0.1.0 src/lib.rs/parse_iso_8601().",
                vec![],
            ),
            (
                "crates/my-crate/src/lib.rs",
                "rust-analyzer cargo my-crate 0.1.0 src/lib.rs/parse_iso_date().",
                vec![],
            ),
            (
                "crates/my-crate/src/lib.rs",
                "rust-analyzer cargo my-crate 0.1.0 src/lib.rs/compute_total().",
                vec![],
            ),
            (
                "crates/my-crate/src/lib.rs",
                "rust-analyzer cargo my-crate 0.1.0 src/lib.rs/flush_buffer().",
                vec![],
            ),
            (
                "crates/my-crate/src/lib.rs",
                "rust-analyzer cargo my-crate 0.1.0 src/lib.rs/sync_state().",
                vec![],
            ),
        ],
    );

    let _ = run_doc_linter(&root, &["check", "--no-vale"]);

    let out = run_doc_linter(&root, &["query", "coverage-report"]);
    assert!(
        out.status.success(),
        "coverage-report failed: stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );
    let parsed: serde_json::Value = serde_json::from_slice(&out.stdout).expect("JSON output");
    assert_eq!(
        parsed["global"]["total_functions"].as_u64(),
        Some(5),
        "5 functions ingested: {parsed}"
    );
    assert_eq!(
        parsed["global"]["exempt_functions"].as_u64(),
        Some(2),
        "two `parse_iso_*` functions exempt: {parsed}"
    );
    assert_eq!(
        parsed["global"]["dark_functions"].as_u64(),
        Some(3),
        "5 total - 2 exempt = 3 dark: {parsed}"
    );
    let crates = parsed["by_crate"].as_array().expect("by_crate array");
    let mc = crates
        .iter()
        .find(|c| c["crate"] == "my-crate")
        .expect("my-crate row present");
    assert_eq!(mc["exempt_functions"].as_u64(), Some(2));
    assert_eq!(mc["dark_functions"].as_u64(), Some(3));
}

/// Phase 5c — `scaffold-coverage --json` emits a header line listing
/// the active exempt patterns, every proposal carries an `exempt`
/// boolean, and exempted rows arrive at the END with
/// `reason: "exempt"`.
#[test]
fn scaffold_coverage_marks_exempt_with_reason() {
    let root = unique_tmpdir("scaffold-exempt-reason");
    seed_minimal_corpus(&root, "");
    write_synthetic_scip_multifile(
        &root.join(".doc-lint/code.scip"),
        &[
            (
                "crates/my-crate/src/lib.rs",
                "rust-analyzer cargo my-crate 0.1.0 src/lib.rs/parse_iso_8601().",
                vec![],
            ),
            (
                "crates/my-crate/src/lib.rs",
                "rust-analyzer cargo my-crate 0.1.0 src/lib.rs/parse_iso_date().",
                vec![],
            ),
            (
                "crates/my-crate/src/lib.rs",
                "rust-analyzer cargo my-crate 0.1.0 src/lib.rs/compute_total().",
                vec![],
            ),
            (
                "crates/my-crate/src/lib.rs",
                "rust-analyzer cargo my-crate 0.1.0 src/lib.rs/flush_buffer().",
                vec![],
            ),
            (
                "crates/my-crate/src/lib.rs",
                "rust-analyzer cargo my-crate 0.1.0 src/lib.rs/sync_state().",
                vec![],
            ),
        ],
    );

    let _ = run_doc_linter(&root, &["check", "--no-vale"]);

    let out = run_doc_linter(&root, &["scaffold-coverage", "my-crate", "--json"]);
    assert!(
        out.status.success(),
        "scaffold-coverage failed: stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    let lines: Vec<&str> = stdout.lines().filter(|l| !l.is_empty()).collect();
    // Header + 3 actionable + 2 exempt = 6 lines.
    assert_eq!(
        lines.len(),
        6,
        "expected 1 header + 3 actionable + 2 exempt JSON lines; got {}: {stdout}",
        lines.len()
    );

    // Header carries `exempt_function_patterns`.
    let header: serde_json::Value = serde_json::from_str(lines[0]).expect("header parses as JSON");
    let patterns = header["exempt_function_patterns"]
        .as_array()
        .expect("exempt_function_patterns array");
    assert!(
        patterns.iter().any(|p| p == "^parse_iso_"),
        "header should include `^parse_iso_`: {header}"
    );

    // Exempt rows arrive at the end with `reason: "exempt"`.
    let last_two: Vec<serde_json::Value> = lines[4..]
        .iter()
        .map(|l| serde_json::from_str(l).expect("proposal parses"))
        .collect();
    for row in &last_two {
        assert_eq!(
            row["exempt"].as_bool(),
            Some(true),
            "expected exempt=true on row {row}"
        );
        assert_eq!(
            row["reason"].as_str(),
            Some("exempt"),
            "expected reason=exempt on row {row}"
        );
    }
    // The first three actionable rows should NOT be exempt.
    for l in &lines[1..4] {
        let row: serde_json::Value = serde_json::from_str(l).expect("actionable row parses");
        assert_eq!(
            row["exempt"].as_bool(),
            Some(false),
            "expected exempt=false on actionable row: {l}"
        );
    }
}
