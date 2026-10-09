#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "integration tests panic on broken invariants — that is the point"
)]
//! Tests for the auto-generated `MAP.md` repo homepage + the
//! `homepage-missing` / `homepage-stale` lint rules. Drives
//! through the public binary so the subcommand wiring and the
//! lint-check integration are both exercised end-to-end.

use std::path::Path;
use std::process::Command;

mod common;
use common::{diagnostic_codes, doc_linter_bin, unique_tmpdir, write};

fn run(root: &Path, args: &[&str]) -> std::process::Output {
    let mut cmd = Command::new(doc_linter_bin());
    cmd.arg("--root").arg(root);
    for a in args {
        cmd.arg(a);
    }
    cmd.output().expect("spawn doc-linter")
}

fn seed_minimal_corpus(root: &Path, extra_config: &str) {
    let mut config = String::from(
        "required_fields = [\"id\", \"role\", \"title\", \"summary\", \"status\", \"updated\"]\n\
         allowed_statuses = [\"draft\", \"stable\"]\n\
         vale_enabled = false\n",
    );
    config.push_str(extra_config);
    write(&root.join(".doc-lint.toml"), &config);

    // role axis + values
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
         ---\n\n# axis\n\nSee [[value-role-doc]], [[value-role-index]], [[axis-kind]].\n",
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
                 ---\n\n# {v}\n\nSee [[axis-role]].\n"
            ),
        );
    }

    // kind axis + how-to value (needed because the how-to doc declares kind: how-to)
    write(
        &root.join("docs/ontology/axes/kind.md"),
        "---\n\
         id: axis-kind\n\
         role: ontology-axis\n\
         title: \"Axis: kind\"\n\
         summary: t\n\
         status: stable\n\
         updated: 2026-04-30\n\
         axis_id: kind\n\
         ---\n\n# axis\n\nSee [[value-kind-how-to]], [[axis-role]].\n",
    );
    write(
        &root.join("docs/ontology/values/kind/how-to.md"),
        "---\n\
         id: value-kind-how-to\n\
         role: ontology-value\n\
         title: \"Kind: how-to\"\n\
         summary: t\n\
         status: stable\n\
         updated: 2026-04-30\n\
         axis_id: kind\n\
         value_id: how-to\n\
         display: how-to\n\
         description: t\n\
         ---\n\n# how-to\n\nSee [[axis-kind]].\n",
    );

    write(
        &root.join("docs/ontology/entities/foo.md"),
        "---\n\
         id: entity-foo\n\
         role: ontology-entity\n\
         title: \"Entity: foo\"\n\
         summary: t\n\
         status: stable\n\
         updated: 2026-04-30\n\
         axis_id: covers\n\
         value_id: foo\n\
         display: Foo\n\
         description: A foo concept used by the test fixture.\n\
         ---\n\n# foo\n\nSee [[uses-foo]].\n",
    );
    write(
        &root.join("docs/ontology/migrations/0001-initial.md"),
        "---\n\
         id: ontology-mig-0001\n\
         role: ontology-migration\n\
         title: \"Migration 0001 — Initial\"\n\
         summary: t\n\
         status: stable\n\
         updated: 2026-04-30\n\
         from_version: 0\n\
         to_version: 1\n\
         applied: true\n\
         ---\n\n# migration\n\nSee [[axis-role]].\n",
    );
    write(
        &root.join("docs/how-to/uses-foo.md"),
        "---\n\
         id: uses-foo\n\
         role: doc\n\
         kind: how-to\n\
         title: Uses foo\n\
         summary: A how-to that covers foo.\n\
         status: stable\n\
         updated: 2026-04-30\n\
         covers: [foo]\n\
         ---\n\n# Uses foo\n\nSee [[entity-foo]].\n",
    );
}

