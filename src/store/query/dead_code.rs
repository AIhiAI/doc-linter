//! Roadmap issue #37 (v0.3.0): `query dead-code` — Functions with
//! no inbound calls and no public entry point. Useful for cleanup,
//! debt assessment, and detecting documented-but-removed APIs.
//!
//! Filter rules (v1):
//!   - No incoming CALLS edge (no other Function calls this one).
//!   - Not handled by any Endpoint (axum/clap/MCP entry points
//!     are by definition "live" even without internal callers).
//!
//! Carve-outs (v1):
//!   - Test functions stay in the result. The issue calls out
//!     `#[test]` / `def test_` exclusion but that needs the #24
//!     TEST_FOR edge (or a heuristic on `f.symbol`); the cleaner
//!     filter lands once TEST_FOR is in place.
//!   - "Public API" exclusion is implicit: Endpoint handlers are
//!     filtered, but library `pub fn` items without endpoints are
//!     still reported. That's the right behaviour for "find the
//!     unused" — if you really want every `pub fn`, an
//!     `--include-public` flag can land later.

use serde::Serialize;

/// One dead-code row: a Function with no inbound calls and no
/// endpoint binding. `entity` is the FUNCTION_BELONGS_TO target
/// when authored (gives the agent "which area is this orphan
/// in?"); empty when no entity owns the source-file region.
#[derive(Debug, Clone, Serialize)]
pub struct DeadCodeRow {
    pub symbol: String,
    #[serde(rename = "crate")]
    pub crate_name: String,
    pub file: String,
    pub line: u32,
    /// First non-empty line of the doc-comment, truncated to a
    /// readable preview. Lets the agent decide at a glance whether
    /// the orphan is intentional documentation surface or genuine
    /// dead code without opening the file.
    pub doc_summary: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub entity: String,
}

/// Trim a multi-line doc-comment to a single preview line. Takes
/// the first non-blank line and caps at 120 chars so the JSON
/// output stays paste-friendly when an agent emits it into a PR
/// checklist. Matches the truncation budget the explain renderer
/// uses for `summary:` previews elsewhere.
pub(crate) fn doc_summary(doc: &str) -> String {
    const MAX_LEN: usize = 120;
    let first = doc.lines().map(str::trim).find(|l| !l.is_empty());
    let Some(first) = first else {
        return String::new();
    };
    if first.len() <= MAX_LEN {
        return first.to_string();
    }
    // Byte-safe truncation — find the largest char boundary at or
    // below MAX_LEN so we never split a UTF-8 codepoint.
    let mut cut = MAX_LEN;
    while !first.is_char_boundary(cut) && cut > 0 {
        cut -= 1;
    }
    let mut out = first[..cut].to_string();
    out.push('…');
    out
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    reason = "tests panic on broken invariants — that is the point of a test"
)]
mod tests {
    use super::doc_summary;

    #[test]
    fn doc_summary_empty_input_returns_empty() {
        assert_eq!(doc_summary(""), "");
        assert_eq!(doc_summary("\n\n  \n"), "", "all-whitespace also empty");
    }

    #[test]
    fn doc_summary_takes_first_non_blank_line() {
        let s = "\n  \nFirst real line.\nSecond line.\n";
        assert_eq!(doc_summary(s), "First real line.");
    }

    #[test]
    fn doc_summary_truncates_with_ellipsis_at_120_chars() {
        let long = "x".repeat(130);
        let trimmed = doc_summary(&long);
        // 120 x's + 1-byte ellipsis char (3 UTF-8 bytes) — assert
        // the byte length is at most 123 and ends with '…'.
        assert!(trimmed.ends_with('…'));
        assert!(trimmed.len() <= 123, "got {} bytes", trimmed.len());
    }

    #[test]
    fn doc_summary_truncation_is_utf8_safe() {
        // Multi-byte codepoints near the boundary mustn't split.
        let mut s = "a".repeat(118);
        s.push('é'); // 2 UTF-8 bytes — would land at byte 119–120.
        s.push('é'); // and this one starts at 120 — boundary check kicks in.
        s.push_str("trailing");
        let trimmed = doc_summary(&s);
        // The function must produce valid UTF-8; round-tripping
        // through `chars()` is the cheapest invariant check.
        assert_eq!(trimmed, trimmed.chars().collect::<String>());
        assert!(trimmed.ends_with('…'));
    }
}
