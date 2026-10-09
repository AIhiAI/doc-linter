//! Gap-005 deferred slice: cross-doc contradiction detection.
//! Walks pairs of `tags=[design]` Docs, sends each pair through
//! the LLM Judge's `compare_pair`, and emits one
//! `Finding(kind="contradiction")` row when the verdict says
//! inconsistent AND confidence ≥ the configured threshold.
//!
//! Opt-in via `--contradictions` CLI flag. NoOp (with a stderr
//! warning) when the Judge backend is `NoOpJudge` — no real LLM
//! is available.
//!
//! The pass runs after the main ingest so it can query the
//! freshly-populated `Doc` table directly.

use anyhow::Result;
use doc_linter::graph_read::GraphRead;
use doc_linter::store::Store;

use doc_linter::llm::{default_judge, Claim};

/// Confidence floor below which the Judge's "inconsistent" verdict
/// is treated as too weak to surface. Conservative default — the
/// agent reading the resulting findings can re-rank with looser
/// thresholds via direct sql if it wants the long tail.
const CONFIDENCE_FLOOR: f32 = 0.6;

#[derive(Debug, Default, Clone, Copy)]
pub(super) struct ContradictionStats {
    pub(super) pairs_evaluated: usize,
    pub(super) findings: usize,
}

/// Read every pair of Doc rows where both carry `tags` including
/// "design", invoke `Judge::compare_pair`, and emit Findings for
/// the contradictory pairs.
///
/// Returns immediately (with a stderr note) when the active Judge
/// is the NoOp placeholder — running pairwise without an LLM
/// produces only neutral verdicts and burns no value.
pub(super) fn run_contradiction_check(
    db: &(impl GraphRead + Store + ?Sized),
) -> Result<ContradictionStats> {
    let mut stats = ContradictionStats::default();
    let judge = default_judge();
    if !judge.is_available() {
        eprintln!(
            "doc-linter: contradictions check skipped — \
             no LLM Judge available (build with --features llm \
             and set ANTHROPIC_API_KEY to enable)"
        );
        return Ok(stats);
    }

    let design_docs = doc_linter::store::typed::design_doc_summaries(db)?;

    // O(N^2 / 2) pairwise LLM calls — only viable for small design
    // corpora. The audit-run-009 plan flagged this as an
    // opt-in / coarse-prefilter follow-up if it gets expensive.
    for i in 0..design_docs.len() {
        for j in (i + 1)..design_docs.len() {
            stats.pairs_evaluated += 1;
            let claim_a = Claim {
                text: design_docs[i].1.clone(),
                source_id: Some(design_docs[i].0.clone()),
                source_line: None,
            };
            let claim_b = Claim {
                text: design_docs[j].1.clone(),
                source_id: Some(design_docs[j].0.clone()),
                source_line: None,
            };
            let verdict = match judge.compare_pair(&claim_a, &claim_b) {
                Ok(v) => v,
                Err(e) => {
                    eprintln!(
                        "doc-linter: contradiction check error on \
                         {} vs {}: {e}",
                        design_docs[i].0, design_docs[j].0
                    );
                    continue;
                }
            };
            if !verdict.inconsistent || verdict.confidence < CONFIDENCE_FLOOR {
                continue;
            }
            let id_a = &design_docs[i].0;
            let id_b = &design_docs[j].0;
            let finding_id = format!("contradiction-{id_a}-{id_b}");
            let message = format!(
                "{} vs {} (confidence {:.2}): {}",
                id_a, id_b, verdict.confidence, verdict.rationale
            );
            if doc_linter::store::typed::insert_contradiction_finding(
                db,
                &finding_id,
                id_a,
                &message,
            )
            .is_ok()
            {
                stats.findings += 1;
            }
        }
    }

    Ok(stats)
}