/// `doc-linter homepage` (no flags) prints the canonical content
/// to stdout without touching disk.
#[test]
fn homepage_subcommand_prints_to_stdout_without_write() {
    let root = unique_tmpdir("print-stdout");
    seed_minimal_corpus(&root, "");

    let out = run(&root, &["homepage"]);
    assert!(out.status.success(), "exit {:?}", out.status);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("## Domain concepts"),
        "missing entity section:\n{stdout}"
    );
    assert!(
        stdout.contains("[[entity-foo]]"),
        "missing entity wikilink:\n{stdout}"
    );
    assert!(
        stdout.contains("[[uses-foo]]"),
        "missing narrative wikilink:\n{stdout}"
    );
    assert!(
        stdout.contains("## Ontology version"),
        "missing version footer:\n{stdout}"
    );
    assert!(
        stdout.contains("v1"),
        "expected v1 from migration to_version; got:\n{stdout}"
    );
    assert!(!root.join("MAP.md").exists(), "dry-run must not write");
}

/// `doc-linter homepage --write` writes the file (in a non-git
/// fixture the dirty-tree guard is permissive). Repeating the
/// write is a no-op when the content matches.
#[test]
fn homepage_subcommand_write_creates_file() {
    let root = unique_tmpdir("write-creates");
    seed_minimal_corpus(&root, "");

    let out = run(&root, &["homepage", "--write"]);
    assert!(out.status.success(), "exit {:?}", out.status);
    let map = root.join("MAP.md");
    assert!(map.exists(), "MAP.md must exist after --write");
    let content = std::fs::read_to_string(&map).unwrap();
    assert!(content.contains("[[entity-foo]]"));

    // Second run is a no-op (content matches).
    let out2 = run(&root, &["homepage", "--write"]);
    assert!(out2.status.success());
    let stderr = String::from_utf8_lossy(&out2.stderr);
    assert!(
        stderr.contains("already up to date"),
        "expected idempotent message; got:\n{stderr}"
    );
}

/// With `require_homepage = true` and no `MAP.md`, `check` fires
/// the `homepage-missing` diagnostic and fails the build at the
/// default error severity.
#[test]
fn homepage_missing_fires_when_required() {
    let root = unique_tmpdir("missing-fires");
    seed_minimal_corpus(&root, "require_homepage = true\n");

    let out = run(&root, &["check", "--no-vale", "--format", "json"]);
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    let codes = diagnostic_codes(&stdout);
    assert!(
        codes.iter().any(|c| c == "homepage-missing"),
        "expected homepage-missing; codes={codes:?}\nstdout={stdout}"
    );
    assert!(
        !out.status.success(),
        "homepage-missing must fail the build at default severity; status={:?}",
        out.status
    );
}

/// `homepage-missing` is silent when the rule is off (default).
#[test]
fn homepage_missing_silent_when_rule_disabled() {
    let root = unique_tmpdir("missing-silent");
    seed_minimal_corpus(&root, ""); // require_homepage defaults to false

    let out = run(&root, &["check", "--no-vale", "--format", "json"]);
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    let codes = diagnostic_codes(&stdout);
    assert!(
        !codes.iter().any(|c| c == "homepage-missing"),
        "rule disabled by default — must stay silent; codes={codes:?}"
    );
}

