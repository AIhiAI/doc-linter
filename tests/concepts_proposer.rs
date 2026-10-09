#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "integration tests panic on broken invariants -- that is the point"
)]
//! `doc-linter ontology propose|accept` end to end
//! (docs/design/local-concept-proposer.md, test plan items 3-8; items 1 and 2
//! are unit tests in `src/cmd/concepts.rs`). The fixture
//! `tests/fixtures/concepts/symbols.json` holds three small domains plus
//! noise and one thin cluster; it is turned into a SCIP index and ingested
//! by `check`, so the proposer reads a real graph.
//!
//! Run: `cargo test --test concepts_proposer -- --test-threads=1`

use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::Value;

mod common;
use common::{check, doc_linter_bin, run_doc_linter, unique_tmpdir, write};

const FIXTURE: &str = include_str!("fixtures/concepts/symbols.json");

fn write_scip(out: &Path) {
    use scip::types::symbol_information::Kind as ScipKind;
    use scip::types::{Document, Index, Occurrence, SymbolInformation};

    let spec: Value = serde_json::from_str(FIXTURE).unwrap();
    let mut index = Index::default();
    for file in spec["files"].as_array().unwrap() {
        let path = file["path"].as_str().unwrap();
        let sym_of = |name: &str| format!("rust-analyzer cargo fixture 0.1.0 {path}/{name}().");
        let mut doc = Document::default();
        doc.relative_path = path.to_string();
        doc.language = "rust".to_string();
        for (i, s) in file["symbols"].as_array().unwrap().iter().enumerate() {
            let line = (i * 2) as i32;
            let name = s["name"].as_str().unwrap();
            let mut info = SymbolInformation::default();
            info.symbol = sym_of(name);
            info.kind = ScipKind::Function.into();
            if let Some(d) = s["doc"].as_str().filter(|d| !d.is_empty()) {
                info.documentation = vec![d.to_string()];
            }
            if let Some(sig) = s["sig"].as_str().filter(|d| !d.is_empty()) {
                let mut sd = Document::default();
                sd.text = sig.to_string();
                info.signature_documentation = Some(sd).into();
            }
            doc.symbols.push(info);
            let mut def = Occurrence::default();
            def.symbol = sym_of(name);
            def.symbol_roles = 1;
            def.range = vec![line, 0, 20];
            def.enclosing_range = vec![line, 0, line, 80];
            doc.occurrences.push(def);
            for callee in s["calls"].as_array().unwrap() {
                let mut call = Occurrence::default();
                call.symbol = sym_of(callee.as_str().unwrap());
                call.range = vec![line, 40, 50];
                doc.occurrences.push(call);
            }
        }
        index.documents.push(doc);
    }
    std::fs::create_dir_all(out.parent().unwrap()).unwrap();
    std::fs::write(out, protobuf::Message::write_to_bytes(&index).unwrap()).unwrap();
}

/// Fresh repo: `init` (starter ontology), fixture SCIP, `check`.
fn seed(label: &str) -> PathBuf {
    let root = unique_tmpdir(label);
    let init = run_doc_linter(&root, &["init"]);
    assert!(
        init.status.success(),
        "{}",
        String::from_utf8_lossy(&init.stderr)
    );
    write_scip(&root.join(".doc-lint/code.scip"));
    check(&root);
    root
}

fn dl(root: &Path, args: &[&str]) -> std::process::Output {
    Command::new(doc_linter_bin())
        .arg("--root")
        .arg(root)
        .args(args)
        .output()
        .unwrap()
}

