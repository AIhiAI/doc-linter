//! Roadmap issue #31 (v0.3.0): Finding node — TODOs, FIXMEs,
//! XXX/HACK markers as first-class graph entries.
//!
//! v1 surface: regex-scan every source File for line-comment
//! markers and emit one `Finding` row per match plus a
//! `File-[:HAS_FINDING]->Finding` edge.
//!
//! ## What's a finding here
//!
//! A line containing one of the recognised marker tokens —
//! `TODO`, `FIXME`, `XXX`, `HACK` — preceded by a line-comment
//! prefix appropriate for the file's language:
//!
//!   - `//` for `.rs` / `.ts` / `.tsx` / `.js` / `.jsx`
//!   - `#` for `.py`
//!
//! The remainder of the line (after the marker + optional `:`)
//! becomes the `message` column. The `kind` column carries the
//! marker token lowercased. `severity` is `info` for `todo` and
//! `xxx`, `warning` for `fixme` and `hack` — the matcher's
//! severity classifier can rank by that.
//!
//! ## What's not here
//!
//! Tree-sitter-aware extraction (so the scan only fires inside
//! actual comment nodes, not string literals or doc-comments) is
//! deferred. v1's regex strategy is intentionally
//! over-inclusive: a `"// TODO"` inside a string literal will
//! match, but the false-positive rate on real code is low enough
//! that the agent UX win outweighs the noise. The follow-up
//! issue can promote this to a tree-sitter pass.
//!
//! Deprecated-API patterns, security-pattern config, and
//! complexity-hotspot detection are also deferred — each is its
//! own focused pipeline.

use std::collections::HashSet;
use std::path::Path;

/// Stats returned to `cmd_check` so the stderr one-liner shows
/// what the scan produced.
#[derive(Debug, Default, Clone, Copy)]
pub struct FindingIngestStats {
    /// Total Finding rows inserted.
    pub findings: usize,
    /// Total HAS_FINDING edges inserted.
    pub edges: usize,
}

/// One staged Finding row. Promoted from a regex hit on a single
/// line of source.
#[derive(Debug, Clone)]
pub(crate) struct FindingRow {
    pub(crate) id: String,
    pub(crate) kind: String,
    pub(crate) file: String,
    pub(crate) line: u32,
    pub(crate) message: String,
    pub(crate) severity: String,
}

/// Scan every `(path, language)` file for TODO/FIXME markers (shared with
/// the SQLite writer).
pub(crate) fn collect_findings(root: &Path, file_rows: &[(String, String)]) -> Vec<FindingRow> {
    let mut findings: Vec<FindingRow> = Vec::new();
    let mut seen_ids: HashSet<String> = HashSet::new();
    for (path, lang) in file_rows {
        let Some(prefix) = comment_prefix_for(path, lang) else {
            continue;
        };
        let full = root.join(path);
        let Ok(content) = std::fs::read_to_string(&full) else {
            continue;
        };
        for finding in scan_for_markers(&content, prefix, path) {
            if seen_ids.insert(finding.id.clone()) {
                findings.push(finding);
            }
        }
    }
    findings
}

/// Pick the line-comment prefix for a file. Prefers the `lang`
/// column from the File row when known; falls back to extension-
/// based decision when `lang` is `"unknown"` or empty.
///
/// Returns `None` for languages without a single canonical
/// line-comment prefix (none today, but defensive against future
/// File-table entries for `.md` etc. that shouldn't be scanned).
fn comment_prefix_for(path: &str, lang: &str) -> Option<&'static str> {
    // Trust the language column first.
    match lang {
        "rust" | "typescript" | "tsx" | "javascript" | "jsx" | "java" => return Some("//"),
        "python" => return Some("#"),
        _ => {}
    }
    // Fallback: extension sniff. Mirrors `scip_ingest::language_from_file`
    // mapping for the languages we care about.
    let ext = path.rsplit_once('.').map(|(_, e)| e)?;
    match ext {
        "rs" | "ts" | "tsx" | "js" | "jsx" | "mjs" | "cjs" | "java" => Some("//"),
        "py" => Some("#"),
        _ => None,
    }
}

