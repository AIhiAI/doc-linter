//! Bounded-context-aware post-processor for Vale alerts (Round 2A).
//!
//! Vale's primitives (`existence`, `substitution`, ...) match isolated
//! tokens — they can't natively check "is this term acceptable given the
//! doc's bounded context?". So Vale fires a coarse `FA.AmbiguousBare`
//! alert on every occurrence, and this module decides per-alert whether
//! to:
//!
//!   - **suppress** the alert (the term resolves to an entity in the
//!     doc's own context — fully-qualified by context, no diagnostic);
//!   - **downgrade** to a `CrossContextReference` issue (the term
//!     resolves to an entity in a *different* context — author should
//!     either fully-qualify with `[[entity-id]]` or move the doc);
//!   - **keep** the alert as-is (the term doesn't resolve to any
//!     ontology entity — Vale's coarse complaint stands).
//!
//! Resolution is case-insensitive over `entity.id`, `entity.display`, every
//! `synonym`, and the naive-plural of each. Pluralization mirrors what
//! `crate::vale::build_accept` writes into the project vocab list, so a term
//! Vale flagged is exactly something we expect to find in our index.

use crate::ontology::{EntityDef, Ontology};
use crate::vale::ValeAlert;
use crate::validator::Issue;
use std::collections::HashMap;

/// Result of post-processing a single Vale alert through the
/// [[entity-doc-graph]] bounded-context disambiguator.
#[derive(Debug, Clone)]
pub enum Resolution {
    /// Term resolves to an entity living in the doc's context — drop the
    /// Vale alert silently.
    Suppress,
    /// Term resolves to an entity in a different context — replace the
    /// Vale alert with a `CrossContextReference` issue.
    Downgrade(Issue),
    /// Term doesn't resolve to any entity — keep Vale's alert as the
    /// caller had it.
    Keep,
}

/// In-memory term index built once per lint run from the
/// [[entity-doc-graph]] ontology. Maps lowercased term (`entity.id`,
/// `entity.display`, every synonym, plus naive plurals) to the entity it
/// points at. We keep `Vec<&EntityDef>` per term so an ambiguous term that
/// maps to multiple entities can still surface — the post-processor treats
/// multi-mapped terms the same as single-mapped ones for context
/// resolution: if ANY of the entities shares the doc's context, suppress;
/// else downgrade to the first one (alphabetical by id) and let the author
/// disambiguate explicitly.
pub struct TermIndex<'a> {
    by_term: HashMap<String, Vec<&'a EntityDef>>,
    /// #269: entities declaring `code_terms` or `code_veto`, checked
    /// against every symbol by `scan_symbol_for_entities`.
    phrased: Vec<&'a EntityDef>,
}

impl<'a> TermIndex<'a> {
    pub fn build(ontology: &'a Ontology) -> Self {
        let mut by_term: HashMap<String, Vec<&'a EntityDef>> = HashMap::new();
        let mut phrased = Vec::new();
        for entity in ontology.entities.values() {
            // Roadmap issue #14 (v0.3.0): `candidate` is the legacy
            // promotion-workflow status. It is excluded from the term
            // index outright — surface forms never resolve, so the
            // mention-coverage / disambiguation lints never fire and
            // FUNCTION_MENTIONS edges are never created. Old stubs
            // emitted by pre-#180 `cluster --write` runs still carry
            // this status and stay invisible to the graph.
            //
            // Issue #180: `auto` is the cluster-derived participation
            // status used by current `cluster --write`. Auto entities
            // DO populate the term index (so FUNCTION_MENTIONS form,
            // god-node ranking works, `query list --ranked` surfaces
            // them) but are exempted from coverage lint at the check
            // call site (`cmd/check/coverage.rs`).
            if entity.status == "candidate" {
                continue;
            }
            push(&mut by_term, &entity.id, entity);
            push(&mut by_term, &entity.display, entity);
            for syn in &entity.synonyms {
                push(&mut by_term, syn, entity);
            }
            if !entity.code_terms.is_empty() || !entity.code_veto.is_empty() {
                phrased.push(entity);
            }
        }
        TermIndex { by_term, phrased }
    }

    /// #269: entities with `code_terms` / `code_veto`.
    pub fn phrased(&self) -> &[&'a EntityDef] {
        &self.phrased
    }

