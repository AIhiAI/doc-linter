#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "integration tests panic on broken invariants — that is the point"
)]
//! Phase 4 of roadmap-43: end-to-end tests for the
//! `doc-linter scaffold-coverage <CRATE>` subcommand.
//!
//! Each test seeds a tmp corpus (markdown ontology + a synthesized
//! SCIP file with dark functions), invokes the scaffolder via the
//! public binary, and asserts the expected proposals fire — either as
//! human preview blocks, JSON-Lines, or in-place edits.

use std::path::Path;

mod common;
use common::{run_doc_linter, unique_tmpdir, write};

/// Lays down the minimum ontology fixture, optionally including some
/// extra entities to drive the symbol-token resolver.
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

/// Seeds a [[entity-doc-graph]] config + ontology with a vale-disabled
/// .doc-lint.toml for scaffold-coverage integration tests.
fn seed_corpus(root: &Path, extra_entities: &[&str]) {
    let config =
        "required_fields = [\"id\", \"role\", \"title\", \"summary\", \"status\", \"updated\"]\n\
                  allowed_statuses = [\"draft\", \"stable\"]\n\
                  vale_enabled = false\n";
    write(&root.join(".doc-lint.toml"), config);
    seed_ontology(root, extra_entities);
}

/// Writes a SCIP file with `funcs` Function entries. `funcs` is a list
/// of `(symbol, doc_lines)`. Empty `doc_lines` → dark function.
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

/// Asserts that the [[entity-coverage]] `scaffold-coverage` dry-run
/// emits one TODO proposal per dark function without touching source.
#[test]
fn scaffold_dry_run_emits_proposals_for_dark_functions() {
    let root = unique_tmpdir("dry-run-emits");
    // Two truly dark functions: `compute_total` and `flush_buffer`.
    // Neither symbol contains "outlet" so the Phase 1 low-confidence
    // symbol-path scanner doesn't auto-bridge them. The fallback list
    // surfaces "outlet" as the closest candidate.
    seed_corpus(&root, &["outlet"]);
    write_synthetic_scip(
        &root.join(".doc-lint/code.scip"),
        &[
            (
                "rust-analyzer cargo my-crate 0.1.0 src/lib.rs/compute_total().",
                vec![],
            ),
            (
                "rust-analyzer cargo my-crate 0.1.0 src/lib.rs/flush_buffer().",
                vec![],
            ),
        ],
    );
    // Prime the graph once so the scaffolder reads populated graph.
    let _ = run_doc_linter(&root, &["check", "--no-vale"]);

    let out = run_doc_linter(&root, &["scaffold-coverage", "my-crate"]);
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    let stderr = String::from_utf8_lossy(&out.stderr).to_string();
    assert!(out.status.success(), "exit nonzero. stderr={stderr}");

    let proposal_count = stdout.matches("proposed:").count();
    assert_eq!(
        proposal_count, 2,
        "expected 2 proposals; stdout={stdout}\nstderr={stderr}"
    );
    assert!(
        stdout.contains("compute_total") && stdout.contains("flush_buffer"),
        "expected both fn names in output; stdout={stdout}"
    );
    // Both should land on the "no entity match" branch with the
    // closest-3 fallback list (which contains the only seeded entity).
    assert!(
        stdout.contains("no entity match"),
        "expected 'no entity match' branch; stdout={stdout}"
    );
}

