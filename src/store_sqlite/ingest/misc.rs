//! SQLite twins of the smaller ingest writers: files / modules,
//! endpoints, imports, coupling, findings, test_for, migrations, repos
//! and repo meta. Row derivation stays in the the store modules (made
//! `pub(crate)`); only the writes are SQL.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use super::{insert_edges, insert_pairs, int, s, upsert};
use crate::config::LintConfig;
use crate::endpoint_extract::{EndpointFact, EndpointKind};
use crate::scip_ingest::ScipFacts;
use crate::store::coupling_ingest::{
    aggregate_pairs, format_iso8601, parse_git_log, run_git_log, CouplingIngestStats, MIN_COMMITS,
    MIN_JACCARD,
};
use crate::store::endpoint_ingest::{resolve_handler_symbol, FunctionIndex};
use crate::store::file_module_ingest::{
    collect_files, collect_modules, in_module_pairs, FileModuleIngestStats,
};
use crate::store::finding_ingest::{collect_findings, FindingIngestStats};
use crate::store::imports_ingest::{import_counts, ImportsIngestStats};
use crate::store::migration_ingest::{read_bool, read_i64, MigrationIngestStats};
use crate::store::projections::EndpointIngestStats;
use crate::store::repo_ingest::{make_repo, IngestedRepo};
use crate::store::test_for_ingest::{plan_test_for, TestForIngestStats};
use crate::store::unstubbed_concepts_ingest::{plan_unstubbed, UnstubbedIngestStats};
use crate::store::{StoreRead, Value};
use crate::store_sqlite::SqliteDb;

pub fn ingest_migrations(db: &SqliteDb, files: &[PathBuf]) -> Result<MigrationIngestStats> {
    let mut stats = MigrationIngestStats::default();
    let mut seen = HashSet::new();
    let mut rows = Vec::new();
    for path in files {
        let Ok(doc) = crate::parser::parse_doc(path) else {
            continue;
        };
        let Some(meta) = doc.meta else { continue };
        if meta.effective_role() != Some("ontology-migration") || !seen.insert(meta.id.clone()) {
            continue;
        }
        let applied = read_bool(&meta, "applied");
        rows.push(vec![
            s(&meta.id),
            int(read_i64(&meta, "from_version")),
            int(read_i64(&meta, "to_version")),
            Value::Bool(applied),
            s(meta.extra_str("applied_at").unwrap_or("")),
            s(&meta.title),
            s(&meta.summary),
        ]);
        stats.migrations += 1;
        stats.applied += usize::from(applied);
    }
    db.transaction(|db| {
        db.execute_batch("DELETE FROM Migration;")?;
        upsert(
            db,
            "Migration",
            "id",
            &[
                "id",
                "from_version",
                "to_version",
                "applied",
                "applied_at",
                "title",
                "summary",
            ],
            rows,
        )
    })?;
    Ok(stats)
}

pub fn ingest_repo_meta(db: &SqliteDb, config: &LintConfig) -> Result<bool> {
    let domain = config.ontology.repo_domain.as_deref().unwrap_or("");
    let problem = config
        .ontology
        .repo_problem_statement
        .as_deref()
        .unwrap_or("");
    if domain.is_empty() && problem.is_empty() {
        return Ok(false);
    }
    upsert(
        db,
        "RepoMeta",
        "id",
        &["id", "domain", "problem_statement"],
        [vec![s("repo"), s(domain), s(problem)]],
    )?;
    Ok(true)
}

pub fn ingest_repos(db: &SqliteDb, root: &Path, config: &LintConfig) -> Result<Vec<IngestedRepo>> {
    let mut all = vec![make_repo(root)?];
    for cross in &config.cross_repo_roots {
        let p = PathBuf::from(cross);
        let abs = if p.is_absolute() { p } else { root.join(p) };
        if let Ok(canon) = abs.canonicalize() {
            all.push(make_repo(&canon)?);
        }
    }
    db.transaction(|db| {
        db.execute_batch("DELETE FROM Repo;")?;
        upsert(
            db,
            "Repo",
            "id",
            &["id", "root_path", "name"],
            all.iter()
                .map(|r| vec![s(&r.id), s(&r.root_path), s(&r.name)]),
        )
    })?;
    Ok(all)
}