/// After `homepage --write` the lint is clean; mutating the
/// underlying ontology fires `homepage-stale`; re-running
/// `homepage --write` (via stdout redirect — the guard is
/// permissive in the test fixture) clears it again.
#[test]
fn homepage_stale_fires_on_drift_and_clears_on_regeneration() {
    let root = unique_tmpdir("stale-roundtrip");
    seed_minimal_corpus(&root, "require_homepage = true\n");

    // Step 1: seed MAP.md.
    let gen = run(&root, &["homepage"]);
    assert!(gen.status.success());
    std::fs::write(root.join("MAP.md"), &gen.stdout).unwrap();

    // Step 2: check is clean.
    let out1 = run(&root, &["check", "--no-vale", "--format", "json"]);
    let stdout1 = String::from_utf8_lossy(&out1.stdout).to_string();
    let codes1 = diagnostic_codes(&stdout1);
    assert!(
        !codes1.iter().any(|c| c.starts_with("homepage-")),
        "homepage rules should clear immediately after seed; codes={codes1:?}\nstdout={stdout1}"
    );

    // Step 3: mutate the entity description so the homepage drifts.
    let ent = root.join("docs/ontology/entities/foo.md");
    let orig = std::fs::read_to_string(&ent).unwrap();
    std::fs::write(
        &ent,
        orig.replace(
            "A foo concept used by the test fixture.",
            "A completely different description used in the drift test.",
        ),
    )
    .unwrap();

    let out2 = run(&root, &["check", "--no-vale", "--format", "json"]);
    let stdout2 = String::from_utf8_lossy(&out2.stdout).to_string();
    let codes2 = diagnostic_codes(&stdout2);
    assert!(
        codes2.iter().any(|c| c == "homepage-stale"),
        "drift must fire homepage-stale; codes={codes2:?}\nstdout={stdout2}"
    );

    // Step 4: regenerate and reseed.
    let gen2 = run(&root, &["homepage"]);
    assert!(gen2.status.success());
    std::fs::write(root.join("MAP.md"), &gen2.stdout).unwrap();

    let out3 = run(&root, &["check", "--no-vale", "--format", "json"]);
    let stdout3 = String::from_utf8_lossy(&out3.stdout).to_string();
    let codes3 = diagnostic_codes(&stdout3);
    assert!(
        !codes3.iter().any(|c| c.starts_with("homepage-")),
        "regeneration must clear homepage-stale; codes={codes3:?}\nstdout={stdout3}"
    );
}

/// `homepage_stale_severity = "warning"` keeps the build green
/// even when the diagnostic fires. Comparison case for the
/// default `"error"` behaviour exercised by
/// `homepage_missing_fires_when_required`.
#[test]
fn homepage_stale_severity_warning_keeps_build_green() {
    let root = unique_tmpdir("stale-warning");
    seed_minimal_corpus(
        &root,
        "require_homepage = true\nhomepage_stale_severity = \"warning\"\n",
    );

    let out = run(&root, &["check", "--no-vale"]);
    assert!(
        out.status.success(),
        "warning severity must not fail the build; status={:?}\nstdout={}\nstderr={}",
        out.status,
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
}

/// Invalid `homepage_stale_severity` rejects at config-load time
/// — mirrors the validation pattern of every other severity knob.
#[test]
fn homepage_stale_severity_invalid_fails_load() {
    let root = unique_tmpdir("stale-invalid");
    seed_minimal_corpus(
        &root,
        "require_homepage = true\nhomepage_stale_severity = \"loud\"\n",
    );

    let out = run(&root, &["check", "--no-vale"]);
    assert!(!out.status.success());
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        combined.contains("homepage_stale_severity"),
        "error must name the offending field; got:\n{combined}"
    );
}

/// `homepage_path` override directs both the subcommand and the
/// lint check to a non-default location. Tests that the wiring
/// reads the config consistently across both code paths.
#[test]
fn homepage_path_override_relocates_the_file() {
    let root = unique_tmpdir("path-override");
    seed_minimal_corpus(
        &root,
        "require_homepage = true\nhomepage_path = \"docs/MAP.md\"\n",
    );

    // Lint should now ask for docs/MAP.md, not MAP.md.
    let out = run(&root, &["check", "--no-vale", "--format", "json"]);
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    assert!(
        stdout.contains("docs/MAP.md"),
        "diagnostic must point at the configured path; stdout={stdout}"
    );

    // --write should write there.
    let gen = run(&root, &["homepage"]);
    assert!(gen.status.success());
    std::fs::create_dir_all(root.join("docs")).unwrap();
    std::fs::write(root.join("docs/MAP.md"), &gen.stdout).unwrap();

    let out2 = run(&root, &["check", "--no-vale", "--format", "json"]);
    let stdout2 = String::from_utf8_lossy(&out2.stdout).to_string();
    let codes2 = diagnostic_codes(&stdout2);
    assert!(
        !codes2.iter().any(|c| c.starts_with("homepage-")),
        "seeding the override path should clear the rule; codes={codes2:?}\nstdout={stdout2}"
    );
}

