#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "integration tests panic on broken invariants — that is the point"
)]
//! Round 4B integration tests: end-to-end behavior of `doc-linter init`
//! against a brand-new directory with no docs and no ontology, followed
//! by a `doc-linter check --no-vale` against the scaffolded result.
//!
//! These exercise the drop-in deployment story: `cargo install doc-linter`
//! → `doc-linter init` → `doc-linter check` → 0 issues. If a future
//! change breaks this chain, this test fails.

use std::path::Path;
use std::process::Command;

fn doc_linter_bin() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_BIN_EXE_doc-linter"))
}

fn write(path: &Path, contents: &str) {
    if let Some(p) = path.parent() {
        std::fs::create_dir_all(p).unwrap();
    }
    std::fs::write(path, contents).unwrap();
}

fn tempdir() -> std::path::PathBuf {
    use std::sync::atomic::{AtomicU64, Ordering};
    static N: AtomicU64 = AtomicU64::new(0);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let n = N.fetch_add(1, Ordering::SeqCst);
    let p = std::env::temp_dir().join(format!("doc-linter-init-test-{nanos}-{n}"));
    std::fs::create_dir_all(&p).unwrap();
    p
}

/// `doc-linter init` against an empty target writes the starter
/// `.doc-lint.toml`, scaffolds `docs/ontology/` for the
/// [[entity-doc-graph]], creates `.doc-lint/`, and appends `.doc-lint/`
/// to `.gitignore`.
#[test]
fn init_scaffolds_target_dir() {
    let dir = tempdir();

    let status = Command::new(doc_linter_bin())
        .arg("--root")
        .arg(&dir)
        .arg("init")
        .status()
        .expect("spawn doc-linter init");
    assert!(status.success(), "init exit {status:?}");

    assert!(
        dir.join(".doc-lint.toml").is_file(),
        ".doc-lint.toml missing"
    );
    assert!(dir.join(".doc-lint").is_dir(), ".doc-lint/ missing");
    assert!(
        dir.join("docs/ontology/README.md").is_file(),
        "ontology README missing"
    );
    assert!(
        dir.join("docs/ontology/axes/role.md").is_file(),
        "axis-role missing"
    );
    assert!(
        dir.join("docs/ontology/values/kind/how-to.md").is_file(),
        "kind value how-to missing"
    );
    assert!(
        dir.join("docs/ontology/values/lifecycle/stable.md")
            .is_file(),
        "lifecycle value stable missing"
    );
    assert!(
        dir.join("docs/ontology/migrations/0001-initial.md")
            .is_file(),
        "bootstrap migration missing"
    );
    assert!(
        dir.join("docs/ontology/entities/example.md").is_file(),
        "example entity missing"
    );

    let gitignore = std::fs::read_to_string(dir.join(".gitignore")).unwrap();
    assert!(
        gitignore.lines().any(|l| l.trim() == ".doc-lint/"),
        ".doc-lint/ not in gitignore: {gitignore:?}"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// After `init`, `doc-linter check --no-vale --root <dir>` returns 0
/// — every scaffolded ontology doc lints clean against the linter's
/// own validator. A user-authored doc with proper frontmatter under
/// the same scaffolded vault also lints clean.
#[test]
fn init_then_check_is_clean() {
    let dir = tempdir();

    let status = Command::new(doc_linter_bin())
        .arg("--root")
        .arg(&dir)
        .arg("init")
        .status()
        .expect("spawn init");
    assert!(status.success(), "init failed");

    // The bare init scaffold ships an `example` placeholder entity
    // whose description identifies it as init debris. The
    // `placeholder-entity` rule fires on the verbatim scaffold by
    // design — replacing the description is the canonical first
    // action an adopter takes. Simulate that here so the rest of
    // the assertion (scaffold + user doc lint clean) holds.
    let ent_path = dir.join("docs/ontology/entities/example.md");
    let ent_orig = std::fs::read_to_string(&ent_path).unwrap();
    let ent_replaced = ent_orig.replace(
        "description: Placeholder domain entity. Replace with concepts from your own domain (e.g. user, order, tenant, …).",
        "description: Example domain concept used by the scaffold smoke test.",
    );
    assert_ne!(
        ent_orig, ent_replaced,
        "expected to find the placeholder description in the init scaffold"
    );
    std::fs::write(&ent_path, &ent_replaced).unwrap();

    // Drop a sample user-authored doc into the scaffolded vault to
    // prove the drop-in flow works for real content (not just the
    // ontology stubs).
    write(
        &dir.join("docs/getting-started.md"),
        r"---
id: getting-started
role: doc
kind: how-to
title: Getting started
summary: Tiny how-to that demonstrates a user-authored doc lints clean against the doc-linter scaffold produced by `doc-linter init`.
status: stable
updated: 2026-04-30
covers: [example]
---

# Getting started

Tiny how-to. References the [[entity-example]] placeholder so the
covers axis resolves to a known entity, and links to
[[ontology-index]] for context.
",
    );

    let output = Command::new(doc_linter_bin())
        .arg("--root")
        .arg(&dir)
        .arg("check")
        .arg("--no-vale")
        .output()
        .expect("spawn check");
    assert!(
        output.status.success(),
        "check failed:\nstdout={}\nstderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    // sanity-check the human "all clean" line shows up
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("all clean"),
        "expected 'all clean' on stdout; got: {stdout}"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// Re-running `init` against an already-scaffolded [[entity-doc-graph]]
/// vault MUST NOT overwrite the existing config or ontology. Drop-in
/// init has to be safe to invoke a second time (e.g. via a Makefile
/// target).
#[test]
fn init_is_idempotent_and_does_not_overwrite() {
    let dir = tempdir();

    // first init
    Command::new(doc_linter_bin())
        .arg("--root")
        .arg(&dir)
        .arg("init")
        .status()
        .unwrap();

    // tweak the scaffolded config so we can detect overwrite
    let cfg_path = dir.join(".doc-lint.toml");
    let cfg_orig = std::fs::read_to_string(&cfg_path).unwrap();
    let sentinel = "# USER-EDITED: do not overwrite\n";
    std::fs::write(&cfg_path, format!("{sentinel}{cfg_orig}")).unwrap();

    // tweak an ontology file so we can detect overwrite there too
    let ent_path = dir.join("docs/ontology/entities/example.md");
    let ent_orig = std::fs::read_to_string(&ent_path).unwrap();
    let user_marker = "user-edited-entity-example";
    let ent_modified = ent_orig.replace("placeholder entity", user_marker);
    std::fs::write(&ent_path, &ent_modified).unwrap();

    // second init
    let status = Command::new(doc_linter_bin())
        .arg("--root")
        .arg(&dir)
        .arg("init")
        .status()
        .unwrap();
    assert!(status.success());

    let cfg_after = std::fs::read_to_string(&cfg_path).unwrap();
    assert!(
        cfg_after.starts_with(sentinel),
        ".doc-lint.toml was overwritten on re-init: {cfg_after}"
    );

    let ent_after = std::fs::read_to_string(&ent_path).unwrap();
    assert!(
        ent_after.contains(user_marker),
        "entity-example was overwritten on re-init"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// Round 4B graceful-degradation smoke: `doc-linter check` against a
/// directory that has zero ontology docs (just one user-authored
/// markdown file with explicit role + kind frontmatter) must NOT panic.
/// Bootstrap meta-roles cover the linter's own validator; a missing
/// ontology means the user's `role: doc` is unknown — that's expected
/// to surface as an `unknown-role` lint issue, not a panic.
#[test]
fn check_does_not_panic_on_zero_ontology_repo() {
    let dir = tempdir();

    // No `docs/ontology/`. Just a bare config and one markdown doc.
    write(
        &dir.join(".doc-lint.toml"),
        // Minimal config — disable vale (no binary on CI) and code-comment
        // lint (no .rs files in this tmp tree).
        "vale_enabled = false\nlint_code_comments = false\n",
    );
    write(
        &dir.join("hello.md"),
        r"---
id: hello
role: doc
kind: how-to
title: Hello
summary: Tiny doc with no ontology — exercises the graceful-degradation path. Should produce unknown-role / unknown-axis-value issues but must NOT panic.
status: stable
updated: 2026-04-30
---

# Hello

This vault has no ontology. The doc-linter should still walk it
without crashing.
",
    );

    let output = Command::new(doc_linter_bin())
        .arg("--root")
        .arg(&dir)
        .arg("check")
        .arg("--no-vale")
        .output()
        .expect("spawn check");

    // We don't care if exit is 0 or 1 here — what we care about is
    // that the binary returned cleanly (no panic, no rust-side abort
    // which would surface as exit code 101 / 134).
    let code = output.status.code().unwrap_or(-1);
    assert!(
        code == 0 || code == 1,
        "doc-linter check on zero-ontology repo crashed (exit {code}): \
         stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let _ = std::fs::remove_dir_all(&dir);
}
