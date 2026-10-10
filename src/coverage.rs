//! Phase 0 of roadmap-43: code↔doc coverage measurement.
//!
//! Reads the SQLite graph populated by `cmd_check` (Doc / Entity / Function
//! nodes plus COVERS / FUNCTION_DEFINED_IN / FUNCTION_MENTIONS edges) and
//! produces a global + per-entity + per-crate reachability report.
//!
//! No enforcement happens here — this phase just makes the gap visible.
//! Phase 3 of the roadmap turns the verdict buckets into hard lint errors.
//!
//! Two consumers:
//!
//! 1. `doc-linter query coverage-report` emits the JSON shape directly to
//!    stdout (machine-readable; what AI agents read).
//! 2. The same subcommand with `--write` regenerates
//!    `docs/coverage-report.md` so the human-readable status doc lives in
//!    the vault and lints clean alongside everything else.

use crate::graph_read::GraphRead;
use crate::store::typed;
use crate::store::StoreRead;

type Database = dyn GraphRead;
use anyhow::{Context, Result};
use serde::Serialize;
use std::collections::BTreeMap;

use crate::config::LintConfig;

/// Verdict bucket for a per-entity ratio.
///
/// Boundaries are fixed (not configurable at this phase) — the
/// `coverage_min_per_entity_doc_ratio` knob in `LintConfig` is read but
/// not currently used to shift these labels. Phase 3 of the roadmap will
/// wire the threshold into a `dark-entity` lint that fires when
/// `ratio_value < coverage_min_per_entity_doc_ratio`.
pub fn entity_verdict(ratio_value: f32) -> &'static str {
    if ratio_value < 0.05 {
        "severely-under-documented"
    } else if ratio_value < 0.20 {
        "under-documented"
    } else if ratio_value <= 5.0 {
        "balanced"
    } else {
        "over-documented"
    }
}

/// Verdict bucket for a per-crate reach percentage.
///
/// Threshold resolution: per-crate override from
/// `coverage_min_per_crate[<crate>]` if set, otherwise the global
/// `coverage_min_global`. A threshold of `0.0` (the default) means "no
/// enforcement" — every crate is reported as `tracked` regardless of
/// reach. Otherwise reach below `threshold * 100` percent fires
/// `below-target`.
pub fn crate_verdict(reach_pct: f32, threshold: f32) -> &'static str {
    if threshold <= 0.0 {
        "tracked"
    } else if reach_pct < threshold * 100.0 {
        "below-target"
    } else {
        "on-target"
    }
}

/// Round a percentage to two decimal places. Centralised so the JSON
/// output, the markdown table, and the tests all see the same number
/// (`format!("{:.2}", x)` rounds half-to-even per IEEE 754; we match that).
fn round2(x: f32) -> f32 {
    (x * 100.0).round() / 100.0
}

/// Round a small ratio (for `ratio_value` — e.g. 0.0244) to four decimal
/// places. Two decimals would collapse most severely-under-documented
/// rows to `0.02`/`0.03`/`0.04` and erase ordering.
fn round4(x: f32) -> f32 {
    (x * 10_000.0).round() / 10_000.0
}

