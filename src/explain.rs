//! Phase 4 of roadmap-43: `doc-linter explain <SYMBOL>` over the
//! [[entity-doc-graph]] code half.
//!
//! Given a SCIP function symbol (or a substring that uniquely
//! identifies one), walk the graph and emit a structured non-tech
//! explanation:
//!
//!   - the function node + its doc-comment
//!   - every Entity it FUNCTION_MENTIONS, with display + summary
//!     (sourced from the Doc node `entity-<id>` if present, or the
//!     Entity node's description otherwise)
//!   - every narrative Doc that COVERS those entities
//!   - the top-N sibling functions in the same crate, ranked by
//!     entity-mention overlap
//!
//! Two output modes (`--json` / human-readable). Two disambiguation
//! modes: when the substring resolves to multiple symbols, default
//! lists candidates with a hint; `--list` skips the explain and just
//! lists.

use anyhow::{Context, Result};
use serde::Serialize;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use crate::config::LintConfig;
use crate::graph_read::{GraphRead, ReadGraph};
use crate::ids::{EntityId, FunctionSymbol};
use crate::store::{ConfidenceTier, CoveringDocRow, FunctionFull, FunctionMentionRow, SiblingRow};

/// One Entity row in the explain output. `summary` is the ontology
/// `description` field (from the entity Doc's frontmatter if present
/// — that's already merged into the Entity node at ingest time).
#[derive(Debug, Clone, Serialize)]
pub struct ExplainEntity {
    pub id: String,
    pub display: String,
    pub summary: String,
    pub confidence: ConfidenceTier,
}

/// One narrative-doc row in the [[entity-doc-graph]] explain output —
/// a Doc node whose `covers:` includes one of the function's entities.
#[derive(Debug, Clone, Serialize)]
pub struct ExplainDoc {
    pub id: String,
    pub title: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
}

/// One sibling-function row in the [[entity-doc-graph]] explain output —
/// another Function in the same crate ranked by entity-mention overlap.
#[derive(Debug, Clone, Serialize)]
pub struct ExplainSibling {
    pub symbol: String,
    pub shared_entities: Vec<String>,
}

/// Full structured response for one [[entity-doc-graph]] explain walk.
/// Mirrors the JSON shape documented in the Phase 4 brief — function +
/// entity neighbours + covering docs + ranked siblings.
#[derive(Debug, Clone, Serialize)]
pub struct Explanation {
    pub symbol: String,
    #[serde(rename = "crate")]
    pub crate_name: String,
    pub file: String,
    pub line: u32,
    pub doc_comment: String,
    pub entities: Vec<ExplainEntity>,
    pub narrative_docs: Vec<ExplainDoc>,
    pub siblings: Vec<ExplainSibling>,
}

/// Build the structured explanation for a single resolved Function symbol
/// in the [[entity-doc-graph]] code-graph. Walks `FUNCTION_MENTIONS`,
/// covering narrative docs, and entity-overlap-ranked siblings. Pure data —
/// the renderers below drive the side effects.
pub fn build_explanation(db: &dyn GraphRead, function: FunctionFull) -> Result<Explanation> {
    // Fetch every entity this function mentions, with confidence.
    let symbol = FunctionSymbol::from(function.symbol.as_str());
    let mention_rows: Vec<FunctionMentionRow> = db.function_mentions(&symbol)?;

    // For each entity, prefer the entity Doc node's description (the
    // authoritative summary). Falls back to the Entity node's own
    // description.
    let entities: Vec<ExplainEntity> = mention_rows
        .into_iter()
        .map(|m| {
            let entity_id = EntityId::from(m.entity_id.as_str());
            let summary = db
                .get_entity(&entity_id)
                .ok()
                .flatten()
                .map(|e| e.description)
                .unwrap_or_default();
            ExplainEntity {
                id: m.entity_id,
                display: m.display,
                summary,
                confidence: m.confidence,
            }
        })
        .collect();

    // Narrative docs covering any of those entities.
    let entity_ids: Vec<EntityId> = entities
        .iter()
        .map(|e| EntityId::from(e.id.as_str()))
        .collect();
    let cov_rows: Vec<CoveringDocRow> = db.docs_covering_entities(&entity_ids)?;
    // Skip the synthetic `entity-<id>` Docs — those are the ontology
    // entries themselves. Authors want how-to / explanation / reference
    // narrative docs, not a self-loop back to the entity definition.
    let narrative_docs: Vec<ExplainDoc> = cov_rows
        .into_iter()
        .filter(|d| !d.id.starts_with("entity-"))
        .map(|d| ExplainDoc {
            id: d.id,
            title: d.title,
            kind: d.kind,
        })
        .collect();

    // Sibling functions ranked by entity-overlap.
    let sib_rows: Vec<SiblingRow> = db.function_siblings(&symbol, 5)?;
    let siblings: Vec<ExplainSibling> = sib_rows
        .into_iter()
        .map(|s| ExplainSibling {
            symbol: s.symbol,
            shared_entities: s.shared_entities,
        })
        .collect();

    Ok(Explanation {
        symbol: function.symbol,
        crate_name: function.crate_name,
        file: function.file,
        line: function.line,
        doc_comment: function.doc_comment,
        entities,
        narrative_docs,
        siblings,
    })
}

