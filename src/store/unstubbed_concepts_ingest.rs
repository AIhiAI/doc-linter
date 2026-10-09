//! Gap-006 v1 — surface proper-noun candidates that the ontology
//! has not yet stubbed as Entity rows. Tokenises `Doc.summary`
//! text, drops known terms (via TermIndex) and a small hardcoded
//! stopword list, then emits one `Finding(kind="unstubbed-concept")`
//! row per surviving candidate per source Doc.
//!
//! ## Why summary only (v1 shortcut)
//!
//! Per [[audit-run-010]]: tokenising Doc body would mean re-reading
//! every markdown file during ingest. The Doc.summary column is
//! already projected and dense — for the v1 corpus (~50 hand-curated
//! docs) it carries most of the proper-noun signal worth surfacing.
//! Body-text tokenisation is a phase-2 extension once we have a
//! signal-noise read on the summary-only output.
//!
//! ## Why reuse Finding instead of a new CandidateTerm table
//!
//! Same audit's shortcut 2: zero-DDL change. Each candidate becomes a
//! Finding row with `kind="unstubbed-concept"`, `file=<source-doc-id>`,
//! `message=<the proper-noun token>`, `severity="info"`. The
//! `list_findings` MCP tool surfaces them unchanged. The
//! `unstubbed-concepts` saved query groups them by `message` so the
//! agent gets a ranked candidate list.
//!
//! ## Output shape
//!
//! ```sql
//! MATCH (f:Finding {kind: 'unstubbed-concept'})
//! RETURN f.message AS term, f.file AS source_doc_id
//! ```

use std::collections::HashSet;
use std::path::Path;

use crate::config::LintConfig;
use crate::disambiguation::TermIndex;
use crate::ontology::Ontology;

#[derive(Debug, Default, Clone, Copy)]
pub struct UnstubbedIngestStats {
    pub findings: usize,
    pub docs_scanned: usize,
    pub unique_candidates: usize,
}

/// Drop common English nouns / words that aren't worth surfacing as
/// candidate entities. Seeded from the v1 design-corpus noise; the
/// second wave (audit-run-014's false-positive list) added active-
/// voice verbs and section-heading words. Extend as new false-
/// positive patterns surface.
const STOPWORDS: &[&str] = &[
    // Wave 1 — quantifiers, articles, simple temporal words.
    "Summary",
    "Note",
    "Result",
    "Three",
    "Four",
    "Five",
    "Each",
    "When",
    "Where",
    "While",
    "What",
    "Which",
    "Why",
    "Also",
    "Some",
    "Many",
    "Most",
    "Both",
    "Today",
    "After",
    "Before",
    "Until",
    "Plus",
    "Per",
    // Wave 2 (audit-run-014) — active-voice verbs at sentence-start
    // position that the tokeniser flagged as candidates. Conservative
    // additions only — anything that's plausibly an entity stays out.
    "Adds",
    "Captures",
    "Closest",
    "Closer",
    "Composing",
    "Converged",
    "Defines",
    "Describes",
    "Establishes",
    "Generated",
    "Holds",
    "Identifies",
    "Indexes",
    "Lets",
    "Lights",
    "Lints",
    "Operates",
    "Plans",
    "Replaced",
    "Resolves",
    "Says",
    "Seeds",
    "Ships",
    "Surfaces",
    "Surveyed",
    "Survives",
    "Tells",
    "Validates",
    "Walks",
    // Wave 2 — common section-heading nouns that aren't entities.
    "Background",
    "Behind",
    "Built",
    "Cheap",
    "Cheapest",
    "Closed",
    "Established",
    "Kept",
    "Meta-role",
    "Navigation",
    "Open",
    "Originally",
    "Question",
    "Records",
    "Remaining",
    "Required",
    "Scope",
    "Step-by-step",
    "Survives",
    "Top-level",
    "User-facing",
    "Zero",
];

