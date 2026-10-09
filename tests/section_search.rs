//! Heading-level Section nodes (handoff cpg-os-vault #6): a rule that
//! lives in a long doc's body, under its own heading, is retrievable by
//! `query similar --type section` even though the doc's summary never
//! mentions it.

mod common;

use common::{run_doc_linter, seed_ontology, unique_tmpdir, write, write_config};

#[test]
fn section_search_finds_rule_in_doc_body() {
    let root = unique_tmpdir("section-search");
    write_config(&root, "");
    seed_ontology(&root, &[]);
    write(
        &root.join("docs/traps.md"),
        "---\nid: traps\nrole: doc\ntitle: Traps\n\
         summary: Mistakes analysts make with the store.\nstatus: stable\n\
         updated: 2026-09-01\n---\n\n# Traps\n\n\
         ## T3: Scheme leakage\nSchemes apply after invoicing.\n\n\
         ## T7: Gross predicate\nAn outlet is still buying when its gross \
         billed value in the window is above zero.\n",
    );

    // cpg-analyst re-probe: entity-doc H1 sections (the definition,
    // already the Doc row) crowded rule sections out of the top hits.
    write(
        &root.join("docs/ontology/entities/dormancy.md"),
        "---\nid: entity-dormancy\nrole: ontology-entity\ntitle: \"Entity: dormancy\"\n\
         summary: t\nstatus: stable\nupdated: 2026-09-01\naxis_id: covers\n\
         value_id: dormancy\ndisplay: dormancy\ndescription: e\n---\n\n\
         # Dormancy\nAn outlet is dormant when it is no longer buying: still, outlet, buying.\n",
    );

    let check = run_doc_linter(&root, &["check", "--no-vale"]);
    assert!(
        root.join(".doc-lint/graph.sqlite").exists(),
        "check did not build the graph: {}",
        String::from_utf8_lossy(&check.stderr)
    );

    let out = run_doc_linter(
        &root,
        &[
            "query",
            "similar",
            "outlet still buying",
            "--type",
            "section",
        ],
    );
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    let top = &v["hits"][0];
    assert_eq!(top["id"], "traps#t7-gross-predicate", "{v}");
    assert_eq!(top["doc_id"], "traps", "{v}");
    let ids: Vec<&str> = v["hits"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|h| h["id"].as_str())
        .collect();
    // Neither the entity H1 nor the body-less `# Traps` title is a section.
    assert!(
        !ids.iter()
            .any(|id| id.starts_with("entity-") || *id == "traps#traps"),
        "{ids:?}"
    );

    // The doc axis only sees title + summary, so it can't find the rule.
    let out = run_doc_linter(&root, &["query", "similar", "outlet still buying"]);
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert!(v["hits"].as_array().unwrap().is_empty(), "{v}");
}