#[derive(Debug, Clone, Serialize)]
pub struct GlobalSection {
    pub total_functions: u64,
    pub functions_reaching_entity: u64,
    pub global_pct: f32,
    pub dark_functions: u64,
    pub documented_functions: u64,
    pub documented_pct: f32,
    /// Phase 5c of roadmap-43: count of Function nodes whose bare
    /// name matched a `coverage_function_exempt` regex. These are
    /// subtracted from the dark count — they're not bona-fide gaps,
    /// just irreducibly-generic utilities.
    pub exempt_functions: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct EntitySection {
    pub entity: String,
    pub doc_count: u64,
    /// Number of `Function` nodes with at least one `FUNCTION_MENTIONS`
    /// edge into this entity — i.e. the entity's mention count. The
    /// `func_count` name is kept for backward compatibility with
    /// existing JSON consumers; `god_node` (computed once the section is
    /// sorted) marks the top-3 mention-rich entities, matching
    /// graphify's god-node vocabulary so downstream tools can pivot on
    /// the same field name across both tools.
    pub func_count: u64,
    pub ratio: String,
    pub ratio_value: f32,
    pub verdict: &'static str,
    /// True for the top-3 entities by `func_count` (ties broken by id),
    /// false otherwise. Matches the graphify convention so downstream
    /// consumers can branch on a single boolean.
    pub god_node: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct CrateSection {
    #[serde(rename = "crate")]
    pub crate_name: String,
    pub total_functions: u64,
    pub dark_functions: u64,
    pub reach_pct: f32,
    pub verdict: &'static str,
    /// Phase 5c of roadmap-43: per-crate count of name-exempt
    /// functions. Subtracted from `dark_functions` for this crate.
    pub exempt_functions: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct EndpointClassSection {
    pub kind: crate::endpoint_extract::EndpointKind,
    pub total: u64,
    pub reach_entity: u64,
    pub reach_pct: f32,
}

#[derive(Debug, Clone, Serialize)]
pub struct CoverageReport {
    pub global: GlobalSection,
    pub by_entity: Vec<EntitySection>,
    pub by_crate: Vec<CrateSection>,
    /// Phase 2 of roadmap-43: per-endpoint-class reach. One row per
    /// `kind` ("axum" / "clap" / "mcp"). Always present even when a
    /// kind has zero endpoints — keeps the JSON shape stable.
    pub by_endpoint_class: Vec<EndpointClassSection>,
}

/// Format `(doc_count : func_count)` as a human-readable ratio string
/// like `1:41` (severely under-documented), `5:1` (over-documented),
/// or `1:1` (perfectly balanced). Zero func_count → `1:0` (no code
/// references the entity at all — possible for entities defined only
/// in prose). Zero doc_count → `0:N`.
fn ratio_string(doc_count: u64, func_count: u64) -> String {
    format!("{doc_count}:{func_count}")
}

/// Compute `doc_count / func_count` as a positive ratio_value (docs per
/// code mention). Uses `f32::INFINITY` for a 0-funcs over-documented
/// edge case so the bucketing function still sorts it as "over-documented".
fn ratio_value(doc_count: u64, func_count: u64) -> f32 {
    if func_count == 0 {
        if doc_count == 0 {
            // Both zero → entity has no doc and no code mention. Treat
            // as balanced; the entity exists in the ontology but
            // nothing references it. Phase 1 of the roadmap may surface
            // these separately.
            return 1.0;
        }
        f32::INFINITY
    } else {
        doc_count as f32 / func_count as f32
    }
}

/// Filters supplied by the CLI. Both are optional; both are case-sensitive
/// matches against the underlying graph data.
#[derive(Debug, Clone, Default)]
pub struct ReportFilters {
    pub crate_filter: Option<String>,
    pub entity_filter: Option<String>,
}

/// Build the report by issuing four read-only SQL queries against the
/// already-populated graph. The DB is opened by the caller; we just borrow
/// a connection.
pub fn build_report(
    db: &Database,
    config: &LintConfig,
    filters: &ReportFilters,
) -> Result<CoverageReport> {
    let conn = db;

    // ---------- Global section ----------
    // Compiler-generated members (`Function.generated`) are call targets,
    // not code anyone wrote or documents; every measure here skips them.
    let total_functions = typed::count_functions(conn)?;
    let functions_reaching_entity = typed::count_functions_reaching_entity(conn)?;
    let documented_functions = typed::count_functions_documented(conn)?;

    // Phase 5c of roadmap-43: walk every dark Function node (no
    // doc-comment AND no FUNCTION_MENTIONS edge), apply the
    // `coverage_function_exempt` regexes against its bare name, and
    // accumulate the exempt count. Per-crate breakdown is captured in
    // `exempt_per_crate` for the by_crate section below.
    let (exempt_total, exempt_per_crate) = exempt_dark_breakdown(conn, config)?;

    let raw_dark = total_functions.saturating_sub(functions_reaching_entity);
    let dark_functions = raw_dark.saturating_sub(exempt_total);
    // Roadmap-51 Move 2: headline metric is *non-exempt* reach. Exempt
    // functions don't have a domain entity by design, so they don't
    // belong in the denominator — including them makes 100% impossible
    // and forces an arbitrary floor like 90%. The new metric makes
    // 100% the honest, achievable target.
    let nonexempt_denom = total_functions.saturating_sub(exempt_total);
    let global_pct = if nonexempt_denom == 0 {
        0.0
    } else {
        round2(functions_reaching_entity as f32 * 100.0 / nonexempt_denom as f32)
    };
    let documented_pct = if total_functions == 0 {
        0.0
    } else {
        round2(documented_functions as f32 * 100.0 / total_functions as f32)
    };

    let global = GlobalSection {
        total_functions,
        functions_reaching_entity,
        global_pct,
        dark_functions,
        documented_functions,
        documented_pct,
        exempt_functions: exempt_total,
    };

    // ---------- Per-entity section ----------
    // Each entity gets:
    //   - doc_count = number of Doc nodes with COVERS edge to this entity
    //   - func_count = number of Function nodes with FUNCTION_MENTIONS edge
    // We query each independently and merge in Rust — a left join
    // with two distinct counts in one query is brittle across versions.
    let entity_doc_counts = typed::entity_doc_counts(conn)?;
    let entity_func_counts = typed::entity_func_counts(conn)?;

    // Union of entity ids from both maps so an entity with zero coverage
    // on either side still appears in the report.
    let mut all_entities: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    all_entities.extend(entity_doc_counts.keys().cloned());
    all_entities.extend(entity_func_counts.keys().cloned());

    let mut by_entity: Vec<EntitySection> = Vec::new();
    for ent in all_entities {
        if let Some(want) = &filters.entity_filter {
            if &ent != want {
                continue;
            }
        }
        let doc_count = entity_doc_counts.get(&ent).copied().unwrap_or(0);
        let func_count = entity_func_counts.get(&ent).copied().unwrap_or(0);
        let rv = round4(ratio_value(doc_count, func_count));
        by_entity.push(EntitySection {
            entity: ent,
            doc_count,
            func_count,
            ratio: ratio_string(doc_count, func_count),
            ratio_value: rv,
            verdict: entity_verdict(rv),
            god_node: false,
        });
    }
    // Mark the top-3 by func_count as god_node before the under-documentation
    // sort below — the flag is a property of mention richness, not of
    // documentation balance, so it has to be assigned before we re-sort.
    {
        const GOD_NODE_TOP_K: usize = 3;
        let mut by_mentions: Vec<usize> = (0..by_entity.len()).collect();
        by_mentions.sort_by(|&a, &b| {
            by_entity[b]
                .func_count
                .cmp(&by_entity[a].func_count)
                .then(by_entity[a].entity.cmp(&by_entity[b].entity))
        });
        for &idx in by_mentions.iter().take(GOD_NODE_TOP_K) {
            if by_entity[idx].func_count > 0 {
                by_entity[idx].god_node = true;
            }
        }
    }
    // Worst first (lowest ratio = most under-documented). Stable secondary
    // sort by entity id so two entities with identical ratios produce
    // deterministic JSON.
    by_entity.sort_by(|a, b| {
        a.ratio_value
            .partial_cmp(&b.ratio_value)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.entity.cmp(&b.entity))
    });

    // ---------- Per-crate section ----------
    let crate_totals = typed::crate_function_totals(conn)?;
    let crate_reach = typed::crate_function_reach(conn)?;

    let mut all_crates: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    all_crates.extend(crate_totals.keys().cloned());
    all_crates.extend(crate_reach.keys().cloned());

    let mut by_crate: Vec<CrateSection> = Vec::new();
    for c in all_crates {
        if c.is_empty() {
            // SCIP can produce Functions with empty crate (e.g. external
            // dependencies it didn't classify). Skip — they're not a
            // crate we can hold to a target.
            continue;
        }
        if let Some(want) = &filters.crate_filter {
            if &c != want {
                continue;
            }
        }
        let total = crate_totals.get(&c).copied().unwrap_or(0);
        let reach = crate_reach.get(&c).copied().unwrap_or(0);
        let raw_dark_c = total.saturating_sub(reach);
        let exempt_c = exempt_per_crate.get(&c).copied().unwrap_or(0);
        let dark = raw_dark_c.saturating_sub(exempt_c);
        let reach_pct = if total == 0 {
            0.0
        } else {
            round2(reach as f32 * 100.0 / total as f32)
        };
        let threshold = config
            .coverage
            .coverage_min_per_crate
            .get(&c)
            .copied()
            .unwrap_or(config.coverage.coverage_min_global);
        let verdict = crate_verdict(reach_pct, threshold);
        by_crate.push(CrateSection {
            crate_name: c,
            total_functions: total,
            dark_functions: dark,
            reach_pct,
            verdict,
            exempt_functions: exempt_c,
        });
    }
    // Worst first (lowest reach_pct), then by name for stable tie-breaks.
    by_crate.sort_by(|a, b| {
        a.reach_pct
            .partial_cmp(&b.reach_pct)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.crate_name.cmp(&b.crate_name))
    });

    // ---------- Per-endpoint-class section (Phase 2 of roadmap-43) ----------
    let endpoint_kind_rows = db.endpoint_reach_by_kind()?;
    let by_endpoint_class: Vec<EndpointClassSection> = endpoint_kind_rows
        .into_iter()
        .map(|(kind, total, reach)| {
            let reach_pct = if total == 0 {
                0.0
            } else {
                round2(reach as f32 * 100.0 / total as f32)
            };
            EndpointClassSection {
                kind,
                total,
                reach_entity: reach,
                reach_pct,
            }
        })
        .collect();

    Ok(CoverageReport {
        global,
        by_entity,
        by_crate,
        by_endpoint_class,
    })
}

/// Phase 5c of roadmap-43: walk every dark Function node (empty
/// `doc_comment` AND no `FUNCTION_MENTIONS` edge), apply each
/// configured `coverage_function_exempt` regex against the SCIP
/// symbol's bare function name (the trailing `name()` after the last
/// `/`), and tally per-crate. Returns `(global_exempt_total,
/// per_crate_exempt_counts)`. When the config has no exempt patterns
/// (i.e. the user explicitly disabled the feature), this returns
/// `(0, empty)` after a single `MATCH (f:Function)` count short-circuit
/// — no SQL walks the whole table for nothing.
pub fn exempt_dark_breakdown(
    conn: &(impl StoreRead + ?Sized),
    config: &LintConfig,
) -> Result<(u64, BTreeMap<String, u64>)> {
    let has_patterns = config
        .coverage
        .coverage_function_exempt_set
        .as_ref()
        .is_some_and(|v| !v.is_empty());
    if !has_patterns {
        return Ok((0, BTreeMap::new()));
    }
    let mut total: u64 = 0;
    let mut per_crate: BTreeMap<String, u64> = BTreeMap::new();
    // Roadmap-51 Move 4: walk every dark Function (no FUNCTION_MENTIONS
    // edge), regardless of `doc_comment` content. Rust-analyzer pulls
    // stdlib trait-method docs into the SCIP `documentation` field for
    // derive / trait impls (e.g. `std::io::Write::write` lands a long
    // doc on every `impl Write for FooWriter` member), so a
    // bare-name-exempt fn can have a non-empty `doc_comment` that
    // doesn't link any entity. The previous filter
    // `(f.doc_comment IS NULL OR f.doc_comment = '')` excluded those
    // from the exempt sweep and over-counted them as dark. The
    // `NOT EXISTS FUNCTION_MENTIONS` predicate already establishes
    // darkness; the doc-comment filter was redundant for the exempt-
    // by-bare-name fallback.
    for (crate_name, symbol) in typed::dark_function_symbols(conn)? {
        let bare = match crate::scaffold::bare_fn_name(&symbol) {
            Some(b) => b,
            None => continue,
        };
        if config.is_function_exempt(&bare) {
            total += 1;
            *per_crate.entry(crate_name).or_insert(0) += 1;
        }
    }
    Ok((total, per_crate))
}

/// Render the report as the `docs/coverage-report.md` body. Frontmatter
/// is added by `write_markdown_report` (kept separate so the body
/// content is identical between idempotent re-renders even when the
/// `updated:` date changes).
pub fn render_markdown_body(report: &CoverageReport) -> String {
    let mut out = String::new();
    out.push_str("# Code-Doc Coverage Report\n\n");
    out.push_str(
        "Auto-generated by `doc-linter query coverage-report --write`. \
         The numbers reflect the live graph at the time of the last \
         full `doc-linter check` — re-run that first if the data looks \
         stale. Phase 0 of `roadmap-43` makes the code-to-doc gap \
         measurable; Phases 1-4 close it.\n\n",
    );
    out.push_str(
        "Three sections: a global summary, a per-entity table sorted \
         worst-first by `doc:func` ratio, and a per-crate table sorted \
         worst-first by reach percentage. The verdict bucket for each \
         row uses fixed thresholds (entities) or the configured \
         `coverage_min_global` / `coverage_min_per_crate` knobs (crates).\n\n",
    );

    // ----- Global -----
    out.push_str("## Global\n\n");
    out.push_str("| Metric | Value |\n");
    out.push_str("|---|---|\n");
    out.push_str(&format!(
        "| Total functions | {} |\n",
        report.global.total_functions
    ));
    out.push_str(&format!(
        "| Functions reaching entity | {} |\n",
        report.global.functions_reaching_entity
    ));
    out.push_str(&format!(
        "| Non-exempt reach | {:.2}% |\n",
        report.global.global_pct
    ));
    out.push_str(&format!(
        "| Dark functions | {} |\n",
        report.global.dark_functions
    ));
    out.push_str(&format!(
        "| Exempt functions | {} |\n",
        report.global.exempt_functions
    ));
    out.push_str(&format!(
        "| Functions with doc comment | {} |\n",
        report.global.documented_functions
    ));
    out.push_str(&format!(
        "| Doc-comment coverage | {:.2}% |\n",
        report.global.documented_pct
    ));
    out.push('\n');

    // ----- Per-entity -----
    out.push_str("## By entity\n\n");
    if report.by_entity.is_empty() {
        out.push_str("No entities in the ontology yet.\n\n");
    } else {
        out.push_str("| Entity | Doc count | Func count | Ratio | Verdict |\n");
        out.push_str("|---|---|---|---|---|\n");
        for e in &report.by_entity {
            out.push_str(&format!(
                "| `{}` | {} | {} | {} | {} |\n",
                e.entity, e.doc_count, e.func_count, e.ratio, e.verdict
            ));
        }
        out.push('\n');
    }

    // ----- Per-crate -----
    out.push_str("## By crate\n\n");
    if report.by_crate.is_empty() {
        out.push_str("No Rust functions ingested yet — run `doc-linter scip-index` then `doc-linter check`.\n\n");
    } else {
        out.push_str("| Crate | Total functions | Dark functions | Exempt | Reach | Verdict |\n");
        out.push_str("|---|---|---|---|---|---|\n");
        for c in &report.by_crate {
            out.push_str(&format!(
                "| `{}` | {} | {} | {} | {:.2}% | {} |\n",
                c.crate_name,
                c.total_functions,
                c.dark_functions,
                c.exempt_functions,
                c.reach_pct,
                c.verdict
            ));
        }
        out.push('\n');
    }

    // ----- Per-endpoint-class (Phase 2 of roadmap-43) -----
    out.push_str("## By endpoint class\n\n");
    if report.by_endpoint_class.is_empty() {
        out.push_str("No endpoints extracted yet — `doc-linter check` populates the `Endpoint` table after SCIP ingest.\n\n");
    } else {
        out.push_str("| Kind | Total | Reach entity | Reach |\n");
        out.push_str("|---|---|---|---|\n");
        for ec in &report.by_endpoint_class {
            out.push_str(&format!(
                "| `{}` | {} | {} | {:.2}% |\n",
                ec.kind, ec.total, ec.reach_entity, ec.reach_pct
            ));
        }
        out.push('\n');
    }

    out
}

/// Compose the full markdown file (frontmatter + body). The `updated:`
/// frontmatter field is the only line that legitimately churns from
/// run to run on identical data; `write_markdown_report` only rewrites
/// when the body itself differs.
pub fn render_full_markdown(report: &CoverageReport, today: &str) -> String {
    let body = render_markdown_body(report);
    format!(
        "---\n\
         id: coverage-report\n\
         role: status-report\n\
         covers: [doc-graph, coverage]\n\
         title: \"Code-Doc Coverage Report\"\n\
         summary: \"Auto-generated reachability report from the graph: how many Rust functions reach the ontology, per-entity doc:code ratios, per-crate dark-function counts. Regenerated by `doc-linter query coverage-report --write`.\"\n\
         status: stable\n\
         updated: {today}\n\
         tags: [docs, lint, status]\n\
         ---\n\n\
         {body}"
    )
}

/// Strip the `updated: …` line from a markdown blob so two renders of
/// the same data compare equal even if today's date moved between
/// runs. The function is intentionally line-precise — only the
/// `updated:` line inside the frontmatter is removed.
fn strip_updated_line(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for line in s.lines() {
        if line.starts_with("updated:") {
            continue;
        }
        out.push_str(line);
        out.push('\n');
    }
    out
}

/// Idempotent write. Compares the existing file's body (everything
/// minus the `updated:` line) to the new body; only writes when they
/// differ. Returns `true` if the file was rewritten.
pub fn write_markdown_report(
    target: &std::path::Path,
    report: &CoverageReport,
    today: &str,
) -> Result<bool> {
    let new_full = render_full_markdown(report, today);
    if let Ok(existing) = std::fs::read_to_string(target) {
        if strip_updated_line(&existing) == strip_updated_line(&new_full) {
            return Ok(false);
        }
    }
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
    }
    std::fs::write(target, new_full).with_context(|| format!("write {}", target.display()))?;
    Ok(true)
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "tests panic on broken invariants — that is the point of a test"
)]
mod tests {
    use super::*;

