#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "integration tests panic on broken invariants — that is the point"
)]
//! Round 3B: SCIP ingest pipeline end-to-end test.
//!
//! Builds a synthetic SCIP index in memory, writes it to disk, runs
//! `doc-linter check` over a tmp-dir vault that includes a stub
//! `crates/<name>/README.md`, then queries the resulting SQLite graph via
//! `doc-linter query functions-mentioning <ent>` and asserts the
//! function/edge pair landed.
//!
//! Why a synthetic SCIP file rather than a real `rust-analyzer` run:
//! rust-analyzer is not always present in CI sandboxes, and its scip
//! subcommand has been moving in 2024-25. The SCIP wire format itself
//! is stable; spinning a synthetic Index via the protobuf types is the
//! cheapest way to pin behaviour without depending on the indexer.

use std::path::Path;

mod common;
use common::{run_doc_linter, unique_tmpdir, write};

/// Lays down a vault with: a minimal ontology (one entity), a couple of
/// docs (one of which is `crates/pricing-core/README.md` as the link
/// target for the Function ↔ Doc edge), and a synthetic SCIP file at
/// `.doc-lint/code.scip` whose only Function fact mentions the entity.
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

    // Ontology axes: role (just enough to validate our docs).
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

    // The entity our synthetic Function will mention.
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

    // The crate README that will be the Function -> Doc target.
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

    // Synthesise the SCIP file using the `scip` crate's protobuf types.
    // We use the `scip` crate via a small test-side helper compiled
    // alongside the main binary's tests.
    write_synthetic_scip(&root.join(".doc-lint").join("code.scip"));
}

/// Encodes a tiny `scip::types::Index` to disk: one Document, one
/// `SymbolInformation` (a Function called `compute` in pricing-core),
/// one Definition Occurrence at line 1.
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

    let symbol_str = "rust-analyzer cargo pricing-core 1.0.0 src/lib.rs/compute().";
    let mut sym = SymbolInformation::default();
    sym.symbol = symbol_str.to_string();
    sym.documentation = vec![
        "Computes pricing for an order using the configured pricing rule.".to_string(),
        "Falls back to the default rule when none matches.".to_string(),
    ];
    sym.kind = ScipKind::Function.into();
    doc.symbols.push(sym);

    let mut occ = Occurrence::default();
    occ.symbol = symbol_str.to_string();
    occ.range = vec![0, 4, 0, 11];
    occ.symbol_roles = 1;
    doc.occurrences.push(occ);

    index.documents.push(doc);
    let bytes = protobuf::Message::write_to_bytes(&index).expect("encode");
    std::fs::write(out, bytes).expect("write scip");
}

#[test]
fn scip_ingest_then_functions_mentioning_returns_match() {
    let root = unique_tmpdir("ingest-then-mentioning");
    seed_vault_with_scip(&root);

    // Run `check` so the SQLite graph is built AND scip is ingested.
    // We allow a non-zero exit because the orphan check may flag the
    // ontology-meta-doc fixtures; the test cares about the SQLite graph
    // landing and the Function + edges existing.
    let _check = run_doc_linter(&root, &["check", "--no-vale"]);

    // Sanity: the SCIP file exists.
    assert!(root.join(".doc-lint/code.scip").exists());
    assert!(root.join(".doc-lint/graph.sqlite").exists());

    // Query: functions mentioning the entity.
    let out = run_doc_linter(&root, &["query", "functions-mentioning", "pricing-rule"]);
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    let stderr = String::from_utf8_lossy(&out.stderr).to_string();
    assert!(
        out.status.success(),
        "query failed:\n--- stdout ---\n{stdout}\n--- stderr ---\n{stderr}"
    );
    let parsed: serde_json::Value =
        serde_json::from_str(&stdout).expect("functions-mentioning emits JSON");
    assert_eq!(parsed["entity"].as_str(), Some("pricing-rule"));
    let functions = parsed["functions"]
        .as_array()
        .expect("functions array present");
    assert_eq!(
        functions.len(),
        1,
        "exactly one matching function: {parsed}"
    );
    let f = &functions[0];
    assert_eq!(
        f["symbol"].as_str(),
        Some("rust-analyzer cargo pricing-core 1.0.0 src/lib.rs/compute().")
    );
    assert_eq!(f["crate"].as_str(), Some("pricing-core"));
    assert_eq!(f["file"].as_str(), Some("crates/pricing-core/src/lib.rs"));
    assert_eq!(f["line"].as_i64(), Some(1));
}

/// Code under a `skip_dirs` folder (a nested submodule a C# indexer
/// reached through project references) is not this repo's code graph.
#[test]
fn scip_ingest_skips_functions_under_skip_dirs() {
    let root = unique_tmpdir("ingest-skip-dirs");
    seed_vault_with_scip(&root);
    let cfg = root.join(".doc-lint.toml");
    let text = std::fs::read_to_string(&cfg).unwrap();
    std::fs::write(
        &cfg,
        format!("skip_dirs = [\"target\", \".git\", \"pricing-core\"]\n{text}"),
    )
    .unwrap();
    let _check = run_doc_linter(&root, &["check", "--no-vale"]);

    let out = run_doc_linter(&root, &["query", "functions-mentioning", "pricing-rule"]);
    let parsed: serde_json::Value = serde_json::from_slice(&out.stdout).expect("json");
    assert_eq!(
        parsed["functions"].as_array().map(Vec::len),
        Some(0),
        "function under a skip dir was ingested: {parsed}"
    );
}

#[test]
fn scip_ingest_then_function_context_returns_doc_and_entity_links() {
    let root = unique_tmpdir("ingest-then-context");
    seed_vault_with_scip(&root);

    let _check = run_doc_linter(&root, &["check", "--no-vale"]);

    let symbol = "rust-analyzer cargo pricing-core 1.0.0 src/lib.rs/compute().";
    let out = run_doc_linter(&root, &["query", "function-context", symbol]);
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    let stderr = String::from_utf8_lossy(&out.stderr).to_string();
    assert!(
        out.status.success(),
        "function-context failed:\n--- stdout ---\n{stdout}\n--- stderr ---\n{stderr}"
    );
    let parsed: serde_json::Value =
        serde_json::from_str(&stdout).expect("function-context emits JSON");
    assert_eq!(parsed["function"]["symbol"].as_str(), Some(symbol));
    assert_eq!(parsed["function"]["crate"].as_str(), Some("pricing-core"));

    let links = parsed["links"].as_array().expect("links array");
    // Expect at minimum: defined-in -> crate-pricing-core (Doc),
    //                    mentions   -> pricing-rule (Entity)
    assert!(
        links
            .iter()
            .any(|l| l["edge"] == "function-defined-in" && l["id"] == "crate-pricing-core"),
        "missing function-defined-in edge: {links:?}"
    );
    assert!(
        links
            .iter()
            .any(|l| l["edge"] == "function-mentions" && l["id"] == "pricing-rule"),
        "missing function-mentions edge: {links:?}"
    );
}

/// Source-modules pipeline: when an entity declares `source_modules:`
/// globs that match the SCIP Function's file, the ingest emits a
/// `FUNCTION_BELONGS_TO` edge. Reads back through `query sql` so the
/// assertion is on the actual SQLite edge table rather than an intermediate.
#[test]
fn scip_ingest_emits_function_belongs_to_when_source_modules_match() {
    let root = unique_tmpdir("ingest-belongs-to");
    seed_vault_with_scip(&root);

    // Overwrite the entity doc to add source_modules covering the SCIP
    // function's file (crates/pricing-core/src/lib.rs).
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
         source_modules: [\"crates/pricing-core/src/**/*.rs\"]\n\
         ---\n\n# pricing-rule\n",
    );

    let _check = run_doc_linter(&root, &["check", "--no-vale"]);

    let out = run_doc_linter(
        &root,
        &[
            "query",
            "sql",
            "SELECT f.symbol AS symbol, b.dst AS entity FROM Function f \
             JOIN FUNCTION_BELONGS_TO b ON b.src = f.symbol ORDER BY entity",
        ],
    );
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    let stderr = String::from_utf8_lossy(&out.stderr).to_string();
    assert!(
        out.status.success(),
        "sql query failed:\n--- stdout ---\n{stdout}\n--- stderr ---\n{stderr}"
    );
    let parsed: serde_json::Value = serde_json::from_str(&stdout).expect("sql emits JSON");
    let rows = parsed["rows"].as_array().expect("rows array");
    assert!(
        rows.iter().any(|r| {
            r["entity"].as_str() == Some("pricing-rule")
                && r["symbol"].as_str()
                    == Some("rust-analyzer cargo pricing-core 1.0.0 src/lib.rs/compute().")
        }),
        "expected FUNCTION_BELONGS_TO compute() -> pricing-rule, got rows: {rows:?}"
    );
}

