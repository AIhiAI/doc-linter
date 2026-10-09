#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "integration tests panic on broken invariants — that is the point"
)]
//! Roadmap-45 phase-1 hook-slice integration tests: end-to-end
//! behaviour of `doc-linter --file <ts-or-tsx-path>` against tiny
//! seeded vaults. Mirrors `code_comments_pipeline.rs` in spirit but
//! exercises the per-file `cmd_check_single_ts` path the `PostToolUse`
//! hook actually invokes.
//!
//! These tests assert three contracts:
//!
//! 1. A `.ts` file with a `JSDoc` comment naming an unknown term fires
//!    `comment-vocab-violation` and the binary exits 1.
//! 2. A `.tsx` file with a `JSDoc` comment naming a known ontology
//!    entity exits 0.
//! 3. A `.tsx` file whose JSX text is "Zorblax" but whose `JSDoc` is
//!    clean exits 0 — proves JSX text isn't fed through the vocab
//!    pipeline.

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
    let p = std::env::temp_dir().join(format!("doc-linter-ts-test-{nanos}-{n}"));
    std::fs::create_dir_all(&p).unwrap();
    p
}

/// Seed a minimal vault with `.doc-lint.toml` + one ontology entity
/// (`Outlet`) so the `TermIndex` can resolve known entity references in
/// the per-file TS lint.
fn seed_ontology(root: &Path) {
    write(
        &root.join(".doc-lint.toml"),
        "include = [\"**/*.md\"]\n\
         allowed_statuses = [\"stable\"]\n\
         required_fields = [\"id\", \"role\", \"title\", \"summary\", \"status\", \"updated\"]\n\
         vale_enabled = false\n",
    );
    write(
        &root.join("docs/ontology/entities/outlet.md"),
        "---\n\
         id: entity-outlet\n\
         role: ontology-entity\n\
         title: \"Entity: Outlet\"\n\
         summary: x\n\
         status: stable\n\
         updated: 2026-04-30\n\
         axis_id: covers\n\
         value_id: outlet\n\
         display: Outlet\n\
         description: x\n\
         synonyms: [shop]\n\
         ---\n\
         \n\
         body\n",
    );
}

/// `.ts` file with a `JSDoc` comment that names an unknown noun fires
/// `comment-vocab-violation` against the [[entity-doc-graph]] vocab and
/// exits 1.
#[test]
fn ts_unknown_noun_in_jsdoc_fires_violation() {
    let tmp = tempdir();
    seed_ontology(&tmp);

    let ts_file = tmp.join("apps/web/src/api.ts");
    write(
        &ts_file,
        "/** Adjusts the Zorblax for downstream consumers. */\n\
         export function adjust() {}\n",
    );

    let output = Command::new(doc_linter_bin())
        .arg("--root")
        .arg(&tmp)
        .arg("--no-vale")
        .arg("--file")
        .arg(&ts_file)
        .arg("check")
        .output()
        .expect("run doc-linter");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !output.status.success(),
        "expected non-zero exit; stdout:\n{stdout}\nstderr:\n{stderr}"
    );
    let combined = format!("{stdout}\n{stderr}");
    // Human-format renderer emits "comment vocab — term '<X>' is not …".
    // The hyphenated `comment-vocab-violation` is the Issue::code()
    // string used in JSON output; we don't pass --format=json here so
    // assert on the human-format substring instead.
    assert!(
        combined.contains("comment vocab"),
        "expected comment-vocab-violation diagnostic in output;\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(
        combined.contains("Zorblax"),
        "expected `Zorblax` in output;\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
}

/// `.tsx` file with a `JSDoc` comment naming a known ontology entity
/// (`Outlet`) exits 0.
#[test]
fn tsx_known_entity_in_jsdoc_passes() {
    let tmp = tempdir();
    seed_ontology(&tmp);

    let tsx_file = tmp.join("apps/web/src/Outlet.tsx");
    write(
        &tsx_file,
        "/** renders the Outlet record for the user. */\n\
         export const OutletView = () => null;\n",
    );

    let output = Command::new(doc_linter_bin())
        .arg("--root")
        .arg(&tmp)
        .arg("--no-vale")
        .arg("--file")
        .arg(&tsx_file)
        .arg("check")
        .output()
        .expect("run doc-linter");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "expected exit 0; stdout:\n{stdout}\nstderr:\n{stderr}"
    );
}

/// `.tsx` file whose JSX text content is `Zorblax` but whose `JSDoc` is clean
/// exits 0 — JSX text is excluded from [[entity-doc-graph]] vocab closure
/// by design (UX copy belongs in i18n review).
#[test]
fn tsx_jsx_text_is_not_linted() {
    let tmp = tempdir();
    seed_ontology(&tmp);

    let tsx_file = tmp.join("apps/web/src/Button.tsx");
    write(
        &tsx_file,
        "/** renders a button for the user. */\n\
         export const Button = () => <button>Zorblax Click</button>;\n",
    );

    let output = Command::new(doc_linter_bin())
        .arg("--root")
        .arg(&tmp)
        .arg("--no-vale")
        .arg("--file")
        .arg(&tsx_file)
        .arg("check")
        .output()
        .expect("run doc-linter");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "JSX text 'Zorblax' must NOT trigger vocab violation;\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
}
