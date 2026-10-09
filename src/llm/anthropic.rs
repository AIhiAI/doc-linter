//! Gap-005 / audit-run-009: Anthropic Judge backend. Single
//! HTTP call per `compare_pair` via the Messages API. The
//! prompt asks for a structured JSON verdict (inconsistent /
//! confidence / rationale); on parse failure we return the
//! NoOp verdict with the parse error in `rationale` so
//! callers see why and can degrade.
//!
//! Gated by the `llm` Cargo feature so default builds don't
//! pull in reqwest + tokio. Activated at runtime when
//! `ANTHROPIC_API_KEY` is set.

use anyhow::{anyhow, Result};
use serde::Deserialize;

use super::{Claim, Judge, PairVerdict};

const DEFAULT_MODEL: &str = "claude-sonnet-4-6";
const DEFAULT_API_URL: &str = "https://api.anthropic.com/v1/messages";
const DEFAULT_API_VERSION: &str = "2023-06-01";
const DEFAULT_MAX_TOKENS: u32 = 256;
const DEFAULT_TIMEOUT_SECS: u64 = 10;

/// Live Anthropic Messages backend. Carries the API key, model
/// id, and a blocking reqwest client.
pub struct AnthropicJudge {
    api_key: String,
    model: String,
    api_url: String,
    api_version: String,
    client: reqwest::blocking::Client,
}

impl AnthropicJudge {
    /// Construct from environment. Returns Err when the API key
    /// isn't set so the factory can fall back to NoOp cleanly.
    /// Optional `ANTHROPIC_MODEL` overrides the default model id;
    /// `ANTHROPIC_API_URL` and `ANTHROPIC_API_VERSION` override the
    /// endpoint + version header for test / proxy setups.
    pub fn try_from_env() -> Result<Self> {
        let api_key =
            std::env::var("ANTHROPIC_API_KEY").map_err(|_| anyhow!("ANTHROPIC_API_KEY not set"))?;
        if api_key.is_empty() {
            return Err(anyhow!("ANTHROPIC_API_KEY is empty"));
        }
        let model = std::env::var("ANTHROPIC_MODEL").unwrap_or_else(|_| DEFAULT_MODEL.to_string());
        let api_url =
            std::env::var("ANTHROPIC_API_URL").unwrap_or_else(|_| DEFAULT_API_URL.to_string());
        let api_version = std::env::var("ANTHROPIC_API_VERSION")
            .unwrap_or_else(|_| DEFAULT_API_VERSION.to_string());
        let client = reqwest::blocking::Client::builder()
            .timeout(std::time::Duration::from_secs(DEFAULT_TIMEOUT_SECS))
            .build()
            .map_err(|e| anyhow!("build reqwest client: {e}"))?;
        Ok(AnthropicJudge {
            api_key,
            model,
            api_url,
            api_version,
            client,
        })
    }
}

impl Judge for AnthropicJudge {
    fn name(&self) -> &'static str {
        "anthropic"
    }

    fn is_available(&self) -> bool {
        !self.api_key.is_empty()
    }

    fn compare_pair(&self, a: &Claim, b: &Claim) -> Result<PairVerdict> {
        let prompt = build_compare_prompt(&a.text, &b.text);
        let body = serde_json::json!({
            "model": self.model,
            "max_tokens": DEFAULT_MAX_TOKENS,
            "messages": [{"role": "user", "content": prompt}],
        });
        let response = self
            .client
            .post(&self.api_url)
            .header("x-api-key", &self.api_key)
            .header("anthropic-version", &self.api_version)
            .header("content-type", "application/json")
            .json(&body)
            .send();
        let resp = match response {
            Ok(r) => r,
            Err(e) => {
                // Network / timeout: degrade to NoOp verdict so
                // batch callers can keep going without blowing up.
                return Ok(PairVerdict {
                    inconsistent: false,
                    confidence: 0.0,
                    rationale: format!("anthropic request error: {e}"),
                });
            }
        };
        if !resp.status().is_success() {
            return Ok(PairVerdict {
                inconsistent: false,
                confidence: 0.0,
                rationale: format!("anthropic http {}", resp.status().as_u16()),
            });
        }
        let parsed: AnthropicMessageResponse = match resp.json() {
            Ok(p) => p,
            Err(e) => {
                return Ok(PairVerdict {
                    inconsistent: false,
                    confidence: 0.0,
                    rationale: format!("anthropic response parse error: {e}"),
                });
            }
        };
        let text = parsed
            .content
            .into_iter()
            .filter_map(|block| match block {
                AnthropicContentBlock::Text { text } => Some(text),
            })
            .next()
            .unwrap_or_default();
        Ok(parse_verdict(&text))
    }
}

