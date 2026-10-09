//! Shared harness for the [[entity-doc-graph]] integration test suite
//! (#56 phase 1). Centralises the helpers that every test reinvented:
//! a per-run unique temp dir, the path to the built `doc-linter`
//! binary, and the `Command::output()` wrapper that runs it against a
//! seeded workspace.
//!
//! ## Why this lives under `tests/common/`
//!
//! Cargo treats top-level `.rs` files under `tests/` as separate test
//! binaries; subdirectories with a `mod.rs` are NOT compiled as
//! standalone tests. Pulling helpers into `tests/common/mod.rs` and
//! re-exporting via `mod common;` from each integration test file
//! keeps the shared code out of the test runner's discovery while
//! letting every test reach the helpers as `common::*`.
//!
//! ## What lands here vs stays per-test
//!
//! Only helpers whose body is byte-identical across ≥2 test files
//! belong here — anything test-specific (per-test fixture builders,
//! per-test domain seeds) stays inline. The bar is "I'd write the
//! same function in any new test" not "this fn happens to be useful
//! once."

#![allow(
    dead_code,
    unreachable_pub,
    reason = "each integration test pulls only the helpers it needs; the rest look unused per-binary"
)]
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "test harness panics on broken invariants — that is the point"
)]

use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{ChildStdout, Command};
use std::sync::mpsc;
use std::time::Duration;

/// Returns the path to the built `doc-linter` CLI binary that this
/// integration test should invoke. Wraps Cargo's `CARGO_BIN_EXE_*`
/// variable so call sites don't repeat the `env!` plumbing.
pub fn doc_linter_bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_doc-linter"))
}

