//! The `check` graph ingest against SQLite: same order as the the store
//! block in `cmd/check/mod.rs`, minus the the store-only readers (coverage
//! lint, cluster lint, sitemap, contradictions, unstubbed concepts),
//! which still need their SQL ports (docs/design/store-trait.md).
//!
//! SCIP cache: when `code.scip` is unchanged since the last successful
//! ingest (and the staged graph still holds that ingest's Function and Type
//! rows) the vault tables are rebuilt with `ResetMode::VaultAndCrossBucket`,
//! the code graph is kept and only its edges into the vault are redrawn;
//! the SCIP file is not parsed. `rebuild` forces the full path.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use super::{code, docs, misc};
use crate::config::LintConfig;
use crate::disambiguation::TermIndex;
use crate::embeddings::Embedder;
use crate::endpoint_extract;
use crate::ontology::Ontology;
use crate::scip_ingest;
use crate::scip_ingest_cache::{self, IngestCache};
use crate::store::{FunctionIndex, ResetMode, StoreRead};
use crate::store_sqlite::SqliteDb;

fn row_count(db: &SqliteDb, table: &str) -> u64 {
    db.query(&format!("SELECT count(*) FROM \"{table}\""), &[])
        .ok()
        .and_then(|r| r.first()?.first()?.as_i64())
        .map_or(0, |n| n as u64)
}

/// Whether the SCIP cache applies: the SCIP file matches the snapshot of
/// the last ingest into this engine, and the staged graph (a copy of the
/// live one) still holds exactly that ingest's Function and Type rows.
fn scip_cache_hit(db: &SqliteDb, root: &Path, scip_path: &Path, rebuild: bool) -> bool {
    if rebuild || !scip_path.exists() {
        return false;
    }
    let Some(cached) = scip_ingest_cache::read_at(&scip_ingest_cache::sqlite_cache_path(root))
    else {
        return false;
    };
    scip_ingest_cache::cache_hit(scip_path, &cached)
        && row_count(db, "Function") == cached.function_count
        && row_count(db, "Type") == cached.type_count
}