/// Scan content for marker tokens. Returns one `FindingRow` per
/// hit. `prefix` is the line-comment lead (`//` or `#`); only
/// lines beginning with that prefix (after leading whitespace)
/// are considered.
///
/// The marker recognition is intentionally narrow: the line must
/// contain one of the recognised tokens as a whole-word match
/// (preceded and followed by a non-alphanumeric boundary) after
/// the comment prefix. The remainder after the token (and
/// optional `:`) becomes the message; empty messages are kept
/// so the `id` is still unique and the line+kind is visible.
fn scan_for_markers(content: &str, prefix: &str, path: &str) -> Vec<FindingRow> {
    const MAX_MESSAGE: usize = 200;
    let mut out: Vec<FindingRow> = Vec::new();
    for (idx, raw) in content.lines().enumerate() {
        let line_1based = (idx as u32) + 1;
        let trimmed = raw.trim_start();
        let Some(after_prefix) = trimmed.strip_prefix(prefix) else {
            continue;
        };
        // Skip doc-comments (`///`, `//!`, `##` etc) — those are
        // narrative documentation, not debt markers.
        let body = after_prefix.trim_start();
        if body.is_empty() {
            continue;
        }
        // Find the first marker token in `body`. Walk the
        // recognised set; whichever appears earliest wins (so a
        // line like `// TODO: fix this — FIXME later` flags as
        // TODO, not FIXME).
        let (kind, marker_idx) = match earliest_marker(body) {
            Some(hit) => hit,
            None => continue,
        };
        let after_marker = &body[marker_idx + kind.len()..];
        let after_marker = after_marker.trim_start();
        let after_marker = after_marker.strip_prefix(':').unwrap_or(after_marker);
        let message_full = after_marker.trim();
        let message = if message_full.len() <= MAX_MESSAGE {
            message_full.to_string()
        } else {
            // Byte-safe cut at MAX_MESSAGE.
            let mut cut = MAX_MESSAGE;
            while !message_full.is_char_boundary(cut) && cut > 0 {
                cut -= 1;
            }
            format!("{}…", &message_full[..cut])
        };
        let kind_lower = kind.to_ascii_lowercase();
        let severity = match kind {
            "FIXME" | "HACK" => "warning",
            _ => "info",
        }
        .to_string();
        let id = format!("{path}:{line_1based}:{kind_lower}");
        out.push(FindingRow {
            id,
            kind: kind_lower,
            file: path.to_string(),
            line: line_1based,
            message,
            severity,
        });
    }
    out
}

/// Find the earliest occurrence of any recognised marker token
/// (whole-word) in `body`. Returns `(token, byte_offset)`.
fn earliest_marker(body: &str) -> Option<(&'static str, usize)> {
    const MARKERS: &[&str] = &["TODO", "FIXME", "XXX", "HACK"];
    let mut best: Option<(&'static str, usize)> = None;
    for marker in MARKERS {
        let mut search_from = 0;
        while let Some(rel) = body[search_from..].find(marker) {
            let abs = search_from + rel;
            let before_ok = abs == 0
                || !body
                    .as_bytes()
                    .get(abs - 1)
                    .copied()
                    .is_some_and(|b| (b as char).is_ascii_alphanumeric());
            let after_idx = abs + marker.len();
            let after_ok = after_idx >= body.len()
                || !body
                    .as_bytes()
                    .get(after_idx)
                    .copied()
                    .is_some_and(|b| (b as char).is_ascii_alphanumeric());
            if before_ok && after_ok {
                if best.is_none_or(|(_, b_idx)| abs < b_idx) {
                    best = Some((marker, abs));
                }
                break;
            }
            search_from = abs + marker.len();
        }
    }
    best
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    reason = "tests panic on broken invariants — that is the point of a test"
)]
mod tests {
    use super::*;

    #[test]
    fn comment_prefix_trusts_known_language() {
        assert_eq!(comment_prefix_for("src/lib.rs", "rust"), Some("//"));
        assert_eq!(comment_prefix_for("backend/app.py", "python"), Some("#"));
        assert_eq!(comment_prefix_for("docs/README.md", "rust"), Some("//")); // language wins
    }

