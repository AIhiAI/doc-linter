#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "integration tests panic on broken invariants — that is the point"
)]
//! Round 2A integration tests: end-to-end behavior of `doc-linter check`
//! when Vale is in the loop.
//!
//! Vale itself is an external binary; on a typical CI runner it isn't
//! installed. Rather than gate every test on its presence, these tests
//! exercise the *Rust-side* pipeline: config generation, the
//! missing-binary path, and the `--no-vale` opt-out. The
//! bounded-context post-processor is unit-tested in
//! `src/disambiguation.rs`.

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

/// Seed a tiny self-contained vault: minimal config, one ontology
/// entity, one bounded-context value, and one "regular" doc that sits
/// in that context. Enough to drive the Vale pipeline through a full
/// generation pass.
fn seed(root: &Path) {
    write(
        &root.join(".doc-lint.toml"),
        "include = [\"**/*.md\"]\n\
         allowed_statuses = [\"stable\"]\n\
         required_fields = [\"id\", \"role\", \"title\", \"summary\", \"status\", \"updated\"]\n",
    );

    // Bounded-context value doc.
    write(
        &root.join("docs/ontology/values/bounded-context/pricing.md"),
        "---\n\
         id: value-bounded-context-pricing\n\
         role: ontology-value\n\
         title: \"Bounded Context: pricing\"\n\
         summary: pricing\n\
         status: stable\n\
         updated: 2026-04-30\n\
         axis_id: bounded-context\n\
         value_id: pricing\n\
         display: pricing\n\
         description: pricing\n\
         ---\n\
         \n\
         body\n",
    );

    // One ontology entity in `pricing`.
    write(
        &root.join("docs/ontology/entities/pricing-rule.md"),
        "---\n\
         id: entity-pricing-rule\n\
         role: ontology-entity\n\
         title: \"Entity: Pricing Rule\"\n\
         summary: pr\n\
         status: stable\n\
         updated: 2026-04-30\n\
         axis_id: covers\n\
         value_id: pricing-rule\n\
         display: Pricing Rule\n\
         description: pr\n\
         synonyms: [rule]\n\
         bounded_contexts: [pricing]\n\
         ---\n\
         \n\
         body\n",
    );

    // A regular doc using the synonym in pricing context.
    write(
        &root.join("docs/some-pricing-doc.md"),
        "---\n\
         id: some-pricing-doc\n\
         role: doc\n\
         kind: reference\n\
         lifecycle: stable\n\
         bounded_context: pricing\n\
         title: x\n\
         summary: y\n\
         status: stable\n\
         updated: 2026-04-30\n\
         ---\n\
         \n\
         The rule resolves quickly.\n",
    );
}

/// `doc-linter check --no-vale` runs successfully and does NOT generate
/// the `.doc-lint/vale/` tree.
#[test]
fn no_vale_flag_skips_config_generation() {
    let tmp = tempdir();
    seed(&tmp);

    let status = Command::new(doc_linter_bin())
        .arg("--root")
        .arg(&tmp)
        .arg("--no-vale")
        .arg("check")
        .output()
        .expect("run doc-linter");

    let stderr = String::from_utf8_lossy(&status.stderr);
    let stdout = String::from_utf8_lossy(&status.stdout);

    // Should not contain the vale-missing message.
    assert!(
        !stderr.contains("vale binary not on PATH"),
        "stderr leaked vale-missing diagnostic with --no-vale:\n{stderr}\n{stdout}"
    );

    // Config tree must NOT have been written.
    assert!(
        !tmp.join(".doc-lint/vale/.vale.ini").exists(),
        "Vale config was generated despite --no-vale"
    );
}

