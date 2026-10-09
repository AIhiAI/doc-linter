#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "integration tests panic on broken invariants — that is the point"
)]
//! #270: a synthetic workload shaped like apache/fineract (66 concepts,
//! ~36k Java methods across 3,000 files, ~2 calls per method) for
//! timing a full `check --rebuild` without building Fineract itself.
//!
//! Ignored by default. Run it with
//! `cargo test --release --test perf_fineract_shape -- --ignored --nocapture`;
//! set `PERF_DIR` to keep the generated workspace for re-runs with a
//! different binary.

use std::fmt::Write as _;
use std::path::Path;

mod common;
use common::{run_doc_linter, unique_tmpdir, write};

const FILES: usize = 3_000;
const METHODS_PER_FILE: usize = 12;

const ENTITIES: [&str; 66] = [
    "loan",
    "client",
    "savings",
    "charge",
    "account",
    "group",
    "office",
    "staff",
    "product",
    "fund",
    "deposit",
    "interest",
    "schedule",
    "transaction",
    "payment",
    "journal",
    "ledger",
    "currency",
    "holiday",
    "calendar",
    "collateral",
    "guarantor",
    "note",
    "report",
    "tax",
    "share",
    "teller",
    "cashier",
    "provisioning",
    "delinquency",
    "repayment",
    "disbursement",
    "accrual",
    "penalty",
    "fee",
    "standing",
    "instruction",
    "batch",
    "job",
    "notification",
    "hook",
    "role",
    "permission",
    "user",
    "address",
    "family",
    "identifier",
    "document",
    "image",
    "survey",
    "meeting",
    "attendance",
    "center",
    "portfolio",
    "rate",
    "floating",
    "tranche",
    "variation",
    "reschedule",
    "writeoff",
    "chargeoff",
    "reversal",
    "adjustment",
    "closure",
    "approval",
    "rejection",
];
const VERBS: [&str; 12] = [
    "calculate",
    "retrieve",
    "create",
    "update",
    "delete",
    "validate",
    "apply",
    "process",
    "find",
    "build",
    "post",
    "handle",
];
const AUX: [&str; 8] = [
    "Data", "Details", "Summary", "Template", "Command", "Event", "Request", "Result",
];

/// Deterministic pseudo-random sequence (LCG), so every run is the same.
struct Lcg(u64);
impl Lcg {
    fn next(&mut self, n: usize) -> usize {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1);
        ((self.0 >> 33) as usize) % n
    }
}

fn cap(s: &str) -> String {
    let mut c = s.chars();
    c.next()
        .map(|f| f.to_ascii_uppercase().to_string() + c.as_str())
        .unwrap_or_default()
}

fn seed(root: &Path) {
    write(
        &root.join(".doc-lint.toml"),
        "exempt = []\nrequired_fields = [\"id\", \"role\", \"title\", \"summary\", \"status\", \"updated\"]\n\
         allowed_statuses = [\"draft\", \"stable\"]\nvale_enabled = false\n",
    );
    let fm = |id: &str, role: &str, extra: &str| {
        format!(
            "---\nid: {id}\nrole: {role}\ntitle: \"{id}\"\nsummary: t\nstatus: stable\n\
             updated: 2026-10-04\n{extra}---\n\n# {id}\n"
        )
    };
    write(
        &root.join("docs/ontology/axes/role.md"),
        &fm("axis-role", "ontology-axis", "axis_id: role\n"),
    );
    for v in ["doc", "index"] {
        write(
            &root.join(format!("docs/ontology/values/role/{v}.md")),
            &fm(
                &format!("value-role-{v}"),
                "ontology-value",
                &format!("axis_id: role\nvalue_id: {v}\ndisplay: {v}\ndescription: t\n"),
            ),
        );
    }
    for e in ENTITIES {
        write(
            &root.join(format!("docs/ontology/entities/{e}.md")),
            &fm(
                &format!("entity-{e}"),
                "ontology-entity",
                &format!(
                    "axis_id: covers\nvalue_id: {e}\ndisplay: {}\ndescription: the {e} concept\n",
                    cap(e)
                ),
            ),
        );
    }
    write_java_and_scip(root);
}