#[test]
fn scaffold_emits_no_match_block_when_symbol_has_no_entity_token() {
    let root = unique_tmpdir("no-match");
    // Ontology has only `outlet`, but the function is `compute_value`
    // — neither `compute` nor `value` matches an entity. The fallback
    // top-N list must include `outlet` (it's the only entity present)
    // even though the function doesn't reference it.
    seed_corpus(&root, &["outlet"]);
    write_synthetic_scip(
        &root.join(".doc-lint/code.scip"),
        &[
            // Other fn that DOES mention outlet — populates fallback list.
            (
                "rust-analyzer cargo my-crate 0.1.0 src/lib.rs/outlet_helper().",
                vec!["Helper for the outlet entity."],
            ),
            // Target: no token match, no entity ref.
            (
                "rust-analyzer cargo my-crate 0.1.0 src/lib.rs/compute_value().",
                vec![],
            ),
        ],
    );
    let _ = run_doc_linter(&root, &["check", "--no-vale"]);

    let out = run_doc_linter(&root, &["scaffold-coverage", "my-crate"]);
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    assert!(out.status.success(), "exit nonzero");
    assert!(
        stdout.contains("compute_value"),
        "expected compute_value in stdout; got {stdout}"
    );
    assert!(
        stdout.contains("no entity match"),
        "expected 'no entity match' branch; got {stdout}"
    );
    assert!(
        stdout.contains("outlet"),
        "expected outlet in closest candidates; got {stdout}"
    );
    assert!(
        stdout.contains("no obvious entity"),
        "expected the TODO 'no obvious entity' marker; got {stdout}"
    );
}

#[test]
/// Asserts that the [[entity-coverage]] `scaffold-coverage --write`
/// mode inserts the proposed doc-comment immediately above the matching
/// fn line.
fn scaffold_write_inserts_doc_comment_above_fn_line() {
    let root = unique_tmpdir("write-inserts");
    seed_corpus(&root, &["outlet"]);

    // Real Rust file the scaffolder will edit. The fn name avoids any
    // ontology-token match so it stays dark after Phase-1 symbol scan.
    write(
        &root.join("crates/my-crate/src/lib.rs"),
        "// header\n\
         pub fn compute_total() {\n\
             // body\n\
         }\n",
    );
    // SCIP fact pointing at line 2 (the `pub fn` line).
    write_synthetic_scip_at(
        &root.join(".doc-lint/code.scip"),
        "crates/my-crate/src/lib.rs",
        &[(
            "rust-analyzer cargo my-crate 0.1.0 src/lib.rs/compute_total().",
            2,
            vec![],
        )],
    );
    let _ = run_doc_linter(&root, &["check", "--no-vale"]);

    let out = run_doc_linter(&root, &["scaffold-coverage", "my-crate", "--write"]);
    let stderr = String::from_utf8_lossy(&out.stderr).to_string();
    assert!(out.status.success(), "exit nonzero. stderr={stderr}");
    assert!(
        stderr.contains("edited 1 file"),
        "expected 'edited 1 file' summary; stderr={stderr}"
    );

    let edited =
        std::fs::read_to_string(root.join("crates/my-crate/src/lib.rs")).expect("read edited file");
    assert!(
        edited.contains("/// TODO(roadmap-43-phase4)"),
        "expected TODO doc comment; got:\n{edited}"
    );
    // The comment must precede the fn line.
    let todo_idx = edited.find("/// TODO").unwrap();
    let fn_idx = edited.find("pub fn compute_total").unwrap();
    assert!(
        todo_idx < fn_idx,
        "TODO must come BEFORE the fn line. todo={todo_idx} fn={fn_idx}\n{edited}"
    );
}

#[test]
fn scaffold_write_refuses_dirty_working_tree() {
    let root = unique_tmpdir("write-dirty");
    seed_corpus(&root, &["outlet"]);
    write(
        &root.join("crates/my-crate/src/lib.rs"),
        "pub fn compute_total() {}\n",
    );
    write_synthetic_scip(
        &root.join(".doc-lint/code.scip"),
        &[(
            "rust-analyzer cargo my-crate 0.1.0 src/lib.rs/compute_total().",
            vec![],
        )],
    );
    // Init git in the seed dir + make a clean commit so the working
    // tree is real, then dirty it with a new untracked file.
    init_git_with_one_commit(&root);
    write(&root.join("dirty.txt"), "uncommitted change\n");

    let _ = run_doc_linter(&root, &["check", "--no-vale"]);
    let out = run_doc_linter(&root, &["scaffold-coverage", "my-crate", "--write"]);
    assert_eq!(
        out.status.code(),
        Some(2),
        "expected exit 2 on dirty tree, got {:?}\nstderr={}",
        out.status.code(),
        String::from_utf8_lossy(&out.stderr)
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("clean working tree"),
        "expected 'clean working tree' message; got {stderr}"
    );
}