    /// Look up a term in the [[entity-doc-graph]] term index
    /// (case-insensitively, after stripping a naive plural suffix) and return
    /// the matching ontology entities, if any. Used by the bounded-context
    /// disambiguator and the function doc-comment scanner.
    pub fn lookup(&self, term: &str) -> &[&'a EntityDef] {
        let key = normalise(term);
        if let Some(v) = self.by_term.get(&key) {
            return v.as_slice();
        }
        // Try singular: drop trailing 's' or convert "ies" → "y".
        if let Some(singular) = depluralise(&key) {
            if let Some(v) = self.by_term.get(&singular) {
                return v.as_slice();
            }
        }
        &[]
    }
}

/// Inserts a `(surface-form -> entity)` mapping (and its naive plural) into
/// the [[entity-doc-graph]] term index used by the bounded-context resolver.
fn push<'a>(map: &mut HashMap<String, Vec<&'a EntityDef>>, raw: &str, entity: &'a EntityDef) {
    let key = normalise(raw);
    if key.is_empty() {
        return;
    }
    let entry = map.entry(key.clone()).or_default();
    if !entry.iter().any(|e| e.id == entity.id) {
        entry.push(entity);
    }
    // Also index the naive plural of the surface form so a Vale alert
    // matching "rules" lands on the same entity as "rule".
    let plural = naive_plural(&key);
    if plural != key {
        let entry = map.entry(plural).or_default();
        if !entry.iter().any(|e| e.id == entity.id) {
            entry.push(entity);
        }
    }
}

/// Lower-cases and trims a surface term so the [[entity-doc-graph]]
/// disambiguator can match Vale alerts against ontology entries
/// case-insensitively.
fn normalise(s: &str) -> String {
    s.trim().to_ascii_lowercase()
}

/// English-ish pluraliser used by the [[entity-doc-graph]] term index so
/// "rule" and "rules" both resolve to the same entity.
fn naive_plural(term: &str) -> String {
    if term.ends_with('s') {
        return term.to_string();
    }
    let chars: Vec<char> = term.chars().collect();
    if chars.len() >= 2 {
        let last = chars[chars.len() - 1];
        let prev = chars[chars.len() - 2];
        if last == 'y' && !matches!(prev, 'a' | 'e' | 'i' | 'o' | 'u') {
            let mut s: String = chars[..chars.len() - 1].iter().collect();
            s.push_str("ies");
            return s;
        }
    }
    let mut s = term.to_string();
    s.push('s');
    s
}

/// Inverse of `naive_plural` — strips a trailing `ies`/`s` so the
/// [[entity-doc-graph]] disambiguator can normalise plural surface forms
/// before lookup.
fn depluralise(term: &str) -> Option<String> {
    if let Some(stripped) = term.strip_suffix("ies") {
        return Some(format!("{stripped}y"));
    }
    if let Some(stripped) = term.strip_suffix('s') {
        if !stripped.is_empty() {
            return Some(stripped.to_string());
        }
    }
    None
}

/// Decide what to do with one Vale alert given the doc's effective
/// bounded context. Only `FA.AmbiguousBare` alerts (and any other rule
/// the caller flags as a vocab-closure check) are run through this
/// path — caller filters by check name before invoking.
pub fn resolve_alert(
    alert: &ValeAlert,
    doc_context: Option<&str>,
    index: &TermIndex<'_>,
) -> Resolution {
    let matches = index.lookup(&alert.match_text);
    if matches.is_empty() {
        return Resolution::Keep;
    }

    // If the doc has no effective context, every entity is "fine" — we
    // can't determine cross-context drift. Suppress the Vale warning
    // rather than fire a noisy alert; downstream rounds will tighten this
    // when bounded-context becomes mandatory.
    let Some(doc_ctx) = doc_context else {
        return Resolution::Suppress;
    };

    // Same-context wins: if ANY matching entity declares this context (or
    // is context-agnostic, signalled by an empty `bounded_contexts` list),
    // the term is excused.
    for entity in matches {
        if entity.bounded_contexts.is_empty()
            || entity.bounded_contexts.iter().any(|c| c == doc_ctx)
        {
            return Resolution::Suppress;
        }
    }

    // No same-context match; pick the first entity (sorted by id for
    // deterministic output) and emit a CrossContextReference.
    let mut sorted: Vec<&&EntityDef> = matches.iter().collect();
    sorted.sort_by(|a, b| a.id.cmp(&b.id));
    let target = sorted[0];
    // Pick a "primary" context for the message. Entities in a single
    // context use that one; multi-context entities use the lowest-sorted
    // for stability.
    let mut entity_ctxs: Vec<&String> = target.bounded_contexts.iter().collect();
    entity_ctxs.sort();
    let entity_context = entity_ctxs
        .first()
        .map_or("(unknown)", |s| s.as_str())
        .to_string();

    Resolution::Downgrade(Issue::CrossContextReference {
        term: alert.match_text.clone(),
        entity_id: target.id.clone(),
        entity_context,
        doc_context: doc_ctx.to_string(),
        line: alert.line,
    })
}