/// Allocates a per-process, per-call unique temp directory under
/// `$TMPDIR` for an integration test fixture. `label` ends up in the
/// path so a failing test's leftover dir is easy to identify
/// (`/tmp/doc-linter-test-<label>-<pid>-<nanos>`).
///
/// The directory is created on disk and returned; callers own the
/// cleanup contract (most integration tests just leak — temp-dir
/// rotation handles it).
pub fn unique_tmpdir(label: &str) -> PathBuf {
    let base = std::env::temp_dir();
    let pid = std::process::id();
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let dir = base.join(format!("doc-linter-test-{label}-{pid}-{nanos}"));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// Spawns the built `doc-linter` binary against the seeded `root`
/// workspace with `args`, blocks until exit, and returns the raw
/// `Output`. Mirrors the shape every integration test wrote inline.
pub fn run_doc_linter(root: &Path, args: &[&str]) -> std::process::Output {
    let mut cmd = Command::new(doc_linter_bin());
    cmd.arg("--root").arg(root);
    for a in args {
        cmd.arg(a);
    }
    cmd.output().expect("spawn doc-linter")
}

/// Writes `contents` to `path`, creating parent directories as
/// needed. Fixture-seeding shorthand used in every integration test
/// to lay down ontology / config / source files into a fresh tmp
/// workspace.
pub fn write(path: &Path, contents: &str) {
    if let Some(p) = path.parent() {
        std::fs::create_dir_all(p).unwrap();
    }
    std::fs::write(path, contents).unwrap();
}

// ---------------------------------------------------------------------
// Shared corpus / fixture helpers (#56 phase 2 — round 2)
//
// `seed_ontology`, `write_synthetic_scip`, `write_config`, and
// `diagnostic_codes` lived inline in force_growth.rs / explain.rs /
// scaffold_coverage.rs / code_comments_pipeline.rs / homepage_rule.rs
// in byte-identical (or near-identical) copies. Pulling them here
// shrinks each of those files by ~80–110 LOC and gives one
// authoring vocabulary for new tests.
// ---------------------------------------------------------------------

/// Lays down a minimal [[entity-doc-graph]] ontology under
/// `<root>/docs/ontology/` — `role` axis + `doc`/`index` role values
/// + the caller-supplied entities under `entities/`. Tests that need
/// other axes (e.g. `kind: reference` value docs) seed them inline
/// alongside the axis doc — bundling them here breaks
/// `orphan-doc`-sensitive tests where an unlinked value doc fires
/// the rule.
pub fn seed_ontology(root: &Path, extra_entities: &[&str]) {
    write(
        &root.join("docs/ontology/axes/role.md"),
        "---\n\
         id: axis-role\n\
         role: ontology-axis\n\
         title: \"Axis: role\"\n\
         summary: t\n\
         status: stable\n\
         updated: 2026-04-30\n\
         axis_id: role\n\
         ---\n\n# axis\n",
    );
    for v in &["doc", "index"] {
        write(
            &root.join(format!("docs/ontology/values/role/{v}.md")),
            &format!(
                "---\n\
                 id: value-role-{v}\n\
                 role: ontology-value\n\
                 title: \"Role: {v}\"\n\
                 summary: t\n\
                 status: stable\n\
                 updated: 2026-04-30\n\
                 axis_id: role\n\
                 value_id: {v}\n\
                 display: {v}\n\
                 description: t\n\
                 ---\n\n# {v}\n"
            ),
        );
    }
    for ent in extra_entities {
        write(
            &root.join(format!("docs/ontology/entities/{ent}.md")),
            &format!(
                "---\n\
                 id: entity-{ent}\n\
                 role: ontology-entity\n\
                 title: \"Entity: {ent}\"\n\
                 summary: t\n\
                 status: stable\n\
                 updated: 2026-04-30\n\
                 axis_id: covers\n\
                 value_id: {ent}\n\
                 display: {ent}\n\
                 description: e\n\
                 ---\n\n# {ent}\n"
            ),
        );
    }
}

/// Writes a single-file synthetic SCIP index at `out`. Each `(symbol,
/// doc_lines)` tuple becomes a `SymbolInformation` entry plus a
/// matching occurrence with role `Definition`. All symbols land under
/// `crates/my-crate/src/lib.rs` — matches what
/// `seed_ontology` ships and what the pre-#56 inline copies used.
pub fn write_synthetic_scip(out: &Path, funcs: &[(&str, Vec<&str>)]) {
    use scip::types::symbol_information::Kind as ScipKind;
    use scip::types::{Document, Index, Occurrence, SymbolInformation};

    if let Some(p) = out.parent() {
        std::fs::create_dir_all(p).unwrap();
    }
    let mut index = Index::default();
    let mut doc = Document::default();
    doc.relative_path = "crates/my-crate/src/lib.rs".to_string();
    doc.language = "rust".to_string();

    for (symbol, doc_lines) in funcs {
        let mut sym = SymbolInformation::default();
        sym.symbol = (*symbol).to_string();
        sym.documentation = doc_lines.iter().map(|s| (*s).to_string()).collect();
        sym.kind = ScipKind::Function.into();
        doc.symbols.push(sym);
        let mut occ = Occurrence::default();
        occ.symbol = (*symbol).to_string();
        occ.range = vec![0, 0, 0, 1];
        occ.symbol_roles = 1;
        doc.occurrences.push(occ);
    }
    index.documents.push(doc);
    let bytes = protobuf::Message::write_to_bytes(&index).expect("encode");
    std::fs::write(out, bytes).expect("write scip");
}

/// Writes a baseline `.doc-lint.toml` at `<root>/.doc-lint.toml`:
/// vale disabled, `allowed_statuses` tightened to `draft` / `stable`,
/// plus `extra` appended verbatim for per-test overrides. The
/// pre-#56 inline copies hard-coded the same baseline.
pub fn write_config(root: &Path, extra: &str) {
    let mut config = String::from(
        "required_fields = [\"id\", \"role\", \"title\", \"summary\", \"status\", \"updated\"]\n\
         allowed_statuses = [\"draft\", \"stable\"]\n\
         vale_enabled = false\n",
    );
    config.push_str(extra);
    write(&root.join(".doc-lint.toml"), &config);
}

/// Parses `doc-linter check --format=json` stdout and returns every
/// diagnostic `code` flattened across every report entry. Empty /
/// unparseable stdout returns an empty list (matches the pre-#56
/// "no diagnostics fired" semantics every caller relied on).
pub fn diagnostic_codes(stdout: &str) -> Vec<String> {
    if stdout.trim().is_empty() {
        return Vec::new();
    }
    let parsed: serde_json::Value = match serde_json::from_str(stdout) {
        Ok(v) => v,
        Err(_) => return Vec::new(),
    };
    let mut out = Vec::new();
    if let Some(report) = parsed["report"].as_array() {
        for entry in report {
            if let Some(codes) = entry["codes"].as_array() {
                for c in codes {
                    if let Some(s) = c.as_str() {
                        out.push(s.to_string());
                    }
                }
            }
        }
    }
    out
}

/// Writes one line (a JSON-RPC message) to a child's stdin.
pub fn write_line(stdin: &mut impl Write, line: &str) {
    stdin
        .write_all(line.as_bytes())
        .expect("write line to child");
    stdin.write_all(b"\n").expect("write newline");
    stdin.flush().expect("flush child stdin");
}

/// Reads a child's stdout on a thread, so each reply can wait with a
/// deadline instead of blocking the test forever.
pub fn spawn_line_reader(stdout: ChildStdout) -> mpsc::Receiver<String> {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines() {
            let Ok(line) = line else { break };
            if tx.send(line).is_err() {
                break;
            }
        }
    });
    rx
}