#[test]
/// Asserts that the [[entity-coverage]] `scaffold-coverage --json`
/// mode emits newline-delimited JSON, one proposal object per line.
fn scaffold_json_emits_one_object_per_line() {
    let root = unique_tmpdir("json-lines");
    // Use names that don't auto-bridge to the seeded entity via
    // Phase-1 symbol-token scanning, so both stay genuinely dark.
    seed_corpus(&root, &["outlet"]);
    write_synthetic_scip(
        &root.join(".doc-lint/code.scip"),
        &[
            (
                "rust-analyzer cargo my-crate 0.1.0 src/lib.rs/compute_total().",
                vec![],
            ),
            (
                "rust-analyzer cargo my-crate 0.1.0 src/lib.rs/flush_buffer().",
                vec![],
            ),
        ],
    );
    let _ = run_doc_linter(&root, &["check", "--no-vale"]);

    let out = run_doc_linter(&root, &["scaffold-coverage", "my-crate", "--json"]);
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    assert!(out.status.success(), "exit nonzero");
    let lines: Vec<&str> = stdout.lines().filter(|l| !l.is_empty()).collect();
    // Phase 5c of roadmap-43: 1 header line (`exempt_function_patterns`)
    // + 2 proposal rows.
    assert_eq!(
        lines.len(),
        3,
        "expected 1 header + 2 proposal lines; got {}: {stdout}",
        lines.len()
    );
    let header: serde_json::Value =
        serde_json::from_str(lines[0]).expect("header line parses as JSON");
    assert!(
        header.get("exempt_function_patterns").is_some(),
        "expected exempt_function_patterns key on header: {}",
        lines[0]
    );
    for l in &lines[1..] {
        let parsed: serde_json::Value =
            serde_json::from_str(l).expect("each proposal line parses as JSON");
        for key in [
            "file",
            "line",
            "fn",
            "entity",
            "matched_via",
            "proposed",
            "exempt",
        ] {
            assert!(
                parsed.get(key).is_some(),
                "expected key `{key}` on line: {l}"
            );
        }
    }
}

// ---------------- Helpers --------------------------------------------

fn write_synthetic_scip_at(out: &Path, relative_path: &str, funcs: &[(&str, u32, Vec<&str>)]) {
    use scip::types::symbol_information::Kind as ScipKind;
    use scip::types::{Document, Index, Occurrence, SymbolInformation};

    if let Some(p) = out.parent() {
        std::fs::create_dir_all(p).unwrap();
    }
    let mut index = Index::default();
    let mut doc = Document::default();
    doc.relative_path = relative_path.to_string();
    doc.language = "rust".to_string();
    for (symbol, line, doc_lines) in funcs {
        let mut sym = SymbolInformation::default();
        sym.symbol = (*symbol).to_string();
        sym.documentation = doc_lines.iter().map(|s| (*s).to_string()).collect();
        sym.kind = ScipKind::Function.into();
        doc.symbols.push(sym);
        let mut occ = Occurrence::default();
        occ.symbol = (*symbol).to_string();
        // SCIP ranges are 0-based; we want line `*line` (1-based) so
        // pass `*line - 1`.
        let zero_based = line.saturating_sub(1);
        occ.range = vec![zero_based as i32, 0, zero_based as i32, 1];
        occ.symbol_roles = 1;
        doc.occurrences.push(occ);
    }
    index.documents.push(doc);
    let bytes = protobuf::Message::write_to_bytes(&index).expect("encode");
    std::fs::write(out, bytes).expect("write scip");
}

fn init_git_with_one_commit(root: &Path) {
    use std::process::Command;
    let _ = Command::new("git")
        .arg("init")
        .arg("-q")
        .current_dir(root)
        .output();
    let _ = Command::new("git")
        .args(["config", "user.email", "test@example.com"])
        .current_dir(root)
        .output();
    let _ = Command::new("git")
        .args(["config", "user.name", "Test"])
        .current_dir(root)
        .output();
    let _ = Command::new("git")
        .arg("add")
        .arg(".")
        .current_dir(root)
        .output();
    let _ = Command::new("git")
        .args(["commit", "-q", "-m", "init"])
        .current_dir(root)
        .output();
}
