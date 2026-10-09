//! SCIP-symbol tokenization + descriptor-path helpers used by the
//! [[entity-doc-graph]] code-graph ingest.
//!
//! The high-level read-side helpers (`scan_doc_comment_for_entities`,
//! `scan_symbol_for_entities`, `symbol_tokens`) are `pub`. The lower
//! tier (`strip_scip_package_prefix`, `strip_scip_descriptor_decoration`,
//! `symbol_descriptor_prefix`, `symbol_descends_from`, …) is `pub(super)`
//! so the sibling SCIP-ingest and endpoint-ingest submodules can reach
//! them without re-exporting through the crate-public surface.
//!
//! Tests live in sibling files (`scan_tests.rs`, `path_tests.rs`) to
//! keep the production module under the per-file LOC ceiling.

use std::collections::HashSet;

use crate::code_comments::COMMENT_STOPWORDS;
use crate::config::LintConfig;
use crate::disambiguation::TermIndex;
use crate::scip_ingest::FunctionKind;

#[cfg(test)]
mod path_tests;
#[cfg(test)]
mod scan_tests;

/// Build the merged stop list for symbol-path tokenization. Reuses
/// the two existing lists called for in roadmap-43's Phase 1 risk
/// mitigation:
///   * `LintConfig::vale_ambiguous_words` — bare ambiguous English
///     nouns the Vale pipeline already gates.
///   * `code_comments::COMMENT_STOPWORDS` — Rust stdlib types /
///     rustdoc section headers shared with the doc-comment lint.
/// All entries are lowercased so the tokenizer (which yields
/// lowercase tokens) can hash-test in one shot.
pub(crate) fn build_symbol_stop_list(config: &LintConfig) -> HashSet<String> {
    let mut stop: HashSet<String> = HashSet::new();
    for w in COMMENT_STOPWORDS {
        stop.insert(w.to_ascii_lowercase());
    }
    for w in &config.vale.vale_ambiguous_words {
        stop.insert(w.to_ascii_lowercase());
    }
    stop
}

/// Returns true when this `FunctionFact.kind` represents a *type
/// definition* whose name should propagate to its methods (sub-
/// feature 1c) for the [[entity-doc-graph]] symbol-token bridge.
/// Functions and methods themselves are NOT type definitions — only
/// struct / enum / trait / type-alias.
pub(crate) fn is_type_definition_kind(kind: FunctionKind) -> bool {
    matches!(
        kind,
        FunctionKind::Struct | FunctionKind::Enum | FunctionKind::Trait | FunctionKind::TypeAlias
    )
}

/// SCIP symbols look like:
///   `rust-analyzer cargo doc-linter 0.1.0 disambiguation/TermIndex#`
///                                          └─ descriptor path ─┘
/// `symbol_descriptor_prefix` returns the descriptor path with the
/// trailing token stripped — the prefix any descendant method's
/// symbol would share. Returns `None` for malformed inputs.
pub(crate) fn symbol_descriptor_prefix(symbol: &str) -> Option<String> {
    let after_prefix = strip_scip_package_prefix(symbol)?;
    let descriptor = after_prefix.trim();
    if descriptor.is_empty() {
        return None;
    }
    let segments: Vec<&str> = descriptor.split('/').filter(|s| !s.is_empty()).collect();
    if segments.is_empty() {
        return None;
    }
    let mut joined = String::new();
    for (i, seg) in segments.iter().enumerate() {
        if i > 0 {
            joined.push('/');
        }
        joined.push_str(strip_scip_descriptor_decoration(seg));
    }
    Some(joined)
}

/// Strip SCIP descriptor decoration suffixes from a single segment of a
/// symbol path during [[entity-doc-graph]] code-graph ingest.
/// `PricingRule#` -> `PricingRule`, `validate().` -> `validate`,
/// `Foo[Bar]` -> `Foo[Bar]` (brackets are kept; impl-target marker is
/// parsed elsewhere).
pub(super) fn strip_scip_descriptor_decoration(seg: &str) -> &str {
    let mut s = seg;
    while let Some(stripped) = s.strip_suffix('.') {
        s = stripped;
    }
    if let Some(stripped) = s.strip_suffix("()") {
        s = stripped;
    }
    if let Some(stripped) = s.strip_suffix('#') {
        s = stripped;
    }
    s
}