/// The next line from [`spawn_line_reader`], or a panic after `deadline`.
pub fn read_line_with_timeout(lines: &mpsc::Receiver<String>, deadline: Duration) -> String {
    lines
        .recv_timeout(deadline)
        .unwrap_or_else(|e| panic!("no reply within {deadline:?}: {e}"))
}

// --- store parity fixtures (store_parity.rs, saved_query_parity.rs) ---

pub fn seed_fixture(root: &Path) {
    write_config(root, "");
    seed_ontology(root, &["pricing", "checkout"]);
    write(
        &root.join("docs/a.md"),
        "---\nid: doc-a\nrole: doc\ntitle: Alpha\nsummary: pricing rules overview\nstatus: draft\n\
         updated: 2026-05-01\ntags: [one, two]\ncovers: [pricing]\n---\n\n# Alpha\n\nSee [[doc-b]].\n\n\
         ## Rules\n\nPricing is computed per line.\n",
    );
    write(
        &root.join("docs/b.md"),
        "---\nid: doc-b\nrole: doc\ntitle: Beta\nsummary: checkout flow\nstatus: stable\n\
         updated: 2026-05-02\ntags: [two]\ncovers: [checkout, pricing]\n---\n\n# Beta\n\n\
         Back to [[doc-a]] and [alpha](a.md).\n\n## Flow\n\nCheckout calls pricing.\n",
    );
    // Entities mapped to crates through `source_modules` (BELONGS_TO, ENTITY_CALLS).
    for (ent, glob) in [("pricing", "crates/a/**"), ("checkout", "crates/b/**")] {
        write(
            &root.join(format!("docs/ontology/entities/{ent}.md")),
            &format!(
                "---\nid: entity-{ent}\nrole: ontology-entity\ntitle: \"Entity: {ent}\"\n\
                 summary: t\nstatus: stable\nupdated: 2026-04-30\naxis_id: covers\n\
                 value_id: {ent}\ndisplay: {ent}\ndescription: e\nsynonyms: [{ent}s]\n\
                 source_modules: [\"{glob}\"]\n---\n\n# {ent}\n"
            ),
        );
    }
    write(
        &root.join("crates/a/README.md"),
        "---\nid: crate-a\nrole: doc\ntitle: Crate a\nsummary: pricing crate\n\
         status: draft\nupdated: 2026-05-01\n---\n\n# a\n",
    );
    let src_a = "// TODO: split this module\npub fn orchestrate() {}\n";
    let src_b = "// FIXME: cache\npub fn compute() {}\nasync fn list_items() {}\n\
                 fn app() { let _ = Router::new().route(\"/items\", get(list_items)); }\n";
    write(&root.join("crates/a/src/lib.rs"), src_a);
    write(&root.join("crates/b/src/lib.rs"), src_b);
    write(
        &root.join("tests/test_math.py"),
        "def test_add():\n    pass\n",
    );
    write(&root.join("math.py"), "def add():\n    pass\n");
    write_rich_scip(&root.join(".doc-lint/code.scip"));
    // Four commits touching both crates, so COUPLED_WITH has pairs.
    let vcs = |args: &[&str]| {
        let out = Command::new("git")
            .args(["-c", "user.name=t", "-c", "user.email=t@t"])
            .args(args)
            .current_dir(root)
            .output()
            .unwrap();
        assert!(out.status.success(), "vcs {args:?}");
    };
    vcs(&["init", "-q"]);
    for i in 0..4 {
        write(
            &root.join("crates/a/src/lib.rs"),
            &format!("{src_a}// rev {i}\n"),
        );
        write(
            &root.join("crates/b/src/lib.rs"),
            &format!("{src_b}// rev {i}\n"),
        );
        vcs(&["add", "-A"]);
        vcs(&["commit", "-q", "-m", &format!("c{i}")]);
    }
}