/// Pure derivation over non-ontology `(doc id, summary)` rows: returns
/// `(finding id, doc id, term)` rows and the stats. Shared with the
/// SQLite writer.
pub(crate) fn plan_unstubbed(
    docs: Vec<(String, String)>,
    root: &Path,
    ontology: &Ontology,
    config: &LintConfig,
) -> (Vec<(String, String, String)>, UnstubbedIngestStats) {
    let mut stats = UnstubbedIngestStats::default();
    let term_index = TermIndex::build(ontology);
    let stopwords: HashSet<&str> = STOPWORDS.iter().copied().collect();
    let accepted = accepted_terms(root, ontology, config);
    let mut doc_tokens: Vec<(String, Vec<(String, bool)>)> = Vec::new();
    for (id, summary) in docs {
        if id.is_empty() || summary.is_empty() {
            continue;
        }
        doc_tokens.push((id, tokenize_candidates(&summary)));
    }
    stats.docs_scanned = doc_tokens.len();
    let capitalised_mid_sentence: HashSet<&str> = doc_tokens
        .iter()
        .flat_map(|(_, toks)| toks)
        .filter(|(_, initial)| !initial)
        .map(|(t, _)| t.as_str())
        .collect();

    let mut seen_terms: HashSet<String> = HashSet::new();
    let mut findings: Vec<(String, String, String)> = Vec::new();
    for (doc_id, tokens) in &doc_tokens {
        for (term, sentence_initial) in tokens {
            if *sentence_initial
                && is_title_case(term)
                && !capitalised_mid_sentence.contains(term.as_str())
            {
                continue;
            }
            if stopwords.contains(term.as_str()) || is_accepted(&accepted, term) {
                continue;
            }
            if !term_index.lookup(term).is_empty() {
                continue;
            }
            let finding_id = format!("unstubbed-{doc_id}-{term}");
            findings.push((finding_id, doc_id.clone(), term.clone()));
            stats.findings += 1;
            seen_terms.insert(term.clone());
        }
    }
    stats.unique_candidates = seen_terms.len();
    (findings, stats)
}

/// Same accept set Vale and the code-comment lint use, so a vault that
/// keeps proper nouns in cspell packs doesn't have to repeat them in
/// `proper_nouns`. Lowercased: Vale's packs are expanded to case variants,
/// so membership is effectively case-insensitive.
///
/// An unreadable `vale_dictionaries` file drops only that pack (and
/// `check` reports it). It used to empty the whole set, letting every
/// capitalised English word ("This", "Please", "IMPORTANT") through.
fn accepted_terms(root: &Path, ontology: &Ontology, config: &LintConfig) -> HashSet<String> {
    crate::vale::accept_terms(root, ontology, config)
        .into_iter()
        .map(|t| t.to_lowercase())
        .collect()
}

/// Extract candidate proper-noun tokens from `text`, each flagged with
/// whether it is the first word of its sentence. Returns Capitalised-
/// word, CamelCase, or snake_case tokens of length >= 4, deduplicated
/// within one call (a token seen mid-sentence anywhere keeps `false`).
fn tokenize_candidates(text: &str) -> Vec<(String, bool)> {
    let mut out: Vec<(String, bool)> = Vec::new();
    for sentence in text.split(['.', '!', '?', ':', ';', '\n']) {
        let mut first = true;
        for raw in sentence.split(|c: char| !(c.is_alphanumeric() || c == '_' || c == '-')) {
            let token = raw.trim_matches('-').trim_matches('_');
            if token.is_empty() {
                continue;
            }
            let initial = std::mem::replace(&mut first, false);
            if token.len() < 4 || !is_candidate_token(token) {
                continue;
            }
            match out.iter_mut().find(|(t, _)| t == token) {
                Some(prev) => prev.1 &= initial,
                None => out.push((token.to_string(), initial)),
            }
        }
    }
    out
}

/// `accepted` holds lowercase terms. A hyphen compound ("Onam-aligned",
/// "September-2026") passes when every part is accepted or numeric.
fn is_accepted(accepted: &HashSet<String>, term: &str) -> bool {
    let lower = term.to_lowercase();
    accepted.contains(&lower)
        || (lower.contains('-')
            && lower.split('-').all(|part| {
                !part.is_empty()
                    && (accepted.contains(part) || part.chars().all(|c| c.is_ascii_digit()))
            }))
}