fn propose_json(root: &Path) -> (String, Value) {
    let out = dl(root, &["ontology", "propose", "--json"]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let text = String::from_utf8(out.stdout).unwrap();
    let v: Value = serde_json::from_str(&text).unwrap();
    (text, v)
}

/// Name of the proposal whose members include a symbol containing `needle`.
fn name_with_member(v: &Value, needle: &str) -> String {
    v["proposals"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| {
            p["members"]
                .as_array()
                .unwrap()
                .iter()
                .any(|m| m.as_str().unwrap().contains(needle))
        })
        .unwrap_or_else(|| panic!("no proposal holds {needle}: {v}"))["name"]
        .as_str()
        .unwrap()
        .to_string()
}

fn run_suite() {
    let root = seed("concepts");

    // 3. At least three candidates with the expected names; byte-identical reruns.
    let (first, v) = propose_json(&root);
    let (second, _) = propose_json(&root);
    assert_eq!(first, second, "propose is not deterministic");
    let on_disk = std::fs::read_to_string(root.join(".doc-lint/proposals.json")).unwrap();
    assert_eq!(on_disk.trim_end(), second.trim_end());
    assert!(v["proposals"].as_array().unwrap().len() >= 3, "{v}");
    let billing = name_with_member(&v, "create_invoice");
    let session = name_with_member(&v, "create_session");
    let report = name_with_member(&v, "export_report_csv");
    assert!(
        ["invoice", "billing"].contains(&billing.as_str()),
        "{billing}"
    );
    assert!(
        ["session", "store"].contains(&session.as_str()),
        "{session}"
    );
    assert!(["report", "export"].contains(&report.as_str()), "{report}");
    for p in v["proposals"].as_array().unwrap() {
        let n = p["name"].as_str().unwrap();
        assert!(!["new", "get", "helper"].contains(&n), "generic name {n}");
    }
    // propose writes nothing under docs/ beyond what init made.
    assert!(!root
        .join("docs/explanations")
        .join(format!("concept-{billing}.md"))
        .exists());

    // 5a. Refusals: unknown name, and no --all flag at all.
    let out = dl(&root, &["ontology", "accept", "nonesuch"]);
    assert_eq!(out.status.code(), Some(1));
    let out = dl(&root, &["ontology", "accept", "--all"]);
    assert!(!out.status.success(), "accept --all must not exist");

    // 5b. Thin cluster: nothing written.
    let thin = name_with_member(&v, "zebra_alpha");
    let out = dl(&root, &["ontology", "accept", &thin]);
    assert_eq!(
        out.status.code(),
        Some(1),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(String::from_utf8_lossy(&out.stderr).contains("too thin"));
    assert!(!root
        .join(format!("docs/ontology/entities/{thin}.md"))
        .exists());
    assert!(!root
        .join(format!("docs/explanations/concept-{thin}.md"))
        .exists());

    // 4. Accept billing: both files, real content, lint stays clean.
    let dry = dl(&root, &["ontology", "accept", &billing, "--dry-run"]);
    assert!(dry.status.success());
    assert!(!root
        .join(format!("docs/explanations/concept-{billing}.md"))
        .exists());
    let out = dl(&root, &["ontology", "accept", &billing]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let entity =
        std::fs::read_to_string(root.join(format!("docs/ontology/entities/{billing}.md"))).unwrap();
    assert!(entity.contains("status: stable") && entity.contains("role: ontology-entity"));
    let narrative =
        std::fs::read_to_string(root.join(format!("docs/explanations/concept-{billing}.md")))
            .unwrap();
    assert!(narrative.contains("compute_invoice_total"), "{narrative}");
    assert!(narrative.contains("pub fn compute_invoice_total(invoice: &Invoice) -> Money"));
    assert!(narrative.contains("Sums the invoice line items and tax into one total."));
    assert!(narrative.contains("src/billing/invoice.rs:"));
    let chk = dl(&root, &["check", "--no-vale", "--format=json"]);
    let text = String::from_utf8_lossy(&chk.stdout).to_string();
    let codes = common::diagnostic_codes(&text);
    assert!(
        !codes.iter().any(|c| c.contains("orphan")
            || c.starts_with("unknown")
            || c.contains("broken")
            || c.contains("id-mismatch")),
        "codes after accept: {codes:?}\n{text}"
    );

    // 5c. Accepting it again, or onto an existing entity id, refuses and writes nothing new.
    let before =
        std::fs::read_to_string(root.join(format!("docs/explanations/concept-{billing}.md")))
            .unwrap();
    let out = dl(&root, &["ontology", "accept", &billing]);
    assert_eq!(out.status.code(), Some(1));
    let out = dl(
        &root,
        &[
            "ontology",
            "accept",
            &session,
            "--rename",
            &format!("{session}={billing}"),
        ],
    );
    assert_eq!(out.status.code(), Some(1));
    assert!(!root
        .join(format!("docs/explanations/concept-{session}.md"))
        .exists());
    assert_eq!(
        before,
        std::fs::read_to_string(root.join(format!("docs/explanations/concept-{billing}.md")))
            .unwrap()
    );

    // Rename at accept time works.
    let out = dl(
        &root,
        &[
            "ontology",
            "accept",
            &session,
            "--rename",
            &format!("{session}=sessions"),
        ],
    );
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(root.join("docs/explanations/concept-sessions.md").exists());

    // 6. Bare `ontology` still prints the JSON dump.
    let out = dl(&root, &["ontology"]);
    assert!(out.status.success());
    let dump: Value = serde_json::from_slice(&out.stdout).unwrap();
    for key in [
        "version",
        "roles",
        "kinds",
        "lifecycles",
        "bounded-contexts",
        "entities",
    ] {
        assert!(dump.get(key).is_some(), "bare ontology lost `{key}`");
    }
}

/// Without `init` there is no `doc` role to write: accept refuses with a
/// one-line instruction and leaves the lint result unchanged.
#[test]
fn accept_without_init_refuses_and_adds_no_errors() {
    let root = unique_tmpdir("concepts-noinit");
    write_scip(&root.join(".doc-lint/code.scip"));
    check(&root);
    let (_, v) = propose_json(&root);
    let billing = name_with_member(&v, "create_invoice");
    let codes = |root: &Path| {
        let chk = dl(root, &["check", "--no-vale", "--format=json"]);
        common::diagnostic_codes(&String::from_utf8_lossy(&chk.stdout))
    };
    let before = codes(&root);
    let out = dl(&root, &["ontology", "accept", &billing]);
    assert_eq!(out.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&out.stderr).contains("run `doc-linter init`"));
    assert!(!root.join("docs").exists(), "nothing may be written");
    assert_eq!(codes(&root), before);
}

#[test]
fn propose_and_accept() {
    run_suite();
}

/// The proposals for the fixture are what the Kuzu-era engine proposed
/// (recorded from it; `RECORD_GOLDEN=1` re-records, review the diff first).
#[test]
fn proposals_match_the_recorded_golden() {
    let root = seed("concepts-golden");
    let (text, _) = propose_json(&root);
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/golden/concepts_proposals.json");
    if std::env::var_os("RECORD_GOLDEN").is_some() {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, &text).unwrap();
        return;
    }
    let want = std::fs::read_to_string(&path).expect("missing golden; run with RECORD_GOLDEN=1");
    assert_eq!(text, want);
}

#[test]
fn propose_without_a_graph_says_to_run_check() {
    let root = unique_tmpdir("concepts-nograph");
    let out = run_doc_linter(&root, &["ontology", "propose"]);
    assert_eq!(out.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&out.stderr).contains("doc-linter check"));
}