/// Two crates; `orchestrate` (a) calls `compute` (b) and references the
/// struct `Widget`, which owns a method.
pub fn write_rich_scip(out: &Path) {
    use scip::types::symbol_information::Kind;
    use scip::types::{Document, Index, Occurrence, SymbolInformation};

    let sym = |name: &str, kind: Kind, doc: &str| {
        let mut s = SymbolInformation::default();
        s.symbol = format!("rust-analyzer cargo {name}");
        s.kind = kind.into();
        s.documentation = vec![doc.to_string()];
        s
    };
    let def = |name: &str, span: [i32; 2]| {
        let mut o = Occurrence::default();
        o.symbol = format!("rust-analyzer cargo {name}");
        o.range = vec![span[0], 4, span[0], 10];
        o.enclosing_range = vec![span[0], 0, span[1], 0];
        o.symbol_roles = 1;
        o
    };
    let reference = |name: &str, line: i32| {
        let mut o = Occurrence::default();
        o.symbol = format!("rust-analyzer cargo {name}");
        o.range = vec![line, 8, line, 15];
        o
    };
    let mut a = Document::default();
    a.relative_path = "crates/a/src/lib.rs".into();
    a.language = "rust".into();
    a.symbols = vec![
        sym(
            "a 0.1.0 orchestrate().",
            Kind::Function,
            "Orchestrates pricing.",
        ),
        sym("a 0.1.0 Widget#", Kind::Struct, "A pricing widget."),
        sym("a 0.1.0 Widget#render().", Kind::Method, "Renders pricing."),
    ];
    a.occurrences = vec![
        def("a 0.1.0 orchestrate().", [0, 9]),
        def("a 0.1.0 Widget#", [20, 22]),
        def("a 0.1.0 Widget#render().", [24, 30]),
        reference("b 0.1.0 compute().", 5),
        reference("a 0.1.0 Widget#", 6),
    ];
    let mut b = Document::default();
    b.relative_path = "crates/b/src/lib.rs".into();
    b.language = "rust".into();
    b.symbols = vec![
        sym(
            "b 0.1.0 compute().",
            Kind::Function,
            "Computes checkout totals.",
        ),
        sym("b 0.1.0 list_items().", Kind::Function, "Lists items."),
    ];
    b.occurrences = vec![
        def("b 0.1.0 compute().", [1, 1]),
        def("b 0.1.0 list_items().", [2, 2]),
    ];
    let mut index = Index::default();
    index.documents = vec![a, b];
    let bytes = protobuf::Message::write_to_bytes(&index).unwrap();
    std::fs::create_dir_all(out.parent().unwrap()).unwrap();
    std::fs::write(out, bytes).unwrap();
}

pub fn copy_dir(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for e in std::fs::read_dir(from).unwrap().flatten() {
        let (src, dst) = (e.path(), to.join(e.file_name()));
        if src.is_dir() {
            copy_dir(&src, &dst);
        } else {
            std::fs::copy(&src, &dst).unwrap();
        }
    }
}

