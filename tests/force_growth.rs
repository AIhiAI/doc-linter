#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "integration tests panic on broken invariants — that is the point"
)]
//! Roadmap-48 — force-graph-growth lint rules.
//!
//! Three new rules invert the lint surface from "structural
//! integrity" to "graph density pressure":
//!
//!   - **Rule A** — `coverage_anchor_exempt_file` + the
//!     `migrate-anchor-required` subcommand. Auto-grandfathers the
//!     existing dark surface into a per-repo allow-list so flipping
//!     `require_anchor_per_pubapi` to default-true doesn't break
//!     already-shipped code.
//!   - **Rule B** — `unknown-noun-promote` diagnostic surface. The
//!     `comment-vocab-violation` diagnostic now carries a copy-paste
//!     `entity-X.md` stub the author can drop in to grow the
//!     ontology instead of rephrasing the unknown term out of
//!     existence.
//!   - **Rule C** — `orphan-entity` immediate warning. An ontology
//!     entity with zero inbound `covers:` references or
//!     `[[entity-<id>]]` wikilinks fires a per-edit warning so new
//!     entities can't ship without at least one narrative doc
//!     covering them.
//!
//! Each test seeds a tmp corpus, runs `doc-linter` via the public
//! binary, and asserts on either the stdout/stderr or the exit
//! status. We drive through the binary because doc-linter exposes
//! its integration surface as the CLI (no `[lib]` target).

mod common;
use common::{
    diagnostic_codes, run_doc_linter, seed_ontology, unique_tmpdir, write, write_config,
    write_synthetic_scip,
};

// ---------------- Rule A — coverage_anchor_exempt_file ---------------

/// Rule A: a dark public function whose bare name is on
/// `.doc-lint/anchor-exempt.txt` is grandfathered in. The
/// `dark-public-function` diagnostic must NOT fire.
#[test]
fn anchor_exempt_file_grandfathered_fn_passes() {
    let root = unique_tmpdir("rule-a-grandfathered");
    write_config(&root, "anchor_required_in = [\"my-crate\"]\n");
    seed_ontology(&root, &[]);
    write_synthetic_scip(
        &root.join(".doc-lint/code.scip"),
        &[(
            "rust-analyzer cargo my-crate 0.1.0 src/lib.rs/dark_fn().",
            vec![],
        )],
    );
    // Seed the grandfathered list with the dark fn's bare name.
    write(
        &root.join(".doc-lint/anchor-exempt.txt"),
        "# grandfathered\ndark_fn\n",
    );

    let _ = run_doc_linter(&root, &["check", "--no-vale"]);
    let out = run_doc_linter(&root, &["check", "--no-vale", "--format", "json"]);
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    let codes = diagnostic_codes(&stdout);
    assert!(
        !codes.iter().any(|c| c == "dark-public-function"),
        "grandfathered fn must not fire dark-public-function: codes={codes:?}\nstdout={stdout}"
    );
}

