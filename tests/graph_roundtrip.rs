#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "integration tests panic on broken invariants — that is the point"
)]
//! Round 2B: Graph store round-trip tests.
//!
//! Builds a tiny ontology + a few docs in a tmp dir, runs the full ingest
//! through the doc-linter binary, then queries the resulting SQLite graph via
//! the query subcommands and via the new raw `query sql` path.
//!
//! The doc-linter binary is the unit under test (rather than calling
//! library functions directly) because we want to guarantee the public
//! CLI contract — that's what AI agents and the `PostToolUse` hook actually
//! invoke.

use std::path::Path;

mod common;
use common::{run_doc_linter, unique_tmpdir, write};

/// Lays down a minimal but realistic vault: an ontology axis/value tree
/// (so `role: doc` validates), a couple of docs with wikilinks, and an
/// entity for the COVERS edge.
fn seed_minimal_vault(root: &Path) {
    write(
        &root.join(".doc-lint.toml"),
        r#"
exempt = []
required_fields = ["id", "role", "title", "summary", "status", "updated"]
allowed_statuses = ["draft", "stable"]
vale_enabled = false
"#,
    );

    // Ontology axis: role
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
    // Ontology values: role=doc, role=index
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
    // Ontology values: kind=reference (so we can test --kind filter)
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
    // One entity for COVERS edge.
    write(
        &root.join("docs/ontology/entities/widget.md"),
        "---\n\
         id: entity-widget\n\
         role: ontology-entity\n\
         title: \"Entity: widget\"\n\
         summary: t\n\
         status: stable\n\
         updated: 2026-04-30\n\
         axis_id: covers\n\
         value_id: widget\n\
         display: Widget\n\
         description: a widget\n\
         ---\n\n# widget\n",
    );

    // Three "real" docs with various edges.
    write(
        &root.join("docs/alpha.md"),
        "---\n\
         id: doc-alpha\n\
         role: doc\n\
         kind: reference\n\
         title: Alpha\n\
         summary: alpha\n\
         status: stable\n\
         updated: 2026-04-30\n\
         covers: [widget]\n\
         ---\n\n# Alpha\n\nLink to [[doc-beta]].\n",
    );
    write(
        &root.join("docs/beta.md"),
        "---\n\
         id: doc-beta\n\
         role: doc\n\
         kind: reference\n\
         title: Beta\n\
         summary: beta\n\
         status: stable\n\
         updated: 2026-04-30\n\
         ---\n\n# Beta\n\nLinks to [[doc-gamma]].\n",
    );
    write(
        &root.join("docs/gamma.md"),
        "---\n\
         id: doc-gamma\n\
         role: doc\n\
         kind: reference\n\
         title: Gamma\n\
         summary: gamma\n\
         status: stable\n\
         updated: 2026-04-30\n\
         ---\n\n# Gamma\n\nBack to [[doc-alpha]].\n",
    );
}

/// Run the doc-linter binary against a fixture root. Used by every test
/// below. We always pass `--no-vale` because the seed doesn't include a
/// Vale config and the test goal is to exercise the graph store, not Vale.