fn build_compare_prompt(a: &str, b: &str) -> String {
    format!(
        "You are evaluating whether two design statements are mutually consistent.\n\n\
         Statement A: {a}\n\n\
         Statement B: {b}\n\n\
         Respond with one JSON object on a single line:\n\
         {{\"inconsistent\": <true|false>, \"confidence\": <0.0..1.0>, \"rationale\": \"<one sentence>\"}}\n\n\
         `inconsistent` is true ONLY when the two statements make claims that cannot \
         simultaneously hold (e.g. \"we ship X first\" vs \"we defer X to phase 2\"). \
         Mere differences in framing or emphasis are NOT inconsistent."
    )
}

/// Parse the JSON-formatted verdict the LLM returns. Falls back to
/// a neutral NoOp-style verdict on any parse error so the caller
/// always sees a usable PairVerdict.
fn parse_verdict(raw: &str) -> PairVerdict {
    let trimmed = raw.trim();
    // The model sometimes wraps the JSON in a fenced block; extract
    // the first {...} substring.
    let start = trimmed.find('{');
    let end = trimmed.rfind('}');
    let slice = match (start, end) {
        (Some(s), Some(e)) if e >= s => &trimmed[s..=e],
        _ => {
            return PairVerdict {
                inconsistent: false,
                confidence: 0.0,
                rationale: format!("no JSON in LLM response: {trimmed}"),
            };
        }
    };
    match serde_json::from_str::<ParsedVerdict>(slice) {
        Ok(p) => PairVerdict {
            inconsistent: p.inconsistent,
            confidence: p.confidence.clamp(0.0, 1.0),
            rationale: p.rationale,
        },
        Err(e) => PairVerdict {
            inconsistent: false,
            confidence: 0.0,
            rationale: format!("JSON parse error: {e}"),
        },
    }
}

#[derive(Deserialize)]
struct ParsedVerdict {
    inconsistent: bool,
    confidence: f32,
    #[serde(default)]
    rationale: String,
}

#[derive(Deserialize)]
struct AnthropicMessageResponse {
    content: Vec<AnthropicContentBlock>,
}

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
enum AnthropicContentBlock {
    Text { text: String },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_verdict_handles_clean_json() {
        let v = parse_verdict(
            r#"{"inconsistent": true, "confidence": 0.85, "rationale": "A defers, B ships"}"#,
        );
        assert!(v.inconsistent);
        assert!((v.confidence - 0.85).abs() < 1e-5);
        assert_eq!(v.rationale, "A defers, B ships");
    }

    #[test]
    fn parse_verdict_extracts_from_fenced_block() {
        let v = parse_verdict(
            "Here is my verdict:\n```json\n{\"inconsistent\": false, \"confidence\": 0.4, \"rationale\": \"different framing\"}\n```",
        );
        assert!(!v.inconsistent);
        assert!((v.confidence - 0.4).abs() < 1e-5);
    }

    #[test]
    fn parse_verdict_clamps_confidence() {
        let v = parse_verdict(r#"{"inconsistent": false, "confidence": 1.5, "rationale": ""}"#);
        assert!((v.confidence - 1.0).abs() < 1e-5);
    }

    #[test]
    fn parse_verdict_no_json_returns_neutral() {
        let v = parse_verdict("I don't know.");
        assert!(!v.inconsistent);
        assert_eq!(v.confidence, 0.0);
        assert!(v.rationale.contains("no JSON"));
    }

    #[test]
    fn build_prompt_includes_both_statements() {
        let p = build_compare_prompt("ships X first", "defers X to phase 2");
        assert!(p.contains("ships X first"));
        assert!(p.contains("defers X to phase 2"));
        assert!(p.contains("JSON"));
    }
}