/// Strip the leading SCIP `<scheme> <manager> <package> <version> `
/// prefix and return the descriptor remainder. Used by the
/// [[entity-doc-graph]] code-graph ingest to derive a clean descriptor
/// path from each Function node's symbol.
pub(crate) fn strip_scip_package_prefix(symbol: &str) -> Option<&str> {
    let chars = symbol.char_indices();
    let mut spaces_seen = 0;
    let mut last_idx = 0;
    for (i, c) in chars {
        if c == ' ' {
            spaces_seen += 1;
            if spaces_seen == 4 {
                last_idx = i + 1;
                break;
            }
        }
    }
    if spaces_seen < 4 {
        return None;
    }
    Some(&symbol[last_idx..])
}

/// True when `symbol` lies under `type_prefix/...` — i.e. the method's
/// SCIP descriptor path starts with the type's prefix and a `/`
/// separator (so `Foo` doesn't accidentally swallow `FooBar`). Used by
/// the [[entity-doc-graph]] symbol-token bridge to attribute a
/// method's mentions up to its enclosing type definition.
#[cfg(test)]
pub(super) fn symbol_descends_from(symbol: &str, type_prefix: &str) -> bool {
    let Some(symbol_prefix) = symbol_descriptor_prefix(symbol) else {
        return false;
    };
    if symbol_prefix == *type_prefix {
        return false;
    }
    let with_sep = format!("{type_prefix}/");
    symbol_prefix.starts_with(&with_sep)
}

/// Extracts a crate name from `crates/<name>/README.md`. Used by the
/// SCIP ingest to map crate -> Doc id without hardcoding the `crate-*`
/// id scheme. Case-sensitive on the `README.md` filename.
pub(crate) fn crate_name_from_readme_path(path: &str) -> Option<String> {
    let p = path.replace('\\', "/");
    let parts: Vec<&str> = p.split('/').collect();
    if parts.len() < 3 {
        return None;
    }
    if parts[0] != "crates" {
        return None;
    }
    let last = parts.last().copied().unwrap_or("");
    if last != "README.md" {
        return None;
    }
    let name = parts[1];
    if name.is_empty() {
        None
    } else {
        Some(name.to_string())
    }
}

/// Scans a doc-comment string for ontology entity references and returns
/// the set of matching entity ids. Matches are case-insensitive over
/// every term registered in `TermIndex` (entity id, display, synonyms,
/// and naive plurals — same machinery the Vale post-processor uses).
///
/// Returns ids in deterministic order (sorted ascending) so re-ingest
/// produces a stable edge list.
pub fn scan_doc_comment_for_entities(text: &str, term_index: &TermIndex<'_>) -> Vec<String> {
    let mut hits: HashSet<String> = HashSet::new();
    for tok in tokenize_for_terms(text) {
        for ent in term_index.lookup(tok) {
            hits.insert(ent.id.clone());
        }
        // Wikilink-form fallback: an `[[entity-doc-graph]]` wikilink
        // tokenises to `entity-doc-graph` (the tokenizer treats `-` as
        // word-internal). Strip the `entity-` prefix and retry the
        // lookup so authored wikilinks register the same way bare
        // tokens do. Roadmap-49 phase 3 surfaced this gap when 12
        // doc-linter clap dispatchers carried `[[entity-doc-graph]]`
        // wikilinks but the scanner missed them.
        if let Some(stripped) = tok.strip_prefix("entity-") {
            for ent in term_index.lookup(stripped) {
                hits.insert(ent.id.clone());
            }
        }
    }
    let mut out: Vec<String> = hits.into_iter().collect();
    out.sort();
    out
}

/// Word-tokenizer for entity scanning. Splits on anything that isn't
/// `[A-Za-z0-9_-]` so multi-word entity displays (e.g. "Pricing Rule")
/// won't match — the TermIndex already builds the lowercase synonym set
/// per-token, and multi-word matching would need a phrase index that
/// Round 2A intentionally deferred.
fn tokenize_for_terms(text: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let bytes = text.as_bytes();
    let mut start = 0;
    let mut in_token = false;
    for (i, &b) in bytes.iter().enumerate() {
        let is_word = b.is_ascii_alphanumeric() || b == b'_' || b == b'-';
        if is_word && !in_token {
            start = i;
            in_token = true;
        } else if !is_word && in_token {
            out.push(&text[start..i]);
            in_token = false;
        }
    }
    if in_token {
        out.push(&text[start..]);
    }
    out
}