/// Rule A: `migrate-anchor-required` writes the bare names of
/// every dark public function in the corpus to the configured
/// `coverage_anchor_exempt_file`. Re-running idempotently
/// overwrites the file.
#[test]
fn migrate_anchor_required_writes_file() {
    let root = unique_tmpdir("rule-a-migrate");
    write_config(&root, "");
    seed_ontology(&root, &[]);
    write_synthetic_scip(
        &root.join(".doc-lint/code.scip"),
        &[
            (
                "rust-analyzer cargo my-crate 0.1.0 src/lib.rs/alpha_fn().",
                vec![],
            ),
            (
                "rust-analyzer cargo my-crate 0.1.0 src/lib.rs/beta_fn().",
                vec![],
            ),
        ],
    );

    // Prime the graph + SCIP ingest via a regular check first;
    // migrate-anchor-required reuses the on-disk graph.
    let _ = run_doc_linter(&root, &["check", "--no-vale"]);

    let out = run_doc_linter(&root, &["migrate-anchor-required"]);
    assert!(
        out.status.success(),
        "migrate-anchor-required should succeed; stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );
    let target = root.join(".doc-lint/anchor-exempt.txt");
    assert!(target.exists(), "expected anchor-exempt.txt to exist");
    let body = std::fs::read_to_string(&target).unwrap();
    assert!(
        body.contains("alpha_fn"),
        "exempt file must contain alpha_fn; got:\n{body}"
    );
    assert!(
        body.contains("beta_fn"),
        "exempt file must contain beta_fn; got:\n{body}"
    );

    // Idempotent — re-running produces the same shape.
    let out2 = run_doc_linter(&root, &["migrate-anchor-required"]);
    assert!(out2.status.success());
    let body2 = std::fs::read_to_string(&target).unwrap();
    assert_eq!(
        body, body2,
        "migrate-anchor-required must be idempotent (same snapshot, byte-for-byte)"
    );
}

// ---------------- Rule B — unknown-noun-promote ----------------------

/// Rule B: a `.rs` file with an unknown token in a doc-comment
/// produces a `comment-vocab-violation` whose human-readable
/// rendering carries the `id: entity-<term>` frontmatter stub.
#[test]
fn vocab_violation_includes_promote_stub_human() {
    let root = unique_tmpdir("rule-b-stub-human");
    write_config(&root, "");
    seed_ontology(&root, &["pricing-rule", "tenant"]);

    let rs = root.join("crates/sample/src/lib.rs");
    write(
        &rs,
        "/// This module handles a Zorblax thing.\n\
         pub fn handler() {}\n",
    );

    let out = run_doc_linter(
        &root,
        &[
            "check",
            "--no-vale",
            "--lint-code-comments",
            "--file",
            rs.to_str().unwrap(),
        ],
    );
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        combined.contains("comment vocab"),
        "expected vocab-violation diagnostic in output; got:\n{combined}"
    );
    assert!(
        combined.contains("id: entity-zorblax"),
        "expected promote stub `id: entity-zorblax` in output; got:\n{combined}"
    );
    assert!(
        combined.contains("axis_id: covers"),
        "expected promote stub frontmatter to include axis_id: covers; got:\n{combined}"
    );
    assert!(
        combined.contains("to docs/ontology/entities/zorblax"),
        "expected promote stub trailing save path; got:\n{combined}"
    );
}

/// Rule B: the closest-by-token-similarity candidate list
/// surfaces ontology entity ids in the rendered diagnostic so the
/// author can pick one to rephrase against rather than promote.
#[test]
fn vocab_violation_promote_includes_closest_candidates() {
    let root = unique_tmpdir("rule-b-closest");
    write_config(&root, "");
    seed_ontology(&root, &["pricing-rule", "tenant"]);

    let rs = root.join("crates/sample/src/lib.rs");
    // Use a token close enough to one of the seeded entities to
    // surface in the closest list.
    write(
        &rs,
        "/// Refer to the Tenent record for context.\n\
         pub fn handler() {}\n",
    );

    let out = run_doc_linter(
        &root,
        &[
            "check",
            "--no-vale",
            "--lint-code-comments",
            "--file",
            rs.to_str().unwrap(),
        ],
    );
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        combined.contains("closest by token similarity"),
        "expected closest-candidates header; got:\n{combined}"
    );
    assert!(
        combined.contains("tenant"),
        "expected `tenant` in closest list (1-edit from `Tenent`); got:\n{combined}"
    );
}

// ---------------- Rule C — orphan-entity -----------------------------

/// Rule C: an entity with zero inbound covers / wikilinks fires
/// the `orphan-entity` diagnostic.
#[test]
fn orphan_entity_fires_with_no_inbound_covers() {
    let root = unique_tmpdir("rule-c-fires");
    write_config(&root, "");
    seed_ontology(&root, &["foo"]);

    let _ = run_doc_linter(&root, &["check", "--no-vale"]);
    let out = run_doc_linter(&root, &["check", "--no-vale", "--format", "json"]);
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    let codes = diagnostic_codes(&stdout);
    assert!(
        codes.iter().any(|c| c == "orphan-entity"),
        "expected orphan-entity for foo; got codes={codes:?}\nstdout={stdout}"
    );
}