fn write_java_and_scip(root: &Path) {
    use scip::types::symbol_information::Kind as ScipKind;
    use scip::types::{Document, Index, Occurrence, SymbolInformation};

    let mut rng = Lcg(270);
    let pkg = "semanticdb maven maven/org.apache.fineract/fineract-core 1.0 ";
    // Every method's symbol first, so calls can target any file.
    let mut classes = Vec::with_capacity(FILES);
    let mut methods: Vec<Vec<String>> = Vec::with_capacity(FILES);
    for f in 0..FILES {
        let a = ENTITIES[rng.next(ENTITIES.len())];
        let b = ENTITIES[rng.next(ENTITIES.len())];
        let class = format!("{}{}Service{f}", cap(a), cap(b));
        let names = (0..METHODS_PER_FILE)
            .map(|m| {
                let i = f * METHODS_PER_FILE + m;
                format!(
                    "{}{}{}",
                    VERBS[i % VERBS.len()],
                    cap(ENTITIES[(i / VERBS.len()) % ENTITIES.len()]),
                    AUX[m % AUX.len()]
                ) + &m.to_string()
            })
            .collect();
        classes.push(class);
        methods.push(names);
    }
    let mut index = Index::default();
    for f in 0..FILES {
        let dir = format!(
            "module{}/src/main/java/org/apache/fineract/p{}",
            f % 40,
            f % 200
        );
        let rel = format!("{dir}/{}.java", classes[f]);
        let class_sym = format!("{pkg}org/apache/fineract/p{}/{}#", f % 200, classes[f]);
        let mut src = format!("public class {} {{\n", classes[f]);
        let mut doc = Document::default();
        doc.relative_path = rel.clone();
        doc.language = "java".to_string();
        let add_def = |doc: &mut Document,
                       sym: &str,
                       kind: ScipKind,
                       line: i32,
                       s: i32,
                       e: i32,
                       docs: Vec<String>| {
            let mut info = SymbolInformation::default();
            info.symbol = sym.to_string();
            info.kind = kind.into();
            info.documentation = docs;
            doc.symbols.push(info);
            let mut o = Occurrence::default();
            o.symbol = sym.to_string();
            o.symbol_roles = 1;
            o.range = vec![line, s, e];
            o.enclosing_range = vec![line, 0, line, 200];
            doc.occurrences.push(o);
        };
        add_def(
            &mut doc,
            &class_sym,
            ScipKind::Class,
            0,
            13,
            13 + classes[f].len() as i32,
            vec![],
        );
        for (m, name) in methods[f].iter().enumerate() {
            let line = (m + 1) as i32;
            let sym = format!("{class_sym}{name}().");
            let mut body = String::new();
            let mut refs = Vec::new();
            for _ in 0..2 {
                let tf = rng.next(FILES);
                let tm = rng.next(METHODS_PER_FILE);
                let callee = format!(
                    "{pkg}org/apache/fineract/p{}/{}#{}().",
                    tf % 200,
                    classes[tf],
                    methods[tf][tm]
                );
                let col = 26 + name.len() + body.len();
                let _ = write!(body, "{}(); ", methods[tf][tm]);
                refs.push((callee, col, methods[tf][tm].len()));
            }
            let _ = writeln!(src, "    public void {name}() {{ {body}}}");
            // About a third of methods carry prose naming two concepts.
            let docs = if rng.next(3) == 0 {
                vec![format!(
                    "Applies the {} rules to the {} before posting.",
                    ENTITIES[rng.next(ENTITIES.len())],
                    ENTITIES[rng.next(ENTITIES.len())]
                )]
            } else {
                vec![]
            };
            add_def(
                &mut doc,
                &sym,
                ScipKind::Method,
                line,
                16,
                16 + name.len() as i32,
                docs,
            );
            for (callee, col, len) in refs {
                let mut o = Occurrence::default();
                o.symbol = callee;
                o.range = vec![line, col as i32, (col + len) as i32];
                doc.occurrences.push(o);
            }
        }
        src.push_str("}\n");
        write(&root.join(&rel), &src);
        index.documents.push(doc);
    }
    let bytes = protobuf::Message::write_to_bytes(&index).expect("encode");
    write_bytes(&root.join(".doc-lint/code.scip"), &bytes);
}

fn write_bytes(path: &Path, bytes: &[u8]) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, bytes).unwrap();
}

#[test]
#[ignore = "perf workload; run explicitly with --ignored --nocapture"]
fn fineract_shape_rebuild_timings() {
    let root = std::env::var_os("PERF_DIR").map_or_else(
        || unique_tmpdir("perf-fineract-shape"),
        std::path::PathBuf::from,
    );
    if !root.join(".doc-lint/code.scip").exists() {
        seed(&root);
    }
    let started = std::time::Instant::now();
    let out = run_doc_linter(&root, &["check", "--no-vale", "--rebuild"]);
    let total = started.elapsed();
    for line in String::from_utf8_lossy(&out.stderr).lines() {
        if line.contains(" in ") && line.contains("doc-linter: ") {
            eprintln!("{line}");
        }
    }
    eprintln!(
        "total check --rebuild: {:.1}s ({})",
        total.as_secs_f32(),
        root.display()
    );
}