/// `query list --ranked` returns entities ordered by `FUNCTION_MENTIONS`
/// edge count, with `god_node` reading the persisted
/// `Entity.is_god_node` column (roadmap issue #11). The fixture has
/// a single entity, so by the `mean + 2*stddev` rule no Entity is a
/// god node — the assertion checks the column is wired through as
/// a boolean rather than missing or stringified. A focused
/// threshold-firing test belongs near the derive_god_nodes unit
/// once an in-memory fixture lands.
#[test]
fn query_list_ranked_orders_entities_by_mention_count() {
    let root = unique_tmpdir("query-ranked");
    seed_vault_with_scip(&root);

    let _ = run_doc_linter(&root, &["check", "--no-vale"]);

    let out = run_doc_linter(&root, &["query", "list", "--ranked"]);
    assert!(
        out.status.success(),
        "ranked failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let parsed: serde_json::Value =
        serde_json::from_str(&String::from_utf8_lossy(&out.stdout)).expect("ranked emits JSON");

    assert!(parsed["count"].as_u64().unwrap() >= 1);
    let entities = parsed["entities"].as_array().expect("entities array");
    let top = &entities[0];
    assert_eq!(top["rank"].as_u64(), Some(1));
    assert_eq!(top["id"].as_str(), Some("pricing-rule"));
    assert!(
        top["mentions"].as_u64().unwrap() >= 1,
        "top entity should have at least one mention"
    );
    // god_node is a boolean read from Entity.is_god_node — verified
    // present and well-typed regardless of the specific value, which
    // depends on the distribution.
    assert!(
        top["god_node"].is_boolean(),
        "god_node should be a boolean, got: {:?}",
        top["god_node"]
    );
    // Bug #150 follow-up: entity_class is serialized with
    // `skip_serializing_if = Option::is_none`, so an unclassified
    // entity (the fixture leaves the frontmatter field unset) must
    // NOT have the key in the JSON — that's how JSON consumers
    // distinguish "no class declared" from "class declared as
    // empty string".
    assert!(
        top.get("entity_class").is_none(),
        "entity_class should be absent for unclassified entities, got: {top}"
    );
}

/// Bug #150 follow-up: `query list --ranked` exposes the
/// `entity_class` field whenever the ontology declares it. The
/// fixture adds a second `python` entity with
/// `entity_class: language` so the field surfaces in JSON output —
/// matching the pattern an agent would use to verify the carve-out
/// is being applied without writing raw SQL.
#[test]
fn query_list_ranked_surfaces_entity_class_for_language_entities() {
    let root = unique_tmpdir("query-ranked-entity-class");
    seed_vault_with_scip(&root);

    // Add a language-class entity alongside the existing pricing-rule.
    common::write(
        &root.join("docs/ontology/entities/python.md"),
        "---\n\
         id: entity-python\n\
         role: ontology-entity\n\
         title: \"Entity: Python\"\n\
         summary: implementation language\n\
         status: stable\n\
         updated: 2026-04-30\n\
         axis_id: covers\n\
         value_id: python\n\
         display: Python\n\
         description: the implementation language\n\
         entity_class: language\n\
         ---\n\n# python\n",
    );

    let _ = run_doc_linter(&root, &["check", "--no-vale"]);

    let out = run_doc_linter(&root, &["query", "list", "--ranked"]);
    let parsed: serde_json::Value =
        serde_json::from_str(&String::from_utf8_lossy(&out.stdout)).expect("ranked emits JSON");
    let entities = parsed["entities"].as_array().expect("entities array");

    let python = entities
        .iter()
        .find(|e| e["id"].as_str() == Some("python"))
        .expect("python entity should appear in ranked output");
    assert_eq!(
        python["entity_class"].as_str(),
        Some("language"),
        "entity_class=language should round-trip through the sqlite \
         column and surface in JSON: {python}",
    );
    assert!(
        !python["god_node"].as_bool().unwrap_or(true),
        "a language-class entity must never be god_node, got: {python}",
    );

    let pricing = entities
        .iter()
        .find(|e| e["id"].as_str() == Some("pricing-rule"))
        .expect("pricing-rule should still appear");
    assert!(
        pricing.get("entity_class").is_none(),
        "unclassified entities keep the key absent in JSON: {pricing}",
    );
}

/// Roadmap issue #32 (v0.3.0): the new `Type` node table is dual-
/// written from struct/enum/trait/module/type_alias FunctionFacts.
/// Adds a `PricingRule` struct symbol to the synthetic SCIP and
/// checks that `MATCH (t:Type {kind: 'struct'})` returns it — the
/// acceptance criterion called out on the issue. The Function table
/// is intentionally left undisturbed (back-compat for every existing
/// FUNCTION_* edge); the full Function/Type split is v0.4.0.
#[test]
fn scip_ingest_populates_type_node_table() {
    let root = unique_tmpdir("type-node-table");
    seed_vault_with_scip(&root);
    write_synthetic_scip_with_type(&root.join(".doc-lint").join("code.scip"));

    let _ = run_doc_linter(&root, &["check", "--no-vale"]);
    assert!(root.join(".doc-lint/graph.sqlite").exists());

    let out = run_doc_linter(
        &root,
        &[
            "query",
            "sql",
            "SELECT symbol, kind FROM Type WHERE kind = 'struct' ORDER BY symbol",
        ],
    );
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    let stderr = String::from_utf8_lossy(&out.stderr).to_string();
    assert!(
        out.status.success(),
        "sql query failed:\n--- stdout ---\n{stdout}\n--- stderr ---\n{stderr}"
    );
    let parsed: serde_json::Value = serde_json::from_str(&stdout).expect("query sql emits JSON");
    let rows = parsed["rows"].as_array().expect("rows array");
    assert!(
        !rows.is_empty(),
        "expected at least one Type row, got: {parsed}"
    );
    assert!(
        rows.iter().any(|r| r["kind"].as_str() == Some("struct")
            && r["symbol"]
                .as_str()
                .is_some_and(|s| s.contains("PricingRule#"))),
        "expected PricingRule struct row, got: {rows:?}"
    );
}

/// Roadmap issue #32 v2: `query types` subcommand lists Type rows
/// without forcing the operator to write raw SQL. Builds on the
/// same `with_type` fixture, then exercises both the kind filter
/// and the unfiltered listing.
#[test]
fn query_types_filters_by_kind() {
    let root = unique_tmpdir("query-types");
    seed_vault_with_scip(&root);
    write_synthetic_scip_with_type(&root.join(".doc-lint").join("code.scip"));
    let _ = run_doc_linter(&root, &["check", "--no-vale"]);

    let out = run_doc_linter(&root, &["query", "types", "--kind", "struct"]);
    assert!(
        out.status.success(),
        "query types failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let parsed: serde_json::Value = serde_json::from_str(&String::from_utf8_lossy(&out.stdout))
        .expect("query types emits JSON");
    assert_eq!(parsed["kind"].as_str(), Some("struct"));
    let rows = parsed["types"].as_array().expect("types array");
    assert!(
        rows.iter().any(|r| r["kind"].as_str() == Some("struct")
            && r["symbol"]
                .as_str()
                .is_some_and(|s| s.contains("PricingRule#"))),
        "expected PricingRule struct in filtered listing: {rows:?}"
    );
    // Every returned row must carry the requested kind.
    for r in rows {
        assert_eq!(r["kind"].as_str(), Some("struct"));
    }
}

#[test]
fn query_types_rejects_unknown_kind() {
    let root = unique_tmpdir("query-types-bad");
    seed_vault_with_scip(&root);
    let _ = run_doc_linter(&root, &["check", "--no-vale"]);

    let out = run_doc_linter(&root, &["query", "types", "--kind", "function"]);
    assert!(!out.status.success(), "unknown kind should fail");
    let stderr = String::from_utf8_lossy(&out.stderr).to_string();
    assert!(
        stderr.contains("unknown --kind"),
        "expected stderr to mention unknown kind: {stderr}"
    );
}

/// Variant of `write_synthetic_scip` that adds a struct symbol
/// alongside the function — exercises the roadmap-32 dual-write
/// path into the new `Type` node table.
fn write_synthetic_scip_with_type(out: &Path) {
    use scip::types::symbol_information::Kind as ScipKind;
    use scip::types::{Document, Index, Occurrence, SymbolInformation};

    if let Some(p) = out.parent() {
        std::fs::create_dir_all(p).unwrap();
    }

    let mut index = Index::default();
    let mut doc = Document::default();
    doc.relative_path = "crates/pricing-core/src/lib.rs".to_string();
    doc.language = "rust".to_string();

    let fn_symbol = "rust-analyzer cargo pricing-core 1.0.0 src/lib.rs/compute().";
    let mut sym_fn = SymbolInformation::default();
    sym_fn.symbol = fn_symbol.to_string();
    sym_fn.documentation = vec!["compute pricing for an order".to_string()];
    sym_fn.kind = ScipKind::Function.into();
    doc.symbols.push(sym_fn);

    let mut occ_fn = Occurrence::default();
    occ_fn.symbol = fn_symbol.to_string();
    occ_fn.range = vec![0, 4, 0, 11];
    occ_fn.symbol_roles = 1;
    doc.occurrences.push(occ_fn);

    let struct_symbol = "rust-analyzer cargo pricing-core 1.0.0 src/lib.rs/PricingRule#";
    let mut sym_struct = SymbolInformation::default();
    sym_struct.symbol = struct_symbol.to_string();
    sym_struct.documentation = vec!["struct holding a pricing rule".to_string()];
    sym_struct.kind = ScipKind::Struct.into();
    doc.symbols.push(sym_struct);

    let mut occ_struct = Occurrence::default();
    occ_struct.symbol = struct_symbol.to_string();
    occ_struct.range = vec![3, 11, 3, 22];
    occ_struct.symbol_roles = 1;
    doc.occurrences.push(occ_struct);

    index.documents.push(doc);
    let bytes = protobuf::Message::write_to_bytes(&index).expect("encode");
    std::fs::write(out, bytes).expect("write scip");
}

/// Roadmap issue #32 v4 (v0.4.0): `METHOD_OF` edge — Function → Type
/// emitted during ingest when a `FunctionKind::Method` row's
/// descriptor-prefix parent matches a Type row. Seeds a struct
/// + one method on that struct and asserts the edge lands.
#[test]
fn scip_ingest_emits_method_of_edge_for_struct_methods() {
    let root = unique_tmpdir("method-of-edge");
    seed_vault_with_scip(&root);
    write_synthetic_scip_with_method(&root.join(".doc-lint").join("code.scip"));

    let out = run_doc_linter(&root, &["check", "--no-vale"]);
    assert!(
        out.status.success() || !String::from_utf8_lossy(&out.stderr).contains("NOT updated"),
        "check failed with sqlite error: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    let out = run_doc_linter(
        &root,
        &[
            "query",
            "sql",
            "SELECT src AS fn_symbol, dst AS type_symbol FROM METHOD_OF ORDER BY fn_symbol",
        ],
    );
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    let stderr = String::from_utf8_lossy(&out.stderr).to_string();
    assert!(
        out.status.success(),
        "sql METHOD_OF query failed:\n--- stdout ---\n{stdout}\n--- stderr ---\n{stderr}"
    );
    let parsed: serde_json::Value = serde_json::from_str(&stdout).expect("query sql emits JSON");
    let rows = parsed["rows"].as_array().expect("rows array");
    assert!(
        !rows.is_empty(),
        "expected at least one METHOD_OF edge, got: {parsed}"
    );
    // The method's fn symbol should end in `validate().` and its
    // type symbol should end in `PricingRule#`.
    assert!(
        rows.iter().any(|r| r["fn_symbol"]
            .as_str()
            .is_some_and(|s| s.contains("PricingRule/validate"))
            && r["type_symbol"]
                .as_str()
                .is_some_and(|s| s.contains("PricingRule#"))),
        "expected validate -> PricingRule METHOD_OF, got: {rows:?}"
    );
}

/// Variant of `write_synthetic_scip_with_type` that adds a method
/// on the struct so the METHOD_OF derivation has something to bind.
fn write_synthetic_scip_with_method(out: &Path) {
    use scip::types::symbol_information::Kind as ScipKind;
    use scip::types::{Document, Index, Occurrence, SymbolInformation};

    if let Some(p) = out.parent() {
        std::fs::create_dir_all(p).unwrap();
    }

    let mut index = Index::default();
    let mut doc = Document::default();
    doc.relative_path = "crates/pricing-core/src/lib.rs".to_string();
    doc.language = "rust".to_string();

    // The struct.
    let struct_symbol = "rust-analyzer cargo pricing-core 1.0.0 src/lib.rs/PricingRule#";
    let mut sym_struct = SymbolInformation::default();
    sym_struct.symbol = struct_symbol.to_string();
    sym_struct.documentation = vec!["struct holding a pricing rule".to_string()];
    sym_struct.kind = ScipKind::Struct.into();
    doc.symbols.push(sym_struct);

    let mut occ_struct = Occurrence::default();
    occ_struct.symbol = struct_symbol.to_string();
    occ_struct.range = vec![0, 7, 0, 18];
    occ_struct.symbol_roles = 1;
    doc.occurrences.push(occ_struct);

    // The method on the struct. Descriptor shape `.../PricingRule/validate().`
    // is the parent-prefix the METHOD_OF derivation looks for.
    let method_symbol = "rust-analyzer cargo pricing-core 1.0.0 src/lib.rs/PricingRule/validate().";
    let mut sym_method = SymbolInformation::default();
    sym_method.symbol = method_symbol.to_string();
    sym_method.documentation = vec!["validate the pricing rule".to_string()];
    sym_method.kind = ScipKind::Method.into();
    doc.symbols.push(sym_method);

    let mut occ_method = Occurrence::default();
    occ_method.symbol = method_symbol.to_string();
    occ_method.range = vec![3, 11, 3, 19];
    occ_method.symbol_roles = 1;
    doc.occurrences.push(occ_method);

    index.documents.push(doc);
    let bytes = protobuf::Message::write_to_bytes(&index).expect("encode");
    std::fs::write(out, bytes).expect("write scip");
}

/// Roadmap issue #32 v5 (v0.4.0): `USES_TYPE` edge — Function →
/// Type emitted when a function body contains a SCIP reference
/// occurrence whose descriptor terminal is a `Type` (suffix `#`).
/// Seeds a struct plus a function that references it inside its
/// body span and asserts the edge round-trips through SQLite.
#[test]
fn scip_ingest_emits_uses_type_edge_for_body_type_references() {
    let root = unique_tmpdir("uses-type-edge");
    seed_vault_with_scip(&root);
    write_synthetic_scip_with_body_type_ref(&root.join(".doc-lint").join("code.scip"));

    let out = run_doc_linter(&root, &["check", "--no-vale"]);
    assert!(
        out.status.success() || !String::from_utf8_lossy(&out.stderr).contains("NOT updated"),
        "check failed with sqlite error: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    let out = run_doc_linter(
        &root,
        &[
            "query",
            "sql",
            "SELECT src AS fn_symbol, dst AS type_symbol FROM USES_TYPE ORDER BY fn_symbol",
        ],
    );
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    let stderr = String::from_utf8_lossy(&out.stderr).to_string();
    assert!(
        out.status.success(),
        "sql USES_TYPE query failed:\n--- stdout ---\n{stdout}\n--- stderr ---\n{stderr}"
    );
    let parsed: serde_json::Value = serde_json::from_str(&stdout).expect("query sql emits JSON");
    let rows = parsed["rows"].as_array().expect("rows array");
    assert!(
        !rows.is_empty(),
        "expected at least one USES_TYPE edge, got: {parsed}"
    );
    assert!(
        rows.iter().any(|r| r["fn_symbol"]
            .as_str()
            .is_some_and(|s| s.ends_with("/apply()."))
            && r["type_symbol"]
                .as_str()
                .is_some_and(|s| s.ends_with("/PricingRule#"))),
        "expected apply -> PricingRule USES_TYPE, got: {rows:?}"
    );
}

/// Variant of `write_synthetic_scip_with_method` that defines a
/// struct plus a free function whose body contains two reference
/// occurrences to the struct — exercises both the type-suffix filter
/// and the per-(function, type) dedupe.
fn write_synthetic_scip_with_body_type_ref(out: &Path) {
    use scip::types::symbol_information::Kind as ScipKind;
    use scip::types::{Document, Index, Occurrence, SymbolInformation};

    if let Some(p) = out.parent() {
        std::fs::create_dir_all(p).unwrap();
    }

    let mut index = Index::default();
    let mut doc = Document::default();
    doc.relative_path = "crates/pricing-core/src/lib.rs".to_string();
    doc.language = "rust".to_string();

    // The struct (TO-side of USES_TYPE).
    let struct_symbol = "rust-analyzer cargo pricing-core 1.0.0 src/lib.rs/PricingRule#";
    let mut sym_struct = SymbolInformation::default();
    sym_struct.symbol = struct_symbol.to_string();
    sym_struct.documentation = vec!["struct holding a pricing rule".to_string()];
    sym_struct.kind = ScipKind::Struct.into();
    doc.symbols.push(sym_struct);

    let mut occ_struct = Occurrence::default();
    occ_struct.symbol = struct_symbol.to_string();
    occ_struct.range = vec![0, 7, 0, 18];
    occ_struct.symbol_roles = 1;
    doc.occurrences.push(occ_struct);

    // The function (FROM-side). Body spans lines 5..15.
    let fn_symbol = "rust-analyzer cargo pricing-core 1.0.0 src/lib.rs/apply().";
    let mut sym_fn = SymbolInformation::default();
    sym_fn.symbol = fn_symbol.to_string();
    sym_fn.documentation = vec!["apply pricing".to_string()];
    sym_fn.kind = ScipKind::Function.into();
    doc.symbols.push(sym_fn);

    let mut occ_fn = Occurrence::default();
    occ_fn.symbol = fn_symbol.to_string();
    occ_fn.range = vec![5, 4, 5, 9];
    occ_fn.enclosing_range = vec![5, 0, 15, 0];
    occ_fn.symbol_roles = 1;
    doc.occurrences.push(occ_fn);

    // Two refs to the struct inside the body. Dedupe must collapse to
    // one USES_TYPE edge.
    let mut ref_a = Occurrence::default();
    ref_a.symbol = struct_symbol.to_string();
    ref_a.range = vec![7, 12, 7, 22];
    doc.occurrences.push(ref_a);

    let mut ref_b = Occurrence::default();
    ref_b.symbol = struct_symbol.to_string();
    ref_b.range = vec![10, 8, 10, 18];
    doc.occurrences.push(ref_b);

    index.documents.push(doc);
    let bytes = protobuf::Message::write_to_bytes(&index).expect("encode");
    std::fs::write(out, bytes).expect("write scip");
}

/// Roadmap issue #32 v7 (v0.4.0): `query function-context` surfaces
/// METHOD_OF and USES_TYPE edges alongside the existing
/// FUNCTION_DEFINED_IN / FUNCTION_MENTIONS links. Reuses the
/// METHOD_OF synthetic fixture (struct + method on it) and asserts
/// the method's function-context includes a `method-of` link to
/// the enclosing type.
#[test]
fn function_context_surfaces_method_of_edge_for_struct_methods() {
    let root = unique_tmpdir("function-context-method-of");
    seed_vault_with_scip(&root);
    write_synthetic_scip_with_method(&root.join(".doc-lint").join("code.scip"));

    let _check = run_doc_linter(&root, &["check", "--no-vale"]);

    let method_symbol = "rust-analyzer cargo pricing-core 1.0.0 src/lib.rs/PricingRule/validate().";
    let out = run_doc_linter(&root, &["query", "function-context", method_symbol]);
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    let stderr = String::from_utf8_lossy(&out.stderr).to_string();
    assert!(
        out.status.success(),
        "function-context failed:\n--- stdout ---\n{stdout}\n--- stderr ---\n{stderr}"
    );
    let parsed: serde_json::Value =
        serde_json::from_str(&stdout).expect("function-context emits JSON");
    let links = parsed["links"].as_array().expect("links array");
    assert!(
        links.iter().any(|l| l["edge"] == "method-of"
            && l["kind"] == "type"
            && l["id"]
                .as_str()
                .is_some_and(|s| s.ends_with("/PricingRule#"))),
        "missing method-of link to PricingRule: {links:?}"
    );
    // Type's `kind` column ("struct") shows up as the link title.
    assert!(
        links
            .iter()
            .any(|l| l["edge"] == "method-of" && l["title"] == "struct"),
        "method-of link title should be the Type kind: {links:?}"
    );
}

/// Roadmap issue #32 v6 (v0.4.0): `IMPLEMENTS` edge — Type → Type
/// emitted when a struct/enum/trait's SCIP
/// `SymbolInformation.relationships` carries an entry with
/// `is_implementation: true`. Seeds a struct that implements a
/// trait (both in the same SCIP index so both endpoints land in
/// the `Type` table) and asserts the edge round-trips through
/// SQLite.
#[test]
fn scip_ingest_emits_implements_edge_for_struct_implementing_trait() {
    let root = unique_tmpdir("implements-edge");
    seed_vault_with_scip(&root);
    write_synthetic_scip_with_implements(&root.join(".doc-lint").join("code.scip"));

    let out = run_doc_linter(&root, &["check", "--no-vale"]);
    assert!(
        out.status.success() || !String::from_utf8_lossy(&out.stderr).contains("NOT updated"),
        "check failed with sqlite error: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    let out = run_doc_linter(
        &root,
        &[
            "query",
            "sql",
            "SELECT src AS from_symbol, dst AS to_symbol FROM IMPLEMENTS ORDER BY from_symbol",
        ],
    );
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    let stderr = String::from_utf8_lossy(&out.stderr).to_string();
    assert!(
        out.status.success(),
        "sql IMPLEMENTS query failed:\n--- stdout ---\n{stdout}\n--- stderr ---\n{stderr}"
    );
    let parsed: serde_json::Value = serde_json::from_str(&stdout).expect("query sql emits JSON");
    let rows = parsed["rows"].as_array().expect("rows array");
    assert!(
        !rows.is_empty(),
        "expected at least one IMPLEMENTS edge, got: {parsed}"
    );
    assert!(
        rows.iter().any(|r| r["from_symbol"]
            .as_str()
            .is_some_and(|s| s.ends_with("/PricingRule#"))
            && r["to_symbol"]
                .as_str()
                .is_some_and(|s| s.ends_with("/Validate#"))),
        "expected PricingRule -> Validate IMPLEMENTS, got: {rows:?}"
    );
}

/// Roadmap issue #32 v9 (v0.4.0): post-SPLIT, type-kind facts no
/// longer dual-write into the Function table and instead get full
/// edge coverage via the symmetric TYPE_DEFINED_IN / TYPE_MENTIONS
/// rels. This test reuses the existing seed (a Function `compute`
/// whose doc-comment mentions `pricing-rule`) plus a struct fact
/// whose doc-comment also mentions the same entity. Asserts:
///   - The Function table holds only `compute`, not the struct.
///   - The Type table holds the struct with its kind label.
///   - FUNCTION_MENTIONS connects compute → pricing-rule (unchanged).
///   - TYPE_MENTIONS connects the struct → pricing-rule (new).
///   - TYPE_DEFINED_IN connects the struct → the crate's README Doc.
#[test]
fn split_routes_type_mentions_and_defined_in_via_type_rel_tables() {
    let root = unique_tmpdir("split-type-edges");
    seed_vault_with_scip(&root);
    write_synthetic_scip_with_function_and_type(&root.join(".doc-lint").join("code.scip"));

    let out = run_doc_linter(&root, &["check", "--no-vale"]);
    assert!(
        out.status.success() || !String::from_utf8_lossy(&out.stderr).contains("NOT updated"),
        "check failed with sqlite error: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    // Function table holds only `compute` — the struct must NOT
    // appear there post-SPLIT.
    let out = run_doc_linter(
        &root,
        &[
            "query",
            "sql",
            "SELECT symbol FROM Function ORDER BY symbol",
        ],
    );
    let parsed: serde_json::Value =
        serde_json::from_str(&String::from_utf8_lossy(&out.stdout)).expect("query sql emits JSON");
    let rows = parsed["rows"].as_array().expect("rows array");
    assert!(
        rows.iter().any(|r| r["symbol"]
            .as_str()
            .is_some_and(|s| s.ends_with("/compute()."))),
        "compute must still be a Function: {rows:?}"
    );
    assert!(
        !rows.iter().any(|r| r["symbol"]
            .as_str()
            .is_some_and(|s| s.ends_with("/PricingRule#"))),
        "post-SPLIT: PricingRule must NOT be in the Function table: {rows:?}"
    );

    // Type table holds the struct.
    let out = run_doc_linter(
        &root,
        &[
            "query",
            "sql",
            "SELECT symbol, kind FROM Type ORDER BY symbol",
        ],
    );
    let parsed: serde_json::Value =
        serde_json::from_str(&String::from_utf8_lossy(&out.stdout)).expect("query sql emits JSON");
    let rows = parsed["rows"].as_array().expect("rows array");
    assert!(
        rows.iter().any(|r| r["symbol"]
            .as_str()
            .is_some_and(|s| s.ends_with("/PricingRule#"))
            && r["kind"] == "struct"),
        "PricingRule must be in the Type table with kind=struct: {rows:?}"
    );

    // TYPE_MENTIONS connects struct → entity.
    let out = run_doc_linter(
        &root,
        &[
            "query",
            "sql",
            "SELECT src AS from_symbol, dst AS entity FROM TYPE_MENTIONS ORDER BY from_symbol",
        ],
    );
    let parsed: serde_json::Value =
        serde_json::from_str(&String::from_utf8_lossy(&out.stdout)).expect("query sql emits JSON");
    let rows = parsed["rows"].as_array().expect("rows array");
    assert!(
        rows.iter().any(|r| r["from_symbol"]
            .as_str()
            .is_some_and(|s| s.ends_with("/PricingRule#"))
            && r["entity"] == "pricing-rule"),
        "expected PricingRule -[:TYPE_MENTIONS]-> pricing-rule, got: {rows:?}"
    );

    // TYPE_DEFINED_IN connects struct → crate README Doc.
    let out = run_doc_linter(
        &root,
        &[
            "query",
            "sql",
            "SELECT src AS from_symbol, dst AS doc_id FROM TYPE_DEFINED_IN ORDER BY from_symbol",
        ],
    );
    let parsed: serde_json::Value =
        serde_json::from_str(&String::from_utf8_lossy(&out.stdout)).expect("query sql emits JSON");
    let rows = parsed["rows"].as_array().expect("rows array");
    assert!(
        rows.iter().any(|r| r["from_symbol"]
            .as_str()
            .is_some_and(|s| s.ends_with("/PricingRule#"))
            && r["doc_id"] == "crate-pricing-core"),
        "expected PricingRule -[:TYPE_DEFINED_IN]-> crate-pricing-core, got: {rows:?}"
    );

    // Sanity: FUNCTION_MENTIONS for `compute` still works (no
    // regression from the SPLIT).
    let out = run_doc_linter(
        &root,
        &[
            "query",
            "sql",
            "SELECT src AS symbol, dst AS entity FROM FUNCTION_MENTIONS ORDER BY symbol",
        ],
    );
    let parsed: serde_json::Value =
        serde_json::from_str(&String::from_utf8_lossy(&out.stdout)).expect("query sql emits JSON");
    let rows = parsed["rows"].as_array().expect("rows array");
    assert!(
        rows.iter().any(|r| r["symbol"]
            .as_str()
            .is_some_and(|s| s.ends_with("/compute()."))
            && r["entity"] == "pricing-rule"),
        "FUNCTION_MENTIONS for compute must still fire post-SPLIT: {rows:?}"
    );
}

/// SCIP fixture: one Function (`compute`) and one struct
/// (`PricingRule`), each with a doc-comment that mentions the
/// `pricing-rule` entity. The two should land in different node
/// tables post-SPLIT.
fn write_synthetic_scip_with_function_and_type(out: &Path) {
    use scip::types::symbol_information::Kind as ScipKind;
    use scip::types::{Document, Index, Occurrence, SymbolInformation};

    if let Some(p) = out.parent() {
        std::fs::create_dir_all(p).unwrap();
    }

    let mut index = Index::default();
    let mut doc = Document::default();
    doc.relative_path = "crates/pricing-core/src/lib.rs".to_string();
    doc.language = "rust".to_string();

    let fn_symbol = "rust-analyzer cargo pricing-core 1.0.0 src/lib.rs/compute().";
    let mut sym_fn = SymbolInformation::default();
    sym_fn.symbol = fn_symbol.to_string();
    sym_fn.documentation = vec!["Computes a pricing rule for an order.".to_string()];
    sym_fn.kind = ScipKind::Function.into();
    doc.symbols.push(sym_fn);

    let mut occ_fn = Occurrence::default();
    occ_fn.symbol = fn_symbol.to_string();
    occ_fn.range = vec![0, 4, 0, 11];
    occ_fn.symbol_roles = 1;
    doc.occurrences.push(occ_fn);

    let type_symbol = "rust-analyzer cargo pricing-core 1.0.0 src/lib.rs/PricingRule#";
    let mut sym_ty = SymbolInformation::default();
    sym_ty.symbol = type_symbol.to_string();
    sym_ty.documentation = vec!["A pricing rule used by compute.".to_string()];
    sym_ty.kind = ScipKind::Struct.into();
    doc.symbols.push(sym_ty);

    let mut occ_ty = Occurrence::default();
    occ_ty.symbol = type_symbol.to_string();
    occ_ty.range = vec![3, 7, 3, 18];
    occ_ty.symbol_roles = 1;
    doc.occurrences.push(occ_ty);

    index.documents.push(doc);
    let bytes = protobuf::Message::write_to_bytes(&index).expect("encode");
    std::fs::write(out, bytes).expect("write scip");
}

/// Roadmap issue #32 v8 (v0.4.0): `EXTENDS` edge — Type → Type
/// emitted when both FROM and TO are trait-kind symbols (trait
/// inheritance). Same SCIP relationship shape as IMPLEMENTS, but
/// the post-walk classifier routes (Trait, Trait) pairs to the
/// EXTENDS rel table.
#[test]
fn scip_ingest_emits_extends_edge_for_trait_inheriting_trait() {
    let root = unique_tmpdir("extends-edge");
    seed_vault_with_scip(&root);
    write_synthetic_scip_with_trait_inheritance(&root.join(".doc-lint").join("code.scip"));

    let out = run_doc_linter(&root, &["check", "--no-vale"]);
    assert!(
        out.status.success() || !String::from_utf8_lossy(&out.stderr).contains("NOT updated"),
        "check failed with sqlite error: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    let out = run_doc_linter(
        &root,
        &[
            "query",
            "sql",
            "SELECT src AS from_symbol, dst AS to_symbol FROM EXTENDS ORDER BY from_symbol",
        ],
    );
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    let stderr = String::from_utf8_lossy(&out.stderr).to_string();
    assert!(
        out.status.success(),
        "sql EXTENDS query failed:\n--- stdout ---\n{stdout}\n--- stderr ---\n{stderr}"
    );
    let parsed: serde_json::Value = serde_json::from_str(&stdout).expect("query sql emits JSON");
    let rows = parsed["rows"].as_array().expect("rows array");
    assert!(
        !rows.is_empty(),
        "expected at least one EXTENDS edge, got: {parsed}"
    );
    assert!(
        rows.iter().any(|r| r["from_symbol"]
            .as_str()
            .is_some_and(|s| s.ends_with("/FastValidate#"))
            && r["to_symbol"]
                .as_str()
                .is_some_and(|s| s.ends_with("/Validate#"))),
        "expected FastValidate -> Validate EXTENDS, got: {rows:?}"
    );

    // And the converse: the (Trait, Trait) pair MUST NOT also land
    // in IMPLEMENTS — the classifier is exclusive.
    let out = run_doc_linter(
        &root,
        &[
            "query",
            "sql",
            "SELECT src AS from_symbol, dst AS to_symbol FROM IMPLEMENTS \
             WHERE instr(src, 'FastValidate') > 0",
        ],
    );
    let parsed: serde_json::Value =
        serde_json::from_str(&String::from_utf8_lossy(&out.stdout)).expect("query sql emits JSON");
    let rows = parsed["rows"].as_array().expect("rows array");
    assert!(
        rows.is_empty(),
        "(Trait, Trait) must not double-up as IMPLEMENTS: {rows:?}"
    );
}

/// Variant that defines two traits where the child trait declares
/// `is_implementation` against the parent. This is the canonical
/// shape rust-analyzer emits for `trait FastValidate: Validate`.
fn write_synthetic_scip_with_trait_inheritance(out: &Path) {
    use scip::types::symbol_information::Kind as ScipKind;
    use scip::types::{Document, Index, Occurrence, Relationship, SymbolInformation};

    if let Some(p) = out.parent() {
        std::fs::create_dir_all(p).unwrap();
    }

    let mut index = Index::default();
    let mut doc = Document::default();
    doc.relative_path = "crates/pricing-core/src/lib.rs".to_string();
    doc.language = "rust".to_string();

    let parent_trait = "rust-analyzer cargo pricing-core 1.0.0 src/lib.rs/Validate#";
    let mut sym_parent = SymbolInformation::default();
    sym_parent.symbol = parent_trait.to_string();
    sym_parent.documentation = vec!["base validation trait".to_string()];
    sym_parent.kind = ScipKind::Trait.into();
    doc.symbols.push(sym_parent);

    let mut occ_parent = Occurrence::default();
    occ_parent.symbol = parent_trait.to_string();
    occ_parent.range = vec![0, 6, 0, 14];
    occ_parent.symbol_roles = 1;
    doc.occurrences.push(occ_parent);

    let child_trait = "rust-analyzer cargo pricing-core 1.0.0 src/lib.rs/FastValidate#";
    let mut sym_child = SymbolInformation::default();
    sym_child.symbol = child_trait.to_string();
    sym_child.documentation = vec!["faster validate variant".to_string()];
    sym_child.kind = ScipKind::Trait.into();
    let mut rel = Relationship::default();
    rel.symbol = parent_trait.to_string();
    rel.is_implementation = true;
    sym_child.relationships.push(rel);
    doc.symbols.push(sym_child);

    let mut occ_child = Occurrence::default();
    occ_child.symbol = child_trait.to_string();
    occ_child.range = vec![3, 6, 3, 18];
    occ_child.symbol_roles = 1;
    doc.occurrences.push(occ_child);

    index.documents.push(doc);
    let bytes = protobuf::Message::write_to_bytes(&index).expect("encode");
    std::fs::write(out, bytes).expect("write scip");
}

/// Variant that defines both a trait and a struct that implements
/// it, attaching an `is_implementation` relationship from the
/// struct's `SymbolInformation` to the trait. Same SCIP doc shape
/// rust-analyzer emits for `impl Validate for PricingRule`.
fn write_synthetic_scip_with_implements(out: &Path) {
    use scip::types::symbol_information::Kind as ScipKind;
    use scip::types::{Document, Index, Occurrence, Relationship, SymbolInformation};

    if let Some(p) = out.parent() {
        std::fs::create_dir_all(p).unwrap();
    }

    let mut index = Index::default();
    let mut doc = Document::default();
    doc.relative_path = "crates/pricing-core/src/lib.rs".to_string();
    doc.language = "rust".to_string();

    let trait_symbol = "rust-analyzer cargo pricing-core 1.0.0 src/lib.rs/Validate#";
    let mut sym_trait = SymbolInformation::default();
    sym_trait.symbol = trait_symbol.to_string();
    sym_trait.documentation = vec!["validation trait".to_string()];
    sym_trait.kind = ScipKind::Trait.into();
    doc.symbols.push(sym_trait);

    let mut occ_trait = Occurrence::default();
    occ_trait.symbol = trait_symbol.to_string();
    occ_trait.range = vec![0, 6, 0, 14];
    occ_trait.symbol_roles = 1;
    doc.occurrences.push(occ_trait);

    let struct_symbol = "rust-analyzer cargo pricing-core 1.0.0 src/lib.rs/PricingRule#";
    let mut sym_struct = SymbolInformation::default();
    sym_struct.symbol = struct_symbol.to_string();
    sym_struct.documentation = vec!["struct holding a pricing rule".to_string()];
    sym_struct.kind = ScipKind::Struct.into();
    let mut rel = Relationship::default();
    rel.symbol = trait_symbol.to_string();
    rel.is_implementation = true;
    sym_struct.relationships.push(rel);
    doc.symbols.push(sym_struct);

    let mut occ_struct = Occurrence::default();
    occ_struct.symbol = struct_symbol.to_string();
    occ_struct.range = vec![3, 7, 3, 18];
    occ_struct.symbol_roles = 1;
    doc.occurrences.push(occ_struct);

    index.documents.push(doc);
    let bytes = protobuf::Message::write_to_bytes(&index).expect("encode");
    std::fs::write(out, bytes).expect("write scip");
}

/// Roadmap issue #33 v3: running `check` twice on an unchanged
/// vault must take the cache-hit fast path on the second call —
/// SCIP parse is skipped, the rederive helper rebuilds cross-bucket
/// edges, and queries return the same results. This is the
/// concrete acceptance check for the 2x speedup criterion.
#[test]
fn second_check_takes_cache_hit_rederive_path() {
    let root = unique_tmpdir("cache-hit-rederive");
    seed_vault_with_scip(&root);

    // First run: cache miss, full SCIP ingest, cache file written.
    let first = run_doc_linter(&root, &["check", "--no-vale"]);
    let first_stderr = String::from_utf8_lossy(&first.stderr).to_string();
    assert!(
        first_stderr.contains("scip ingest (sqlite) — ") && !first_stderr.contains("cache hit"),
        "first run should be a full ingest, no cache-hit line; got: {first_stderr}"
    );
    assert!(
        root.join(".doc-lint/ingest-cache.sqlite.json").exists(),
        "first run should write the ingest cache"
    );

    // Second run: cache file present + SCIP unchanged → cache hit.
    let second = run_doc_linter(&root, &["check", "--no-vale"]);
    let second_stderr = String::from_utf8_lossy(&second.stderr).to_string();
    assert!(
        second_stderr.contains("cache hit, rederived"),
        "second run should take the cache-hit rederive path; got: {second_stderr}"
    );

    // Queries against the rederived graph must still produce the
    // same FUNCTION_MENTIONS result.
    let out = run_doc_linter(&root, &["query", "functions-mentioning", "pricing-rule"]);
    assert!(
        out.status.success(),
        "post-rederive query failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let parsed: serde_json::Value =
        serde_json::from_str(&String::from_utf8_lossy(&out.stdout)).expect("emits JSON");
    let functions = parsed["functions"].as_array().expect("functions array");
    assert_eq!(
        functions.len(),
        1,
        "rederive should preserve the FUNCTION_MENTIONS edge; got: {parsed}"
    );
    assert_eq!(
        functions[0]["symbol"].as_str(),
        Some("rust-analyzer cargo pricing-core 1.0.0 src/lib.rs/compute().")
    );
}

/// #260: endpoints come from source, not SCIP, so one added or
/// removed between two cache-hit `check` runs is picked up without a
/// re-index.
#[test]
fn cache_hit_rereads_endpoints() {
    let root = unique_tmpdir("cache-hit-endpoints");
    seed_vault_with_scip(&root);
    let _ = run_doc_linter(&root, &["check", "--no-vale"]); // prime the cache

    let endpoint_ids = |root: &Path| -> Vec<String> {
        let check = run_doc_linter(root, &["check", "--no-vale"]);
        let stderr = String::from_utf8_lossy(&check.stderr).to_string();
        assert!(stderr.contains("cache hit, rederived"), "{stderr}");
        let out = run_doc_linter(root, &["query", "endpoints"]);
        let parsed: serde_json::Value =
            serde_json::from_str(&String::from_utf8_lossy(&out.stdout)).expect("emits JSON");
        parsed
            .as_array()
            .expect("array")
            .iter()
            .filter_map(|r| r["id"].as_str().map(str::to_string))
            .collect()
    };

    let api = root.join("src/api.rs");
    write(
        &api,
        "/// Lists prices.\n///\n/// @endpoint GET /v1/prices\npub async fn list_prices() {}\n",
    );
    let added = endpoint_ids(&root);
    assert!(
        added.iter().any(|id| id == "axum:GET:/v1/prices"),
        "added endpoint missing on cache hit: {added:?}"
    );

    std::fs::remove_file(&api).unwrap();
    let removed = endpoint_ids(&root);
    assert!(
        !removed.iter().any(|id| id == "axum:GET:/v1/prices"),
        "removed endpoint still present on cache hit: {removed:?}"
    );
}

/// #264: File / Finding / COUPLED_WITH come from source and git, not
/// SCIP, so a cache-hit `check` refreshes them, without duplicating
/// File rows or losing the SCIP-derived IMPORTS between files.
#[test]
fn cache_hit_refreshes_source_derived_passes() {
    let root = unique_tmpdir("cache-hit-source-passes");
    seed_vault_with_scip(&root);
    write(&root.join("src/a.rs"), "pub fn a() {}\n");
    let _ = run_doc_linter(&root, &["check", "--no-vale"]); // prime the cache
    let count = |q: &str| -> i64 {
        let out = run_doc_linter(&root, &["query", "sql", q]);
        let v: serde_json::Value = serde_json::from_slice(&out.stdout).expect("sql json");
        v["rows"][0]["n"].as_i64().unwrap_or(-1)
    };
    write(
        &root.join("src/b.rs"),
        "// TODO: wire up b\npub fn b() {}\n",
    );
    let check = run_doc_linter(&root, &["check", "--no-vale"]);
    let stderr = String::from_utf8_lossy(&check.stderr).to_string();
    assert!(stderr.contains("cache hit, rederived"), "{stderr}");
    for path in ["src/a.rs", "src/b.rs"] {
        assert_eq!(
            count(&format!(
                "SELECT count(*) AS n FROM File WHERE path = '{path}'"
            )),
            1,
            "{path}: File row missing or duplicated on cache hit"
        );
    }
    assert_eq!(
        count("SELECT count(*) AS n FROM HAS_FINDING WHERE src = 'src/b.rs'"),
        1,
        "new TODO not seen on cache hit"
    );
}

/// #268: a call through an interface method lands in the graph as a
/// `dispatch` CALLS edge to the implementation too, so callers / impact
/// see the implementation as called.
#[test]
fn interface_call_reaches_implementation_in_graph() {
    use scip::types::symbol_information::Kind as ScipKind;
    use scip::types::{Document, Index, Occurrence, Relationship, SymbolInformation};
    let root = unique_tmpdir("interface-dispatch");
    seed_vault_with_scip(&root);
    let pkg = "semanticdb maven . 1.0 org/x/";
    let mut doc = Document::default();
    doc.relative_path = "src/X.java".to_string();
    for (line, sym, kind, implements) in [
        (0, "Handler#process().", ScipKind::AbstractMethod, None),
        (
            1,
            "LoanHandler#process().",
            ScipKind::Method,
            Some("Handler#process()."),
        ),
        (2, "Runner#run().", ScipKind::Method, None),
    ] {
        let mut info = SymbolInformation::default();
        info.symbol = format!("{pkg}{sym}");
        info.kind = kind.into();
        if let Some(target) = implements {
            let mut rel = Relationship::default();
            rel.symbol = format!("{pkg}{target}");
            rel.is_implementation = true;
            info.relationships.push(rel);
        }
        doc.symbols.push(info);
        let mut occ = Occurrence::default();
        occ.symbol = format!("{pkg}{sym}");
        occ.symbol_roles = 1;
        occ.range = vec![line, 0, 1];
        occ.enclosing_range = vec![line, 0, line, 80];
        doc.occurrences.push(occ);
    }
    let mut call = Occurrence::default();
    call.symbol = format!("{pkg}Handler#process().");
    call.range = vec![2, 20, 27];
    doc.occurrences.push(call);
    let mut index = Index::default();
    index.documents.push(doc);
    std::fs::write(
        root.join(".doc-lint/code.scip"),
        protobuf::Message::write_to_bytes(&index).unwrap(),
    )
    .unwrap();

    let _ = run_doc_linter(&root, &["check", "--no-vale"]);
    let out = run_doc_linter(
        &root,
        &[
            "query",
            "sql",
            "SELECT src AS a, dst AS b, dispatch AS d FROM CALLS ORDER BY dst",
        ],
    );
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).expect("sql json");
    let rows: Vec<String> = v["rows"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| {
            format!(
                "{} -> {} {}",
                r["a"].as_str().unwrap().rsplit('/').next().unwrap(),
                r["b"].as_str().unwrap().rsplit('/').next().unwrap(),
                r["d"]
            )
        })
        .collect();
    assert_eq!(
        rows,
        [
            "Runner#run(). -> Handler#process(). 0",
            "Runner#run(). -> LoanHandler#process(). 1",
        ]
    );
}

/// #262: a frontmatter-less `.adoc` chapter binds to the concepts its
/// title and headings name (an `inferred` COVERS edge), and its
/// `xref:` links become doc → doc edges.
#[test]
fn adoc_chapter_infers_covers_and_links_by_xref() {
    let root = unique_tmpdir("adoc-covers");
    seed_vault_with_scip(&root);
    write(
        &root.join("docs/guide/rules.adoc"),
        "= Rule Engine\n\nHow the engine runs.\n\n== Ordering\n\nSee xref:other.adoc#top[the other chapter].\n",
    );
    write(
        &root.join("docs/guide/other.adoc"),
        "= Other Chapter\n\nNothing about concepts.\n",
    );
    let _ = run_doc_linter(&root, &["check", "--no-vale"]);
    let rows = |q: &str| -> serde_json::Value {
        let out = run_doc_linter(&root, &["query", "sql", q]);
        serde_json::from_slice::<serde_json::Value>(&out.stdout).expect("sql json")["rows"].clone()
    };
    let covers = rows(
        "SELECT d.id AS doc, c.dst AS ent, c.inferred AS inferred FROM Doc d \
         JOIN COVERS c ON c.src = d.id WHERE d.path LIKE '%.adoc'",
    );
    assert_eq!(
        covers,
        serde_json::json!([{"doc": "docs-guide-rules", "ent": "pricing-rule", "inferred": 1}]),
        "{covers}"
    );
    let links = rows("SELECT src AS a, dst AS b FROM MD_LINK");
    assert_eq!(
        links,
        serde_json::json!([{"a": "docs-guide-rules", "b": "docs-guide-other"}]),
        "{links}"
    );
}

/// Roadmap issue #33 v3: `--rebuild` forces the full ingest path
/// even when the cache file is present, recovering the type-prop
/// behavior the rederive path documents as a carve-out.
#[test]
fn rebuild_flag_overrides_cache_hit() {
    let root = unique_tmpdir("cache-rebuild-override");
    seed_vault_with_scip(&root);

    // Prime the cache.
    let _ = run_doc_linter(&root, &["check", "--no-vale"]);

    // --rebuild should bypass the cache regardless of state.
    let out = run_doc_linter(&root, &["check", "--no-vale", "--rebuild"]);
    let stderr = String::from_utf8_lossy(&out.stderr).to_string();
    assert!(
        !stderr.contains("cache hit, rederived"),
        "--rebuild must skip the rederive path; got: {stderr}"
    );
    assert!(
        stderr.contains("scip ingest (sqlite) — ") && stderr.contains("functions"),
        "--rebuild should run the full SCIP ingest; got: {stderr}"
    );
}

/// Asserts the [[entity-doc-graph]] `check` is silent when no SCIP
/// index exists — missing-indexer is not a lint failure.
#[test]
fn check_without_scip_file_does_not_emit_diagnostics() {
    // Sanity: a vault without a SCIP file at the conventional path
    // produces no scip-* diagnostic and the lint count is unchanged.
    let root = unique_tmpdir("no-scip");
    seed_vault_with_scip(&root);
    // Remove the scip file to simulate no indexer ever ran.
    std::fs::remove_file(root.join(".doc-lint/code.scip")).unwrap();

    let out = run_doc_linter(&root, &["check", "--no-vale"]);
    let stderr = String::from_utf8_lossy(&out.stderr).to_string();
    assert!(
        !stderr.contains("scip-missing"),
        "scip-missing should be silent without scip_required: stderr was {stderr}"
    );
    assert!(
        !stderr.contains("scip-stale"),
        "scip-stale should be silent without a file: {stderr}"
    );
}

/// TypeScript sources with no tsconfig.json / package.json used to be
/// skipped by `scip-index` without a word, so they silently never reached
/// the graph. Now the skip is reported with the fix.
#[test]
fn scip_index_reports_sources_without_a_project_file() {
    let root = unique_tmpdir("scip-no-marker");
    write(&root.join("web/api.ts"), "export const x = 1;\n");
    // Nothing gets indexed, so the memory pre-flight has nothing to guard.
    let out = std::process::Command::new(common::doc_linter_bin())
        .env("DOC_LINTER_SCIP_MIN_FREE_MB", "0")
        .arg("--root")
        .arg(&root)
        .arg("scip-index")
        .output()
        .expect("spawn doc-linter");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("typescript sources found but no tsconfig.json / package.json"),
        "skip not reported:\n{stderr}"
    );
}

/// Legacy gap 2, layout (a): a vendored / submodule tree whose code joins
/// the graph but whose docs don't. `exempt` (docs only) keeps its .md /
/// .adoc out of the lint and the Doc table; `skip_dirs` is not used, so
/// its code is still ingested and `scip-index` still indexes it.
#[test]
fn exempt_tree_keeps_code_but_drops_docs() {
    let root = unique_tmpdir("layout-a");
    seed_vault_with_scip(&root);
    let cfg = root.join(".doc-lint.toml");
    let text = std::fs::read_to_string(&cfg).unwrap().replace(
        "exempt = []",
        "exempt = [\"crates/pricing-core/**\", \"legacy/**\"]",
    );
    std::fs::write(&cfg, text).unwrap();
    // Frontmatter-less docs inside the exempt trees, and one main doc that
    // must still be linted.
    write(
        &root.join("crates/pricing-core/NOTES.md"),
        "# Notes\n\nno frontmatter\n",
    );
    write(
        &root.join("crates/pricing-core/guide.adoc"),
        "= Guide\n\nText.\n",
    );
    write(&root.join("legacy/README.md"), "# Legacy\n");
    write(
        &root.join("legacy/package.json"),
        "{\"name\": \"legacy\"}\n",
    );
    write(
        &root.join("legacy/a.ts"),
        "export function total(x: number): number { return helper(x); }\n\
         function helper(x: number): number { return x + 1; }\n",
    );
    write(
        &root.join("docs/bad.md"),
        "# Main doc with no frontmatter\n",
    );

    let check = run_doc_linter(&root, &["check", "--no-vale", "--format", "json"]);
    let report: serde_json::Value = serde_json::from_slice(&check.stdout).expect("json");
    let files: Vec<&str> = report["report"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|e| e["file"].as_str())
        .collect();
    assert!(
        files.iter().any(|f| f.ends_with("docs/bad.md")),
        "main docs not linted: {files:?}"
    );
    assert!(
        !files
            .iter()
            .any(|f| f.contains("pricing-core/") || f.contains("legacy/")),
        "exempt docs linted: {files:?}"
    );

    let cypher = |q: &str| -> serde_json::Value {
        let out = run_doc_linter(&root, &["query", "sql", q]);
        serde_json::from_slice(&out.stdout).expect("sql json")
    };
    let docs = cypher(
        "SELECT count(*) AS n FROM Doc WHERE instr(path, 'pricing-core/') > 0 \
         OR path LIKE 'legacy/%'",
    );
    assert_eq!(
        docs["rows"][0]["n"], 0,
        "exempt docs became Doc nodes: {docs}"
    );
    let fns = cypher("SELECT count(*) AS n FROM Function WHERE file LIKE 'crates/pricing-core/%'");
    assert_eq!(
        fns["rows"][0]["n"], 1,
        "exempt tree's code not ingested: {fns}"
    );

    // scip-index still walks the exempt tree (needs scip-typescript).
    if which::which("scip-typescript").is_err() {
        return;
    }
    let out = std::process::Command::new(common::doc_linter_bin())
        .env("DOC_LINTER_SCIP_MIN_FREE_MB", "0")
        .arg("--root")
        .arg(&root)
        .arg("scip-index")
        .output()
        .expect("spawn doc-linter");
    let part = std::fs::read(root.join(".doc-lint/scip/typescript.scip")).unwrap_or_default();
    assert!(
        part.windows(b"legacy/a.ts".len())
            .any(|w| w == b"legacy/a.ts"),
        "scip-index skipped the exempt tree:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// Legacy gap 7: a property read inside a function lands as a Field node
/// plus a REFERENCES edge, and `query saved field-references` finds it by
/// a partial name.
#[test]
fn field_references_round_trip() {
    use scip::types::{Document, Index, Occurrence, SymbolInformation};

    let root = unique_tmpdir("field-refs");
    seed_vault_with_scip(&root);
    let base = "scip-dotnet nuget . . Lib/Pricing#Base().";
    let field = "scip-dotnet nuget . . Lib/Product#IsFreeProduct.";
    let mut doc = Document::default();
    doc.relative_path = "Lib/Pricing.cs".to_string();
    for s in [base, field] {
        let mut sym = SymbolInformation::default();
        sym.symbol = s.to_string();
        doc.symbols.push(sym);
    }
    for (s, line) in [(base, 8), (field, 2)] {
        let mut def = Occurrence::default();
        def.symbol = s.to_string();
        def.range = vec![line, 4, line, 10];
        def.symbol_roles = 1;
        doc.occurrences.push(def);
    }
    let mut read = Occurrence::default();
    read.symbol = field.to_string();
    read.range = vec![9, 12, 9, 25];
    doc.occurrences.push(read);
    let mut index = Index::default();
    index.documents.push(doc);
    std::fs::write(
        root.join(".doc-lint/code.scip"),
        protobuf::Message::write_to_bytes(&index).unwrap(),
    )
    .unwrap();

    let _check = run_doc_linter(&root, &["check", "--no-vale", "--rebuild"]);
    let out = run_doc_linter(
        &root,
        &[
            "query",
            "saved",
            "field-references",
            "--param",
            "name=IsFree",
        ],
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains(field), "field not found:\n{stdout}");
    assert!(stdout.contains(base), "reader not found:\n{stdout}");
    assert!(
        stdout.contains("\"line\": 10"),
        "reference line missing:\n{stdout}"
    );
}