/// Rule C: a how-to doc with `covers: [foo]` clears the orphan
/// warning.
#[test]
fn orphan_entity_clears_with_inbound_cover() {
    let root = unique_tmpdir("rule-c-cover");
    write_config(&root, "");
    seed_ontology(&root, &["foo"]);
    // Cover the entity from a how-to doc. The doc's id matches its
    // filename stem so the linter's id/stem-match check passes; we
    // keep frontmatter minimal (no kind/lifecycle) so the test
    // ontology doesn't need value-lifecycle-* / value-kind-* docs.
    write(
        &root.join("docs/how-to/uses-foo.md"),
        "---\n\
         id: uses-foo\n\
         role: doc\n\
         title: Use foo\n\
         summary: Walks through using foo.\n\
         status: stable\n\
         updated: 2026-04-30\n\
         covers: [foo]\n\
         ---\n\n\
         # Use foo\n\n\
         How to use foo.\n",
    );

    let _ = run_doc_linter(&root, &["check", "--no-vale"]);
    let out = run_doc_linter(&root, &["check", "--no-vale", "--format", "json"]);
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    let codes = diagnostic_codes(&stdout);
    assert!(
        !codes.iter().any(|c| c == "orphan-entity"),
        "covers: [foo] should clear orphan-entity; got codes={codes:?}\nstdout={stdout}"
    );
}

/// Rule C: a body wikilink `[[entity-foo]]` clears the orphan
/// warning even without a `covers:` array entry.
#[test]
fn orphan_entity_clears_with_inbound_wikilink() {
    let root = unique_tmpdir("rule-c-wikilink");
    write_config(&root, "");
    seed_ontology(&root, &["foo"]);
    // No covers: array — only a body wikilink.
    write(
        &root.join("docs/how-to/about-foo.md"),
        "---\n\
         id: about-foo\n\
         role: doc\n\
         title: About foo\n\
         summary: Mentions foo by wikilink.\n\
         status: stable\n\
         updated: 2026-04-30\n\
         ---\n\n\
         # About foo\n\n\
         See [[entity-foo]] for the canonical definition.\n",
    );

    let _ = run_doc_linter(&root, &["check", "--no-vale"]);
    let out = run_doc_linter(&root, &["check", "--no-vale", "--format", "json"]);
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    let codes = diagnostic_codes(&stdout);
    assert!(
        !codes.iter().any(|c| c == "orphan-entity"),
        "[[entity-foo]] wikilink should clear orphan-entity; codes={codes:?}\nstdout={stdout}"
    );
}

/// Rule C: when `orphan_entity_severity = "error"`, the
/// `orphan-entity` diagnostic counts toward the `error_count` and
/// drives `ExitCode::FAILURE`. Default severity (`"warning"`)
/// keeps the build green even when the diagnostic fires.
#[test]
fn orphan_entity_severity_error_fails_build() {
    let root = unique_tmpdir("rule-c-error");
    write_config(&root, "orphan_entity_severity = \"error\"\n");
    seed_ontology(&root, &["foo"]);

    let _ = run_doc_linter(&root, &["check", "--no-vale"]);
    let out = run_doc_linter(&root, &["check", "--no-vale", "--format", "json"]);
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    let codes = diagnostic_codes(&stdout);
    assert!(
        codes.iter().any(|c| c == "orphan-entity"),
        "expected orphan-entity to fire; got codes={codes:?}\nstdout={stdout}"
    );
    assert!(
        !out.status.success(),
        "orphan_entity_severity=error must fail the build; status={:?}",
        out.status
    );
}

/// Rule C: invalid `orphan_entity_severity` rejects at config
/// load time so typos in `.doc-lint.toml` surface immediately
/// rather than at the first orphan-entity emission.
#[test]
fn orphan_entity_severity_invalid_value_fails_load() {
    let root = unique_tmpdir("rule-c-invalid-severity");
    write_config(&root, "orphan_entity_severity = \"hard-fail\"\n");
    seed_ontology(&root, &["foo"]);

    let out = run_doc_linter(&root, &["check", "--no-vale"]);
    assert!(
        !out.status.success(),
        "invalid orphan_entity_severity must fail config load; status={:?}",
        out.status
    );
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        combined.contains("orphan_entity_severity"),
        "error must name the offending field; got:\n{combined}"
    );
}

// ---------------- Graph-density rules --------------------------------
//
// Three opt-in rules whose goal is "every narrative doc names the
// ontology entities it explains AND its bounded-context, every
// entity gets a narrative cover, and the init scaffold doesn't ship
// as production ontology." Default-off so existing repos can adopt
// incrementally; severity is configurable per-rule.