/// Returns true if `check` is a Vale rule whose alerts this module knows
/// how to post-process. Currently only `FA.AmbiguousBare`; the closure
/// rule (`Vocabulary.Vocabulary`) flags unknown capitalised tokens that
/// don't fit the bare-ambiguous-noun pattern, and we keep those as-is.
pub fn is_vocab_closure_rule(check: &str) -> bool {
    check == "FA.AmbiguousBare"
}

/// Roadmap-48 Rule B: rank ontology entity ids + synonyms by token
/// similarity to `term` and return up to `k` distinct entity ids in
/// closest-first order. Levenshtein distance over the lowercased
/// surface forms; ties are broken alphabetically by entity id so two
/// runs over the same ontology produce identical output.
///
/// Used by the `unknown-noun-promote` diagnostic surface to suggest
/// "rephrase candidates" alongside the new-entity stub. We include
/// every synonym (so a noun close to `Pricing Rule`'s synonym `rule`
/// surfaces `pricing-rule` even when the entity id itself is far)
/// and dedupe on entity id (the candidate list is "which entity is
/// closest", not "which surface form").
pub fn closest_entities_for_token(term: &str, ontology: &Ontology, k: usize) -> Vec<String> {
    if k == 0 || term.is_empty() {
        return Vec::new();
    }
    let target = normalise(term);
    // (entity_id, distance, tie-breaker = entity_id)
    let mut scored: Vec<(String, usize)> = Vec::new();
    for entity in ontology.entities.values() {
        let mut surfaces: Vec<String> = Vec::with_capacity(2 + entity.synonyms.len());
        surfaces.push(entity.id.clone());
        surfaces.push(entity.display.clone());
        for syn in &entity.synonyms {
            surfaces.push(syn.clone());
        }
        let mut best: Option<usize> = None;
        for surface in &surfaces {
            let d = levenshtein(&target, &normalise(surface));
            best = match best {
                None => Some(d),
                Some(b) if d < b => Some(d),
                Some(b) => Some(b),
            };
        }
        if let Some(b) = best {
            scored.push((entity.id.clone(), b));
        }
    }
    // Stable sort: distance ASC, then id ASC.
    scored.sort_by(|a, b| a.1.cmp(&b.1).then_with(|| a.0.cmp(&b.0)));
    scored.into_iter().take(k).map(|(id, _)| id).collect()
}

/// Levenshtein edit distance between two strings (insertions, deletions,
/// substitutions all cost 1) — used by the [[entity-doc-graph]]
/// did-you-mean fallback when a term doesn't resolve. Iterative two-row
/// DP — fits the small ontology surface (entity ids + synonyms cap at low
/// thousands) without pulling a fresh dependency.
fn levenshtein(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    if a.is_empty() {
        return b.len();
    }
    if b.is_empty() {
        return a.len();
    }
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    let mut curr: Vec<usize> = vec![0; b.len() + 1];
    for i in 1..=a.len() {
        curr[0] = i;
        for j in 1..=b.len() {
            let cost = usize::from(a[i - 1] != b[j - 1]);
            curr[j] = (prev[j] + 1).min(curr[j - 1] + 1).min(prev[j - 1] + cost);
        }
        std::mem::swap(&mut prev, &mut curr);
    }
    prev[b.len()]
}

/// Bounded-context disambiguation tests for the [[entity-doc-graph]]
/// Vale alert resolver.
#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "tests panic on broken invariants — that is the point of a test"
)]
mod tests {
    use super::*;
    use crate::ontology::EntityDef;

    fn fake_alert(term: &str) -> ValeAlert {
        ValeAlert {
            check: "FA.AmbiguousBare".to_string(),
            match_text: term.to_string(),
            line: 7,
            message: format!("bare {term}"),
            severity: "warning".to_string(),
            span: [1, 1],
        }
    }