/// Human-readable rendering of an [[entity-doc-graph]] Explanation.
/// Three labelled sections + the heading.
pub fn render_human(exp: &Explanation) -> String {
    let mut s = String::new();
    s.push_str("== Function ==\n");
    s.push_str(&format!("symbol: {}\n", exp.symbol));
    s.push_str(&format!("file: {}:{}\n", exp.file, exp.line));
    s.push_str(&format!("crate: {}\n", exp.crate_name));
    let dc = if exp.doc_comment.is_empty() {
        "(none)".to_string()
    } else {
        let mut t = exp.doc_comment.replace('\n', " ");
        if t.len() > 200 {
            t.truncate(200);
        }
        t
    };
    s.push_str(&format!("doc-comment: {dc}\n"));
    s.push('\n');

    s.push_str("== Entities (from FUNCTION_MENTIONS) ==\n");
    if exp.entities.is_empty() {
        s.push_str("(none — this function is dark)\n");
    } else {
        for e in &exp.entities {
            let conf_label = match e.confidence {
                ConfidenceTier::Low => "low confidence — symbol-path match",
                ConfidenceTier::High => "high confidence",
            };
            s.push_str(&format!("- entity-{} ({})\n", e.id, conf_label));
            if !e.display.is_empty() {
                s.push_str(&format!("  display: {}\n", e.display));
            }
            if !e.summary.is_empty() {
                let mut t = e.summary.replace('\n', " ");
                if t.len() > 240 {
                    t.truncate(240);
                }
                s.push_str(&format!("  summary: {t}\n"));
            }
        }
    }
    s.push('\n');

    s.push_str("== Narrative docs covering these entities ==\n");
    if exp.narrative_docs.is_empty() {
        s.push_str(
            "(none — no how-to / explanation / reference doc covers any of the listed entities)\n",
        );
    } else {
        for d in &exp.narrative_docs {
            let kind = d.kind.as_deref().unwrap_or("?");
            s.push_str(&format!("- {} (kind={})\n", d.id, kind));
            if !d.title.is_empty() {
                s.push_str(&format!("  {}\n", d.title));
            }
        }
    }
    s.push('\n');

    s.push_str(&format!(
        "== Sibling functions in {} (same entities, ranked by overlap) ==\n",
        exp.crate_name
    ));
    if exp.siblings.is_empty() {
        s.push_str("(none — no other function in the crate shares an entity)\n");
    } else {
        for sib in &exp.siblings {
            s.push_str(&format!(
                "- {} — shares: {}\n",
                sib.symbol,
                sib.shared_entities.join(", ")
            ));
        }
    }

    s
}

/// JSON rendering of an [[entity-doc-graph]] Explanation — single
/// object, pretty-printed.
pub fn render_json(exp: &Explanation) -> Result<String> {
    serde_json::to_string_pretty(exp).context("serialize explanation")
}

