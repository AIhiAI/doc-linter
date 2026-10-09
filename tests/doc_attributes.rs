//! Non-standard frontmatter (handoff cpg-os-vault #9): RCA docs carry
//! `company`, `window` and `findings`, which the Doc table used to drop,
//! so "RCAs for company 234032" couldn't be answered from the graph.

mod common;

use common::{run_doc_linter, seed_ontology, unique_tmpdir, write, write_config};

fn rca(id: &str, company: u32, window: &str) -> String {
    format!(
        "---\nid: {id}\nrole: doc\ntitle: RCA {id}\nsummary: Root cause analysis.\n\
         status: stable\nupdated: 2026-09-01\ncompany: {company}\nwindow: {window}\n\
         findings:\n  - claims-above-peer-norm\n  - billed-base-shrinking\n---\n\n# RCA\n"
    )
}

#[test]
fn custom_frontmatter_is_queryable_as_doc_attributes() {
    let root = unique_tmpdir("doc-attributes");
    write_config(&root, "");
    seed_ontology(&root, &[]);
    write(&root.join("rca/a.md"), &rca("rca-a", 234_032, "2026-08"));
    write(&root.join("rca/b.md"), &rca("rca-b", 99, "2026-08"));
    run_doc_linter(&root, &["check", "--no-vale"]);

    let out = run_doc_linter(
        &root,
        &[
            "query",
            "sql",
            "SELECT d.id AS id FROM Doc d \
             WHERE EXISTS(SELECT 1 FROM doc_attributes a WHERE a.doc_id = d.id AND a.value = 'company=234032') \
             AND EXISTS(SELECT 1 FROM doc_attributes a WHERE a.doc_id = d.id AND a.value = 'findings=billed-base-shrinking')",
        ],
    );
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    let ids: Vec<&str> = v["rows"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|r| r["id"].as_str())
        .collect();
    assert_eq!(ids, ["rca-a"], "{v}");
}
