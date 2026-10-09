#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "integration tests panic on broken invariants — that is the point"
)]
//! Phase 3 of roadmap-43: end-to-end tests for the new coverage-driven
//! lint diagnostics.
//!
//! Each test seeds a tmp corpus (markdown ontology + a synthesized
//! SCIP file), invokes `doc-linter check` via the public binary, and
//! parses the JSON output to assert which Phase-3 diagnostics fire.
//! We drive through the binary because doc-linter has no `[lib]`
//! target — the integration surface is the CLI.
//!
//! The Graph-side helpers themselves (`list_dark_public_functions` /
//! `list_dark_endpoints` / `list_entity_coverage_gaps` /
//! `global_function_reach`) are exercised indirectly: each test sets up
//! a corpus that should produce a known number of diagnostics, then
//! the binary's full pipeline (markdown lint → graph ingest → scip
//! ingest → endpoint extract → coverage lint) runs and we check the
//! JSON for the expected codes.

use std::path::Path;

mod common;
use common::{run_doc_linter, unique_tmpdir, write};

/// Lays down the minimum ontology fixture: role + kind + the two
/// roles every Phase-3 corpus uses (`doc`, `ontology-axis`,
/// `ontology-value`, `ontology-entity`).
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
                 summary: t\n\
                 status: stable\n\
                 updated: 2026-04-30\n\
                 axis_id: covers\n\
                 value_id: {ent}\n\
                 display: {ent}\n\
                 description: e\n\
                 ---\n\n# {ent}\n"
            ),
        );
    }
}

/// Writes a SCIP file with `funcs` Function entries. Each entry has
/// `(symbol, doc_comment, optional_mention_doc_comment_text)`. To
/// produce a function with NO mentions but WITH a doc comment, pass
/// non-empty `doc_lines` that don't reference any entity. To produce a
/// function with neither (truly dark), pass an empty `doc_lines` vec.
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

/// Seeds a [[entity-doc-graph]] config + ontology with a fixed shape:
///   - `vale_enabled = false` (no vale shellout)
///   - extra entities from caller
///   - `anchor_required_in` from caller
///   - `coverage_min_per_entity_doc_ratio` from caller (default 0.0)
fn seed_corpus_with_config(
    root: &Path,
    extra_entities: &[&str],
    anchor_required_in: &[&str],
    entity_threshold: f32,
    entity_exempt: &[&str],
) {
    let mut config = String::from(
        r#"required_fields = ["id", "role", "title", "summary", "status", "updated"]
allowed_statuses = ["draft", "stable"]
vale_enabled = false
# Roadmap-49 phase 3 flipped the default to `true`; this fixture
# seeds an axum router without markers, so the regex extractor is
# the only path that can produce dark-endpoints for the assertions.
endpoint_marker_exclusive = false
"#,
    );
    config.push_str(&format!(
        "coverage_min_per_entity_doc_ratio = {entity_threshold}\n"
    ));
    if !anchor_required_in.is_empty() {
        config.push_str("anchor_required_in = [");
        for (i, p) in anchor_required_in.iter().enumerate() {
            if i > 0 {
                config.push_str(", ");
            }
            config.push_str(&format!("\"{p}\""));
        }
        config.push_str("]\n");
    }
    if !entity_exempt.is_empty() {
        config.push_str("coverage_entity_exempt = [");
        for (i, e) in entity_exempt.iter().enumerate() {
            if i > 0 {
                config.push_str(", ");
            }
            config.push_str(&format!("\"{e}\""));
        }
        config.push_str("]\n");
    }
    write(&root.join(".doc-lint.toml"), &config);
    seed_ontology(root, extra_entities);
}

/// Parse JSON from doc-linter check stdout and collect every diagnostic
/// code across every file. Empty stdout returns an empty list.
fn diagnostic_codes(stdout: &str) -> Vec<String> {
    if stdout.trim().is_empty() {
        return Vec::new();
    }
    let parsed: serde_json::Value = match serde_json::from_str(stdout) {
        Ok(v) => v,
        Err(_) => return Vec::new(),
    };
    let mut out = Vec::new();
    if let Some(report) = parsed["report"].as_array() {
        for entry in report {
            if let Some(codes) = entry["codes"].as_array() {
                for c in codes {
                    if let Some(s) = c.as_str() {
                        out.push(s.to_string());
                    }
                }
            }
        }
    }
    out
}

// ---------------- DarkPublicFunction --------------------------------