/// 7. Hook: a stub namer's fixed name is used for the first candidate;
/// failure or timeout falls back to the TF-IDF name.
#[test]
fn namer_command_hook_with_fallbacks() {
    let root = seed("concepts-hook");
    let dir = unique_tmpdir("concepts-hook-scripts");
    let ok = dir.join("ok.sh");
    write(&ok, "#!/bin/sh\ncat >/dev/null\necho '{\"name\":\"ledger\",\"description\":\"Money owed by customers.\"}'\n");
    let bad = dir.join("bad.sh");
    write(&bad, "#!/bin/sh\nexit 3\n");
    let slow = dir.join("slow.sh");
    write(&slow, "#!/bin/sh\nsleep 5\necho '{\"name\":\"ledger\"}'\n");

    let set = |cmd: &str| {
        let base = std::fs::read_to_string(root.join(".doc-lint.toml"))
            .unwrap()
            .split("\n[concepts]")
            .next()
            .unwrap()
            .to_string();
        std::fs::write(
            root.join(".doc-lint.toml"),
            format!("{base}\n[concepts]\nnamer_command = \"{cmd}\"\n"),
        )
        .unwrap();
    };
    let namers = |v: &Value| -> Vec<(String, String)> {
        v["proposals"]
            .as_array()
            .unwrap()
            .iter()
            .map(|p| {
                (
                    p["name"].as_str().unwrap().into(),
                    p["namer"].as_str().unwrap().into(),
                )
            })
            .collect()
    };
    let (_, plain) = propose_json(&root);

    set(&format!("sh {}", ok.display()));
    let (_, v) = propose_json(&root);
    let n = namers(&v);
    assert_eq!(
        n.iter()
            .filter(|(name, how)| name == "ledger" && how == "namer_command")
            .count(),
        1,
        "{n:?}"
    );
    assert_eq!(
        v["proposals"].as_array().unwrap().len(),
        plain["proposals"].as_array().unwrap().len()
    );
    let ledger = v["proposals"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["name"] == "ledger")
        .unwrap();
    assert_eq!(ledger["description"], "Money owed by customers.");

    set(&format!("sh {}", bad.display()));
    assert_eq!(propose_json(&root).1, plain, "failing namer must fall back");

    set(&format!("sh {}", slow.display()));
    let started = std::time::Instant::now();
    let out = Command::new(doc_linter_bin())
        .arg("--root")
        .arg(&root)
        .args(["ontology", "propose", "--json"])
        .env("DOC_LINTER_NAMER_TIMEOUT_MS", "300")
        .output()
        .unwrap();
    assert!(out.status.success());
    assert!(
        started.elapsed().as_secs() < 4,
        "namer timeout not enforced"
    );
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v, plain, "timed-out namer must fall back");
}

