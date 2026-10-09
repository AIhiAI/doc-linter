#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "integration tests panic on broken invariants — that is the point"
)]
//! Phase 2 of roadmap-43: end-to-end Endpoint extraction pipeline test.
//!
//! Builds a minimal tmp-dir Rust source tree with one axum router file
//! and a clap-style command enum, runs `doc-linter check` to ingest
//! them into the graph, then asserts:
//!
//!   - `query endpoints` returns the expected list.
//!   - `query endpoints --kind axum` filters to axum only.
//!   - `query endpoints --dark` lists endpoints with no entity edge.
//!
//! No SCIP file is needed for the extraction itself — endpoints are
//! pulled from .rs source via tree-sitter. The handler-symbol resolver
//! is exercised separately in the unit tests inside
//! `endpoint_extract.rs`; here we just verify the wiring.

use std::path::{Path, PathBuf};
use std::process::Command;

mod common;
use common::{doc_linter_bin, run_doc_linter, unique_tmpdir, write};

fn seed_vault(root: &Path) {
    write(
        &root.join(".doc-lint.toml"),
        r#"
exempt = []
required_fields = ["id", "role", "title", "summary", "status", "updated"]
allowed_statuses = ["draft", "stable"]
vale_enabled = false
clap_crates = ["example-api"]
# Roadmap-49 phase 3 flipped the default to `true`; this test
# fixture predates markers so it relies on the regex extractor.
endpoint_marker_exclusive = false
"#,
    );

    // Minimal ontology axes / values so cmd_check doesn't bail.
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

    // One entity so the ENTITY/COVERS path resolves at all (the
    // endpoint pipeline emits ENDPOINT_TOUCHES_ENTITY edges via
    // FUNCTION_MENTIONS — but we don't have SCIP in this fixture so
    // every endpoint stays dark, which is exactly what `--dark`
    // tests).
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

    // Minimal crate README so cmd_check has a non-empty doc set.
    write(
        &root.join("crates/example-api/README.md"),
        "---\n\
         id: crate-example-api\n\
         role: doc\n\
         kind: reference\n\
         title: example-api\n\
         summary: tenant-facing API\n\
         status: stable\n\
         updated: 2026-04-30\n\
         covers: [outlet]\n\
         ---\n\n# example-api\n\nLinks to [[entity-outlet]].\n",
    );

    // Axum router: two routes + one nested router resolves cross-fn.
    write(
        &root.join("crates/example-api/src/lib.rs"),
        r#"
            use axum::Router;
            use axum::routing::{get, post};

            pub fn outlets_router() -> Router {
                Router::new()
                    .route("/outlets/:id", get(get_outlet))
                    .route("/outlets", post(create_outlet))
            }

            pub fn root_router() -> Router {
                Router::new()
                    .route("/healthz", get(healthz))
                    .nest("/v1", outlets_router())
            }

            async fn healthz() {}
            async fn get_outlet() {}
            async fn create_outlet() {}
        "#,
    );

    // Clap-style command enum.
    write(
        &root.join("crates/example-api/src/main.rs"),
        r"
            use clap::Subcommand;

            #[derive(Subcommand)]
            enum Commands {
                Check,
                ScipIndex,
                Lsp,
            }
        ",
    );
}