#[test]
fn ingest_then_list_returns_seeded_docs() {
    let root = unique_tmpdir("ingest-then-list");
    seed_minimal_vault(&root);

    // First run: cmd_check ingests into the graph. We allow non-zero exit
    // because the orphan check may flag the ontology fixture entries; we
    // only care that the graph lands.
    let _check = run_doc_linter(&root, &["check", "--no-vale"]);

    let out = run_doc_linter(&root, &["query", "list", "--kind", "reference"]);
    assert!(
        out.status.success(),
        "list failed: stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    let v: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    let ids: Vec<String> = v["docs"]
        .as_array()
        .unwrap()
        .iter()
        .map(|d| d["id"].as_str().unwrap().to_string())
        .collect();
    assert!(ids.contains(&"doc-alpha".to_string()), "ids: {ids:?}");
    assert!(ids.contains(&"doc-beta".to_string()));
    assert!(ids.contains(&"doc-gamma".to_string()));
    assert_eq!(v["count"].as_u64().unwrap() as usize, 3);
}

/// Roundtrip: asserts `query neighbors` reads back the outbound wikilinks
/// previously ingested into the [[entity-doc-graph]] store.
#[test]
fn neighbors_returns_outbound_wikilinks() {
    let root = unique_tmpdir("neighbors");
    seed_minimal_vault(&root);
    let _ = run_doc_linter(&root, &["check", "--no-vale"]);

    let out = run_doc_linter(&root, &["query", "neighbors", "doc-alpha"]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let v: serde_json::Value = serde_json::from_str(&String::from_utf8_lossy(&out.stdout)).unwrap();
    let outbound = v["outbound"].as_array().unwrap();
    // doc-alpha -> doc-beta (wikilink) + doc-alpha -> entity-widget (covers)
    let ids: Vec<&str> = outbound.iter().map(|e| e["id"].as_str().unwrap()).collect();
    assert!(ids.contains(&"doc-beta"), "outbound: {outbound:#?}");
    let edge_types: Vec<&str> = outbound
        .iter()
        .map(|e| e["edge_type"].as_str().unwrap())
        .collect();
    assert!(edge_types.contains(&"wikilink"));
    assert!(edge_types.contains(&"covers"));
}

#[test]
/// Roundtrip: asserts the [[entity-doc-graph]] `query sql` escape
/// hatch returns typed JSON rows for an arbitrary read-only SQL string.
fn sql_subcommand_returns_typed_rows() {
    let root = unique_tmpdir("sql");
    seed_minimal_vault(&root);
    let _ = run_doc_linter(&root, &["check", "--no-vale"]);

    let out = run_doc_linter(
        &root,
        &[
            "query",
            "sql",
            "SELECT id FROM Doc WHERE kind = 'reference' ORDER BY id",
        ],
    );
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let v: serde_json::Value = serde_json::from_str(&String::from_utf8_lossy(&out.stdout)).unwrap();
    assert_eq!(v["row_count"].as_u64().unwrap(), 3);
    let ids: Vec<&str> = v["rows"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["id"].as_str().unwrap())
        .collect();
    assert_eq!(ids, vec!["doc-alpha", "doc-beta", "doc-gamma"]);
}

#[test]
/// Roundtrip: asserts that `query path` resolves a shortest path over
/// the [[entity-doc-graph]] store (recursive CTE).
fn path_uses_sqlite_shortest() {
    // S4 closure regression test: the shortest path resolves in
    // one query and emits the legacy hop list shape. The
    // seed forms an undirected triangle (alpha->beta->gamma->alpha),
    // so alpha->gamma is one undirected hop via the back-edge.
    let root = unique_tmpdir("path-shortest");
    seed_minimal_vault(&root);
    let _ = run_doc_linter(&root, &["check", "--no-vale"]);

    let out = run_doc_linter(&root, &["query", "path", "doc-alpha", "doc-gamma"]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let v: serde_json::Value = serde_json::from_str(&String::from_utf8_lossy(&out.stdout)).unwrap();
    assert_eq!(v["found"], true);
    let hops = v["hops"].as_array().unwrap();
    let ids: Vec<&str> = hops.iter().map(|h| h["id"].as_str().unwrap()).collect();
    assert_eq!(ids, vec!["doc-alpha", "doc-gamma"]);
    // First hop carries no via_*; subsequent hops do.
    assert!(hops[0].get("via_line").is_none());
    assert_eq!(hops[1]["via_edge_type"].as_str().unwrap(), "wikilink");

    // Multi-hop case: filter out the alpha<->gamma back-edge by going
    // alpha->beta, then beta->gamma in two separate queries; both are
    // 1-hop, but together they exercise the chain SHORTEST would walk
    // when no direct edge exists.
    for (from, to) in &[("doc-alpha", "doc-beta"), ("doc-beta", "doc-gamma")] {
        let leg = run_doc_linter(&root, &["query", "path", from, to]);
        let v: serde_json::Value =
            serde_json::from_str(&String::from_utf8_lossy(&leg.stdout)).unwrap();
        assert_eq!(v["found"], true, "leg {from} -> {to}");
        let hops = v["hops"].as_array().unwrap();
        assert_eq!(hops.len(), 2, "leg {from} -> {to}");
        assert_eq!(hops[0]["id"].as_str().unwrap(), *from);
        assert_eq!(hops[1]["id"].as_str().unwrap(), *to);
    }

    // Same-node short-circuit: PathStep with no via_*.
    let same = run_doc_linter(&root, &["query", "path", "doc-alpha", "doc-alpha"]);
    let v: serde_json::Value =
        serde_json::from_str(&String::from_utf8_lossy(&same.stdout)).unwrap();
    assert_eq!(v["found"], true);
    let hops = v["hops"].as_array().unwrap();
    assert_eq!(hops.len(), 1);
    assert_eq!(hops[0]["id"].as_str().unwrap(), "doc-alpha");
    assert!(hops[0].get("via_edge_type").is_none());

    // Edge-type filter that resolves to only non-Doc-Doc edges returns
    // no path (matches BFS behaviour: covers is filtered out as a
    // Doc->Entity edge so the search has nowhere to go).
    let filtered = run_doc_linter(
        &root,
        &[
            "query",
            "path",
            "doc-alpha",
            "doc-gamma",
            "--edge-type",
            "covers",
        ],
    );
    let v: serde_json::Value =
        serde_json::from_str(&String::from_utf8_lossy(&filtered.stdout)).unwrap();
    assert_eq!(v["found"], false);
}

#[test]
/// Regression test for the P0 query-empty bug: when two markdown files
/// declare the same frontmatter `id:`, the ingest used to crash
/// mid-transaction on the primary-key constraint — leaving a partial
/// graph with Doc nodes but zero edges, which broke every downstream
/// `query` against the [[entity-doc-graph]]. The fix is in
/// `graph::build_plan`: dedupe by id, eprintln-warn, continue.
fn duplicate_doc_id_does_not_break_edges() {
    let root = unique_tmpdir("duplicate-id");
    seed_minimal_vault(&root);

    // Drop a baked copy of `doc-gamma` under a different on-disk path
    // — same `id:`, same body — mirroring the workspace-baked scenario
    // (`deploy/network/workspace-baked/clients/<tenant>/...`).
    write(
        &root.join("baked/gamma-copy.md"),
        "---\n\
         id: doc-gamma\n\
         role: doc\n\
         kind: reference\n\
         title: Gamma copy\n\
         summary: gamma copy\n\
         status: stable\n\
         updated: 2026-04-30\n\
         ---\n\n# Gamma copy\n\nBack to [[doc-alpha]].\n",
    );

    let check = run_doc_linter(&root, &["check", "--no-vale"]);
    let stderr = String::from_utf8_lossy(&check.stderr);
    // The dupe must be logged with both paths so authors can find it.
    assert!(
        stderr.contains("duplicate doc id `doc-gamma`"),
        "expected duplicate-id warning in stderr; got:\n{stderr}"
    );
    assert!(
        stderr.contains("baked/gamma-copy.md"),
        "warning should name the skipped file; got:\n{stderr}"
    );
    // The crash signature from before the fix — must not appear.
    assert!(
        !stderr.contains("failed to ingest"),
        "ingest must not crash on duplicate id; got:\n{stderr}"
    );
    assert!(
        !stderr.contains("duplicated primary key value"),
        "PK constraint must not fire; got:\n{stderr}"
    );

    // The whole graph survived: a context query on a doc with edges
    // must return populated outbound *and* inbound arrays.
    let ctx = run_doc_linter(&root, &["query", "context", "doc-alpha"]);
    assert!(
        ctx.status.success(),
        "context query failed: stderr={}",
        String::from_utf8_lossy(&ctx.stderr)
    );
    let v: serde_json::Value = serde_json::from_str(&String::from_utf8_lossy(&ctx.stdout)).unwrap();
    let outbound = v["outbound"].as_array().unwrap();
    let inbound = v["inbound"].as_array().unwrap();
    assert!(
        !outbound.is_empty(),
        "outbound edges missing — ingest aborted mid-flight: {v:#?}"
    );
    assert!(
        !inbound.is_empty(),
        "inbound edges missing — ingest aborted mid-flight: {v:#?}"
    );

    // The kept doc (alpha-side) keeps its wikilink to gamma; only the
    // baked copy got skipped, not the original.
    let outbound_ids: Vec<&str> = outbound.iter().map(|e| e["id"].as_str().unwrap()).collect();
    assert!(
        outbound_ids.contains(&"doc-beta"),
        "expected alpha->beta wikilink; outbound: {outbound:#?}"
    );
}

#[test]
/// Roundtrip: author-declared `relates_to:` on entity frontmatter
/// becomes a typed `RELATES_TO` edge. Pins both the edge
/// presence and the type column so downstream queries that filter
/// on `r.type` keep working.
fn entity_relates_to_writes_typed_edge() {
    let root = unique_tmpdir("relates-to");
    seed_minimal_vault(&root);

    // Second entity so the relates_to target resolves to a registered
    // entity (otherwise build_plan drops the edge).
    write(
        &root.join("docs/ontology/entities/gizmo.md"),
        "---\n\
         id: entity-gizmo\n\
         role: ontology-entity\n\
         title: \"Entity: gizmo\"\n\
         summary: t\n\
         status: stable\n\
         updated: 2026-04-30\n\
         axis_id: covers\n\
         value_id: gizmo\n\
         display: Gizmo\n\
         description: a gizmo\n\
         ---\n\n# gizmo\n",
    );
    // Overwrite the widget entity to declare a typed relationship.
    write(
        &root.join("docs/ontology/entities/widget.md"),
        "---\n\
         id: entity-widget\n\
         role: ontology-entity\n\
         title: \"Entity: widget\"\n\
         summary: t\n\
         status: stable\n\
         updated: 2026-04-30\n\
         axis_id: covers\n\
         value_id: widget\n\
         display: Widget\n\
         description: a widget\n\
         relates_to:\n  \
         - target: gizmo\n    \
         type: depends_on\n\
         ---\n\n# widget\n",
    );

    let _ = run_doc_linter(&root, &["check", "--no-vale"]);

    let out = run_doc_linter(
        &root,
        &[
            "query",
            "sql",
            "SELECT src AS from_id, type AS rel_type, dst AS to_id \
             FROM RELATES_TO ORDER BY from_id, to_id",
        ],
    );
    assert!(
        out.status.success(),
        "sql failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let parsed: serde_json::Value =
        serde_json::from_str(&String::from_utf8_lossy(&out.stdout)).unwrap();
    let rows = parsed["rows"].as_array().expect("rows array");
    assert!(
        rows.iter().any(|r| {
            r["from_id"].as_str() == Some("widget")
                && r["to_id"].as_str() == Some("gizmo")
                && r["rel_type"].as_str() == Some("depends_on")
        }),
        "expected widget -[depends_on]-> gizmo edge, got: {rows:?}"
    );
}

#[test]
/// Roundtrip: an entity with `relates_to:` whose `type:` is outside
/// the closed vocabulary gets flagged by the validator AND the bad
/// edge is dropped at ingest (the graph stays clean).
fn entity_relates_to_unknown_type_is_flagged_and_dropped() {
    let root = unique_tmpdir("relates-to-bad");
    seed_minimal_vault(&root);

    write(
        &root.join("docs/ontology/entities/gizmo.md"),
        "---\n\
         id: entity-gizmo\n\
         role: ontology-entity\n\
         title: \"Entity: gizmo\"\n\
         summary: t\n\
         status: stable\n\
         updated: 2026-04-30\n\
         axis_id: covers\n\
         value_id: gizmo\n\
         display: Gizmo\n\
         description: a gizmo\n\
         ---\n\n# gizmo\n",
    );
    write(
        &root.join("docs/ontology/entities/widget.md"),
        "---\n\
         id: entity-widget\n\
         role: ontology-entity\n\
         title: \"Entity: widget\"\n\
         summary: t\n\
         status: stable\n\
         updated: 2026-04-30\n\
         axis_id: covers\n\
         value_id: widget\n\
         display: Widget\n\
         description: a widget\n\
         relates_to:\n  \
         - target: gizmo\n    \
         type: shouts_at\n\
         ---\n\n# widget\n",
    );

    let check = run_doc_linter(&root, &["check", "--no-vale"]);
    let stderr = String::from_utf8_lossy(&check.stderr);
    assert!(
        stderr.contains("relates_to_type") && stderr.contains("shouts_at"),
        "expected unknown-axis-value diagnostic for relates_to_type=shouts_at, stderr was:\n{stderr}"
    );

    let out = run_doc_linter(&root, &["query", "sql", "SELECT src, dst FROM RELATES_TO"]);
    let parsed: serde_json::Value =
        serde_json::from_str(&String::from_utf8_lossy(&out.stdout)).unwrap();
    assert_eq!(
        parsed["row_count"].as_u64(),
        Some(0),
        "bad-type RELATES_TO edge should be dropped, got: {parsed}"
    );
}

#[test]
/// Roundtrip: asserts the [[entity-doc-graph]] SQLite schema survives a
/// close/reopen cycle so back-to-back invocations don't reset the DB.
fn schema_persists_across_reopens() {
    // Ingest once, then query without re-ingesting. The DB must be fully
    // self-describing after close — the schema lives in the file, so
    // a fresh process opening the same dir sees the same schema. The
    // query path opens its own DB handle, separate from cmd_check's.
    let root = unique_tmpdir("schema-persist");
    seed_minimal_vault(&root);

    // First invocation seeds the DB.
    let _ = run_doc_linter(&root, &["check", "--no-vale"]);
    // Confirm the DB file landed where we expect.
    let db_path = root.join(".doc-lint/graph.sqlite");
    assert!(
        db_path.exists(),
        "expected SQLite graph at {}",
        db_path.display()
    );

    // Second invocation — query only, exercising the open-existing path.
    let out = run_doc_linter(&root, &["query", "list"]);
    assert!(
        out.status.success(),
        "second-process query failed: stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );
    let v: serde_json::Value = serde_json::from_str(&String::from_utf8_lossy(&out.stdout)).unwrap();
    assert!(v["count"].as_u64().unwrap() >= 3);
}