/// `require_covers_per_doc = true` makes a `role: doc` with an
/// empty `covers:` array fire `missing-covers`.
#[test]
fn missing_covers_fires_when_required_and_absent() {
    let root = unique_tmpdir("density-covers-fires");
    write_config(&root, "require_covers_per_doc = true\n");
    seed_ontology(&root, &["foo"]);
    write(
        &root.join("docs/how-to/bare.md"),
        "---\n\
         id: bare\n\
         role: doc\n\
         title: Bare doc\n\
         summary: A how-to with no covers array.\n\
         status: stable\n\
         updated: 2026-04-30\n\
         ---\n\n\
         # Bare\n",
    );

    let out = run_doc_linter(&root, &["check", "--no-vale", "--format", "json"]);
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    let codes = diagnostic_codes(&stdout);
    assert!(
        codes.iter().any(|c| c == "missing-covers"),
        "expected missing-covers when covers is empty; codes={codes:?}\nstdout={stdout}"
    );
    assert!(
        !out.status.success(),
        "default missing_covers_severity=error must fail the build; status={:?}",
        out.status
    );
}

/// A `role: doc` that declares `covers: [...]` clears the rule.
#[test]
fn missing_covers_clears_when_covers_present() {
    let root = unique_tmpdir("density-covers-clears");
    write_config(&root, "require_covers_per_doc = true\n");
    seed_ontology(&root, &["foo"]);
    write(
        &root.join("docs/how-to/covers-foo.md"),
        "---\n\
         id: covers-foo\n\
         role: doc\n\
         title: Covers foo\n\
         summary: A how-to that names foo.\n\
         status: stable\n\
         updated: 2026-04-30\n\
         covers: [foo]\n\
         ---\n\n\
         # Covers foo\n",
    );

    let out = run_doc_linter(&root, &["check", "--no-vale", "--format", "json"]);
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    let codes = diagnostic_codes(&stdout);
    assert!(
        !codes.iter().any(|c| c == "missing-covers"),
        "covers: [foo] should clear missing-covers; codes={codes:?}\nstdout={stdout}"
    );
}

/// Default (`require_covers_per_doc = false`): the rule is silent
/// even when a narrative doc has no covers — backwards-compat for
/// repos that pre-existed the rule.
#[test]
fn missing_covers_silent_when_rule_disabled() {
    let root = unique_tmpdir("density-covers-disabled");
    write_config(&root, ""); // require_covers_per_doc defaults to false
    seed_ontology(&root, &["foo"]);
    write(
        &root.join("docs/how-to/bare.md"),
        "---\n\
         id: bare\n\
         role: doc\n\
         title: Bare\n\
         summary: A how-to with no covers.\n\
         status: stable\n\
         updated: 2026-04-30\n\
         ---\n\n# Bare\n",
    );

    let out = run_doc_linter(&root, &["check", "--no-vale", "--format", "json"]);
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    let codes = diagnostic_codes(&stdout);
    assert!(
        !codes.iter().any(|c| c == "missing-covers"),
        "rule disabled by default — must stay silent; codes={codes:?}"
    );
}

/// `lifecycle: archived` docs are exempt from the rule.
#[test]
fn missing_covers_skips_archived_lifecycle() {
    let root = unique_tmpdir("density-covers-archived");
    write_config(&root, "require_covers_per_doc = true\n");
    seed_ontology(&root, &["foo"]);
    // value-lifecycle-archived for the ontology to resolve the
    // lifecycle axis.
    write(
        &root.join("docs/ontology/values/lifecycle/archived.md"),
        "---\n\
         id: value-lifecycle-archived\n\
         role: ontology-value\n\
         title: \"Lifecycle: archived\"\n\
         summary: t\n\
         status: stable\n\
         updated: 2026-04-30\n\
         axis_id: lifecycle\n\
         value_id: archived\n\
         display: archived\n\
         description: t\n\
         ---\n\n# archived\n",
    );
    write(
        &root.join("docs/ontology/axes/lifecycle.md"),
        "---\n\
         id: axis-lifecycle\n\
         role: ontology-axis\n\
         title: \"Axis: lifecycle\"\n\
         summary: t\n\
         status: stable\n\
         updated: 2026-04-30\n\
         axis_id: lifecycle\n\
         ---\n\n# axis\n",
    );
    write(
        &root.join("docs/archive/old.md"),
        "---\n\
         id: old\n\
         role: doc\n\
         lifecycle: archived\n\
         title: Old\n\
         summary: Archived doc — covers no longer required.\n\
         status: archived\n\
         updated: 2026-04-30\n\
         ---\n\n# Old\n",
    );

    // Ensure archived appears in allowed_statuses
    let cfg_path = root.join(".doc-lint.toml");
    let cfg = std::fs::read_to_string(&cfg_path).unwrap();
    std::fs::write(
        &cfg_path,
        cfg.replace(
            "allowed_statuses = [\"draft\", \"stable\"]",
            "allowed_statuses = [\"draft\", \"stable\", \"archived\"]",
        ),
    )
    .unwrap();

    let out = run_doc_linter(&root, &["check", "--no-vale", "--format", "json"]);
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    let codes = diagnostic_codes(&stdout);
    let on_old: Vec<&String> = codes
        .iter()
        .filter(|c| c.as_str() == "missing-covers")
        .collect();
    assert!(
        on_old.is_empty(),
        "archived doc must skip missing-covers; codes={codes:?}\nstdout={stdout}"
    );
}