pub fn check(root: &Path) {
    let out = Command::new(doc_linter_bin())
        .arg("--root")
        .arg(root)
        .args(["check", "--no-vale"])
        .output()
        .expect("spawn doc-linter");
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(!err.contains("index NOT updated"), "check failed:\n{err}");
}

/// `seed_fixture` plus the shapes many catalog queries need before they
/// return anything: research docs with family tags, python/rust test
/// files paired with production files, and more co-changing commits.
pub fn seed_rich(root: &Path) {
    seed_fixture(root);
    let research = [
        (
            "r1",
            "[research, chunking, dense-retrieval, graph-rag]",
            "chunking dense-retrieval",
        ),
        (
            "r2",
            "[research, reranking, dense-retrieval, query-expansion]",
            "reranking",
        ),
        (
            "r3",
            "[research, llm-era-query-rewriting, query-expansion, rag-evaluation]",
            "rewriting",
        ),
        (
            "r4",
            "[research, deployment-model, graph-rag, graph-similarity]",
            "embedding graph-rag",
        ),
        (
            "r5",
            "[research, rag-evaluation, singleton-topic, iter-7]",
            "evaluation",
        ),
    ];
    for (id, tags, sum) in research {
        write(
            &root.join(format!("docs/research/{id}.md")),
            &format!(
                "---\nid: {id}\nrole: doc\ntitle: Research {id}\nsummary: {sum}\nstatus: stable\n\
                 updated: 2026-05-0{}\ntags: {tags}\ncovers: [pricing]\n---\n\n# {id}\n\n\
                 See [[doc-a]] and [[doc-b]].\n",
                id.len() + 1
            ),
        );
    }
    for (path, body) in [
        (
            "crates/b/src/lib.rs",
            "// FIXME: cache\n/// Computes checkout totals.\n/// @endpoint GET /totals\npub fn compute() {}\n\
             /// @endpoint POST /items\nasync fn list_items() {}\n\
             fn app() { let _ = Router::new().route(\"/items\", get(list_items)); }\n",
        ),
        ("crates/a/tests/it.rs", "#[test]\nfn pricing_it() {}\n"),
        ("tests/test_widget.py", "def test_widget():\n    pass\n"),
        ("widget.py", "def widget():\n    pass\n"),
        (
            "src/app/routes/items.py",
            "@app.get(\"/items\")\ndef list_items():\n    pass\n\n@router.post(\"/items\")\ndef add_item():\n    pass\n",
        ),
        ("src/app/models/items.py", "class Item:\n    pass\n"),
        ("web/src/components/Items.tsx", "export const Items = () => null;\n"),
    ] {
        write(&root.join(path), body);
    }
    for i in 0..3 {
        write(
            &root.join("widget.py"),
            &format!("def widget():\n    pass\n# {i}\n"),
        );
        write(
            &root.join("tests/test_widget.py"),
            &format!("def test_widget():\n    pass\n# {i}\n"),
        );
        write(
            &root.join("crates/a/src/lib.rs"),
            &format!("pub fn orchestrate() {{}}\n// more {i}\n"),
        );
        for args in [vec!["add", "-A"], vec!["commit", "-q", "-m", "rich"]] {
            let out = std::process::Command::new("git")
                .args(["-c", "user.name=t", "-c", "user.email=t@t"])
                .args(&args)
                .current_dir(root)
                .output()
                .unwrap();
            assert!(out.status.success(), "git {args:?}");
        }
    }
    // File.last_touched is the on-disk mtime at one-second resolution, so
    // files written either side of a second boundary (slow CI runner) order
    // differently. Pin every mtime so the recency queries are deterministic.
    let pinned = std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_700_000_000);
    for e in walkdir::WalkDir::new(root)
        .into_iter()
        .filter_entry(|e| e.file_name() != ".git")
        .filter_map(Result::ok)
        .filter(|e| e.file_type().is_file())
    {
        std::fs::File::options()
            .write(true)
            .open(e.path())
            .and_then(|f| f.set_modified(pinned))
            .unwrap();
    }
}