    fn make_ontology() -> Ontology {
        let mut ont = Ontology::bootstrap();
        ont.entities.insert(
            "pricing-rule".to_string(),
            EntityDef {
                id: "pricing-rule".to_string(),
                display: "Pricing Rule".to_string(),
                description: "test".to_string(),
                synonyms: vec!["rule".to_string()],
                bounded_contexts: vec!["pricing".to_string()],
                scanner_coverage: vec![],
                source_modules: vec![],
                relates_to: vec![],
                status: "stable".to_string(),
                entity_class: None,
                attributes: Vec::new(),
                code_terms: Vec::new(),
                code_veto: Vec::new(),
            },
        );
        ont.entities.insert(
            "sync-job".to_string(),
            EntityDef {
                id: "sync-job".to_string(),
                display: "Sync Job".to_string(),
                description: "test".to_string(),
                synonyms: vec!["job".to_string()],
                bounded_contexts: vec!["sync".to_string()],
                scanner_coverage: vec![],
                source_modules: vec![],
                relates_to: vec![],
                status: "stable".to_string(),
                entity_class: None,
                attributes: Vec::new(),
                code_terms: Vec::new(),
                code_veto: Vec::new(),
            },
        );
        ont.entities.insert(
            "tenant".to_string(),
            EntityDef {
                id: "tenant".to_string(),
                display: "Tenant".to_string(),
                description: "test".to_string(),
                synonyms: vec![],
                bounded_contexts: vec![], // context-agnostic
                scanner_coverage: vec![],
                source_modules: vec![],
                relates_to: vec![],
                status: "stable".to_string(),
                entity_class: None,
                attributes: Vec::new(),
                code_terms: Vec::new(),
                code_veto: Vec::new(),
            },
        );
        ont
    }

    /// Asserts that a bare term inside its own bounded context is suppressed
    /// by the [[entity-doc-graph]] disambiguator (no cross-context warning).
    #[test]
    fn same_context_term_is_suppressed() {
        let ont = make_ontology();
        let idx = TermIndex::build(&ont);
        let r = resolve_alert(&fake_alert("rule"), Some("pricing"), &idx);
        assert!(matches!(r, Resolution::Suppress));
    }

    /// Roadmap issue #14 (v0.3.0): entities with `status: candidate`
    /// must NOT show up in the term index — the cluster subcommand
    /// emits them for review, and we mustn't enforce coverage /
    /// disambiguation lints against them until a human promotes
    /// `status` to `stable` (or deletes the file).
    #[test]
    fn candidate_entity_is_dropped_from_term_index() {
        let mut ont = make_ontology();
        ont.entities.insert(
            "draft-scorer".to_string(),
            EntityDef {
                id: "draft-scorer".to_string(),
                display: "Draft Scorer".to_string(),
                description: "cluster-derived".to_string(),
                synonyms: vec!["scorer".to_string()],
                bounded_contexts: vec![],
                scanner_coverage: vec![],
                source_modules: vec![],
                relates_to: vec![],
                status: "candidate".to_string(),
                entity_class: None,
                attributes: Vec::new(),
                code_terms: Vec::new(),
                code_veto: Vec::new(),
            },
        );
        let idx = TermIndex::build(&ont);
        // Both the synonym and the display name should miss — the
        // entity is filtered out wholesale.
        assert!(
            idx.lookup("scorer").is_empty(),
            "candidate synonym should not resolve"
        );
        assert!(
            idx.lookup("draft scorer").is_empty(),
            "candidate display should not resolve"
        );
        // Sanity: non-candidate entities still resolve.
        assert!(!idx.lookup("pricing rule").is_empty());
    }

    /// Issue #180: `auto` entities (the new cluster-derived
    /// participation status) participate in the term index — they
    /// resolve, accumulate FUNCTION_MENTIONS, and show up in
    /// `query list --ranked`. Lint exemption is enforced separately
    /// at the check call site (`cmd/check/coverage.rs`).
    #[test]
    fn auto_entity_participates_in_term_index() {
        let mut ont = make_ontology();
        ont.entities.insert(
            "auto-scorer".to_string(),
            EntityDef {
                id: "auto-scorer".to_string(),
                display: "Auto Scorer".to_string(),
                description: "cluster-derived auto entity".to_string(),
                synonyms: vec!["autoscorer".to_string()],
                bounded_contexts: vec![],
                scanner_coverage: vec![],
                source_modules: vec![],
                relates_to: vec![],
                status: "auto".to_string(),
                entity_class: None,
                attributes: Vec::new(),
                code_terms: Vec::new(),
                code_veto: Vec::new(),
            },
        );
        let idx = TermIndex::build(&ont);
        assert!(
            !idx.lookup("autoscorer").is_empty(),
            "auto synonym should resolve"
        );
        assert!(
            !idx.lookup("auto scorer").is_empty(),
            "auto display should resolve"
        );
    }