/// Ingest the whole corpus into `db` (a `build_and_swap` staging db).
/// Returns the SCIP cache snapshot of a fresh SCIP ingest; the caller
/// writes it once the swap has committed (a snapshot of a graph that never
/// went live would let the next run trust code rows that are not there).
pub fn ingest_corpus(
    db: &SqliteDb,
    root: &Path,
    config: &LintConfig,
    files: &[PathBuf],
    ontology: &Ontology,
    embedder: Option<&dyn Embedder>,
    rebuild: bool,
) -> Result<Option<IngestCache>> {
    let scip_path = scip_ingest::default_scip_path(root);
    let cache_hit = scip_cache_hit(db, root, &scip_path, rebuild);
    let mode = if cache_hit {
        ResetMode::VaultAndCrossBucket
    } else {
        ResetMode::Full
    };
    docs::ingest_with_mode(db, root, config, files, ontology, mode)
        .context("ingest docs into sqlite")?;
    let warn = |what: &str, e: anyhow::Error| eprintln!("doc-linter: {what} failed: {e:#}");
    match misc::ingest_migrations(db, files) {
        Ok(st) if st.migrations > 0 => eprintln!(
            "doc-linter: migration ingest — {} rows ({} applied)",
            st.migrations, st.applied
        ),
        Ok(_) => {}
        Err(e) => warn("migration ingest", e),
    }
    match misc::ingest_repo_meta(db, config) {
        Ok(true) => eprintln!("doc-linter: repo-meta ingest — 1 row"),
        Ok(false) => {}
        Err(e) => warn("repo-meta ingest", e),
    }
    match misc::ingest_repos(db, root, config) {
        Ok(r) if !r.is_empty() => eprintln!("doc-linter: repo ingest — {} row(s)", r.len()),
        Ok(_) => {}
        Err(e) => warn("repo ingest", e),
    }

    let term_index = TermIndex::build(ontology);
    let mut pending_cache = None;
    let facts = if cache_hit {
        let started = std::time::Instant::now();
        let st = code::rederive_cross_bucket(db, &term_index, config)
            .context("scip cache hit: rederive failed; graph NOT updated")?;
        eprintln!(
            "doc-linter: scip ingest (sqlite) — cache hit, rederived {} defined-in edges, \
             {} mention edges, {} belongs-to edges, {} entity-call edges, {} god nodes in \
             {:.2}s. Pass --rebuild to force a full re-ingest.",
            st.defined_in_edges + st.type_defined_in_edges,
            st.mentions_edges + st.type_mentions_edges,
            st.belongs_to_edges + st.type_belongs_to_edges,
            st.entity_calls_edges,
            st.god_nodes,
            started.elapsed().as_secs_f32()
        );
        // The File, Module and Finding rows are rebuilt from the current
        // tree below; IMPORTS are read back first as the facts they came from.
        facts_for_cache_hit(db)?
    } else if scip_path.exists() {
        let f = scip_ingest::parse_scip(&scip_path, root)
            .with_context(|| format!("parse {}", scip_path.display()))?;
        let st = code::ingest_scip(db, &f, &term_index, config, root)
            .context("scip ingest failed; graph NOT updated")?;
        eprintln!(
            "doc-linter: scip ingest (sqlite) — {} functions ({} codegen-excluded), {} types, \
             {} call edges, {} mention edges, {} entity-call edges, {} god nodes",
            st.functions,
            st.codegen_excluded,
            st.types,
            st.calls_edges,
            st.mentions_edges + st.type_mentions_edges,
            st.entity_calls_edges,
            st.god_nodes
        );
        pending_cache = scip_ingest_cache::build_snapshot(
            &scip_path,
            row_count(db, "Function"),
            row_count(db, "Type"),
        );
        f
    } else {
        scip_ingest::ScipFacts::default()
    };

    // After the SCIP step so the cache-hit clear of source-derived rows
    // (Finding included) cannot drop these.
    match misc::ingest_unstubbed_concepts(db, root, ontology, config) {
        Ok(st) if st.findings > 0 => eprintln!(
            "doc-linter: unstubbed-concept ingest — {} finding(s), {} unique candidate(s) \
             across {} doc(s)",
            st.findings, st.unique_candidates, st.docs_scanned
        ),
        Ok(_) => {}
        Err(e) => warn("unstubbed-concept ingest", e),
    }

    endpoints_step(db, root, config, &facts);
    match misc::ingest_files_modules(db, &facts, config, root) {
        Ok(st) => eprintln!(
            "doc-linter: file/module ingest — {} files, {} modules, {} defined-in-file edges, \
             {} in-module edges, {} described-by edges",
            st.files,
            st.modules,
            st.defined_in_file_edges,
            st.in_module_edges,
            st.described_by_edges
        ),
        Err(e) => warn("file/module ingest", e),
    }
    match misc::ingest_imports(db, &facts) {
        Ok(st) if st.pairs > 0 => eprintln!(
            "doc-linter: imports ingest — {} pairs, {} IMPORTS edges",
            st.pairs, st.edges
        ),
        Ok(_) => {}
        Err(e) => warn("imports ingest", e),
    }
    // TEST_FOR is Function to Function: preserved on a cache hit.
    if !cache_hit {
        match misc::ingest_test_for(db) {
            Ok(st) if st.test_functions > 0 => eprintln!(
                "doc-linter: test_for ingest — {} test functions, {} TEST_FOR edges",
                st.test_functions, st.edges
            ),
            Ok(_) => {}
            Err(e) => warn("test_for ingest", e),
        }
    }
    match misc::ingest_coupling(db, root) {
        Ok(st) => eprintln!(
            "doc-linter: coupling ingest — {} commits scanned, {} COUPLED_WITH edges",
            st.commits_scanned, st.coupled_with_edges
        ),
        Err(e) => warn("coupling ingest", e),
    }
    match misc::ingest_findings(db, root) {
        Ok(st) => eprintln!(
            "doc-linter: finding ingest — {} findings, {} HAS_FINDING edges",
            st.findings, st.edges
        ),
        Err(e) => warn("finding ingest", e),
    }
    embeddings_step(db, embedder);
    Ok(pending_cache)
}