/// Regression snapshot of many named results (replaces the Kuzu-versus-SQLite
/// parity harnesses). Each entry is `label`, a hash of its text and a short
/// preview, kept in `tests/golden/<name>.tsv`. The goldens were recorded from
/// the Kuzu engine (the reference before it was removed) with the documented
/// intentional differences normalised away; `RECORD_GOLDEN=1` rewrites one
/// from the current engine, so review the diff before committing it.
pub struct Snapshot {
    name: String,
    entries: Vec<(String, String)>,
}

impl Snapshot {
    pub fn new(name: &str) -> Self {
        Self {
            name: name.to_string(),
            entries: Vec::new(),
        }
    }

    /// File mtimes and temp roots vary per run; everything else is stable.
    fn scrub(text: &str) -> String {
        let t = regex::Regex::new(r"\d{4}-\d\d-\d\dT\d\d:\d\d:\d\d(\.\d+)?Z").unwrap();
        t.replace_all(text, "<T>").into_owned()
    }

    pub fn add(&mut self, label: &str, text: &str) {
        // Repeated labels (the same call made twice) get a numeric suffix.
        let dups = self
            .entries
            .iter()
            .filter(|(l, _)| l.split(" #").next() == Some(label))
            .count();
        let label = if dups == 0 {
            label.to_string()
        } else {
            format!("{label} #{}", dups + 1)
        };
        self.entries.push((label, Self::scrub(text)));
    }

    /// Entries whose text is non-empty and not an empty list/object: a guard
    /// against a snapshot that passes because every query came back empty.
    pub fn nonempty(&self) -> usize {
        self.entries
            .iter()
            .filter(|(_, t)| !matches!(t.trim(), "" | "[]" | "{}" | "null"))
            .count()
    }

    pub fn finish(self) {
        use sha2::{Digest, Sha256};
        let line = |l: &str, t: &str| {
            let h = Sha256::digest(t.as_bytes());
            let preview: String = t
                .chars()
                .take(70)
                .collect::<String>()
                .replace(['\n', '\t'], " ");
            format!("{l}\t{h:x}\t{preview}")
        };
        let mut entries = self.entries;
        entries.sort();
        let got: Vec<String> = entries.iter().map(|(l, t)| line(l, t)).collect();
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/golden")
            .join(format!("{}.tsv", self.name));
        let body = got.join("\n") + "\n";
        if std::env::var_os("RECORD_GOLDEN").is_some() {
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, body).unwrap();
            return;
        }
        let want = std::fs::read_to_string(&path)
            .unwrap_or_else(|_| panic!("missing golden {}; run with RECORD_GOLDEN=1", self.name));
        let by_label = |l: &str| l.split('\t').next().unwrap_or("").to_string();
        let want_by: std::collections::BTreeMap<String, &str> =
            want.lines().map(|l| (by_label(l), l)).collect();
        let hash_of = |l: &str| l.split('\t').take(2).collect::<Vec<_>>().join("\t");
        let mut bad = Vec::new();
        for ((label, text), g) in entries.iter().zip(&got) {
            match want_by.get(label) {
                Some(w) if hash_of(w) == hash_of(g) => {}
                other => bad.push(format!(
                    "{label}\n  want: {}\n  got:  {}",
                    other.map_or("(missing)", |w| w.split('\t').nth(2).unwrap_or("")),
                    text.chars().take(600).collect::<String>()
                )),
            }
        }
        let got_labels: std::collections::BTreeSet<&str> =
            entries.iter().map(|(l, _)| l.as_str()).collect();
        for label in want_by.keys() {
            if !got_labels.contains(label.as_str()) {
                bad.push(format!("{label}\n  in the golden, not produced"));
            }
        }
        if !bad.is_empty() {
            let mut dump = String::new();
            for (l, t) in &entries {
                dump += &format!("### {l}\n{t}\n");
            }
            let _ = std::fs::write(
                std::env::temp_dir().join(format!("{}.got.txt", self.name)),
                dump,
            );
        }
        assert!(
            bad.is_empty(),
            "{}: {} of {} entries differ from the golden:\n{}",
            self.name,
            bad.len(),
            got.len(),
            bad.iter().take(25).cloned().collect::<Vec<_>>().join("\n")
        );
    }
}