/// Asserts that the [[entity-coverage]] lint rule fires for a dark
/// public function whose crate is inside the configured anchor scope.
#[test]
fn dark_public_function_fires_when_in_anchor_scope() {
    let root = unique_tmpdir("dark-fn-fires");
    seed_corpus_with_config(
        &root,
        &[],
        &["my-crate"], // anchor scope
        0.0,           // entity threshold off
        &[],
    );
    write_synthetic_scip(
        &root.join(".doc-lint/code.scip"),
        &[(
            "rust-analyzer cargo my-crate 0.1.0 src/lib.rs/dark_fn().",
            vec![], // no doc comment, no mentions → dark
        )],
    );

    // First run lays down the graph + ingests SCIP.
    let _ = run_doc_linter(&root, &["check", "--no-vale"]);
    // Second run: now the graph + functions are in place; the lint
    // pass picks up the dark fn.
    let out = run_doc_linter(&root, &["check", "--no-vale", "--format", "json"]);
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    let codes = diagnostic_codes(&stdout);
    assert!(
        codes.iter().any(|c| c == "dark-public-function"),
        "expected dark-public-function, got: {codes:?}\nstdout={stdout}"
    );
}

/// Asserts that the [[entity-coverage]] dark-function rule is suppressed
/// when the function carries a doc-comment with an entity wikilink.
#[test]
fn dark_public_function_skipped_when_function_has_doc_comment() {
    let root = unique_tmpdir("dark-fn-doc");
    seed_corpus_with_config(&root, &[], &["my-crate"], 0.0, &[]);
    write_synthetic_scip(
        &root.join(".doc-lint/code.scip"),
        &[(
            "rust-analyzer cargo my-crate 0.1.0 src/lib.rs/documented_fn().",
            vec!["A documented function. No entity reference but at least documented."],
        )],
    );
    let _ = run_doc_linter(&root, &["check", "--no-vale"]);
    let out = run_doc_linter(&root, &["check", "--no-vale", "--format", "json"]);
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    let codes = diagnostic_codes(&stdout);
    assert!(
        !codes.iter().any(|c| c == "dark-public-function"),
        "doc_comment present → no dark-public-function: {codes:?}\nstdout={stdout}"
    );
}

/// Asserts that the [[entity-coverage]] dark-function rule is suppressed
/// when the function has at least one `FUNCTION_MENTIONS` edge.
#[test]
fn dark_public_function_skipped_when_function_has_mention_edge() {
    let root = unique_tmpdir("dark-fn-mention");
    seed_corpus_with_config(&root, &["outlet"], &["my-crate"], 0.0, &[]);
    // Doc-comment text mentions "outlet" → SCIP ingest creates a
    // FUNCTION_MENTIONS edge → not dark.
    write_synthetic_scip(
        &root.join(".doc-lint/code.scip"),
        &[(
            "rust-analyzer cargo my-crate 0.1.0 src/lib.rs/outlet_handler().",
            vec!["Handler for the outlet entity."],
        )],
    );
    let _ = run_doc_linter(&root, &["check", "--no-vale"]);
    let out = run_doc_linter(&root, &["check", "--no-vale", "--format", "json"]);
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    let codes = diagnostic_codes(&stdout);
    assert!(
        !codes.iter().any(|c| c == "dark-public-function"),
        "FUNCTION_MENTIONS present → no dark-public-function: {codes:?}\nstdout={stdout}"
    );
}

// ---------------- DarkEndpoint --------------------------------------

/// Asserts that the [[entity-coverage]] dark-endpoint rule fires for an
/// endpoint with no `ENDPOINT_TOUCHES_ENTITY` edge.
#[test]
fn dark_endpoint_fires_for_uncovered_endpoint() {
    let root = unique_tmpdir("dark-endpoint");
    seed_corpus_with_config(&root, &[], &[], 0.0, &[]);
    // An axum route in the endpoint extractor. No SCIP, so the
    // endpoint won't have ENDPOINT_HANDLED_BY → no
    // ENDPOINT_TOUCHES_ENTITY → it's dark.
    write(
        &root.join("crates/api/src/routes.rs"),
        "use axum::{routing::get, Router};\n\
         async fn dark_handler() {}\n\
         pub fn router() -> Router {\n\
             Router::new().route(\"/v1/dark\", get(dark_handler))\n\
         }\n",
    );

    let _ = run_doc_linter(&root, &["check", "--no-vale"]);
    let out = run_doc_linter(&root, &["check", "--no-vale", "--format", "json"]);
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    let codes = diagnostic_codes(&stdout);
    assert!(
        codes.iter().any(|c| c == "dark-endpoint"),
        "expected dark-endpoint, got: {codes:?}\nstdout={stdout}"
    );
}

