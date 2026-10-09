//! `doc-linter report`: the zero-config first run (plan item C2). One
//! command, no `init` and no ontology needed: it refreshes the graph, then
//! prints the coverage table, the stale docs (starter-ontology docs
//! excluded) and the concept work list, with candidate concepts mined from
//! the code when the ontology does not name them yet.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use anyhow::{Context, Result};
use serde_json::{json, Value};

use doc_linter::config::LintConfig;
use doc_linter::coverage::{self, ReportFilters};
use doc_linter::graph_read::{GraphRead, ReadGraph};

use super::concepts::{self, ProposeArgs};
use super::{ontology, OutputFormat};

fn rows(v: &Value) -> Vec<Value> {
    v.get("rows")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

fn s<'a>(row: &'a Value, key: &str) -> &'a str {
    row.get(key).and_then(Value::as_str).unwrap_or("")
}

fn n(row: &Value, key: &str) -> i64 {
    row.get(key).and_then(Value::as_i64).unwrap_or(0)
}

pub(crate) fn run(
    root: &Path,
    config: &LintConfig,
    all_files: &[PathBuf],
    json_out: bool,
    no_refresh: bool,
) -> Result<ExitCode> {
    if !no_refresh {
        // Lint findings are not this command's business: ingest silently.
        super::check::run(
            root,
            config,
            all_files,
            None,
            OutputFormat::Human,
            true,
            false,
            None,
            false,
            false,
            false,
            true,
        )
        .context("refresh the graph (use --no-refresh to read the existing one)")?;
    }
    let db = ReadGraph::open(root).context("no graph to read: run `doc-linter check` first")?;
    let g: &dyn GraphRead = &*db;

    let cov = coverage::build_report(g, config, &ReportFilters::default())?;
    let saved = |name: &str| -> Result<Vec<Value>> {
        Ok(rows(&g.run_saved_query(
            Some(root),
            name,
            &HashMap::new(),
        )?))
    };
    let stale = saved("stale-narrative-docs")?;
    let work = saved("concept-work-list")?;
    let hist = doc_linter::store_sqlite::history::read(g, 10)?;
    let ont = ontology::load(all_files);
    let candidates = concepts::collect_proposals(
        g,
        config,
        &ont,
        &ProposeArgs {
            top_n: 5,
            min_members: 3,
            algorithm: "leiden".to_string(),
            embeddings: false,
            json: false,
            out: PathBuf::new(),
        },
    )
    .unwrap_or_default();

    if json_out {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "coverage": cov,
                "history": hist,
                "stale_docs": stale,
                "concept_work_list": work,
                "candidate_concepts": candidates,
            }))?
        );
        return Ok(ExitCode::SUCCESS);
    }

    let gl = &cov.global;
    println!("COVERAGE");
    if gl.total_functions == 0 {
        println!(
            "  No code index yet (0 functions). Run `doc-linter scip-index`, then `doc-linter report` again."
        );
    } else {
        println!(
            "  {} functions: {} reach an entity ({:.1}%), {} have a doc comment ({:.1}%)",
            gl.total_functions,
            gl.functions_reaching_entity,
            gl.global_pct,
            gl.documented_functions,
            gl.documented_pct
        );
    }
    if let Some(t) = doc_linter::store_sqlite::history::trend_line(&hist) {
        println!("  TREND {t}");
    }
    let mut ents: Vec<_> = cov.by_entity.iter().filter(|e| e.func_count > 0).collect();
    ents.sort_by(|a, b| {
        b.func_count
            .cmp(&a.func_count)
            .then_with(|| a.entity.cmp(&b.entity))
    });
    if ents.is_empty() {
        println!("  No entity is mentioned by code yet (no ontology is needed to start; see the work list).");
    } else {
        println!(
            "  {:<24} {:>5} {:>10}  VERDICT",
            "ENTITY", "DOCS", "FUNCTIONS"
        );
        for e in ents.iter().take(10) {
            println!(
                "  {:<24} {:>5} {:>10}  {}",
                e.entity, e.doc_count, e.func_count, e.verdict
            );
        }
    }

    println!("\nSTALE DOCS (stalest first; starter ontology docs excluded)");
    if stale.is_empty() {
        println!("  none");
    }
    for r in stale.iter().take(10) {
        println!(
            "  {:<10} {:<12} {:<28} {}",
            s(r, "updated"),
            s(r, "kind"),
            s(r, "id"),
            s(r, "title")
        );
    }

    println!("\nCONCEPT WORK LIST");
    if !work.is_empty() {
        println!("  Entities with code, fewest chapters first:");
        println!("  {:<24} {:>10} {:>9}", "ENTITY", "FUNCTIONS", "CHAPTERS");
        for r in work.iter().take(10) {
            println!(
                "  {:<24} {:>10} {:>9}",
                s(r, "entity"),
                n(r, "functions"),
                n(r, "chapters")
            );
        }
    }
    if candidates.is_empty() {
        if work.is_empty() {
            println!("  Nothing to list yet: no entity is mentioned by code and no candidate concept was found.");
        }
    } else {
        println!("  Candidate concepts found in the code (not in the ontology yet):");
        println!("  {:<24} {:>7}  SAMPLE DOC COMMENT", "NAME", "SYMBOLS");
        for c in &candidates {
            println!(
                "  {:<24} {:>7}  {}",
                c.name, c.member_count, c.sample_doc_comment
            );
        }
        println!(
            "  Next: `doc-linter ontology propose`, then `doc-linter ontology accept <name>`."
        );
    }
    Ok(ExitCode::SUCCESS)
}
