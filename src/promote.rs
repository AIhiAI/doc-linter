//! Roadmap-48 Rule B — actionable promotion stubs for unknown nouns.
//!
//! When the vocab-closure lint flags a term that doesn't resolve to any
//! ontology entity, the diagnostic surface (in both human and JSON
//! output) appends a copy-paste-ready `entity-<term>.md` frontmatter
//! body so the author's choice becomes a bounded "rephrase OR promote"
//! decision rather than the open-ended "make this lint stop" of the
//! pre-Rule-B diagnostic.
//!
//! The stub is generated once per diagnostic by [`format_promote_stub`].
//! Callers also pass the closest-by-token-similarity entity ids
//! (`crate::disambiguation::closest_entities_for_token`) so the rendered
//! template's `## Related entities` section points at concrete neighbours
//! the author can wikilink against.

use chrono::Local;

/// Render a copy-paste promotion stub for an unknown noun. The output
/// is intended to be appended to a diagnostic message; it includes:
///
/// - The four-space indent so the block reads as a code-fenced
///   instruction inside the surrounding human-output flow.
/// - Pre-filled frontmatter (id, role, title, summary, status,
///   updated, axis_id=covers, value_id, display, description,
///   synonyms, bounded_contexts, introduced_in_version).
/// - A skeleton body with `# Entity — Title` and a `## Related
///   entities` list seeded from the closest candidates.
/// - A trailing `to docs/ontology/entities/<slug>.md` line that tells
///   the author where to save the file.
///
/// Token shape: the raw `unknown_noun` is normalised by
/// [`stub_slug`] (lowercase, kebab-cased) for the id / value_id /
/// filename, and by [`stub_display`] (title-cased per word) for the
/// human-facing title / display fields.
pub fn format_promote_stub(unknown_noun: &str, closest: &[String]) -> String {
    let slug = stub_slug(unknown_noun);
    let display = stub_display(unknown_noun);
    let today = Local::now().date_naive();

    let mut s = String::new();
    s.push_str("\n\nTo resolve, either:\n");
    s.push_str("  (a) rephrase using an existing entity");
    if !closest.is_empty() {
        s.push_str(" (closest by token similarity:\n      ");
        s.push_str(&closest.join(", "));
        s.push(')');
    }
    s.push('\n');
    s.push_str("  (b) promote to a new entity by saving the following file:\n\n");
    s.push_str("      ---\n");
    s.push_str(&format!("      id: entity-{slug}\n"));
    s.push_str("      role: ontology-entity\n");
    s.push_str(&format!("      title: \"Entity: {display}\"\n"));
    s.push_str("      summary: \"<one-sentence definition you fill in>\"\n");
    s.push_str("      status: stable\n");
    s.push_str(&format!("      updated: {today}\n"));
    s.push_str("      axis_id: covers\n");
    s.push_str(&format!("      value_id: {slug}\n"));
    s.push_str(&format!("      display: {display}\n"));
    s.push_str("      description: \"<one-sentence definition>\"\n");
    s.push_str("      synonyms: []\n");
    s.push_str("      bounded_contexts: []\n");
    s.push_str("      introduced_in_version: 4\n");
    s.push_str("      ---\n\n");
    s.push_str(&format!("      # Entity — {display}\n\n"));
    s.push_str("      <one-paragraph definition>\n\n");
    s.push_str("      ## Related entities\n\n");
    if closest.is_empty() {
        s.push_str("      - <no neighbours yet — wikilink at least one related entity>\n");
    } else {
        for c in closest {
            s.push_str(&format!("      - [[entity-{c}]]\n"));
        }
    }
    s.push('\n');
    s.push_str(&format!("      to docs/ontology/entities/{slug}.md"));
    s
}

/// Normalise an unknown noun to a kebab-case slug usable as the
/// [[entity-doc-graph]] entity id (and the on-disk filename stem) when
/// promoting. Leading/trailing non-alphanumerics drop; runs of
/// non-alphanumerics collapse to a single `-`. Snake-case input
/// (`throttling_policy`) and space-separated input (`Throttling Policy`)
/// both produce the same `throttling-policy`.
pub fn stub_slug(noun: &str) -> String {
    let mut out = String::with_capacity(noun.len());
    let mut prev_dash = true;
    for c in noun.chars() {
        if c.is_ascii_alphanumeric() {
            for lc in c.to_lowercase() {
                out.push(lc);
            }
            prev_dash = false;
        } else if !prev_dash {
            out.push('-');
            prev_dash = true;
        }
    }
    while out.ends_with('-') {
        out.pop();
    }
    out
}

/// Title-case an unknown noun for the stub's `title:` / `display:`
/// fields. Splits on `-`, `_`, whitespace; capitalises each token's
/// first ASCII letter; rejoins with single spaces.
pub fn stub_display(noun: &str) -> String {
    let parts: Vec<String> = noun
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|w| !w.is_empty())
        .map(|w| {
            let mut chars = w.chars();
            match chars.next() {
                Some(first) => {
                    let head: String = first.to_uppercase().collect();
                    let tail: String = chars.flat_map(char::to_lowercase).collect();
                    format!("{head}{tail}")
                }
                None => String::new(),
            }
        })
        .collect();
    parts.join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Pins the kebab-slug normaliser used by the doc-graph promote-stub
    /// generator: snake_case, mixed-case spaces, and already-kebab
    /// inputs all collapse to the same lowercase form.
    #[test]
    fn slug_kebab_cases_snake_and_space_inputs() {
        assert_eq!(stub_slug("throttling_policy"), "throttling-policy");
        assert_eq!(stub_slug("Throttling Policy"), "throttling-policy");
        assert_eq!(stub_slug("Throttling-Policy"), "throttling-policy");
        assert_eq!(stub_slug("UPPER"), "upper");
    }

    /// Pins the display-title humaniser used by doc-graph promote-stub
    /// scaffolding: every word in the input gets title-cased, hyphens
    /// and underscores become spaces, already-cased input stays stable.
    #[test]
    fn display_titlecases_each_word() {
        assert_eq!(stub_display("throttling-policy"), "Throttling Policy");
        assert_eq!(stub_display("Pricing Rule"), "Pricing Rule");
        assert_eq!(stub_display("MIXED_word"), "Mixed Word");
    }

    /// Pins the doc-graph promote-stub frontmatter shape: every key
    /// the linter requires (id, role, title, axis_id, value_id,
    /// display) plus the body skeleton with closest-entity wikilinks.
    #[test]
    fn stub_includes_frontmatter_keys_and_body() {
        let s = format_promote_stub("throttling-policy", &["pricing-rule".to_string()]);
        // Spot-check key fragments callers / tests assert on.
        assert!(s.contains("id: entity-throttling-policy"));
        assert!(s.contains("role: ontology-entity"));
        assert!(s.contains("title: \"Entity: Throttling Policy\""));
        assert!(s.contains("axis_id: covers"));
        assert!(s.contains("value_id: throttling-policy"));
        assert!(s.contains("display: Throttling Policy"));
        assert!(s.contains("# Entity — Throttling Policy"));
        assert!(s.contains("[[entity-pricing-rule]]"));
        assert!(s.contains("to docs/ontology/entities/throttling-policy.md"));
    }

    /// Pins the doc-graph promote-stub no-neighbours fallback: when
    /// the closest-entity list is empty, the body embeds an explicit
    /// `<no neighbours yet>` placeholder.
    #[test]
    fn stub_with_no_closest_uses_placeholder() {
        let s = format_promote_stub("foo", &[]);
        assert!(s.contains("<no neighbours yet"));
        assert!(s.contains("id: entity-foo"));
    }
}