// ---------------- EntityCoverageGap ---------------------------------

#[test]
fn entity_coverage_gap_fires_below_threshold() {
    let root = unique_tmpdir("entity-gap-fires");
    // entity `outlet` has 0 docs covering it but functions mentioning
    // it via doc-comment text → 0:N ratio = 0 < 0.001.
    seed_corpus_with_config(&root, &["outlet"], &[], 0.001, &[]);
    // 5 functions all mentioning outlet via doc-comment → 0:5.
    let funcs: Vec<(&str, Vec<&str>)> = (0..5)
        .map(|i| {
            // Need different symbols. Use leaked strings.
            let sym = Box::leak(
                format!("rust-analyzer cargo my-crate 0.1.0 src/lib.rs/fn{i}().").into_boxed_str(),
            ) as &'static str;
            (sym, vec!["This function handles an outlet."])
        })
        .collect();
    write_synthetic_scip(&root.join(".doc-lint/code.scip"), &funcs);

    let _ = run_doc_linter(&root, &["check", "--no-vale"]);
    let out = run_doc_linter(&root, &["check", "--no-vale", "--format", "json"]);
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    let codes = diagnostic_codes(&stdout);
    assert!(
        codes.iter().any(|c| c == "entity-coverage-gap"),
        "expected entity-coverage-gap, got: {codes:?}\nstdout={stdout}"
    );
}

#[test]
fn entity_coverage_gap_skipped_when_in_exempt() {
    let root = unique_tmpdir("entity-gap-exempt");
    seed_corpus_with_config(&root, &["outlet"], &[], 0.001, &["outlet"]);
    let funcs: Vec<(&str, Vec<&str>)> = (0..5)
        .map(|i| {
            let sym = Box::leak(
                format!("rust-analyzer cargo my-crate 0.1.0 src/lib.rs/fn{i}().").into_boxed_str(),
            ) as &'static str;
            (sym, vec!["This function handles an outlet."])
        })
        .collect();
    write_synthetic_scip(&root.join(".doc-lint/code.scip"), &funcs);

    let _ = run_doc_linter(&root, &["check", "--no-vale"]);
    let out = run_doc_linter(&root, &["check", "--no-vale", "--format", "json"]);
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    let codes = diagnostic_codes(&stdout);
    assert!(
        !codes.iter().any(|c| c == "entity-coverage-gap"),
        "exempt entity → no diagnostic: {codes:?}\nstdout={stdout}"
    );
}

/// Issue #180 regression: `status: auto` entities (cluster-derived stubs)
/// must NOT trigger `entity-coverage-gap` even when functions mention them
/// and no docs cover them. The exemption is enforced at the coverage-lint
/// call site by building an exempt set that includes all auto-status entities.
#[test]
fn entity_coverage_gap_skipped_for_auto_status_entity() {
    let root = unique_tmpdir("entity-gap-auto");
    // Seed the standard corpus config but without the auto entity in the
    // extra_entities list (we write the file manually with status: auto).
    seed_corpus_with_config(&root, &[], &[], 0.001, &[]);
    // Write an entity with status: auto — cluster-derived, not enforced.
    write(
        &root.join("docs/ontology/entities/candidates/scorer.md"),
        "---\n\
         id: entity-scorer\n\
         role: ontology-entity\n\
         title: \"Entity: scorer (auto)\"\n\
         summary: cluster-derived auto-entity; review before promoting\n\
         status: auto\n\
         updated: 2026-05-28\n\
         axis_id: covers\n\
         value_id: scorer\n\
         display: scorer\n\
         description: \"Cluster anchored on scorer module.\"\n\
         source_modules:\n\
           - src/scorer/**\n\
         ---\n\n# scorer\n",
    );
    // 5 functions all mentioning scorer via doc-comment → 0 docs cover it.
    let funcs: Vec<(&str, Vec<&str>)> = (0..5)
        .map(|i| {
            let sym = Box::leak(
                format!("rust-analyzer cargo my-crate 0.1.0 src/lib.rs/score{i}().")
                    .into_boxed_str(),
            ) as &'static str;
            (sym, vec!["Feeds the scorer pipeline."])
        })
        .collect();
    write_synthetic_scip(&root.join(".doc-lint/code.scip"), &funcs);

    let _ = run_doc_linter(&root, &["check", "--no-vale"]);
    let out = run_doc_linter(&root, &["check", "--no-vale", "--format", "json"]);
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    let codes = diagnostic_codes(&stdout);
    assert!(
        !codes.iter().any(|c| c == "entity-coverage-gap"),
        "auto entity must be exempt from coverage-gap: {codes:?}\nstdout={stdout}"
    );
}

