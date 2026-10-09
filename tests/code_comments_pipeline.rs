#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "integration tests panic on broken invariants — that is the point"
)]
//! Round 3A integration tests: end-to-end behavior of `doc-linter
//! check --lint-code-comments` against a tiny seeded vault that
//! includes one `.rs` file with a doc comment.
//!
//! These exercise the full `cmd_check` pipeline:
//!   1. Markdown lint walks the seeded ontology (one entity).
//!   2. The Round 3A code-comment pass discovers the `.rs` file,
//!      extracts its doc comment, and runs the prose through the
//!      `TermIndex`.
//!
//! `--no-vale` is passed throughout — Vale is an external dep we don't
//! want to require for these tests, and it isn't relevant to Round 3A.

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
    let p = std::env::temp_dir().join(format!("doc-linter-cc-test-{nanos}-{n}"));
    std::fs::create_dir_all(&p).unwrap();
    p
}

/// Seed a minimal ontology vault with one entity (`Outlet`) so the
/// `TermIndex` has something to resolve against. The `--lint-code-comments`
/// pass picks up `.rs` files written under `crates/foo/`.
fn seed_ontology(root: &Path) {
    write(
        &root.join(".doc-lint.toml"),
        "include = [\"**/*.md\"]\n\
         allowed_statuses = [\"stable\"]\n\
         required_fields = [\"id\", \"role\", \"title\", \"summary\", \"status\", \"updated\"]\n\
         vale_enabled = false\n",
    );
    // One ontology entity that the .rs comments can reference.
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

/// `--lint-code-comments` flagging an unknown noun in a doc comment fires
/// `comment-vocab-violation` against the [[entity-doc-graph]] vocab.
#[test]
fn unknown_noun_in_doc_comment_fires_violation() {
    let tmp = tempdir();
    seed_ontology(&tmp);

    // .rs file with a doc comment that mentions an unregistered entity.
    write(
        &tmp.join("crates/foo/src/lib.rs"),
        "/// Adjusts the Zorblax for downstream consumers.\n\
         pub fn adjust() {}\n",
    );

    let output = Command::new(doc_linter_bin())
        .arg("--root")
        .arg(&tmp)
        .arg("--no-vale")
        .arg("--lint-code-comments")
        .arg("--format")
        .arg("json")
        .arg("check")
        .output()
        .expect("run doc-linter");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let v: serde_json::Value = serde_json::from_str(&stdout).expect("doc-linter json output");
    let report = v["report"].as_array().expect("report array");

    // We expect at least one entry with code `comment-vocab-violation`
    // for `Zorblax`, attached to the .rs file.
    let mut found = false;
    for entry in report {
        let file = entry["file"].as_str().unwrap_or("");
        if !file.ends_with("lib.rs") {
            continue;
        }
        let codes = entry["codes"].as_array().unwrap();
        for code in codes {
            if code.as_str() == Some("comment-vocab-violation") {
                let issues = entry["issues"].as_array().unwrap();
                let any_widget = issues
                    .iter()
                    .any(|i| i.as_str().is_some_and(|s| s.contains("Zorblax")));
                if any_widget {
                    found = true;
                }
            }
        }
    }
    assert!(
        found,
        "expected comment-vocab-violation for `Zorblax` in lib.rs;\nstdout:\n{stdout}"
    );
}

/// Issue #199: a doc comment that names an unknown term but wraps it in
/// inline backticks is treated as a code reference, not natural-language
/// prose, and does NOT fire `comment-vocab-violation`. Keeps the lint
/// honest — backticked identifiers are name-drops, not vocab claims.
#[test]
fn backticked_unknown_term_in_doc_comment_is_silenced() {
    let tmp = tempdir();
    seed_ontology(&tmp);

    write(
        &tmp.join("crates/foo/src/lib.rs"),
        "/// Spawns a `Zorblax` for downstream consumers.\n\
         pub fn adjust() {}\n",
    );

    let output = Command::new(doc_linter_bin())
        .arg("--root")
        .arg(&tmp)
        .arg("--no-vale")
        .arg("--lint-code-comments")
        .arg("--format")
        .arg("json")
        .arg("check")
        .output()
        .expect("run doc-linter");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let v: serde_json::Value = serde_json::from_str(&stdout).expect("doc-linter json output");
    let report = v["report"].as_array().expect("report");

    for entry in report {
        let file = entry["file"].as_str().unwrap_or("");
        if !file.ends_with("lib.rs") {
            continue;
        }
        let issues = entry["issues"].as_array().unwrap();
        for issue in issues {
            let s = issue.as_str().unwrap_or("");
            assert!(
                !s.contains("'Zorblax'"),
                "expected no violation for backticked `Zorblax`; got: {s}"
            );
        }
    }
}

/// Issue #199 mixed case: a comment that mentions the same unknown term
/// twice — once bare, once backticked — fires exactly one violation, on
/// the bare occurrence. Guards against the guard accidentally silencing
/// the bare site.
#[test]
fn mixed_bare_and_backticked_terms_fire_only_on_the_bare_one() {
    let tmp = tempdir();
    seed_ontology(&tmp);

    write(
        &tmp.join("crates/foo/src/lib.rs"),
        "/// Wraps a Zorblax; see `Zorblax` for the storage layout.\n\
         pub fn adjust() {}\n",
    );

    let output = Command::new(doc_linter_bin())
        .arg("--root")
        .arg(&tmp)
        .arg("--no-vale")
        .arg("--lint-code-comments")
        .arg("--format")
        .arg("json")
        .arg("check")
        .output()
        .expect("run doc-linter");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let v: serde_json::Value = serde_json::from_str(&stdout).expect("doc-linter json output");
    let report = v["report"].as_array().expect("report");

    let widget_violations: usize = report
        .iter()
        .filter(|entry| {
            entry["file"]
                .as_str()
                .is_some_and(|f| f.ends_with("lib.rs"))
        })
        .flat_map(|entry| entry["issues"].as_array().unwrap().iter())
        .filter(|issue| issue.as_str().is_some_and(|s| s.contains("'Zorblax'")))
        .count();

    assert_eq!(
        widget_violations, 1,
        "expected exactly one Zorblax violation (on the bare token); stdout:\n{stdout}"
    );
}

/// Same setup but the noun is a registered [[entity-doc-graph]] synonym
/// → no `comment-vocab-violation`.
#[test]
fn known_synonym_in_doc_comment_is_quiet() {
    let tmp = tempdir();
    seed_ontology(&tmp);

    // Use the entity's display ("Outlet") in a doc comment — should
    // resolve through the TermIndex and not fire. The rest of the prose
    // is intentionally lowercase so only the entity reference is a
    // tokenization candidate.
    write(
        &tmp.join("crates/foo/src/lib.rs"),
        "/// returns the Outlet record\npub fn x() {}\n",
    );

    let output = Command::new(doc_linter_bin())
        .arg("--root")
        .arg(&tmp)
        .arg("--no-vale")
        .arg("--lint-code-comments")
        .arg("--format")
        .arg("json")
        .arg("check")
        .output()
        .expect("run doc-linter");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let v: serde_json::Value = serde_json::from_str(&stdout).expect("doc-linter json output");
    let report = v["report"].as_array().expect("report");

    // Anything mentioning lib.rs should NOT contain a violation for
    // `Outlet`; the only candidate token in our test prose is the
    // entity itself.
    for entry in report {
        let file = entry["file"].as_str().unwrap_or("");
        if !file.ends_with("lib.rs") {
            continue;
        }
        let issues = entry["issues"].as_array().unwrap();
        for issue in issues {
            let s = issue.as_str().unwrap_or("");
            assert!(
                !s.contains("'Outlet'"),
                "expected no violation for known entity `Outlet`; got: {s}"
            );
        }
    }
}

/// C#, Dart, Vue and TS doc comments flow through discovery → extraction →
/// the shared comment linter: the jargon term fires in each file (at its
/// real source line), while sentence-initial English and the registered
/// `Outlet` entity stay quiet — the same verdicts markdown gets.
#[test]
fn csharp_dart_and_vue_comments_are_linted_like_markdown() {
    let tmp = tempdir();
    seed_ontology(&tmp);
    write(
        &tmp.join("Api/OutletController.cs"),
        "/// <summary>\n/// Gets the Outlet for a Zorblax near the Lantern.\n/// </summary>\npublic class OutletController {}\n",
    );
    write(
        &tmp.join("lib/cache.dart"),
        "/// Holds the Outlet and one Zorblax.\nclass Cache {}\n",
    );
    // Multi-line JSDoc: the empty `/**` line must keep its slot so the
    // hit lands on the source line that holds the term.
    write(
        &tmp.join("src/api.ts"),
        "export class Api {\n  /**\n   * Loads the Outlet for a Zorblax.\n   */\n  load() {}\n}\n",
    );
    // Indented code sample inside a JSDoc is code, as in markdown: quiet.
    write(
        &tmp.join("src/hash.ts"),
        "/**\n * Hashes the Outlet code.\n *\n *     var sb = new Frobnicator();\n */\nexport function hash() {}\n",
    );
    write(
        &tmp.join("pages/index.vue"),
        "<template><div/></template>\n<script>\n/** Shows the Outlet next to a Zorblax. */\nexport function show() {}\n</script>\n",
    );

    let output = Command::new(doc_linter_bin())
        .arg("--root")
        .arg(&tmp)
        .arg("--no-vale")
        .arg("--lint-code-comments")
        .arg("--format")
        .arg("json")
        .arg("check")
        .output()
        .expect("run doc-linter");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let v: serde_json::Value = serde_json::from_str(&stdout).expect("doc-linter json output");
    let issues_for = |suffix: &str| -> Vec<String> {
        v["report"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|e| e["file"].as_str().is_some_and(|f| f.ends_with(suffix)))
            .flat_map(|e| e["issues"].as_array().unwrap().iter())
            .map(|i| i.as_str().unwrap_or("").to_string())
            .collect()
    };
    assert!(
        issues_for("hash.ts").is_empty(),
        "indented code sample linted: {:?}",
        issues_for("hash.ts")
    );
    for (file, line) in [
        ("OutletController.cs", 2),
        ("cache.dart", 1),
        ("index.vue", 3),
        ("api.ts", 3),
    ] {
        let issues = issues_for(file);
        assert_eq!(
            issues.len(),
            1,
            "{file}: expected only the Zorblax hit, got {issues:?}\n{stdout}"
        );
        assert!(issues[0].contains("'Zorblax'"), "{file}: {issues:?}");
        assert!(
            issues[0].contains(&format!("{file}:{line}:")),
            "{file}: expected source line {line}: {issues:?}"
        );
    }
}