/// `homepage_root_entity` override pins the TL;DR's anchor entity
/// instead of letting the generator pick by repo-name / inbound
/// covers. Verifies the configured id appears in the "What this
/// repo is" section.
#[test]
fn homepage_root_entity_override_pins_anchor() {
    let root = unique_tmpdir("root-entity");
    seed_minimal_corpus(&root, "homepage_root_entity = \"foo\"\n");

    let out = run(&root, &["homepage"]);
    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("Anchor entity: [[entity-foo]]"),
        "explicit anchor must appear; got:\n{stdout}"
    );
    assert!(
        stdout.contains("A foo concept used by the test fixture."),
        "description must be inlined; got:\n{stdout}"
    );
}

/// The generated MAP.md must surface the graph-query families
/// (structural / SQL / code-side) — that's the cold-start
/// affordance that distinguishes the linter from a static
/// table-of-contents. Each family's headline command should
/// appear verbatim so an agent reading the file can copy-paste.
#[test]
fn homepage_surfaces_graph_query_families() {
    let root = unique_tmpdir("graph-queries-section");
    seed_minimal_corpus(&root, "homepage_root_entity = \"foo\"\n");

    let out = run(&root, &["homepage"]);
    assert!(out.status.success());
    let s = String::from_utf8_lossy(&out.stdout);

    // Section header is present.
    assert!(
        s.contains("## Graph queries"),
        "missing Graph queries section:\n{s}"
    );

    // Each query family is represented with at least one
    // representative subcommand the agent can run.
    for cmd in &[
        "query neighbors",            // structural
        "query backlinks",            // structural
        "query path",                 // structural
        "query subgraph",             // structural
        "query context",              // structural
        "query sql",                  // sql entry point
        "query schema",               // schema introspection example
        "query functions-mentioning", // code-side
        "query coverage-report",      // code-side
        "explain",                    // code-side
    ] {
        assert!(
            s.contains(cmd),
            "graph-queries section missing `{cmd}`:\n{s}"
        );
    }

    // SQL examples are pinned to the resolved anchor entity
    // so they're immediately copy-pasteable.
    assert!(
        s.contains("entity-foo"),
        "SQL examples should reference the anchor entity-foo:\n{s}"
    );
}

/// When no anchor entity resolves, the graph-query examples fall
/// back to a generic placeholder so the section still renders
/// (rather than crashing or omitting). Ensures the section is
/// agent-useful even on a corpus with zero entities.
#[test]
fn homepage_graph_queries_render_without_anchor() {
    let root = unique_tmpdir("graph-queries-no-anchor");
    // Author a corpus with zero entities — just the role axis.
    // The MAP.md generator should still emit the Graph queries
    // section with placeholder identifiers.
    write(
        &root.join(".doc-lint.toml"),
        "required_fields = [\"id\", \"role\", \"title\", \"summary\", \"status\", \"updated\"]\n\
         allowed_statuses = [\"draft\", \"stable\"]\n\
         vale_enabled = false\n",
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

    let out = run(&root, &["homepage"]);
    assert!(
        out.status.success(),
        "homepage must render even without entities; status={:?}",
        out.status
    );
    let s = String::from_utf8_lossy(&out.stdout);
    assert!(
        s.contains("## Graph queries"),
        "Graph queries section missing"
    );
    assert!(s.contains("query sql"), "SQL entry point missing");
}

/// The generator output is deterministic — the same corpus
/// produces identical bytes across runs, which is the property
/// the byte-compare in `homepage-stale` relies on.
#[test]
fn homepage_generation_is_deterministic() {
    let root = unique_tmpdir("deterministic");
    seed_minimal_corpus(&root, "");

    let a = run(&root, &["homepage"]);
    let b = run(&root, &["homepage"]);
    assert!(a.status.success() && b.status.success());
    assert_eq!(
        a.stdout, b.stdout,
        "two runs over the same corpus must produce identical bytes"
    );
}