pub fn ingest_files_modules(
    db: &SqliteDb,
    facts: &ScipFacts,
    config: &LintConfig,
    root: &Path,
) -> Result<FileModuleIngestStats> {
    let files = collect_files(root, config);
    let modules = collect_modules(root, config);
    let mut stats = FileModuleIngestStats::default();
    db.transaction(|db| {
        stats.files += upsert(
            db,
            "File",
            "path",
            &["path", "language", "loc", "last_touched"],
            files
                .iter()
                .map(|f| vec![s(&f.path), s(&f.language), int(f.loc), s(&f.last_touched)]),
        )?;
        stats.modules += upsert(
            db,
            "Module",
            "id",
            &["id", "kind", "path", "name"],
            modules
                .iter()
                .map(|m| vec![s(&m.id), s(&m.kind), s(&m.path), s(&m.name)]),
        )?;
        let known: HashSet<&str> = files.iter().map(|f| f.path.as_str()).collect();
        let mut seen: HashSet<(&str, &str)> = HashSet::new();
        stats.defined_in_file_edges += insert_pairs(
            db,
            "DEFINED_IN_FILE",
            ("Function", "symbol"),
            ("File", "path"),
            facts
                .functions
                .iter()
                .filter(|f| known.contains(f.file.as_str()))
                .filter(|f| seen.insert((f.symbol.as_str(), f.file.as_str())))
                .map(|f| (f.symbol.clone(), f.file.clone())),
        )?;
        stats.in_module_edges += insert_pairs(
            db,
            "IN_MODULE",
            ("File", "path"),
            ("Module", "id"),
            in_module_pairs(&files, &modules),
        )?;
        stats.described_by_edges += described_by(db)?;
        Ok(())
    })?;
    Ok(stats)
}

/// `Module -> README Doc` edges, matched on the README path.
fn described_by(db: &SqliteDb) -> Result<usize> {
    db.write_rows(
        "INSERT INTO DESCRIBED_BY(src, dst) \
         SELECT m.id, d.id FROM Module m JOIN Doc d \
           ON d.path = CASE WHEN m.path = '' THEN 'README.md' ELSE m.path || '/README.md' END \
         WHERE ?1 = 1",
        [vec![Value::Int(1)]],
    )
}

pub fn ingest_imports(db: &SqliteDb, facts: &ScipFacts) -> Result<ImportsIngestStats> {
    let counts = import_counts(facts);
    let mut stats = ImportsIngestStats {
        pairs: counts.len(),
        edges: 0,
    };
    stats.edges = insert_edges(
        db,
        "IMPORTS",
        ("File", "path"),
        ("File", "path"),
        &["import_count", "source"],
        counts
            .iter()
            .map(|((a, b), n)| vec![s(a), s(b), int(*n), s("scip_import")]),
    )?;
    Ok(stats)
}

pub fn ingest_test_for(db: &SqliteDb) -> Result<TestForIngestStats> {
    let rows = db
        .query("SELECT symbol, file FROM Function", &[])?
        .iter()
        .map(|r| (r[0].str_or_empty(), r[1].str_or_empty()))
        .collect();
    let (stats, edges) = plan_test_for(rows);
    insert_edges(
        db,
        "TEST_FOR",
        ("Function", "symbol"),
        ("Function", "symbol"),
        &["confidence"],
        edges.iter().map(|(a, b, c)| vec![s(a), s(b), s(*c)]),
    )?;
    Ok(stats)
}

pub fn ingest_coupling(db: &SqliteDb, root: &Path) -> Result<CouplingIngestStats> {
    let mut stats = CouplingIngestStats::default();
    let file_paths: HashSet<String> = db
        .strings("SELECT path FROM File")?
        .into_iter()
        .filter(|p| !p.is_empty())
        .collect();
    if file_paths.is_empty() {
        return Ok(stats);
    }
    let Some(log) = run_git_log(root) else {
        return Ok(stats);
    };
    let commits = parse_git_log(&log, &file_paths);
    stats.commits_scanned = commits.len();
    if commits.is_empty() {
        return Ok(stats);
    }
    let (pair_counts, per_file) = aggregate_pairs(&commits);
    stats.pairs_examined = pair_counts.len();
    let mut rows = Vec::new();
    for ((a, b), (count, last_ts)) in &pair_counts {
        if *count < MIN_COMMITS {
            continue;
        }
        let (Some(sa), Some(sb)) = (per_file.get(a), per_file.get(b)) else {
            continue;
        };
        let union = sa.union(sb).count() as f64;
        if union <= 0.0 {
            continue;
        }
        let jaccard = f64::from(*count) / union;
        if jaccard < MIN_JACCARD {
            continue;
        }
        rows.push(vec![
            s(a),
            s(b),
            int(*count),
            s(format_iso8601(*last_ts)),
            Value::Float(jaccard),
        ]);
    }
    stats.coupled_with_edges = insert_edges(
        db,
        "COUPLED_WITH",
        ("File", "path"),
        ("File", "path"),
        &["commits", "last_co_change_at", "jaccard"],
        rows,
    )?;
    Ok(stats)
}