/// `doc-linter check` (Vale enabled) regenerates the Vale config tree
/// idempotently. Whether or not Vale is installed, the config files
/// must be present after the run.
#[test]
fn vale_config_tree_is_generated() {
    let tmp = tempdir();
    seed(&tmp);

    let _ = Command::new(doc_linter_bin())
        .arg("--root")
        .arg(&tmp)
        .arg("check")
        .output()
        .expect("run doc-linter");

    let vale_root = tmp.join(".doc-lint/vale");
    assert!(vale_root.join(".vale.ini").exists(), ".vale.ini missing");
    assert!(
        vale_root.join("styles/Vocabulary/Vocabulary.yml").exists(),
        "Vocabulary rule missing"
    );
    assert!(
        vale_root.join("styles/FA/AmbiguousBare.yml").exists(),
        "AmbiguousBare rule missing"
    );
    let accept =
        std::fs::read_to_string(vale_root.join("styles/config/vocabularies/FA/accept.txt"))
            .expect("read accept.txt");
    assert!(
        accept.contains("Pricing Rule"),
        "accept.txt missing entity display:\n{accept}"
    );
    assert!(
        accept.contains("Pricing Rules"),
        "accept.txt missing naive plural:\n{accept}"
    );
    assert!(
        accept.contains("rule"),
        "accept.txt missing synonym:\n{accept}"
    );

    // Re-run; mtime stability isn't tested here (filesystems vary), but
    // we verify content stability.
    let before = std::fs::read_to_string(vale_root.join(".vale.ini")).unwrap();
    let _ = Command::new(doc_linter_bin())
        .arg("--root")
        .arg(&tmp)
        .arg("check")
        .output()
        .unwrap();
    let after = std::fs::read_to_string(vale_root.join(".vale.ini")).unwrap();
    assert_eq!(before, after, ".vale.ini diverged across runs");
}

/// Files matching `exempt` patterns must be skipped uniformly: every
/// downstream validator (frontmatter parser, link checker, Vale
/// post-processor) sees the same target set. We assert the exempt
/// path never appears in the issue list — independent of any unrelated
/// issues in the rest of the vault.
///
/// (a) full-corpus `check` does not report on the exempt file, even
///     when its frontmatter (Claude Code skill schema:
///     `name`/`description`/`trigger`) doesn't match the vault
///     ontology and would otherwise fail parsing.
/// (b) `--file <exempt-path>` returns success without producing
///     issues for that file (silent no-op semantics).
///
/// Regression for the bug where `target_files` didn't apply the exempt
/// filter — Vale alerts and per-file validators both fired on
/// `.claude/skills/**` and `.opencode/skills/**` even though the
/// exempt-list comment promised "the linter ignores these files
/// entirely." Audit ref: S11 in `docs/cypher-graph-gaps.md`.
#[test]
fn exempt_files_excluded_from_target_set() {
    let tmp = tempdir();

    // Minimal config: no required_fields beyond the defaults; the
    // important entry is `exempt`. We keep allowed_statuses generous
    // so any seed-side noise doesn't mask the assertion.
    write(
        &tmp.join(".doc-lint.toml"),
        "include = [\"**/*.md\"]\n\
         exempt = [\".claude/skills/**\"]\n",
    );

    // The malformed-from-the-vault's-perspective skill file. Without
    // the exempt filter the parser bails on the missing `id` etc. and
    // the file path appears in the issue report.
    let skill_rel = ".claude/skills/example/SKILL.md";
    write(
        &tmp.join(skill_rel),
        "---\n\
         name: example\n\
         description: a skill, not a vault doc\n\
         trigger: /example\n\
         ---\n\
         \n\
         body\n",
    );

    let canon_skill = tmp
        .join(skill_rel)
        .canonicalize()
        .expect("canonicalize skill path");
    let canon_skill_str = canon_skill.to_string_lossy().to_string();

    // (a) Full-corpus check: exempt path must not appear in stderr.
    let out = Command::new(doc_linter_bin())
        .arg("--root")
        .arg(&tmp)
        .arg("check")
        .output()
        .expect("run doc-linter check");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        !stderr.contains(skill_rel) && !stderr.contains(&canon_skill_str),
        "exempt file leaked into full-corpus report:\n--- stderr ---\n{stderr}",
    );

    // (b) --file <exempt>: silent success (no issues, exit 0).
    let out = Command::new(doc_linter_bin())
        .arg("--root")
        .arg(&tmp)
        .arg("check")
        .arg("--file")
        .arg(skill_rel)
        .output()
        .expect("run doc-linter check --file");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        out.status.success(),
        "--file <exempt> should exit 0; got {:?}\n--- stderr ---\n{stderr}",
        out.status.code(),
    );
    assert!(
        !stderr.contains(skill_rel) && !stderr.contains(&canon_skill_str),
        "--file <exempt> should not produce issues for the exempt path:\n--- stderr ---\n{stderr}",
    );
}

