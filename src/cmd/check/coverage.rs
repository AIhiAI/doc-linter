//! Phase 3 of roadmap-43: post-ingest coverage lint. Reads the
//! freshly-ingested SQLite graph and emits any of the three new coverage
//! diagnostics that fire — plus the optional `coverage-below-min`
//! repo-level diagnostic when the global reach falls below the
//! configured / CLI-supplied threshold.

use anyhow::Result;

use doc_linter::config::LintConfig;
use doc_linter::ontology::Ontology;
use doc_linter::validator::Issue;

/// Returns the diagnostic list ordered by code so two runs over the
/// same corpus produce identical output. The caller folds this into
/// the per-file report under the repo-root path.
pub(crate) fn run_coverage_lint(
    db: &dyn doc_linter::graph_read::GraphRead,
    config: &LintConfig,
    cli_coverage_min_global: Option<f32>,
    ontology: &Ontology,
) -> Result<Vec<Issue>> {
    let mut out: Vec<Issue> = Vec::new();

    // ---- 3a: dark-public-function ---------------------------------
    let dark_funcs = db.list_dark_public_functions(&config.coverage.anchor_required_in, config)?;
    for row in dark_funcs {
        out.push(Issue::DarkPublicFunction {
            crate_name: row.crate_name,
            file: row.file,
            line: row.line,
            symbol: row.symbol,
        });
    }

    // ---- 3b: dark-endpoint ----------------------------------------
    // Roadmap-49 phase 3: defense-in-depth filter on
    // `coverage_endpoint_exempt`. The ingest path also drops these
    // before the Endpoint node is created, so this filter is
    // belt-and-braces — covers ad-hoc Database fixtures that ingest
    // bypassing the main pipeline.
    let dark_endpoints = db.list_dark_endpoints()?;
    let endpoint_exempt: std::collections::HashSet<&str> = config
        .coverage
        .coverage_endpoint_exempt
        .iter()
        .map(std::string::String::as_str)
        .collect();
    for row in dark_endpoints {
        let id = format!("{}:{}:{}", row.kind, row.method, row.path);
        if endpoint_exempt.contains(id.as_str()) {
            continue;
        }
        out.push(Issue::DarkEndpoint {
            kind: row.kind,
            method: row.method,
            path: row.path,
            file: row.file,
            line: row.line,
        });
    }

    // ---- 3c: entity-coverage-gap ----------------------------------
    let threshold = config.coverage.coverage_min_per_entity_doc_ratio;
    if threshold > 0.0 {
        // Issue #180: `auto` entities are cluster-derived stubs that
        // populate FUNCTION_MENTIONS for graph richness but must NOT
        // trigger coverage-gap diagnostics — a human review owns the
        // promotion to `stable` (which earns enforcement). Build the
        // exempt set as configured exemptions ∪ auto-status entity
        // ids.
        let mut exempt: Vec<String> = config.coverage.coverage_entity_exempt.clone();
        for entity in ontology.entities.values() {
            if entity.status == "auto" {
                exempt.push(entity.id.clone());
            }
        }
        let gaps = db.list_entity_coverage_gaps(threshold, &exempt)?;
        for row in gaps {
            out.push(Issue::EntityCoverageGap {
                entity: row.entity,
                doc_count: row.doc_count,
                func_count: row.func_count,
                ratio: row.ratio,
                threshold,
            });
        }
    }

    // ---- coverage-below-min (single repo-level diagnostic) --------
    // CLI flag overrides config. The CLI value is a percentage (`70`
    // means "fail when reach < 70%"), the config field is also a
    // percentage by Phase-0 convention.
    //
    // Roadmap-51 Move 2: the metric is now *non-exempt reach* —
    // `reach / (total - exempt)` instead of `reach / total`. Exempt
    // functions (parser internals, derive-impls, FFI shims —
    // matched by `coverage_function_exempt`) don't have a domain
    // entity by design, so they shouldn't dilute the percentage.
    // 100% is the honest, achievable target: every function that
    // *should* reach an entity does.
    let threshold_pct = cli_coverage_min_global.unwrap_or(config.coverage.coverage_min_global);
    if threshold_pct > 0.0 {
        let (total, reach, exempt) = db.global_function_reach(config)?;
        let denom = total.saturating_sub(exempt);
        let actual_pct = if denom == 0 {
            0.0
        } else {
            reach as f32 * 100.0 / denom as f32
        };
        if actual_pct < threshold_pct {
            out.push(Issue::CoverageBelowMinimum {
                actual_pct,
                threshold_pct,
            });
        }
    }

    Ok(out)
}