/// Phase 1 (roadmap-43): tokenize a SCIP symbol path into candidate
/// domain-noun tokens.
///
/// SCIP symbols look like:
///   `rust-analyzer cargo doc-linter 0.1.0 outlet_repo/create_outlet().`
///
/// The leading four whitespace-delimited tokens (scheme, package
/// manager, package name, version) carry no domain meaning so we
/// strip them. The remaining descriptor path is split on `/`,
/// trailing decoration (`#`, `()`, `().`) is stripped, then each
/// segment is broken into smaller tokens by:
///   * snake_case (`outlet_repo` -> `outlet`, `repo`)
///   * kebab-case (`pricing-rule` -> `pricing`, `rule`)
///   * camelCase (`PricingRule` -> `pricing`, `rule`)
///
/// Tokens are lowercased, deduped, and tokens shorter than 3 chars
/// are dropped. Returns the tokens in stable insertion order so
/// downstream consumers can hash-test against an entity index
/// without re-sorting.
///
/// `impl#[Foo<Bar>]` markers (sub-feature 1b) participate
/// automatically: `symbol_tokens` descends into the bracketed type
/// segment too. The combined token list contains both the impl-target's
/// tokens and the method name's tokens.
pub fn symbol_tokens(symbol: &str) -> Vec<String> {
    let Some(descriptor) = strip_scip_package_prefix(symbol) else {
        return Vec::new();
    };
    let mut out: Vec<String> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    let push = |tok: String, out: &mut Vec<String>, seen: &mut HashSet<String>| {
        if tok.len() < 3 {
            return;
        }
        if seen.insert(tok.clone()) {
            out.push(tok);
        }
    };
    for raw_seg in descriptor.split('/') {
        let seg = raw_seg.trim();
        if seg.is_empty() {
            continue;
        }
        let segment_to_split = if let Some(after_marker) = seg.strip_prefix("impl#") {
            after_marker
        } else {
            strip_scip_descriptor_decoration(seg)
        };
        for tok in split_identifier(segment_to_split) {
            push(tok, &mut out, &mut seen);
        }
    }
    out
}

/// Pull the bracketed target out of an `impl#[Foo<Bar>]` SCIP segment
/// during [[entity-doc-graph]] code-graph ingest, or return None when
/// the segment isn't an impl marker. Generics (`<Bar>`) are kept inside
/// the returned string — `split_identifier` handles them. Used by
/// `extract_impl_target`.
///
/// Real-world SCIP segments may have the impl marker mid-segment (e.g.
/// `impl#[CaptureSink]into_lines().`), so we search for `impl#[` rather
/// than requiring it as a prefix.
#[cfg(test)]
fn extract_impl_target_segment(seg: &str) -> Option<String> {
    let marker_pos = seg.find("impl#[")?;
    let after_open = &seg[marker_pos + "impl#[".len()..];
    let close = after_open.find(']')?;
    let target = &after_open[..close];
    if target.is_empty() {
        None
    } else {
        Some(target.to_string())
    }
}

/// Phase 1 (roadmap-43): pull an `impl#[Foo]`-style impl-target type
/// name out of an entire SCIP symbol path. Returns `None` when the
/// symbol contains no impl marker. The returned string is the raw
/// bracketed payload (e.g. `Foo`, `Foo<Bar>`); call `split_identifier`
/// on it to get domain tokens.
///
/// `symbol_tokens` already descends into the impl-marker bracket during
/// its main pass, so this helper is exposed primarily for the
/// impl-target unit tests. Compiled out of release builds.
#[cfg(test)]
pub fn extract_impl_target(symbol: &str) -> Option<String> {
    let descriptor = strip_scip_package_prefix(symbol).unwrap_or(symbol);
    for seg in descriptor.split('/') {
        if let Some(target) = extract_impl_target_segment(seg.trim()) {
            return Some(target);
        }
    }
    None
}

/// Split a single identifier into lowercase tokens by snake_case,
/// kebab-case, and camelCase boundaries. Used by `symbol_tokens`.
/// Tokens shorter than 1 char are dropped at this layer; the 3-char
/// minimum is enforced by the caller after stop-list filtering so the
/// same util is reusable for ad-hoc tests.
fn split_identifier(ident: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut current = String::new();
    for ch in ident.chars() {
        if ch == '_' || ch == '-' || (!ch.is_ascii_alphanumeric()) {
            if !current.is_empty() {
                push_camel_split(&current, &mut out);
                current.clear();
            }
        } else {
            current.push(ch);
        }
    }
    if !current.is_empty() {
        push_camel_split(&current, &mut out);
    }
    out
}