/// Frontmatter is metadata: a jargon word in `summary:` must not raise a
/// Vale vocabulary alert, while the same kind of word in the body still
/// does (proving Vale ran). Skipped when `vale` is not installed.
#[test]
fn vale_alerts_inside_frontmatter_are_dropped() {
    if which::which("vale").is_err() {
        return; // needs the Vale binary
    }
    let tmp = tempdir();
    let init = Command::new(doc_linter_bin())
        .arg("--root")
        .arg(&tmp)
        .arg("init")
        .output()
        .expect("run doc-linter init");
    assert!(init.status.success(), "init failed");
    // `init` turns Vale on by default; no config edit needed.
    write(
        &tmp.join("docs/note.md"),
        "---\n\
         id: note\n\
         role: doc\n\
         kind: reference\n\
         title: \"Note\"\n\
         summary: \"Loads the Qwertyzorb cache.\"\n\
         status: stable\n\
         updated: 2026-01-01\n\
         ---\n\
         \n\
         The body names a Blorptastic widget.\n",
    );
    let out = Command::new(doc_linter_bin())
        .arg("--root")
        .arg(&tmp)
        .arg("check")
        .output()
        .expect("run doc-linter check");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("Blorptastic"),
        "Vale did not run on the body:\n{stderr}"
    );
    assert!(
        !stderr.contains("Qwertyzorb"),
        "frontmatter summary was linted:\n{stderr}"
    );
}

/// A Vale runtime failure (exit 2, `E100` on stderr, empty stdout) used to
/// read as "no alerts", so every file reported clean. It must surface as
/// `vale-failed`. Uses a fake `vale` on PATH, so it runs without Vale.
#[cfg(unix)]
#[test]
fn vale_runtime_failure_is_reported_not_clean() {
    use std::os::unix::fs::PermissionsExt;
    let tmp = tempdir();
    seed(&tmp);
    let bin = tmp.join("fakebin");
    let vale = bin.join("vale");
    write(
        &vale,
        "#!/bin/sh\necho '{\"Code\": \"E100\", \"Text\": \"boom\"}' >&2\nexit 2\n",
    );
    std::fs::set_permissions(&vale, std::fs::Permissions::from_mode(0o755)).unwrap();
    let path = format!(
        "{}:{}",
        bin.display(),
        std::env::var("PATH").unwrap_or_default()
    );
    let out = Command::new(doc_linter_bin())
        .env("PATH", path)
        .arg("--root")
        .arg(&tmp)
        .args(["check", "--format", "json"])
        .output()
        .expect("run doc-linter check");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("vale-failed"), "no vale-failed:\n{stdout}");
    assert!(
        stdout.contains("boom"),
        "vale stderr not surfaced:\n{stdout}"
    );
    assert!(!out.status.success(), "a failed Vale run must not pass");
}

/// One dangling symlink anywhere under the root made Vale abort the whole
/// run when it walked the root. Vale now gets the target files only, so
/// the body alert still lands. Skipped when `vale` is not installed.
#[cfg(unix)]
#[test]
fn dangling_symlink_under_root_does_not_blind_vale() {
    if which::which("vale").is_err() {
        return; // needs the Vale binary
    }
    let tmp = tempdir();
    let init = Command::new(doc_linter_bin())
        .arg("--root")
        .arg(&tmp)
        .arg("init")
        .output()
        .expect("run doc-linter init");
    assert!(init.status.success(), "init failed");
    write(
        &tmp.join("docs/note.md"),
        "---\n\
         id: note\n\
         role: doc\n\
         kind: reference\n\
         title: \"Note\"\n\
         summary: \"A note.\"\n\
         status: stable\n\
         updated: 2026-01-01\n\
         ---\n\
         \n\
         The body names a Blorptastic widget.\n",
    );
    std::fs::create_dir_all(tmp.join("build")).unwrap();
    std::os::unix::fs::symlink("/nonexistent/doc-linter-test", tmp.join("build/dangling")).unwrap();
    let out = Command::new(doc_linter_bin())
        .arg("--root")
        .arg(&tmp)
        .arg("check")
        .output()
        .expect("run doc-linter check");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("Blorptastic"),
        "Vale alerts were lost:\n{stderr}"
    );
}