pub fn ingest_findings(db: &SqliteDb, root: &Path) -> Result<FindingIngestStats> {
    let files: Vec<(String, String)> = db
        .query("SELECT path, language FROM File", &[])?
        .iter()
        .map(|r| (r[0].str_or_empty(), r[1].str_or_empty()))
        .filter(|(p, _)| !p.is_empty())
        .collect();
    let findings = collect_findings(root, &files);
    let mut stats = FindingIngestStats::default();
    stats.findings = upsert(
        db,
        "Finding",
        "id",
        &["id", "kind", "file", "line", "message", "severity"],
        findings.iter().map(|f| {
            vec![
                s(&f.id),
                s(&f.kind),
                s(&f.file),
                int(f.line),
                s(&f.message),
                s(&f.severity),
            ]
        }),
    )?;
    stats.edges = insert_pairs(
        db,
        "HAS_FINDING",
        ("File", "path"),
        ("Finding", "id"),
        findings.iter().map(|f| (f.file.clone(), f.id.clone())),
    )?;
    Ok(stats)
}

pub fn ingest_endpoints(
    db: &SqliteDb,
    facts: &[EndpointFact],
    func_index: &FunctionIndex,
) -> Result<EndpointIngestStats> {
    let mut stats = EndpointIngestStats::default();
    let resolved: Vec<(String, &EndpointFact)> = facts
        .iter()
        .map(|f| (resolve_handler_symbol(f, func_index), f))
        .collect();
    db.transaction(|db| {
        db.execute_batch("DELETE FROM Endpoint; DELETE FROM ENDPOINT_HANDLED_BY;")?;
        let mut seen = HashSet::new();
        let mut rows = Vec::new();
        for (symbol, fact) in &resolved {
            if !seen.insert(fact.id.clone()) {
                continue;
            }
            rows.push(vec![
                s(&fact.id),
                s(fact.kind.as_str()),
                s(&fact.method),
                s(&fact.path),
                s(symbol),
                s(&fact.source_file),
                int(fact.source_line),
            ]);
            match fact.kind {
                EndpointKind::Axum => stats.axum += 1,
                EndpointKind::Clap => stats.clap += 1,
                EndpointKind::Mcp => stats.mcp += 1,
                EndpointKind::Fastapi => stats.fastapi += 1,
                EndpointKind::Flask => stats.flask += 1,
                EndpointKind::Express => stats.express += 1,
                EndpointKind::Jaxrs => stats.jaxrs += 1,
            }
        }
        upsert(
            db,
            "Endpoint",
            "id",
            &[
                "id",
                "kind",
                "method",
                "path",
                "handler_symbol",
                "source_file",
                "source_line",
            ],
            rows,
        )?;
        stats.handled_by_edges = insert_pairs(
            db,
            "ENDPOINT_HANDLED_BY",
            ("Endpoint", "id"),
            ("Function", "symbol"),
            resolved
                .iter()
                .filter(|(sym, _)| !sym.is_empty())
                .map(|(sym, f)| (f.id.clone(), sym.clone())),
        )?;
        stats.touches_entity_edges = derive_endpoint_touches_entity(db)?;
        Ok(())
    })
    .context("endpoint ingest")?;
    Ok(stats)
}

/// `Finding(kind='unstubbed-concept')` rows from Doc summaries.
pub fn ingest_unstubbed_concepts(
    db: &SqliteDb,
    root: &Path,
    ontology: &crate::ontology::Ontology,
    config: &LintConfig,
) -> Result<UnstubbedIngestStats> {
    let docs = db
        .query(
            "SELECT id, summary FROM Doc WHERE role NOT LIKE 'ontology-%'",
            &[],
        )?
        .iter()
        .map(|r| (r[0].str_or_empty(), r[1].str_or_empty()))
        .collect();
    let (findings, stats) = plan_unstubbed(docs, root, ontology, config);
    upsert(
        db,
        "Finding",
        "id",
        &["id", "kind", "file", "line", "message", "severity"],
        findings.iter().map(|(id, doc, term)| {
            vec![
                s(id),
                s("unstubbed-concept"),
                s(doc),
                int(0),
                s(term),
                s("info"),
            ]
        }),
    )?;
    Ok(stats)
}

/// `Endpoint -> Entity` via the handler's mentions and belongs-to edges.
pub fn derive_endpoint_touches_entity(db: &SqliteDb) -> Result<usize> {
    db.write_rows(
        "INSERT INTO ENDPOINT_TOUCHES_ENTITY(src, dst) \
         SELECT DISTINCT h.src, x.dst FROM ENDPOINT_HANDLED_BY h \
         JOIN (SELECT src, dst FROM FUNCTION_MENTIONS \
               UNION SELECT src, dst FROM FUNCTION_BELONGS_TO) x ON x.src = h.dst \
         WHERE ?1 = 1",
        [vec![Value::Int(1)]],
    )
}