/// Inner helper for `split_identifier`: take a segment that's already
/// free of `_`/`-`/non-alphanumeric chars and split it at camelCase
/// boundaries (lowercase→uppercase transitions, plus `XMLParser`-style
/// ALL_CAPS-to-camel transitions). Feeds tokens to the
/// [[entity-doc-graph]] symbol-token bridge that resolves identifiers
/// to entity ids.
fn push_camel_split(seg: &str, out: &mut Vec<String>) {
    if seg.is_empty() {
        return;
    }
    let chars: Vec<char> = seg.chars().collect();
    let mut start = 0;
    for i in 1..chars.len() {
        let prev = chars[i - 1];
        let cur = chars[i];
        let lower_to_upper = prev.is_ascii_lowercase() && cur.is_ascii_uppercase();
        let upper_run_break = i >= 2
            && chars[i - 2].is_ascii_uppercase()
            && prev.is_ascii_uppercase()
            && cur.is_ascii_lowercase();
        if lower_to_upper {
            if start < i {
                out.push(
                    chars[start..i]
                        .iter()
                        .collect::<String>()
                        .to_ascii_lowercase(),
                );
            }
            start = i;
        } else if upper_run_break {
            if start < i - 1 {
                out.push(
                    chars[start..i - 1]
                        .iter()
                        .collect::<String>()
                        .to_ascii_lowercase(),
                );
            }
            start = i - 1;
        }
    }
    if start < chars.len() {
        out.push(
            chars[start..]
                .iter()
                .collect::<String>()
                .to_ascii_lowercase(),
        );
    }
}

/// Phase 1 (roadmap-43): scan a SCIP symbol's descriptor path for
/// ontology entity matches. Returns sorted unique entity ids.
///
/// Tokens are produced via `symbol_tokens`, then filtered against
/// the merged stop list (vale_ambiguous_words +
/// `code_comments::COMMENT_STOPWORDS`) before being looked up in
/// the `TermIndex`. The stop list is the false-positive guard
/// from the roadmap-43 risk section: filters out generic words
/// like "rule", "system", "data" that match entity synonyms but
/// don't carry domain meaning when they're part of a symbol path.
pub fn scan_symbol_for_entities(
    symbol: &str,
    term_index: &TermIndex<'_>,
    stop: &HashSet<String>,
) -> Vec<String> {
    let mut hits: HashSet<String> = HashSet::new();
    for tok in symbol_tokens(symbol) {
        if stop.contains(&tok) {
            continue;
        }
        for ent in term_index.lookup(&tok) {
            // An entity with `code_terms` binds only through them (#269).
            if ent.code_terms.is_empty() {
                hits.insert(ent.id.clone());
            }
        }
    }
    if !term_index.phrased().is_empty() {
        let runs = symbol_token_runs(symbol);
        for ent in term_index.phrased() {
            if ent.code_terms.iter().any(|t| has_phrase(&runs, t)) {
                hits.insert(ent.id.clone());
            }
            if ent.code_veto.iter().any(|t| has_phrase(&runs, t)) {
                hits.remove(&ent.id);
            }
        }
    }
    let mut out: Vec<String> = hits.into_iter().collect();
    out.sort();
    out
}

/// #269: the lowercase identifier tokens of each `/` segment of a
/// symbol's descriptor, in order (`symbol_tokens` dedupes and drops
/// short tokens, losing the adjacency a compound term needs).
fn symbol_token_runs(symbol: &str) -> Vec<Vec<String>> {
    let Some(descriptor) = strip_scip_package_prefix(symbol) else {
        return Vec::new();
    };
    descriptor
        .split('/')
        .map(|seg| split_identifier(strip_scip_descriptor_decoration(seg.trim())))
        .filter(|run| !run.is_empty())
        .collect()
}

/// Whether `phrase` (`business-date`) occurs as adjacent tokens in one
/// of `runs`. The last token may carry a plural `s` (`BusinessDates`).
fn has_phrase(runs: &[Vec<String>], phrase: &str) -> bool {
    let want = split_identifier(phrase);
    if want.is_empty() {
        return false;
    }
    let last = want.len() - 1;
    runs.iter().any(|run| {
        run.windows(want.len()).any(|w| {
            w[..last] == want[..last]
                && (w[last] == want[last] || w[last] == format!("{}s", want[last]))
        })
    })
}