    /// Asserts that a term claimed by another bounded context is downgraded
    /// to a `CrossContextReference` issue by the [[entity-doc-graph]]
    /// disambiguator instead of a hard fail.
    #[test]
    fn cross_context_term_is_downgraded() {
        let ont = make_ontology();
        let idx = TermIndex::build(&ont);
        let r = resolve_alert(&fake_alert("rule"), Some("sync"), &idx);
        match r {
            Resolution::Downgrade(Issue::CrossContextReference {
                term,
                entity_id,
                entity_context,
                doc_context,
                line,
            }) => {
                assert_eq!(term, "rule");
                assert_eq!(entity_id, "pricing-rule");
                assert_eq!(entity_context, "pricing");
                assert_eq!(doc_context, "sync");
                assert_eq!(line, 7);
            }
            other => panic!("expected Downgrade(CrossContextReference), got {other:?}"),
        }
    }

    /// Asserts that a term not present in the ontology bypasses the
    /// [[entity-doc-graph]] resolver and keeps Vale's original alert.
    #[test]
    fn unknown_term_is_kept() {
        let ont = make_ontology();
        let idx = TermIndex::build(&ont);
        let r = resolve_alert(&fake_alert("widget"), Some("pricing"), &idx);
        assert!(matches!(r, Resolution::Keep));
    }

    /// Asserts that the [[entity-doc-graph]] term index resolves plural
    /// surface forms (e.g. "rules") to the same entity as the singular.
    #[test]
    fn plural_form_resolves() {
        // Vale matches "rules" in body; index keys off "rule".
        let ont = make_ontology();
        let idx = TermIndex::build(&ont);
        let r = resolve_alert(&fake_alert("rules"), Some("pricing"), &idx);
        assert!(matches!(r, Resolution::Suppress));
    }

    /// Asserts that an ontology entity declared with no bounded contexts is
    /// always suppressed by the [[entity-doc-graph]] disambiguator
    /// regardless of doc context.
    #[test]
    fn context_agnostic_entity_always_suppressed() {
        let ont = make_ontology();
        let idx = TermIndex::build(&ont);
        // tenant has no bounded_contexts → matches every doc context.
        let r = resolve_alert(&fake_alert("tenant"), Some("pricing"), &idx);
        assert!(matches!(r, Resolution::Suppress));
        let r = resolve_alert(&fake_alert("tenant"), Some("sync"), &idx);
        assert!(matches!(r, Resolution::Suppress));
    }

    /// Asserts that a doc with no resolvable bounded context falls through
    /// to suppression in the [[entity-doc-graph]] disambiguator (we don't
    /// fail-closed on missing context).
    #[test]
    fn no_doc_context_falls_through_to_suppress() {
        let ont = make_ontology();
        let idx = TermIndex::build(&ont);
        let r = resolve_alert(&fake_alert("rule"), None, &idx);
        assert!(matches!(r, Resolution::Suppress));
    }

    /// Roadmap-48 Rule B: closest_entities_for_token returns up to `k`
    /// entity ids in closest-first Levenshtein order. The fake ontology
    /// here has `pricing-rule`, `sync-job`, `tenant`; a token spelled
    /// like a near-miss of one of them should rank that entity first.
    #[test]
    fn closest_entities_orders_by_levenshtein() {
        let ont = make_ontology();
        let out = closest_entities_for_token("tenent", &ont, 3);
        assert_eq!(out.first().map(std::string::String::as_str), Some("tenant"));
        assert!(out.len() <= 3);
    }

    /// Roadmap-48 Rule B: when k=0 or term is empty, no candidates.
    #[test]
    fn closest_entities_empty_inputs_return_empty() {
        let ont = make_ontology();
        assert!(closest_entities_for_token("anything", &ont, 0).is_empty());
        assert!(closest_entities_for_token("", &ont, 3).is_empty());
    }
}