/// `missing_covers_severity = "warning"` keeps the build green even
/// when the rule fires. Comparison: under default `"error"`
/// severity the same fixture fails; under `"warning"` it passes.
/// Both runs emit the diagnostic — only the exit code differs.
#[test]
fn missing_covers_severity_warning_keeps_build_green() {
    let root = unique_tmpdir("density-covers-warn");
    write_config(
        &root,
        "require_covers_per_doc = true\nmissing_covers_severity = \"warning\"\n",
    );
    seed_ontology(&root, &["foo"]);
    // Wire every stub doc into a closed cycle so OrphanDoc (always
    // an error) doesn't contaminate the exit code we're measuring.
    // The cycle: bare → entity-foo → axis-role → value-role-doc → bare.
    write(
        &root.join("docs/how-to/bare.md"),
        "---\n\
         id: bare\n\
         role: doc\n\
         title: Bare\n\
         summary: A how-to with no covers.\n\
         status: stable\n\
         updated: 2026-04-30\n\
         ---\n\n\
         # Bare\n\n\
         See [[entity-foo]].\n",
    );
    // Append a closing wikilink to each stub so the graph has no
    // OrphanDoc nodes. seed_ontology writes the file fresh, so we
    // rewrite each with a `## Related` section that closes the loop.
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
         display: foo\n\
         description: e\n\
         ---\n\n\
         # foo\n\n\
         See [[axis-role]].\n",
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
         ---\n\n\
         # axis\n\n\
         See [[value-role-doc]].\n",
    );
    write(
        &root.join("docs/ontology/values/role/doc.md"),
        "---\n\
         id: value-role-doc\n\
         role: ontology-value\n\
         title: \"Role: doc\"\n\
         summary: t\n\
         status: stable\n\
         updated: 2026-04-30\n\
         axis_id: role\n\
         value_id: doc\n\
         display: doc\n\
         description: t\n\
         ---\n\n\
         # doc\n\n\
         See [[bare]] and [[value-role-index]].\n",
    );
    write(
        &root.join("docs/ontology/values/role/index.md"),
        "---\n\
         id: value-role-index\n\
         role: ontology-value\n\
         title: \"Role: index\"\n\
         summary: t\n\
         status: stable\n\
         updated: 2026-04-30\n\
         axis_id: role\n\
         value_id: index\n\
         display: index\n\
         description: t\n\
         ---\n\n\
         # index\n\n\
         See [[axis-role]].\n",
    );

    let out = run_doc_linter(&root, &["check", "--no-vale"]);
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    let stderr = String::from_utf8_lossy(&out.stderr).to_string();
    assert!(
        out.status.success(),
        "warning severity must not fail the build; status={:?}\nstdout={stdout}\nstderr={stderr}",
        out.status
    );
}

/// Invalid `missing_covers_severity` rejects at config load time.
#[test]
fn missing_covers_severity_invalid_value_fails_load() {
    let root = unique_tmpdir("density-covers-invalid-sev");
    write_config(
        &root,
        "require_covers_per_doc = true\nmissing_covers_severity = \"shouty\"\n",
    );
    seed_ontology(&root, &["foo"]);

    let out = run_doc_linter(&root, &["check", "--no-vale"]);
    assert!(!out.status.success());
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        combined.contains("missing_covers_severity"),
        "error must name the offending field; got:\n{combined}"
    );
}

