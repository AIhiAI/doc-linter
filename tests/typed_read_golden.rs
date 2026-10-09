#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "integration tests panic on broken invariants — that is the point"
)]
//! Typed-read golden (docs/design/store-trait.md step 3): every `GraphRead`
//! method and the `store::typed` helpers, run against a seeded corpus, must
//! keep returning what they returned when the goldens were recorded from the
//! Kuzu engine (the reference before it was removed). `RECORD_GOLDEN=1`
//! re-records from SQLite: review the diff first.
//!
//! Run: `cargo test --test typed_read_golden -- --test-threads=1`

use std::collections::HashSet;
use std::path::Path;

mod common;
use common::{check, seed_fixture, seed_rich, unique_tmpdir, Snapshot};

use doc_linter::config::LintConfig;
use doc_linter::graph::EdgeKind;
use doc_linter::graph_read::GraphRead;
use doc_linter::ids::{DocId, EntityId, FunctionSymbol};
use doc_linter::store::typed;
use doc_linter::store_sqlite;

/// Collects named results into a [`Snapshot`]. `ordered` keeps row order;
/// otherwise a list result is sorted first (no engine promises an order for
/// unsorted scans).
struct Cmp<'a> {
    g: &'a dyn GraphRead,
    snap: Snapshot,
}

impl Cmp<'_> {
    fn same(&mut self, label: &str, f: impl Fn(&dyn GraphRead) -> anyhow::Result<Vec<String>>) {
        self.cmp(label, false, f);
    }

    fn ordered(&mut self, label: &str, f: impl Fn(&dyn GraphRead) -> anyhow::Result<Vec<String>>) {
        self.cmp(label, true, f);
    }

    fn cmp(
        &mut self,
        label: &str,
        ordered: bool,
        f: impl Fn(&dyn GraphRead) -> anyhow::Result<Vec<String>>,
    ) {
        // Error text is engine-specific; only "it failed" is part of the contract.
        let mut r = f(self.g).unwrap_or_else(|_| vec!["ERR".to_string()]);
        if !ordered {
            r.sort();
        }
        self.snap.add(label, &r.join("\n"));
    }
}

fn dbg<T: std::fmt::Debug>(xs: Vec<T>) -> Vec<String> {
    xs.iter().map(|x| format!("{x:?}")).collect()
}

fn one<T: std::fmt::Debug>(x: Option<T>) -> Vec<String> {
    x.iter().map(|x| format!("{x:?}")).collect()
}

fn json<T: serde::Serialize>(x: &T) -> Vec<String> {
    vec![serde_json::to_string(x).unwrap()]
}