    #[test]
    fn comment_prefix_falls_through_to_extension() {
        // Unknown language → extension fallback.
        assert_eq!(comment_prefix_for("foo.rs", "unknown"), Some("//"));
        assert_eq!(comment_prefix_for("foo.py", ""), Some("#"));
        assert_eq!(comment_prefix_for("README.md", "unknown"), None);
        assert_eq!(comment_prefix_for("noextension", ""), None);
    }

    #[test]
    fn earliest_marker_picks_first_whole_word() {
        assert_eq!(earliest_marker("TODO fix this"), Some(("TODO", 0)));
        // Em-dash is 3 UTF-8 bytes so FIXME starts at byte 13, not 12.
        assert_eq!(
            earliest_marker("fix this — FIXME later"),
            Some(("FIXME", 13)),
        );
        // TODO comes before FIXME on the same line.
        assert_eq!(
            earliest_marker("TODO: handle FIXME case"),
            Some(("TODO", 0))
        );
    }

    #[test]
    fn earliest_marker_rejects_alphanumeric_substring_matches() {
        // `TODOLIST` and `TODOing` aren't TODO markers — the
        // alphanumeric trailing character makes them part of a
        // longer identifier.
        assert_eq!(earliest_marker("TODOLIST is too long"), None);
        assert_eq!(earliest_marker("PSEUDOTODOhandler"), None);
        // Hyphen IS treated as a word boundary in v1 — a
        // `FIXME-pattern` reference still matches. That's
        // reasonable for a regex-style scan; tightening it can
        // come with the tree-sitter promotion.
        assert_eq!(
            earliest_marker("the FIXME-pattern crate"),
            Some(("FIXME", 4))
        );
    }

    #[test]
    fn scan_finds_todo_in_rust_line_comment() {
        let src = "fn main() {\n    // TODO: implement the body\n}\n";
        let hits = scan_for_markers(src, "//", "src/lib.rs");
        assert_eq!(hits.len(), 1);
        let h = &hits[0];
        assert_eq!(h.kind, "todo");
        assert_eq!(h.severity, "info");
        assert_eq!(h.line, 2);
        assert_eq!(h.message, "implement the body");
        assert_eq!(h.id, "src/lib.rs:2:todo");
    }

    #[test]
    fn scan_finds_fixme_warning_in_python() {
        let src = "def f():\n    # FIXME this is broken\n    pass\n";
        let hits = scan_for_markers(src, "#", "app/foo.py");
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].kind, "fixme");
        assert_eq!(hits[0].severity, "warning");
        assert_eq!(hits[0].message, "this is broken");
    }

    #[test]
    fn scan_skips_non_comment_lines_containing_marker_text() {
        // The marker has to be in a line-comment; bare strings
        // (line 2) don't qualify.
        let src = r#"
let s = "TODO is a marker";
/// TODO this looks like a doc-comment — v1 still picks it up
// TODO real one
"#;
        let hits = scan_for_markers(src, "//", "x.rs");
        // Line 2 (`let s = "TODO..."`) is skipped — no `//` lead.
        // Line 3 (`/// TODO ...`) matches — `///` still starts with
        // `//`; v1's regex strategy is over-inclusive on doc-
        // comments, which is acceptable noise.
        // Line 4 (`// TODO real one`) matches.
        let lines: Vec<u32> = hits.iter().map(|h| h.line).collect();
        assert_eq!(lines, vec![3, 4], "got: {hits:?}");
    }

    #[test]
    fn scan_truncates_long_message_with_ellipsis() {
        let mut long = String::from("// TODO ");
        long.push_str(&"x".repeat(300));
        let hits = scan_for_markers(&long, "//", "x.rs");
        assert_eq!(hits.len(), 1);
        assert!(hits[0].message.ends_with('…'));
        // 200-char cap + 3-byte ellipsis.
        assert!(
            hits[0].message.len() <= 203,
            "got {} bytes",
            hits[0].message.len()
        );
    }

    #[test]
    fn scan_handles_empty_message() {
        let src = "// TODO\n";
        let hits = scan_for_markers(src, "//", "x.rs");
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].message, "");
        // id stays unique even on empty message.
        assert_eq!(hits[0].id, "x.rs:1:todo");
    }
}