/// `require_bounded_context_per_doc = true` fires
/// `missing-bounded-context` when a `role: doc` has no
/// `bounded_context:` set.
#[test]
fn missing_bounded_context_fires_when_required_and_absent() {
    let root = unique_tmpdir("density-bc-fires");
    write_config(&root, "require_bounded_context_per_doc = true\n");
    seed_ontology(&root, &["foo"]);
    write(
        &root.join("docs/how-to/bare.md"),
        "---\n\
         id: bare\n\
         role: doc\n\
         title: Bare\n\
         summary: A how-to with no bounded_context.\n\
         status: stable\n\
         updated: 2026-04-30\n\
         covers: [foo]\n\
         ---\n\n# Bare\n",
    );

    let out = run_doc_linter(&root, &["check", "--no-vale", "--format", "json"]);
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    let codes = diagnostic_codes(&stdout);
    assert!(
        codes.iter().any(|c| c == "missing-bounded-context"),
        "expected missing-bounded-context; codes={codes:?}\nstdout={stdout}"
    );
    assert!(!out.status.success(), "default severity error must fail");
}

/// `orphan_entity_requires_covers = true` rejects wikilinks as
/// satisfying coverage — only `covers:` array references count.
#[test]
fn orphan_entity_strict_mode_rejects_wikilink_only() {
    let root = unique_tmpdir("density-orphan-strict");
    write_config(
        &root,
        "orphan_entity_requires_covers = true\norphan_entity_severity = \"error\"\n",
    );
    seed_ontology(&root, &["foo"]);
    // Non-strict mode would clear this; strict mode does not.
    write(
        &root.join("docs/how-to/wikilink-only.md"),
        "---\n\
         id: wikilink-only\n\
         role: doc\n\
         title: Wikilink only\n\
         summary: Mentions foo by wikilink without listing it in covers.\n\
         status: stable\n\
         updated: 2026-04-30\n\
         ---\n\n\
         # Wikilink only\n\n\
         See [[entity-foo]] for context.\n",
    );

    let out = run_doc_linter(&root, &["check", "--no-vale", "--format", "json"]);
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    let codes = diagnostic_codes(&stdout);
    assert!(
        codes.iter().any(|c| c == "orphan-entity"),
        "strict orphan-entity must fire when only a wikilink references the entity; codes={codes:?}\nstdout={stdout}"
    );
}

/// Strict mode still clears when a doc declares `covers:`.
#[test]
fn orphan_entity_strict_mode_clears_with_covers() {
    let root = unique_tmpdir("density-orphan-strict-clear");
    write_config(
        &root,
        "orphan_entity_requires_covers = true\norphan_entity_severity = \"error\"\n",
    );
    seed_ontology(&root, &["foo"]);
    write(
        &root.join("docs/how-to/covers-foo.md"),
        "---\n\
         id: covers-foo\n\
         role: doc\n\
         title: Covers foo\n\
         summary: Names foo in covers.\n\
         status: stable\n\
         updated: 2026-04-30\n\
         covers: [foo]\n\
         ---\n\n# Covers foo\n",
    );

    let out = run_doc_linter(&root, &["check", "--no-vale", "--format", "json"]);
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    let codes = diagnostic_codes(&stdout);
    assert!(
        !codes.iter().any(|c| c == "orphan-entity"),
        "covers: [foo] should clear strict orphan-entity; codes={codes:?}\nstdout={stdout}"
    );
}

/// The init-scaffolded `example` entity (verbatim description) fires
/// `placeholder-entity`. Severity defaults to "warning" so the
/// build stays green; flipping to "error" hard-fails.
#[test]
fn placeholder_entity_fires_on_init_scaffold_description() {
    let root = unique_tmpdir("density-placeholder");
    write_config(&root, "");
    // Seed an entity whose description matches the init template.
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
        &root.join("docs/ontology/entities/example.md"),
        "---\n\
         id: entity-example\n\
         role: ontology-entity\n\
         title: \"Entity: Example\"\n\
         summary: t\n\
         status: stable\n\
         updated: 2026-04-30\n\
         axis_id: covers\n\
         value_id: example\n\
         display: Example\n\
         description: Placeholder domain entity. Replace with concepts from your own domain (e.g. user, order, tenant, …).\n\
         ---\n\n# Example\n",
    );

    let out = run_doc_linter(&root, &["check", "--no-vale", "--format", "json"]);
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    let codes = diagnostic_codes(&stdout);
    assert!(
        codes.iter().any(|c| c == "placeholder-entity"),
        "expected placeholder-entity on verbatim init scaffold; codes={codes:?}\nstdout={stdout}"
    );
}

