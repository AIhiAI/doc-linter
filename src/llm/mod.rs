//! Gap-005 + audit_doc_region rerank scaffold. v1 first slice:
//! the `Judge` trait + a `NoOpJudge` placeholder + a
//! `default_judge()` factory. Mirrors the [`crate::embeddings`]
//! Embedder pattern.
//!
//! ## What this is for
//!
//! Two upcoming features need the same LLM surface:
//!
//! 1. **`audit_doc_region` rerank** — the v1 MVP currently returns
//!    raw BM25/embedding scores. Wrapping that response with an
//!    LLM "what's missing in the target vs the top hits" pass
//!    lifts the tool from retrieval-only to suggestion-grade. See
//!    `cmd/mcp::tool_audit_doc_region` for the call site that
//!    will plug a `Judge` in.
//! 2. **`contradiction` detection** ([[gap-005]]) — given two
//!    `tags=design`-cluster docs, the LLM decides whether their
//!    claims are mutually consistent. Emits
//!    `Finding(kind="contradiction")` rows.
//!
//! Both want the same shape: take two (or more) text blobs, ask
//! an LLM a structured question, get a structured verdict back.
//! That shape is the `Judge` trait.
//!
//! ## v1 first slice scope
//!
//! Only the trait + NoOp impl + factory ship in this slice. Real
//! backends (Anthropic, OpenAI) and the consuming features land
//! in follow-ups, behind the existing `ANTHROPIC_API_KEY` and a
//! new `--features llm` Cargo feature so default builds incur
//! no LLM-runtime dependency.

use anyhow::Result;
use serde::{Deserialize, Serialize};

#[cfg(feature = "llm")]
pub mod anthropic;

/// One claim plus its provenance — used as input to the `Judge`
/// when the caller already extracted atomic claims (e.g. for
/// contradiction detection).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Claim {
    /// Free-text claim. Should be a self-contained statement,
    /// not a paragraph.
    pub text: String,
    /// Optional source doc/entity/function id. Carried through so
    /// downstream `Finding` rows can attribute the claim back to
    /// its origin without a second graph lookup.
    pub source_id: Option<String>,
    /// Optional line number in `source_id` where the claim
    /// originated. Same provenance role as `source_id`.
    pub source_line: Option<u32>,
}

/// Verdict from a pairwise comparison — used by both the
/// contradiction-detection feature and the audit_doc_region
/// rerank.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PairVerdict {
    /// `true` when the Judge reads the inputs as inconsistent /
    /// contradictory. For audit_doc_region, this maps to "the
    /// target doc is missing something the candidate has."
    pub inconsistent: bool,
    /// Calibrated [0.0, 1.0]. NoOp returns 0.0.
    pub confidence: f32,
    /// Short one-line summary the caller can surface to the
    /// user. Empty string for NoOp.
    pub rationale: String,
}

/// LLM judge surface — the v1 single abstraction for "ask an
/// LLM a structured question and get a structured answer."
///
/// `Send + Sync` so a single shared judge can sit behind an
/// `Arc` and be used across parallel corpus iteration.
pub trait Judge: Send + Sync {
    /// Stable identifier persisted alongside any output (e.g. a
    /// `Finding` row) so a model swap forces re-evaluation
    /// instead of silently mixing verdicts from different
    /// backends. `"noop"` for the placeholder.
    fn name(&self) -> &'static str;

    /// `true` when this backend produces real verdicts. The
    /// consuming features short-circuit when `false` so default
    /// builds don't error out — they just skip the LLM-gated
    /// step. Convenience for the call sites.
    fn is_available(&self) -> bool;

    /// Compare two claims for consistency. Default impl returns
    /// the NoOp verdict (inconsistent: false, confidence: 0.0,
    /// rationale: ""). Real backends override.
    fn compare_pair(&self, _a: &Claim, _b: &Claim) -> Result<PairVerdict> {
        Ok(PairVerdict {
            inconsistent: false,
            confidence: 0.0,
            rationale: String::new(),
        })
    }
}

/// Placeholder Judge. Always reports unavailable; the default
/// build never reaches its `compare_pair` because consumers gate
/// on `is_available()`.
#[derive(Debug, Clone, Copy, Default)]
pub struct NoOpJudge;

impl Judge for NoOpJudge {
    fn name(&self) -> &'static str {
        "noop"
    }
    fn is_available(&self) -> bool {
        false
    }
}

/// Factory mirroring [`crate::embeddings::default_embedder`]: the
/// build picks the right backend at startup.
///
/// v1 first slice always returns [`NoOpJudge`]. Subsequent slices
/// add an Anthropic backend behind the `llm` Cargo feature + an
/// `ANTHROPIC_API_KEY` env check. On layer 1 init failure (key
/// set but invalid), the factory falls back to NoOp with a
/// one-line stderr warning so operators see why the LLM-gated
/// passes silently skipped.
pub fn default_judge() -> Box<dyn Judge> {
    #[cfg(feature = "llm")]
    {
        match anthropic::AnthropicJudge::try_from_env() {
            Ok(judge) => return Box::new(judge),
            Err(err) => {
                let key_set = std::env::var_os("ANTHROPIC_API_KEY").is_some();
                if key_set {
                    eprintln!(
                        "doc-linter: ANTHROPIC_API_KEY set but Anthropic Judge \
                         init failed ({err}); falling back to NoOp."
                    );
                }
            }
        }
    }
    Box::new(NoOpJudge)
}

#[cfg(test)]
#[allow(clippy::float_cmp, reason = "asserts exact constants")]
mod tests {
    use super::*;

    #[test]
    fn noop_judge_reports_unavailable() {
        let j = NoOpJudge;
        assert!(!j.is_available());
        assert_eq!(j.name(), "noop");
    }

    #[test]
    fn noop_judge_default_compare_returns_neutral() {
        let j = NoOpJudge;
        let v = j
            .compare_pair(
                &Claim {
                    text: "a".into(),
                    source_id: None,
                    source_line: None,
                },
                &Claim {
                    text: "b".into(),
                    source_id: None,
                    source_line: None,
                },
            )
            .unwrap();
        assert!(!v.inconsistent);
        assert_eq!(v.confidence, 0.0);
    }

    #[test]
    fn default_factory_yields_noop_in_default_build() {
        let j = default_judge();
        assert_eq!(j.name(), "noop");
        assert!(!j.is_available());
    }
}