    #[test]
    fn entity_verdict_buckets_match_brief() {
        // Severely under: ratio_value < 0.05.
        assert_eq!(entity_verdict(0.0), "severely-under-documented");
        assert_eq!(entity_verdict(0.0244), "severely-under-documented");
        assert_eq!(entity_verdict(0.0499), "severely-under-documented");
        // Under: 0.05 <= rv < 0.20.
        assert_eq!(entity_verdict(0.05), "under-documented");
        assert_eq!(entity_verdict(0.111), "under-documented");
        assert_eq!(entity_verdict(0.1999), "under-documented");
        // Balanced: 0.20 <= rv <= 5.0.
        assert_eq!(entity_verdict(0.20), "balanced");
        assert_eq!(entity_verdict(1.0), "balanced");
        assert_eq!(entity_verdict(5.0), "balanced");
        // Over: rv > 5.0.
        assert_eq!(entity_verdict(5.0001), "over-documented");
        assert_eq!(entity_verdict(100.0), "over-documented");
    }

    #[test]
    fn crate_verdict_respects_threshold() {
        // Threshold 0.0 = no enforcement → every crate is "tracked".
        assert_eq!(crate_verdict(0.0, 0.0), "tracked");
        assert_eq!(crate_verdict(50.0, 0.0), "tracked");
        assert_eq!(crate_verdict(100.0, 0.0), "tracked");

        // Threshold 0.95: reach < 95% is below-target.
        assert_eq!(crate_verdict(94.99, 0.95), "below-target");
        assert_eq!(crate_verdict(95.0, 0.95), "on-target");
        assert_eq!(crate_verdict(99.99, 0.95), "on-target");

        // Threshold 0.20: reach < 20% is below-target.
        assert_eq!(crate_verdict(12.10, 0.20), "below-target");
        assert_eq!(crate_verdict(20.0, 0.20), "on-target");
    }