/// The facts the source-derived passes need on a SCIP cache hit, read
/// back from the preserved code graph (Function / Type rows and the
/// IMPORTS between files); the File, Module and Finding rows are then
/// deleted so the passes rebuild them from the current tree.
fn facts_for_cache_hit(db: &SqliteDb) -> Result<scip_ingest::ScipFacts> {
    let functions = code::read_function_facts(db)?;
    let mut imports = Vec::new();
    for (a, b, n) in crate::store::typed::file_import_counts(db)? {
        // ponytail: one fact per import, as parse_scip emits them;
        // ingest_imports sums them back into import_count.
        for _ in 0..n {
            imports.push(scip_ingest::ImportFact {
                importer_file: a.clone(),
                imported_file: b.clone(),
            });
        }
    }
    crate::store::typed::clear_source_derived_rows(db)?;
    Ok(scip_ingest::ScipFacts {
        functions,
        imports,
        ..Default::default()
    })
}

/// Fill the usearch index; `None` only restores vectors from the cache.
#[cfg(feature = "vector-usearch")]
fn embeddings_step(db: &SqliteDb, embedder: Option<&dyn Embedder>) {
    use crate::store_sqlite::{vector::UsearchIndex, vectors};
    match vectors::populate(db, embedder, UsearchIndex::new) {
        Ok(st) if st.rows_written + st.rows_skipped_unchanged > 0 => eprintln!(
            "doc-linter: embeddings ({}) — {} rows visited, {} written, {} reused",
            st.backend, st.rows_visited, st.rows_written, st.rows_skipped_unchanged
        ),
        Ok(_) => {}
        Err(e) => eprintln!("doc-linter: embedding population failed: {e:#}"),
    }
}

#[cfg(not(feature = "vector-usearch"))]
fn embeddings_step(_db: &SqliteDb, embedder: Option<&dyn Embedder>) {
    if embedder.is_some() {
        eprintln!("doc-linter: --embeddings with the sqlite store needs --features vector-usearch");
    }
}

/// Endpoint discovery mirrors `ingest_endpoints_step` in
/// `cmd/check/scip_pipeline.rs` (markers, optional regex extractor,
/// `coverage_endpoint_exempt`).
fn endpoints_step(db: &SqliteDb, root: &Path, config: &LintConfig, facts: &scip_ingest::ScipFacts) {
    let found = (|| -> Result<Vec<endpoint_extract::EndpointFact>> {
        let markers = endpoint_extract::extract_endpoint_markers_workspace(root, config)?;
        let all = if config.coverage.endpoint_marker_exclusive {
            markers
        } else {
            let regex = endpoint_extract::extract_endpoints(root, config)?;
            endpoint_extract::merge_endpoint_facts(markers, regex).0
        };
        let exempt: std::collections::HashSet<&str> = config
            .coverage
            .coverage_endpoint_exempt
            .iter()
            .map(String::as_str)
            .collect();
        Ok(all
            .into_iter()
            .filter(|f| !exempt.contains(f.id.as_str()))
            .collect())
    })();
    match found {
        Ok(ep) => match misc::ingest_endpoints(db, &ep, &FunctionIndex::build(facts)) {
            Ok(st) => eprintln!(
                "doc-linter: endpoint ingest — {} axum, {} clap, {} mcp, {} jaxrs endpoints; \
                 {} handler links; {} entity links",
                st.axum, st.clap, st.mcp, st.jaxrs, st.handled_by_edges, st.touches_entity_edges
            ),
            Err(e) => eprintln!("doc-linter: endpoint ingest failed: {e:#}"),
        },
        Err(e) => eprintln!("doc-linter: endpoint discovery failed: {e:#}"),
    }
}
