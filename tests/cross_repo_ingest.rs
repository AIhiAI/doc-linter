//! `cross_repo_roots` docs are ingested with their own repo_id, walked
//! with that repo's own `.doc-lint.toml`, and not validated against the
//! primary's schema. cpg-os-vault setup: the vault's `skip_dirs` has
//! `docs`, which used to prune the external repo's `data/docs/` tree, so
//! only the Repo row landed and no external Doc did.

mod common;

use common::{run_doc_linter, seed_ontology, unique_tmpdir, write, write_config};

#[test]
fn cross_repo_docs_are_ingested_with_their_repo_id() {
    let vault = unique_tmpdir("xrepo-vault");
    let ext = unique_tmpdir("xrepo-ext");
    write_config(
        &vault,
        &format!(
            "skip_dirs = [\".doc-lint\", \".git\", \"docs\"]\ncross_repo_roots = [{:?}]\n",
            ext.display().to_string()
        ),
    );
    seed_ontology(&vault, &[]);
    write(&ext.join(".doc-lint.toml"), "vale_enabled = false\n");
    // `role: knowledge` isn't in the vault's ontology: validating this
    // doc against the vault would fail the check.
    write(
        &ext.join("data/docs/news/gcpl-ceo-resigns.md"),
        "---\nid: gcpl-ceo-resigns\nrole: knowledge\ntitle: GCPL CEO resigns\n\
         summary: Godrej Consumer CEO resigns weeks after reappointment.\n\
         status: stable\nupdated: 2026-08-11\n---\n\n# GCPL\n",
    );

    let check = run_doc_linter(&vault, &["check", "--no-vale"]);
    assert!(
        check.status.success(),
        "external doc was validated against the vault schema: {}",
        String::from_utf8_lossy(&check.stdout)
    );

    let out = run_doc_linter(
        &vault,
        &[
            "query",
            "sql",
            "SELECT repo_id AS repo FROM Doc WHERE id = 'gcpl-ceo-resigns'",
        ],
    );
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(
        v["rows"][0]["repo"],
        ext.canonicalize().unwrap().display().to_string(),
        "{v}"
    );
}

#[test]
fn missing_cross_repo_root_fails_with_one_error() {
    let vault = unique_tmpdir("xrepo-missing");
    write_config(&vault, "cross_repo_roots = [\"../sibling/docs\"]\n");
    seed_ontology(&vault, &[]);

    let check = run_doc_linter(&vault, &["check", "--no-vale"]);
    assert!(!check.status.success());
    let stderr = String::from_utf8_lossy(&check.stderr);
    assert!(
        stderr.contains("cross_repo_roots entry \"../sibling/docs\"")
            && stderr.contains(".doc-lint.toml")
            && stderr.contains("not a directory"),
        "{stderr}"
    );
    assert!(
        check.stdout.is_empty(),
        "lint ran anyway: {}",
        String::from_utf8_lossy(&check.stdout)
    );
}