fn run(label: &str, seed: impl Fn(&Path)) {
    let sroot = unique_tmpdir(&format!("trg-{label}"));
    seed(&sroot);
    check(&sroot);
    let sdb = store_sqlite::open_ro(&sroot).unwrap();
    let mut c = Cmp {
        g: &sdb,
        snap: Snapshot::new(&format!("typed_reads_{label}")),
    };

    // Sample ids from the graph.
    let docs = sdb.list_all_docs().unwrap();
    let ents = sdb.ranked_entities().unwrap();
    let funcs: Vec<String> = ents
        .iter()
        .flat_map(|e| {
            sdb.functions_mentioning(&EntityId::from(e.id.as_str()))
                .unwrap()
        })
        .map(|f| f.symbol)
        .collect();
    let funcs: Vec<String> = {
        let mut f = funcs;
        f.extend(
            sdb.search_functions_by_symbol_substring("", 100)
                .unwrap()
                .into_iter()
                .map(|f| f.symbol),
        );
        f.sort();
        f.dedup();
        f
    };
    let cfg = LintConfig::default();

    // --- docs ---
    c.ordered("list_all_docs", |g| Ok(dbg(g.list_all_docs()?)));
    for d in docs.iter().map(|d| d.id.as_str()).chain(["no-such-doc"]) {
        let id = DocId::from(d);
        c.same(&format!("get_doc {d}"), |g| Ok(one(g.get_doc(&id)?)));
        c.same(&format!("outbound {d}"), |g| Ok(dbg(g.outbound(&id)?)));
        c.same(&format!("inbound {d}"), |g| Ok(dbg(g.inbound(&id)?)));
        c.same(&format!("covers_inbound {d}"), |g| {
            Ok(dbg(g.covers_inbound_for_entity_doc(&id)?))
        });
    }
    // Shortest path: tie-break between equal-length paths is engine-chosen,
    // so compare hop count and endpoints.
    let ids: Vec<&str> = docs.iter().map(|d| d.id.as_str()).collect();
    let mut pairs: Vec<(&str, &str)> = Vec::new();
    for (i, a) in ids.iter().enumerate().step_by(3) {
        for b in ids.iter().skip(i + 1).step_by(5) {
            pairs.push((a, b));
        }
    }
    let wiki: HashSet<EdgeKind> = HashSet::from([EdgeKind::Wikilink]);
    let covers_only: HashSet<EdgeKind> = HashSet::from([EdgeKind::Covers]);
    for (a, b) in pairs.into_iter().take(80) {
        for (tag, filter) in [
            ("any", None),
            ("wiki", Some(&wiki)),
            ("covers", Some(&covers_only)),
        ] {
            let (from, to) = (DocId::from(a), DocId::from(b));
            c.same(&format!("shortest_path {a}->{b} {tag}"), |g| {
                let p = g.shortest_path(&from, &to, filter, 10)?;
                Ok(vec![format!(
                    "{} hops, {:?}..{:?}",
                    p.len(),
                    p.first().map(|s| s.id.clone()),
                    p.last().map(|s| s.id.clone())
                )])
            });
        }
    }

    // --- entities ---
    c.ordered("ranked_entities", |g| Ok(dbg(g.ranked_entities()?)));
    for e in ents.iter().map(|e| e.id.as_str()).chain(["no-such-entity"]) {
        let id = EntityId::from(e);
        c.same(&format!("get_entity {e}"), |g| Ok(one(g.get_entity(&id)?)));
        c.same(&format!("functions_mentioning {e}"), |g| {
            Ok(dbg(g.functions_mentioning(&id)?))
        });
        for depth in [1, 2] {
            c.same(&format!("subgraph {e} d{depth}"), |g| {
                Ok(g.query_entity_subgraph(e, depth)?
                    .iter()
                    .flat_map(json)
                    .collect())
            });
            c.same(&format!("ego {e} d{depth}"), |g| {
                Ok(g.query_entity_ego_graph(e, depth)?
                    .iter()
                    .flat_map(json)
                    .collect())
            });
        }
    }
    let all_ents: Vec<EntityId> = ents.iter().map(|e| EntityId::from(e.id.as_str())).collect();
    c.same("docs_covering_entities", |g| {
        Ok(dbg(g.docs_covering_entities(&all_ents)?))
    });
    for krate in ["a", "b", "my-crate", "none"] {
        c.ordered(&format!("top_entities_in_crate {krate}"), |g| {
            g.top_entities_in_crate(krate, 5)
        });
        c.same(&format!("dark_functions_in_crate {krate}"), |g| {
            Ok(dbg(g.list_dark_functions_in_crate(krate)?))
        });
    }

    // --- endpoints ---
    for (kind, dark) in [
        (None, false),
        (Some("axum"), false),
        (None, true),
        (Some("clap"), true),
    ] {
        c.same(&format!("list_endpoints {kind:?} {dark}"), |g| {
            Ok(dbg(g.list_endpoints(kind, dark)?))
        });
    }
    c.ordered("endpoint_reach_by_kind", |g| {
        Ok(dbg(g.endpoint_reach_by_kind()?))
    });

    // --- functions ---
    c.same("search_functions", |g| {
        Ok(dbg(g.search_functions_by_symbol_substring("a", 50)?))
    });
    for f in &funcs {
        let sym = FunctionSymbol::from(f.as_str());
        c.same(&format!("function_context {f}"), |g| {
            Ok(one(g.function_context(&sym)?))
        });
        c.same(&format!("function_mentions {f}"), |g| {
            Ok(dbg(g.function_mentions(&sym)?))
        });
        c.same(&format!("function_siblings {f}"), |g| {
            Ok(dbg(g.function_siblings(&sym, 5)?))
        });
        for depth in [1, 3] {
            c.same(&format!("query_impact {f} d{depth}"), |g| {
                Ok(g.query_impact(f, depth)?.iter().flat_map(json).collect())
            });
        }
    }
    c.same("query_impact missing", |g| {
        Ok(one(g.query_impact("nope", 2)?))
    });
    for file in [
        "crates/a/src/lib.rs",
        "crates/b/src/lib.rs",
        "math.py",
        "none.rs",
    ] {
        for line in [0, 1, 2, 3, 100] {
            c.same(&format!("query_at {file}:{line}"), |g| {
                Ok(json(&g.query_at(file, line)?))
            });
        }
    }
    c.same("list_dead_code", |g| Ok(dbg(g.list_dead_code()?)));

    // --- coverage ---
    let anchors = vec![
        "crates/a".to_string(),
        "crates/b".to_string(),
        "math.py".to_string(),
    ];
    c.same("list_dark_public_functions", |g| {
        Ok(dbg(g.list_dark_public_functions(&anchors, &cfg)?))
    });
    c.same("list_all_dark_public_functions", |g| {
        Ok(dbg(g.list_all_dark_public_functions(&cfg)?))
    });
    c.same("list_dark_endpoints", |g| Ok(dbg(g.list_dark_endpoints()?)));
    for th in [0.5_f32, 1.0, 5.0] {
        c.same(&format!("coverage_gaps {th}"), |g| {
            Ok(dbg(g.list_entity_coverage_gaps(th, &[])?))
        });
    }
    c.same("global_function_reach", |g| {
        Ok(dbg(vec![g.global_function_reach(&cfg)?]))
    });

    // --- typed helpers (any StoreRead) ---
    c.same("typed counts", |g| {
        Ok(dbg(vec![
            typed::count_functions(g)?,
            typed::count_functions_reaching_entity(g)?,
            typed::count_functions_documented(g)?,
        ]))
    });
    c.same("typed entity counts", |g| {
        Ok(dbg(vec![
            typed::entity_doc_counts(g)?,
            typed::entity_func_counts(g)?,
        ]))
    });
    c.same("typed crate counts", |g| {
        Ok(dbg(vec![
            typed::crate_function_totals(g)?,
            typed::crate_function_reach(g)?,
        ]))
    });
    c.same("typed dark symbols", |g| {
        Ok(dbg(typed::dark_function_symbols(g)?))
    });
    for t in [
        typed::NodeTable::Function,
        typed::NodeTable::Doc,
        typed::NodeTable::Entity,
        typed::NodeTable::Type,
        typed::NodeTable::Module,
    ] {
        c.same(&format!("cluster_nodes {t:?}"), |g| {
            Ok(dbg(typed::cluster_nodes(g, t)))
        });
    }
    c.same("cluster_edges", |g| Ok(dbg(typed::cluster_edges(g))));
    c.same("design_doc_summaries", |g| {
        Ok(dbg(typed::design_doc_summaries(g)?))
    });
    c.same("file_import_counts", |g| {
        Ok(dbg(typed::file_import_counts(g)?))
    });

    let snap = c.snap;
    assert!(
        snap.nonempty() > 20,
        "{label}: vacuous snapshot ({} non-empty)",
        snap.nonempty()
    );
    snap.finish();
    let _ = std::fs::remove_dir_all(&sroot);
}

#[test]
fn fixture_typed_reads_match() {
    run("fixture", seed_fixture);
}

#[test]
fn rich_typed_reads_match() {
    run("rich", seed_rich);
}