// ---------------- CoverageBelowMinimum ------------------------------

#[test]
fn entity_coverage_gap_skipped_when_stable_and_already_exempt() {
    // Regression guard: a stable entity in the exempt list should still
    // be excluded (the explicit exempt list still works alongside auto).
    let root = unique_tmpdir("entity-gap-exempt-stable");
    seed_corpus_with_config(&root, &["outlet"], &[], 0.001, &["outlet"]);
    let funcs: Vec<(&str, Vec<&str>)> = (0..3)
        .map(|i| {
            let sym = Box::leak(
                format!("rust-analyzer cargo my-crate 0.1.0 src/lib.rs/fn{i}().").into_boxed_str(),
            ) as &'static str;
            (sym, vec!["Handles an outlet."])
        })
        .collect();
    write_synthetic_scip(&root.join(".doc-lint/code.scip"), &funcs);
    let _ = run_doc_linter(&root, &["check", "--no-vale"]);
    let out = run_doc_linter(&root, &["check", "--no-vale", "--format", "json"]);
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    let codes = diagnostic_codes(&stdout);
    assert!(
        !codes.iter().any(|c| c == "entity-coverage-gap"),
        "explicitly-exempt stable entity must not fire: {codes:?}\nstdout={stdout}"
    );
}

#[test]
fn coverage_below_min_global_fires_when_under_threshold() {
    // 5 functions, only 1 reaches an entity → 20% global reach. With
    // --coverage-min-global=70 the diagnostic fires.
    let root = unique_tmpdir("global-below");
    seed_corpus_with_config(&root, &["outlet"], &[], 0.0, &[]);
    let mut funcs: Vec<(&str, Vec<&str>)> = vec![(
        "rust-analyzer cargo my-crate 0.1.0 src/lib.rs/fn_outlet().",
        vec!["Handles an outlet."],
    )];
    for i in 0..4 {
        let sym = Box::leak(
            format!("rust-analyzer cargo my-crate 0.1.0 src/lib.rs/dark{i}().").into_boxed_str(),
        ) as &'static str;
        funcs.push((sym, vec![]));
    }
    write_synthetic_scip(&root.join(".doc-lint/code.scip"), &funcs);

    let _ = run_doc_linter(&root, &["check", "--no-vale"]);
    let out = run_doc_linter(
        &root,
        &[
            "check",
            "--no-vale",
            "--format",
            "json",
            "--coverage-min-global=70",
        ],
    );
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    let codes = diagnostic_codes(&stdout);
    assert!(
        codes.iter().any(|c| c == "coverage-below-min"),
        "expected coverage-below-min for 20% < 70%: {codes:?}\nstdout={stdout}"
    );

    // Same corpus, threshold at 10% → does NOT fire.
    let out = run_doc_linter(
        &root,
        &[
            "check",
            "--no-vale",
            "--format",
            "json",
            "--coverage-min-global=10",
        ],
    );
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    let codes = diagnostic_codes(&stdout);
    assert!(
        !codes.iter().any(|c| c == "coverage-below-min"),
        "20% > 10% → no diagnostic: {codes:?}\nstdout={stdout}"
    );
}

// ---------------- --file mode skips Phase-3 ------------------------

#[test]
fn coverage_lint_skipped_in_file_mode() {
    let root = unique_tmpdir("file-mode-skip");
    seed_corpus_with_config(&root, &[], &["my-crate"], 0.001, &[]);
    write_synthetic_scip(
        &root.join(".doc-lint/code.scip"),
        &[(
            "rust-analyzer cargo my-crate 0.1.0 src/lib.rs/dark_fn().",
            vec![],
        )],
    );
    // First run populates the graph so Phase-3 helpers WOULD have data.
    let _ = run_doc_linter(&root, &["check", "--no-vale"]);
    // Second run with --file → Phase-3 must NOT fire.
    let out = run_doc_linter(
        &root,
        &[
            "check",
            "--no-vale",
            "--format",
            "json",
            "--file",
            "docs/ontology/axes/role.md",
            "--coverage-min-global=99",
        ],
    );
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    let codes = diagnostic_codes(&stdout);
    for code in [
        "dark-public-function",
        "dark-endpoint",
        "entity-coverage-gap",
        "coverage-below-min",
    ] {
        assert!(
            !codes.iter().any(|c| c == code),
            "Phase-3 code `{code}` must not fire in --file mode: \
             codes={codes:?}\nstdout={stdout}"
        );
    }
}