    #[test]
    fn ratio_string_renders_canonical_form() {
        assert_eq!(ratio_string(1, 41), "1:41");
        assert_eq!(ratio_string(13, 119), "13:119");
        assert_eq!(ratio_string(0, 5), "0:5");
        assert_eq!(ratio_string(5, 0), "5:0");
    }

    #[test]
    fn ratio_value_handles_zero_func_count() {
        assert!(ratio_value(0, 0).is_finite());
        assert!(ratio_value(5, 0).is_infinite());
        assert!((ratio_value(1, 41) - (1.0 / 41.0)).abs() < 1e-6);
    }

    #[test]
    fn strip_updated_drops_only_updated_line() {
        let input = "---\n\
                     id: x\n\
                     updated: 2026-05-03\n\
                     tags: [a]\n\
                     ---\n\
                     body line\n";
        let stripped = strip_updated_line(input);
        assert!(!stripped.contains("updated:"));
        assert!(stripped.contains("id: x"));
        assert!(stripped.contains("body line"));
    }

    #[test]
    fn render_markdown_body_lists_worst_entity_first() {
        let report = CoverageReport {
            global: GlobalSection {
                total_functions: 100,
                functions_reaching_entity: 22,
                global_pct: 22.0,
                dark_functions: 78,
                documented_functions: 43,
                documented_pct: 43.0,
                exempt_functions: 0,
            },
            by_entity: vec![
                EntitySection {
                    entity: "beat".to_string(),
                    doc_count: 1,
                    func_count: 41,
                    ratio: "1:41".to_string(),
                    ratio_value: 0.0244,
                    verdict: "severely-under-documented",
                    god_node: false,
                },
                EntitySection {
                    entity: "outlet".to_string(),
                    doc_count: 13,
                    func_count: 119,
                    ratio: "13:119".to_string(),
                    ratio_value: 0.1092,
                    verdict: "under-documented",
                    god_node: false,
                },
            ],
            by_crate: vec![CrateSection {
                crate_name: "doc-linter".to_string(),
                total_functions: 314,
                dark_functions: 276,
                reach_pct: 12.10,
                verdict: "below-target",
                exempt_functions: 0,
            }],
            by_endpoint_class: vec![
                EndpointClassSection {
                    kind: crate::endpoint_extract::EndpointKind::Axum,
                    total: 89,
                    reach_entity: 18,
                    reach_pct: 20.22,
                },
                EndpointClassSection {
                    kind: crate::endpoint_extract::EndpointKind::Clap,
                    total: 7,
                    reach_entity: 7,
                    reach_pct: 100.0,
                },
                EndpointClassSection {
                    kind: crate::endpoint_extract::EndpointKind::Mcp,
                    total: 0,
                    reach_entity: 0,
                    reach_pct: 0.0,
                },
            ],
        };
        let body = render_markdown_body(&report);
        assert!(body.contains("## Global"));
        assert!(body.contains("## By entity"));
        assert!(body.contains("## By crate"));
        assert!(body.contains("## By endpoint class"));
        assert!(body.contains("`axum`"));
        assert!(body.contains("`clap`"));
        // Worst entity first → `beat` before `outlet`.
        let beat_pos = body.find("`beat`").expect("beat in body");
        let outlet_pos = body.find("`outlet`").expect("outlet in body");
        assert!(beat_pos < outlet_pos);
    }
}