/// `Whether`, `Kerala`: one leading capital, the rest lowercase. The
/// shape sentence-case produces by accident; acronyms and CamelCase
/// don't.
fn is_title_case(token: &str) -> bool {
    let mut chars = token.chars();
    chars.next().is_some_and(char::is_uppercase) && chars.all(|c| !c.is_uppercase())
}

/// Token shape filter: Capitalised first letter, CamelCase, or
/// snake_case with at least one underscore.
fn is_candidate_token(token: &str) -> bool {
    let Some(first) = token.chars().next() else {
        return false;
    };
    if first.is_ascii_uppercase() {
        return true;
    }
    if first.is_ascii_lowercase() && token.contains('_') {
        return true;
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    fn words(toks: &[(String, bool)]) -> Vec<String> {
        toks.iter().map(|(t, _)| t.clone()).collect()
    }

    /// Handoff cpg-os-vault #7: sentence-initial capitals ("Whether",
    /// "Checking") were ~all of 85 findings. They're flagged initial so
    /// the ingest can drop them unless the word is capitalised elsewhere.
    #[test]
    fn tokeniser_flags_sentence_initial_words() {
        let toks = tokenize_candidates(
            "Whether an outlet buys. Checking Swiggy orders: Onam spikes in Kerala",
        );
        assert_eq!(
            toks,
            [
                ("Whether".to_string(), true),
                ("Checking".to_string(), true),
                ("Swiggy".to_string(), false),
                ("Onam".to_string(), true),
                ("Kerala".to_string(), false),
            ]
        );
        assert!(is_title_case("Whether"));
        assert!(!is_title_case("HAGGIS"));
        assert!(!is_title_case("PricingRule"));
    }

    /// cpg-analyst: the vault keeps proper nouns in `vale_dictionaries`
    /// packs; those (and hyphen compounds of them) must suppress findings.
    #[test]
    fn accepted_terms_cover_case_and_hyphen_compounds() {
        let accepted: HashSet<String> = ["onam", "september", "aligned", "colgate", "palmolive"]
            .into_iter()
            .map(String::from)
            .collect();
        assert!(is_accepted(&accepted, "Onam"));
        assert!(is_accepted(&accepted, "Onam-aligned"));
        assert!(is_accepted(&accepted, "September-2026"));
        assert!(is_accepted(&accepted, "Colgate-Palmolive"));
        assert!(!is_accepted(&accepted, "Onam-Diwali"));
        assert!(!is_accepted(&accepted, "Kerala"));
    }

    /// A missing `vale_dictionaries` file used to empty the whole accept
    /// set, so common words surfaced as unstubbed concepts.
    #[test]
    fn unreadable_dictionary_keeps_common_english_accepted() {
        let config: LintConfig =
            toml::from_str("[vale_dictionaries]\nMissing = \"no/such/dict.txt\"\n").unwrap();
        let ontology = Ontology::bootstrap();
        let accepted = accepted_terms(Path::new("/nonexistent"), &ontology, &config);
        for word in ["This", "Please", "IMPORTANT"] {
            assert!(is_accepted(&accepted, word), "{word} not accepted");
        }
        assert!(!is_accepted(&accepted, "Zorblax"));
    }

    #[test]
    fn tokeniser_keeps_capitalised_words() {
        let toks = words(&tokenize_candidates("Sourcegraph and Cody beat HAGGIS"));
        assert!(toks.contains(&"Sourcegraph".to_string()));
        assert!(toks.contains(&"HAGGIS".to_string()));
        assert!(toks.contains(&"Cody".to_string()));
    }

    #[test]
    fn tokeniser_keeps_snake_case() {
        let toks = words(&tokenize_candidates(
            "the find_patterns and audit_doc_region tools",
        ));
        assert!(toks.contains(&"find_patterns".to_string()));
        assert!(toks.contains(&"audit_doc_region".to_string()));
    }

    #[test]
    fn tokeniser_drops_short_tokens() {
        let toks = tokenize_candidates("API CLI MCP TCP");
        // All <4 chars
        assert!(toks.is_empty());
    }

    #[test]
    fn tokeniser_drops_lowercase_words() {
        let toks = tokenize_candidates("the quick brown fox");
        assert!(toks.is_empty());
    }
}