/// Render the [[entity-doc-graph]] candidate-list when a substring
/// resolves ambiguously. `with_list_flag` toggles between the
/// "ambiguous; pass --list to see all" hint and the bare list.
fn render_candidate_list(candidates: &[FunctionFull], with_list_flag: bool) -> String {
    let mut s = String::new();
    if with_list_flag {
        s.push_str(&format!(
            "doc-linter explain: {} candidates:\n",
            candidates.len()
        ));
    } else {
        s.push_str(&format!(
            "doc-linter explain: {} candidates match — pass `--list` to see all, \
             or refine the substring:\n",
            candidates.len()
        ));
    }
    let preview = if with_list_flag {
        candidates
    } else {
        // Cap the preview at 5 so a substring like `fn` doesn't dump
        // thousands of rows on the user's terminal. The hint above
        // tells them how to see the full list.
        &candidates[..candidates.len().min(5)]
    };
    for c in preview {
        s.push_str(&format!(
            "  {}  ({}:{})  [{}]\n",
            c.symbol, c.file, c.line, c.crate_name
        ));
    }
    s
}

/// CLI entry point. Returns the appropriate exit code:
///   - 0: explain rendered, candidate list rendered (--list), or
///        helpful "no match" message.
///   - 1: ambiguous match without --list (so CI scripts can detect
///        an unresolved input).
///
/// @endpoint CLI explain
/// Operates on [[entity-doc-graph]].
pub fn run(
    root: &Path,
    _config: &LintConfig,
    _all_files: &[PathBuf],
    needle: &str,
    list: bool,
    json: bool,
) -> Result<ExitCode> {
    let db = ReadGraph::open(root)?;

    // Search by substring — the substring match is the literal
    // substring test we want. Cap at 50 candidates so a degenerate
    // substring doesn't pull half the corpus.
    let candidates = db.search_functions_by_symbol_substring(needle, 50)?;

    if candidates.is_empty() {
        if json {
            println!(
                "{}",
                serde_json::json!({"error": "no-match", "needle": needle})
            );
        } else {
            println!(
                "doc-linter explain: no function matches `{needle}`. Try \
                 `doc-linter query functions-mentioning <entity>` to find \
                 candidates by entity."
            );
        }
        // Exit 0 (per the brief: "exit 0 (or 1 — pick one; not 2)" —
        // we pick 0 so JSON consumers don't get spurious failures).
        return Ok(ExitCode::SUCCESS);
    }

    if candidates.len() == 1 {
        // Just checked `len() == 1`; the iterator yields exactly one element.
        #[allow(clippy::unwrap_used, reason = "len()==1 guarantees next() is Some")]
        let exp = build_explanation(&*db, candidates.into_iter().next().unwrap())?;
        if json {
            println!("{}", render_json(&exp)?);
        } else {
            print!("{}", render_human(&exp));
        }
        return Ok(ExitCode::SUCCESS);
    }

    // 2+ candidates.
    if list {
        if json {
            /// Serde shape for one candidate row in the
            /// [[entity-doc-graph]] explain command's JSON output.
            #[derive(Serialize)]
            struct CandidateOut<'a> {
                symbol: &'a str,
                #[serde(rename = "crate")]
                crate_name: &'a str,
                file: &'a str,
                line: u32,
            }
            let body = serde_json::json!({
                "needle": needle,
                "candidates": candidates.iter().map(|c| CandidateOut {
                    symbol: &c.symbol,
                    crate_name: &c.crate_name,
                    file: &c.file,
                    line: c.line,
                }).collect::<Vec<_>>(),
            });
            println!("{}", serde_json::to_string_pretty(&body)?);
        } else {
            print!("{}", render_candidate_list(&candidates, true));
        }
        return Ok(ExitCode::SUCCESS);
    }

    // Ambiguous without --list: show the hint + first 5 and exit 1.
    print!("{}", render_candidate_list(&candidates, false));
    Ok(ExitCode::from(1))
}