/// End-to-end pipeline test: asserts that the [[entity-doc-graph]]
/// extractor + ingest + `query endpoints` round-trip lists every axum
/// route, clap subcommand, and MCP tool.
#[test]
fn endpoint_pipeline_extracts_axum_clap_and_lists_them() {
    let root = unique_tmpdir("pipeline");
    seed_vault(&root);

    let _check = run_doc_linter(&root, &["check", "--no-vale"]);
    assert!(root.join(".doc-lint/graph.sqlite").exists());

    // `query endpoints` returns every endpoint regardless of kind.
    let out = run_doc_linter(&root, &["query", "endpoints"]);
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    let stderr = String::from_utf8_lossy(&out.stderr).to_string();
    assert!(
        out.status.success(),
        "query endpoints failed:\n--- stdout ---\n{stdout}\n--- stderr ---\n{stderr}"
    );
    let parsed: serde_json::Value = serde_json::from_str(&stdout).expect("endpoints emits JSON");
    let rows = parsed.as_array().expect("array");

    let ids: Vec<&str> = rows.iter().filter_map(|r| r["id"].as_str()).collect();

    // Axum: healthz GET, /v1/outlets/:id GET (path-prefixed by .nest),
    // /v1/outlets POST.
    assert!(
        ids.contains(&"axum:GET:/healthz"),
        "expected /healthz GET in {ids:?}"
    );
    assert!(
        ids.contains(&"axum:GET:/v1/outlets/:id"),
        "expected /v1/outlets/:id GET (path-prefixed nest) in {ids:?}"
    );
    assert!(
        ids.contains(&"axum:POST:/v1/outlets"),
        "expected /v1/outlets POST in {ids:?}"
    );
    // Clap: three variants → kebab-cased.
    assert!(
        ids.contains(&"clap:cli:check"),
        "expected clap check in {ids:?}"
    );
    assert!(
        ids.contains(&"clap:cli:scip-index"),
        "expected clap scip-index in {ids:?}"
    );
    assert!(
        ids.contains(&"clap:cli:lsp"),
        "expected clap lsp in {ids:?}"
    );
}

/// Asserts that `query endpoints --kind axum` returns only axum rows
/// in the [[entity-doc-graph]] pipeline.
#[test]
fn endpoint_pipeline_kind_filter_axum() {
    let root = unique_tmpdir("kind-filter");
    seed_vault(&root);
    let _check = run_doc_linter(&root, &["check", "--no-vale"]);

    let out = run_doc_linter(&root, &["query", "endpoints", "--kind", "axum"]);
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    assert!(out.status.success(), "axum filter failed:\n{stdout}");
    let parsed: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    let rows = parsed.as_array().unwrap();
    // Every row's kind is "axum".
    for row in rows {
        assert_eq!(
            row["kind"].as_str(),
            Some("axum"),
            "non-axum row in axum filter: {row:?}"
        );
    }
    assert!(!rows.is_empty(), "expected at least one axum endpoint");
}

/// Asserts that `query endpoints --dark` returns endpoints with no
/// `ENDPOINT_TOUCHES_ENTITY` edge in the [[entity-doc-graph]] pipeline.
#[test]
fn endpoint_pipeline_dark_lists_endpoints_with_no_entity() {
    let root = unique_tmpdir("dark");
    seed_vault(&root);
    let _check = run_doc_linter(&root, &["check", "--no-vale"]);

    // No SCIP fixture in this test → no FUNCTION_MENTIONS exists →
    // every endpoint stays dark. `--dark` should return them all.
    let out = run_doc_linter(&root, &["query", "endpoints", "--dark"]);
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    assert!(out.status.success(), "dark filter failed:\n{stdout}");
    let parsed: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    let rows = parsed.as_array().unwrap();
    assert!(!rows.is_empty(), "expected dark endpoints");
    for row in rows {
        let entities = row["entities"].as_array().expect("entities array");
        assert!(
            entities.is_empty(),
            "dark endpoint should have no entities: {row:?}"
        );
    }
}

// ---------- roadmap-49 phase 1 + 2 tests ----------

