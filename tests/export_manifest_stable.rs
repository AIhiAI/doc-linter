#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "integration tests panic on broken invariants — that is the point"
)]
//! Roadmap 21 follow-up: ensure `doc-linter export` produces a
//! byte-identical `.publish-manifest.json` across consecutive runs
//! over the same input. The publish-pipeline shell script depends on
//! this for its idempotency check ("nothing changed → no commit").
//!
//! `HashSet` → Vec is order-nondeterministic by default; a stable
//! manifest needs an explicit sort step.

use std::process::Command;

fn doc_linter_bin() -> std::path::PathBuf {
    // CARGO_BIN_EXE_<name> is set by Cargo when integration tests run.
    std::path::PathBuf::from(env!("CARGO_BIN_EXE_doc-linter"))
}

fn write(path: &std::path::Path, contents: &str) {
    if let Some(p) = path.parent() {
        std::fs::create_dir_all(p).unwrap();
    }
    std::fs::write(path, contents).unwrap();
}

/// Build a minimal corpus that exercises BOTH `strip_visibility`
/// values (`internal` plus a custom `red-team`) so the resulting Vec
/// has at least two entries — without that we can't detect `HashSet`
/// reordering.
fn seed(root: &std::path::Path) {
    // Minimal `.doc-lint.toml` — point the linter at this fixture
    // root and (default) ontology at `docs/ontology` (which we
    // also seed). We turn off most rigour by minimal vocabulary.
    write(
        &root.join(".doc-lint.toml"),
        r#"
[paths]
docs_root = "docs"
"#,
    );

    // Ontology is loaded from frontmatter on README/index docs; we
    // seed an empty README so cmd_check has something to point at.
    // A `kind: doc` is enough vocabulary to pass.
    write(
        &root.join("docs/ontology/README.md"),
        r"---
kind: index
ontology_version: 3
---

# Ontology

(test fixture)

## Axes

(none)
",
    );

    // One doc with `visibility: internal`, one with `visibility:
    // public`, one with a custom `visibility: red-team` so the
    // strip-vec has TWO distinct values when --strip is specified,
    // plus a fourth public doc.
    let make_doc = |slug: &str, vis: &str| {
        format!("---\nkind: doc\ntitle: {slug}\nvisibility: {vis}\n---\n\n# {slug}\n\n(body)\n")
    };
    write(&root.join("docs/a.md"), &make_doc("a", "public"));
    write(&root.join("docs/b.md"), &make_doc("b", "internal"));
    write(&root.join("docs/c.md"), &make_doc("c", "red-team"));
    write(&root.join("docs/d.md"), &make_doc("d", "public"));
}

/// Asserts that the [[entity-doc-graph]] `export-redacted` manifest is
/// byte-identical across back-to-back runs against the same vault.
#[test]
fn export_manifest_is_byte_stable_across_runs() {
    let tmp = tempdir_workdir();
    seed(&tmp);

    let run_export = |out: &std::path::Path| {
        let status = Command::new(doc_linter_bin())
            .args([
                "export",
                "--root",
                tmp.to_str().unwrap(),
                "--out",
                out.to_str().unwrap(),
                "--strip",
                "internal",
                "--strip",
                "red-team",
                "--no-prelint",
            ])
            .output()
            .expect("spawn doc-linter export");
        assert!(
            status.status.success(),
            "doc-linter export failed: stderr={}",
            String::from_utf8_lossy(&status.stderr)
        );
    };

    let out1 = tmp.join("out-1");
    let out2 = tmp.join("out-2");
    run_export(&out1);
    run_export(&out2);

    let m1 = std::fs::read_to_string(out1.join(".publish-manifest.json")).unwrap();
    let m2 = std::fs::read_to_string(out2.join(".publish-manifest.json")).unwrap();
    assert_eq!(
        m1, m2,
        "two consecutive `doc-linter export` runs over the same input must \
         produce byte-identical manifests"
    );

    // Belt-and-braces: parse the manifest and confirm
    // `strip_visibility` is sorted lexicographically (the underlying
    // bug we're guarding against).
    let v: serde_json::Value = serde_json::from_str(&m1).unwrap();
    let strip = v["strip_visibility"]
        .as_array()
        .expect("strip_visibility is array");
    let raw: Vec<&str> = strip.iter().map(|x| x.as_str().unwrap()).collect();
    let mut sorted = raw.clone();
    sorted.sort_unstable();
    assert_eq!(raw, sorted, "strip_visibility must be lex-sorted");
    assert_eq!(raw, vec!["internal", "red-team"]);
}

/// Hand-rolled tempdir helper for the [[entity-doc-graph]] export-manifest
/// test — we don't pull in `tempfile` for one test. Returns a unique dir
/// under the cargo target tmp area; the dir is intentionally NOT cleaned
/// up so a failed test leaves the fixture for inspection.
fn tempdir_workdir() -> std::path::PathBuf {
    let base = std::env::temp_dir();
    let pid = std::process::id();
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let dir = base.join(format!("doc-linter-export-test-{pid}-{nanos}"));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}