/// 8. Embeddings: no stored vectors here, so the flag is skipped with a
/// clear message and the proposals equal the plain run.
#[test]
fn embeddings_flag_skips_cleanly_without_vectors() {
    let root = seed("concepts-emb");
    let (_, plain) = propose_json(&root);
    let out = dl(&root, &["ontology", "propose", "--json", "--embeddings"]);
    assert!(out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("no readable stored vectors"));
    assert_eq!(serde_json::from_slice::<Value>(&out.stdout).unwrap(), plain);
}

/// Zero-config first run: no `init`, no config, no ontology -- one
/// `report` shows coverage, stale docs and the concept work list.
#[test]
fn report_works_without_init_or_ontology() {
    let root = unique_tmpdir("concepts-report-bare");
    write_scip(&root.join(".doc-lint/code.scip"));
    write(
        &root.join("docs/notes.md"),
        "---\nid: notes\nrole: doc\nkind: explanation\ntitle: Notes\nsummary: s\nstatus: draft\nupdated: 2020-01-01\n---\n\n# Notes\n",
    );
    let out = run_doc_linter(&root, &["report"]);
    let text = String::from_utf8_lossy(&out.stdout).to_string();
    assert!(
        out.status.success(),
        "{}\n{text}",
        String::from_utf8_lossy(&out.stderr)
    );
    for head in ["COVERAGE", "STALE DOCS", "CONCEPT WORK LIST"] {
        assert!(text.contains(head), "missing {head}:\n{text}");
    }
    assert!(text.contains("25 functions"), "{text}");
    assert!(
        text.contains("notes"),
        "stale authored doc missing:\n{text}"
    );
    assert!(text.contains("Candidate concepts"), "{text}");
    for name in ["invoice", "session"] {
        assert!(text.contains(name), "candidate {name} missing:\n{text}");
    }
    let j = run_doc_linter(&root, &["report", "--json", "--no-refresh"]);
    let v: Value = serde_json::from_slice(&j.stdout).unwrap();
    for key in [
        "coverage",
        "stale_docs",
        "concept_work_list",
        "candidate_concepts",
    ] {
        assert!(v.get(key).is_some(), "{key}");
    }
}

/// After `init`, the 20-odd starter ontology docs are not "stale".
#[test]
fn report_stale_list_skips_all_starter_roles() {
    let root = seed("concepts-report-init");
    let out = run_doc_linter(&root, &["report", "--no-refresh"]);
    let text = String::from_utf8_lossy(&out.stdout).to_string();
    assert!(out.status.success());
    let stale = text
        .split("STALE DOCS")
        .nth(1)
        .unwrap()
        .split("CONCEPT WORK LIST")
        .next()
        .unwrap();
    for starter in ["value-", "axis-", "entity-example", "ontology-mig", "index"] {
        assert!(
            !stale.contains(starter),
            "{starter} listed as stale:\n{stale}"
        );
    }
}