/// Minimal vault seed for the marker-only test cases. The marker
/// pipeline doesn't need an axum router fixture — a single handler
/// with a `///` doc-comment carrying the marker is enough.
fn seed_marker_vault(root: &Path) {
    write(
        &root.join(".doc-lint.toml"),
        r#"
exempt = []
required_fields = ["id", "role", "title", "summary", "status", "updated"]
allowed_statuses = ["draft", "stable"]
vale_enabled = false
clap_crates = []
# Roadmap-49 phase 3 flipped the default to `true`; keep the
# dual-pipeline marker tests on the merge-and-dedup path so they
# exercise the regex fallback alongside the marker reader.
endpoint_marker_exclusive = false
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
    {
        let v = &"doc";
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
        &root.join("crates/example-api/README.md"),
        "---\n\
         id: crate-example-api\n\
         role: doc\n\
         kind: reference\n\
         title: example-api\n\
         summary: api crate\n\
         status: stable\n\
         updated: 2026-04-30\n\
         ---\n\n# example-api\n",
    );
}

/// Roadmap-49 phase 1: a handler declared via `@endpoint` with NO
/// `Router::route` registration synthesises an Endpoint node. The
/// regex extractor would have produced nothing here; the marker
/// reader is the sole source of truth.
#[test]
fn markers_synthesize_endpoints() {
    let root = unique_tmpdir("markers-synth");
    seed_marker_vault(&root);
    write(
        &root.join("crates/example-api/src/lib.rs"),
        r"
            /// Returns a single outlet by id.
            ///
            /// @endpoint GET /v1/outlets/:id
            pub async fn get_outlet() {}
        ",
    );

    let _check = run_doc_linter(&root, &["check", "--no-vale"]);
    let out = run_doc_linter(&root, &["query", "endpoints"]);
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    assert!(out.status.success(), "query endpoints failed: {stdout}");
    let parsed: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    let rows = parsed.as_array().unwrap();
    let ids: Vec<&str> = rows.iter().filter_map(|r| r["id"].as_str()).collect();
    assert!(
        ids.contains(&"axum:GET:/v1/outlets/:id"),
        "expected marker-synthesized endpoint in {ids:?}"
    );
}

/// Roadmap-49 phase 1: when a handler has BOTH a `Router::route`
/// registration AND a `@endpoint` marker, exactly one Endpoint node
/// exists for the (method, path) pair — the marker version wins.
#[test]
fn markers_dedupe_with_regex_extractor() {
    let root = unique_tmpdir("markers-dedupe");
    seed_marker_vault(&root);
    write(
        &root.join("crates/example-api/src/lib.rs"),
        r#"
            use axum::Router;
            use axum::routing::get;

            pub fn router() -> Router {
                Router::new().route("/x", get(handler))
            }

            /// @endpoint GET /x
            pub async fn handler() {}
        "#,
    );

    let _check = run_doc_linter(&root, &["check", "--no-vale"]);
    let out = run_doc_linter(&root, &["query", "endpoints"]);
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    assert!(out.status.success(), "query endpoints failed: {stdout}");
    let parsed: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    let rows = parsed.as_array().unwrap();
    let matching: Vec<&serde_json::Value> = rows
        .iter()
        .filter(|r| r["id"].as_str() == Some("axum:GET:/x"))
        .collect();
    assert_eq!(
        matching.len(),
        1,
        "expected exactly one Endpoint for /x; saw {matching:?}"
    );
}

/// Roadmap-49 phase 1: with `endpoint_marker_exclusive = true`, the
/// regex extractor is skipped — a `Router::route` registration with
/// NO marker yields zero Endpoint nodes.
#[test]
fn markers_exclusive_skips_regex() {
    let root = unique_tmpdir("markers-exclusive");
    seed_marker_vault(&root);
    // Override the .doc-lint.toml to flip the knob on.
    write(
        &root.join(".doc-lint.toml"),
        r#"
exempt = []
required_fields = ["id", "role", "title", "summary", "status", "updated"]
allowed_statuses = ["draft", "stable"]
vale_enabled = false
clap_crates = []
endpoint_marker_exclusive = true
"#,
    );
    write(
        &root.join("crates/example-api/src/lib.rs"),
        r#"
            use axum::Router;
            use axum::routing::get;

            pub fn router() -> Router {
                Router::new().route("/x", get(handler))
            }

            pub async fn handler() {}
        "#,
    );

    let _check = run_doc_linter(&root, &["check", "--no-vale"]);
    let out = run_doc_linter(&root, &["query", "endpoints"]);
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    assert!(out.status.success(), "query endpoints failed: {stdout}");
    let parsed: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    let rows = parsed.as_array().unwrap();
    assert!(
        rows.is_empty(),
        "expected no endpoints in marker-exclusive mode without markers; saw {rows:?}"
    );
}

// Tests for the migrate-endpoint-markers subcommand below. They
// live in the same integration test file because the subcommand
// runs against an on-disk source tree and shells out via the
// `CARGO_BIN_EXE` wrapper.

/// Each migrate test gets a unique manual-report path so concurrent
/// runs don't clobber each other. The env var is read inside the
/// child process; we set it before spawning.
fn run_migrate(root: &Path, args: &[&str], manual_path: &Path) -> std::process::Output {
    let mut cmd = Command::new(doc_linter_bin());
    cmd.arg("--root").arg(root);
    cmd.arg("migrate-endpoint-markers");
    for a in args {
        cmd.arg(a);
    }
    cmd.env("DOC_LINTER_ENDPOINT_MARKER_MANUAL_PATH", manual_path);
    cmd.output().expect("spawn doc-linter")
}

/// Init a non-bare git repo with one initial commit so the
/// dirty-tree check sees a tracked tree to begin with.
fn init_git_with_one_commit(root: &Path) {
    let run = |args: &[&str]| {
        let status = std::process::Command::new("git")
            .arg("-C")
            .arg(root)
            .args(args)
            .output()
            .expect("git");
        assert!(
            status.status.success(),
            "git {:?} failed: {}",
            args,
            String::from_utf8_lossy(&status.stderr)
        );
    };
    run(&["init", "-q"]);
    run(&["config", "user.email", "test@example.com"]);
    run(&["config", "user.name", "Test"]);
    run(&["config", "commit.gpgsign", "false"]);
    run(&["add", "-A"]);
    run(&["commit", "-q", "-m", "init"]);
}

/// Seeds the migrate fixture: a vault + one axum router file with a
/// handler that does NOT yet have a marker. Returns the path to
/// the handler source file so the test can assert post-edit.
fn seed_migrate_vault(root: &Path) -> PathBuf {
    seed_marker_vault(root);
    let lib = root.join("crates/example-api/src/lib.rs");
    write(
        &lib,
        "use axum::Router;\n\
         use axum::routing::get;\n\
         \n\
         pub fn router() -> Router {\n\
             Router::new().route(\"/x\", get(handler))\n\
         }\n\
         \n\
         /// Existing prose.\n\
         pub async fn handler() {}\n",
    );
    lib
}

/// Drop a synthetic SCIP file at `<root>/.doc-lint/code.scip`
/// containing one `Function` for the named handler. The SCIP
/// protobuf is non-trivial to author by hand; we use the `scip`
/// crate's proto types so the migrate tool gets a real `FunctionFact`
/// to resolve against.
fn write_synthetic_scip_at(
    root: &Path,
    rel_file: &str,
    handler_name: &str,
    line: u32,
    crate_name: &str,
) {
    use scip::types::symbol_information::Kind as ScipKind;
    use scip::types::{Document, Index, Occurrence, SymbolInformation};

    let symbol = format!("rust-analyzer cargo {crate_name} 0.0.0 test/{handler_name}().");
    let mut occ = Occurrence::default();
    occ.symbol = symbol.clone();
    occ.symbol_roles = 1;
    // [start_line, start_col, end_line, end_col] — 0-based; line 7
    // (1-based) → 6 here.
    occ.range = vec![(line as i32) - 1, 0, (line as i32) - 1, 1];

    let mut sym_info = SymbolInformation::default();
    sym_info.symbol = symbol;
    sym_info.kind = ScipKind::Function.into();
    sym_info.documentation = vec![];

    let mut doc = Document::default();
    doc.relative_path = rel_file.to_string();
    doc.language = "rust".to_string();
    doc.occurrences = vec![occ];
    doc.symbols = vec![sym_info];

    let mut index = Index::default();
    index.documents = vec![doc];

    let bytes = protobuf::Message::write_to_bytes(&index).expect("encode scip");
    let scip_dir = root.join(".doc-lint");
    std::fs::create_dir_all(&scip_dir).unwrap();
    std::fs::write(scip_dir.join("code.scip"), bytes).unwrap();
}

/// Roadmap-49 phase 2 [[entity-doc-graph]]: `--dry-run` lists the @endpoint
/// stamp proposal but never touches the source file.
#[test]
fn migrate_dry_run_emits_proposals_without_writing() {
    let root = unique_tmpdir("migrate-dry");
    let lib = seed_migrate_vault(&root);
    write_synthetic_scip_at(
        &root,
        "crates/example-api/src/lib.rs",
        "handler",
        8,
        "example-api",
    );

    let original = std::fs::read_to_string(&lib).unwrap();
    let manual = root.join("manual-report.txt");
    let out = run_migrate(&root, &["--dry-run"], &manual);
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    let stderr = String::from_utf8_lossy(&out.stderr).to_string();
    assert!(
        out.status.success(),
        "dry-run failed:\nstdout: {stdout}\nstderr: {stderr}"
    );
    assert!(
        stdout.contains("would stamp GET:/x"),
        "expected proposal in stdout; got: {stdout}"
    );
    let after = std::fs::read_to_string(&lib).unwrap();
    assert_eq!(after, original, "dry-run must not modify source");
}

/// Roadmap-49 phase 2 [[entity-doc-graph]]: a non-dry run inserts the
/// marker line ABOVE the existing doc-comment block's end (i.e. between
/// the last `///` line and the `pub async fn` keyword).
#[test]
fn migrate_writes_marker_above_existing_doc_comment() {
    let root = unique_tmpdir("migrate-write");
    let lib = seed_migrate_vault(&root);
    write_synthetic_scip_at(
        &root,
        "crates/example-api/src/lib.rs",
        "handler",
        8,
        "example-api",
    );
    init_git_with_one_commit(&root);

    let manual = root.join("manual-report.txt");
    let out = run_migrate(&root, &[], &manual);
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    let stderr = String::from_utf8_lossy(&out.stderr).to_string();
    assert!(
        out.status.success(),
        "migrate failed:\nstdout: {stdout}\nstderr: {stderr}"
    );
    let after = std::fs::read_to_string(&lib).unwrap();
    assert!(
        after.contains("/// @endpoint GET /x"),
        "expected marker in updated source; got:\n{after}"
    );
    // Sanity: the marker lives ABOVE the `pub async fn handler` line.
    let marker_pos = after.find("/// @endpoint GET /x").expect("marker present");
    let fn_pos = after.find("pub async fn handler").expect("fn present");
    assert!(
        marker_pos < fn_pos,
        "marker should appear above fn; got marker at {marker_pos}, fn at {fn_pos}"
    );
}

/// Roadmap-49 phase 2: a dirty git tree blocks the writer. Exit
/// code 2; stderr explains why.
#[test]
fn migrate_refuses_dirty_tree() {
    let root = unique_tmpdir("migrate-dirty");
    let _lib = seed_migrate_vault(&root);
    write_synthetic_scip_at(
        &root,
        "crates/example-api/src/lib.rs",
        "handler",
        8,
        "example-api",
    );
    init_git_with_one_commit(&root);

    // Touch a tracked file so `git status --porcelain` reports it.
    let dirty = root.join("crates/example-api/README.md");
    let mut contents = std::fs::read_to_string(&dirty).unwrap();
    contents.push_str("\n<!-- dirty -->\n");
    std::fs::write(&dirty, contents).unwrap();

    let manual = root.join("manual-report.txt");
    let out = run_migrate(&root, &[], &manual);
    let stderr = String::from_utf8_lossy(&out.stderr).to_string();
    assert_eq!(
        out.status.code(),
        Some(2),
        "expected exit code 2 on dirty tree; got {:?}\nstderr: {stderr}",
        out.status.code()
    );
    assert!(
        stderr.contains("clean working tree"),
        "expected dirty-tree error message; got: {stderr}"
    );
}

/// Roadmap-49 phase 2: an unresolvable endpoint (closure handler /
/// macro-opaque MCP tool) ends up in the manual report rather than
/// silently disappearing.
#[test]
fn migrate_unresolved_endpoints_go_to_manual_report() {
    let root = unique_tmpdir("migrate-manual");
    seed_marker_vault(&root);
    // McpTool inside a vec! macro — extract_mcp can find the name
    // (via the regex fallback) but no handler symbol resolves.
    write(
        &root.join("crates/fa-mcp/src/lib.rs"),
        r#"
            fn tools() -> Vec<McpTool> {
                vec![
                    McpTool {
                        name: "read_state",
                        description: "x",
                        handler: Arc::new(ReadStateTool),
                    },
                ]
            }
        "#,
    );
    init_git_with_one_commit(&root);

    let manual = root.join("manual-report.txt");
    let out = run_migrate(&root, &["--dry-run"], &manual);
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    let stderr = String::from_utf8_lossy(&out.stderr).to_string();
    assert!(
        out.status.success(),
        "dry-run failed:\nstdout: {stdout}\nstderr: {stderr}"
    );
    assert!(
        manual.exists(),
        "expected manual report at {} after run",
        manual.display()
    );
    let body = std::fs::read_to_string(&manual).unwrap();
    assert!(
        body.contains("read_state"),
        "expected unresolved tool in manual report; got:\n{body}"
    );
}