/// AsciiDoc without frontmatter joins the corpus without frontmatter
/// errors. Without `asciidoctor` it is left out of Vale with one
/// `asciidoctor-missing` warning, and markdown alerts still land.
/// Skipped unless `vale` is installed and `asciidoctor` is not.
#[test]
fn adoc_without_asciidoctor_is_skipped_by_vale_with_a_warning() {
    if which::which("vale").is_err() || which::which("asciidoctor").is_ok() {
        return;
    }
    let tmp = tempdir();
    let init = Command::new(doc_linter_bin())
        .arg("--root")
        .arg(&tmp)
        .arg("init")
        .output()
        .expect("run doc-linter init");
    assert!(init.status.success(), "init failed");
    write(
        &tmp.join("guide/index.adoc"),
        "= Loan Guide\n\n== Approve\nPress the Blorptastic button.\n\n[[an-anchor]]\nText.\n",
    );
    write(
        &tmp.join("docs/note.md"),
        "---\nid: note\nrole: doc\nkind: reference\ntitle: Note\nsummary: A note.\n\
         status: stable\nupdated: 2026-01-01\n---\n\nThe Qwertyzorb widget.\n",
    );
    let out = Command::new(doc_linter_bin())
        .arg("--root")
        .arg(&tmp)
        .args(["check", "--format", "json"])
        .output()
        .expect("run doc-linter check");
    let report: serde_json::Value =
        serde_json::from_slice(&out.stdout).expect("check --format json");
    let codes = |suffix: &str| -> Vec<String> {
        report["report"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|e| e["file"].as_str().unwrap().ends_with(suffix))
            .flat_map(|e| e["codes"].as_array().unwrap().clone())
            .map(|c| c.as_str().unwrap().to_string())
            .collect()
    };
    assert_eq!(codes("guide/index.adoc"), ["asciidoctor-missing"]);
    assert!(
        codes("docs/note.md").contains(&"vale-vocabulary-vocabulary".to_string()),
        "markdown Vale alerts lost: {report}"
    );
}

/// With `asciidoctor` on PATH, `.adoc` bodies get vocab-closure too,
/// including docs with `:toc:` (which used to hide every alert).
/// Skipped unless both `vale` and `asciidoctor` are installed.
#[test]
fn adoc_is_linted_when_asciidoctor_is_installed() {
    if which::which("vale").is_err() || which::which("asciidoctor").is_err() {
        return;
    }
    let tmp = tempdir();
    let init = Command::new(doc_linter_bin())
        .arg("--root")
        .arg(&tmp)
        .arg("init")
        .output()
        .expect("run doc-linter init");
    assert!(init.status.success(), "init failed");
    write(
        &tmp.join("guide/index.adoc"),
        "= Loan Guide\n:toc:\n\n== Approve\nPress the Blorptastic button.\n",
    );
    let out = Command::new(doc_linter_bin())
        .arg("--root")
        .arg(&tmp)
        .arg("check")
        .output()
        .expect("run doc-linter check");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("Blorptastic"),
        "Vale did not lint the AsciiDoc body:\n{stderr}"
    );
}

/// An unreadable `vale_dictionaries` file used to empty the accept set
/// (or abort `check`) without a diagnostic. It is now reported by name
/// and path, and the rest of the run carries on.
#[test]
fn unreadable_vale_dictionary_is_reported() {
    let tmp = tempdir();
    seed(&tmp);
    let cfg = tmp.join(".doc-lint.toml");
    let text = std::fs::read_to_string(&cfg).unwrap();
    std::fs::write(
        &cfg,
        format!("{text}\n[vale_dictionaries]\nCompany = \"dicts/missing.txt\"\n"),
    )
    .unwrap();
    let out = Command::new(doc_linter_bin())
        .arg("--root")
        .arg(&tmp)
        .args(["check", "--format", "json"])
        .output()
        .expect("run doc-linter check");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("vocab-dictionary-unreadable") && stdout.contains("dicts/missing.txt"),
        "no diagnostic naming the dictionary:\n{stdout}\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(!out.status.success());
}

fn tempdir() -> std::path::PathBuf {
    use std::sync::atomic::{AtomicU64, Ordering};
    static N: AtomicU64 = AtomicU64::new(0);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let n = N.fetch_add(1, Ordering::SeqCst);
    let p = std::env::temp_dir().join(format!("doc-linter-vale-test-{nanos}-{n}"));
    std::fs::create_dir_all(&p).unwrap();
    p
}