/// `require_body_wikilink_per_cover` fires for every entity in
/// `covers:` that isn't named as `[[entity-X]]` in the body.
#[test]
fn missing_body_wikilink_fires_for_uncited_covered_entity() {
    let root = unique_tmpdir("body-wikilink-fires");
    write_config(
        &root,
        "require_body_wikilink_per_cover = true\n\
         missing_body_wikilink_severity = \"error\"\n",
    );
    seed_ontology(&root, &["foo", "bar"]);
    write(
        &root.join("docs/how-to/covers-both-cites-foo.md"),
        "---\n\
         id: covers-both-cites-foo\n\
         role: doc\n\
         title: covers both cites foo\n\
         summary: Mentions only one of the two declared entities.\n\
         status: stable\n\
         updated: 2026-04-30\n\
         covers: [foo, bar]\n\
         ---\n\n\
         # body\n\n\
         See [[entity-foo]] for context.\n",
    );

    let out = run_doc_linter(&root, &["check", "--no-vale", "--format", "json"]);
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    let codes = diagnostic_codes(&stdout);
    let fires: Vec<&String> = codes
        .iter()
        .filter(|c| c.as_str() == "missing-body-wikilink")
        .collect();
    assert_eq!(
        fires.len(),
        1,
        "exactly one entity (bar) should fire; codes={codes:?}\nstdout={stdout}"
    );
    assert!(
        stdout.contains("entity-bar"),
        "diagnostic should name bar; stdout={stdout}"
    );
    assert!(
        !stdout.contains("but no `[[entity-foo]]`"),
        "foo is cited; should NOT fire"
    );
}

/// All declared entities cited in body → rule clears.
#[test]
fn missing_body_wikilink_clears_when_all_cited() {
    let root = unique_tmpdir("body-wikilink-clears");
    write_config(&root, "require_body_wikilink_per_cover = true\n");
    seed_ontology(&root, &["foo"]);
    write(
        &root.join("docs/how-to/cites-foo.md"),
        "---\n\
         id: cites-foo\n\
         role: doc\n\
         title: cites foo\n\
         summary: A doc that cites its covered entity in body.\n\
         status: stable\n\
         updated: 2026-04-30\n\
         covers: [foo]\n\
         ---\n\n\
         # body\n\n\
         See [[entity-foo]].\n",
    );

    let out = run_doc_linter(&root, &["check", "--no-vale", "--format", "json"]);
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    let codes = diagnostic_codes(&stdout);
    assert!(
        !codes.iter().any(|c| c == "missing-body-wikilink"),
        "body wikilink present — rule must clear; codes={codes:?}"
    );
}

/// Default (rule disabled) — silent even when a doc has covers
/// without body wikilinks.
#[test]
fn missing_body_wikilink_silent_when_rule_disabled() {
    let root = unique_tmpdir("body-wikilink-silent");
    write_config(&root, "");
    seed_ontology(&root, &["foo"]);
    write(
        &root.join("docs/how-to/no-wikilink.md"),
        "---\n\
         id: no-wikilink\n\
         role: doc\n\
         title: no wikilink\n\
         summary: covers foo but no body wikilink.\n\
         status: stable\n\
         updated: 2026-04-30\n\
         covers: [foo]\n\
         ---\n\n# body\n",
    );

    let out = run_doc_linter(&root, &["check", "--no-vale", "--format", "json"]);
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    let codes = diagnostic_codes(&stdout);
    assert!(
        !codes.iter().any(|c| c == "missing-body-wikilink"),
        "rule disabled — must stay silent; codes={codes:?}"
    );
}

/// `require_bounded_context_usage = true` fires
/// `unused-bounded-context` for every declared value with zero
/// docs claiming it.
#[test]
fn unused_bounded_context_fires_for_unclaimed_value() {
    let root = unique_tmpdir("unused-bc-fires");
    write_config(&root, "require_bounded_context_usage = true\n");
    seed_ontology(&root, &["foo"]);
    // Author a bounded-context axis with two values; claim only one.
    write(
        &root.join("docs/ontology/axes/bounded-context.md"),
        "---\n\
         id: axis-bounded-context\n\
         role: ontology-axis\n\
         title: \"Axis: bounded-context\"\n\
         summary: t\n\
         status: stable\n\
         updated: 2026-04-30\n\
         axis_id: bounded-context\n\
         ---\n\n# bc axis\n",
    );
    for v in &["claimed", "unclaimed"] {
        write(
            &root.join(format!("docs/ontology/values/bounded-context/{v}.md")),
            &format!(
                "---\n\
                 id: value-bounded-context-{v}\n\
                 role: ontology-value\n\
                 title: \"BC: {v}\"\n\
                 summary: t\n\
                 status: stable\n\
                 updated: 2026-04-30\n\
                 axis_id: bounded-context\n\
                 value_id: {v}\n\
                 display: {v}\n\
                 description: t\n\
                 ---\n\n# {v}\n"
            ),
        );
    }
    write(
        &root.join("docs/how-to/in-claimed.md"),
        "---\n\
         id: in-claimed\n\
         role: doc\n\
         bounded_context: claimed\n\
         title: in claimed\n\
         summary: A doc in the claimed context.\n\
         status: stable\n\
         updated: 2026-04-30\n\
         covers: [foo]\n\
         ---\n\n# x\n\n[[entity-foo]]\n",
    );

    let out = run_doc_linter(&root, &["check", "--no-vale", "--format", "json"]);
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    let codes = diagnostic_codes(&stdout);
    assert!(
        codes.iter().any(|c| c == "unused-bounded-context"),
        "unclaimed value must fire; codes={codes:?}\nstdout={stdout}"
    );
    assert!(
        stdout.contains("`unclaimed`"),
        "diagnostic must name unclaimed; stdout={stdout}"
    );
    assert!(
        !stdout.contains("`claimed` is declared"),
        "claimed value must not fire"
    );
}

/// Exempt list silences the rule for explicitly-reserved values.
#[test]
fn unused_bounded_context_skips_exempt_value() {
    let root = unique_tmpdir("unused-bc-exempt");
    write_config(
        &root,
        "require_bounded_context_usage = true\n\
         bounded_context_usage_exempt = [\"reserved\"]\n",
    );
    seed_ontology(&root, &["foo"]);
    write(
        &root.join("docs/ontology/axes/bounded-context.md"),
        "---\n\
         id: axis-bounded-context\n\
         role: ontology-axis\n\
         title: \"Axis: bounded-context\"\n\
         summary: t\n\
         status: stable\n\
         updated: 2026-04-30\n\
         axis_id: bounded-context\n\
         ---\n\n# bc axis\n",
    );
    write(
        &root.join("docs/ontology/values/bounded-context/reserved.md"),
        "---\n\
         id: value-bounded-context-reserved\n\
         role: ontology-value\n\
         title: \"BC: reserved\"\n\
         summary: t\n\
         status: stable\n\
         updated: 2026-04-30\n\
         axis_id: bounded-context\n\
         value_id: reserved\n\
         display: reserved\n\
         description: t\n\
         ---\n\n# reserved\n",
    );

    let out = run_doc_linter(&root, &["check", "--no-vale", "--format", "json"]);
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    let codes = diagnostic_codes(&stdout);
    assert!(
        !codes.iter().any(|c| c == "unused-bounded-context"),
        "exempt value must not fire; codes={codes:?}"
    );
}

/// Rewriting the description so it no longer matches the init
/// template clears `placeholder-entity` even when `value_id` is
/// still `example`.
#[test]
fn placeholder_entity_clears_when_description_rewritten() {
    let root = unique_tmpdir("density-placeholder-clear");
    write_config(&root, "");
    seed_ontology(&root, &[]); // includes axes + role values
    write(
        &root.join("docs/ontology/entities/example.md"),
        "---\n\
         id: entity-example\n\
         role: ontology-entity\n\
         title: \"Entity: Example\"\n\
         summary: t\n\
         status: stable\n\
         updated: 2026-04-30\n\
         axis_id: covers\n\
         value_id: example\n\
         display: Example\n\
         description: A real domain concept that happens to be named example.\n\
         ---\n\n# Example\n",
    );

    let out = run_doc_linter(&root, &["check", "--no-vale", "--format", "json"]);
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    let codes = diagnostic_codes(&stdout);
    assert!(
        !codes.iter().any(|c| c == "placeholder-entity"),
        "rewritten description should clear placeholder-entity; codes={codes:?}\nstdout={stdout}"
    );
}
