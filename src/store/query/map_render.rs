//! Roadmap issue #39 (v0.3.0): render a slice of the graph as a
//! Mermaid diagram suitable for embedding in markdown docs.
//!
//! v1 scope: entity ego graph (reuses `query_entity_subgraph`
//! from #12). Module / bounded-context scopes defer to a follow-
//! up — Module imports need the IMPORTS_MODULE tree-sitter pass
//! (#17 carve-out) and bounded-context needs a focused traversal
//! over `Doc.bounded_context` + cross-context RELATES_TO.
//!
//! Mermaid syntax for the entity case:
//!
//! ```text
//! graph LR
//!   pricing_rule["Pricing Rule<br/>★ god-node"]
//!   backend_api["Backend API"]
//!   database["Database"]
//!   backend_api --|depends_on|--> pricing_rule
//!   backend_api --|depends_on|--> database
//! ```
//!
//! Node ids are sanitized to alphanumerics + underscores (Mermaid
//! is finicky about identifier characters). Labels carry the
//! human-readable `display` field; god-nodes get a ★ marker so
//! readers can spot the central concepts at a glance.

use crate::store::query::entity_subgraph::EntitySubgraph;

/// Render an `EntitySubgraph` as a Mermaid `graph LR` diagram.
/// Caller is responsible for upper-bounding the node count (the
/// issue calls out ≤ 30 nodes for GitHub-markdown legibility) —
/// this function just renders what it's given.
pub fn render_entity_mermaid(subgraph: &EntitySubgraph) -> String {
    let mut out = String::with_capacity(256);
    out.push_str("graph LR\n");

    // Nodes first. Each line: `<sanitized-id>["<display>"]`. God
    // nodes get a ★ marker prefix on the label.
    for node in &subgraph.nodes {
        let id = mermaid_id(&node.id);
        let display = if node.display.is_empty() {
            node.id.as_str()
        } else {
            node.display.as_str()
        };
        let label = mermaid_label(display);
        if node.is_god_node {
            out.push_str(&format!("  {id}[\"★ {label}\"]\n"));
        } else {
            out.push_str(&format!("  {id}[\"{label}\"]\n"));
        }
    }

    // Edges. Mermaid's labeled-arrow shape is `A -- "label" --> B`.
    // Weight is included when non-default so visualization tools
    // can pick it up (e.g. line-thickness encoding).
    for edge in &subgraph.edges {
        let src = mermaid_id(&edge.source);
        let dst = mermaid_id(&edge.target);
        let label = mermaid_label(&edge.type_);
        out.push_str(&format!("  {src} -- \"{label}\" --> {dst}\n"));
    }

    out
}

/// Sanitize an entity id (or any graph id) to a valid Mermaid
/// node identifier. Mermaid identifiers must be alphanumerics +
/// underscore; hyphens, dots, slashes, and colons need
/// replacement. Empty result falls back to `"_"` so the diagram
/// renders.
fn mermaid_id(raw: &str) -> String {
    let mut out: String = raw
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect();
    if out.is_empty() {
        out.push('_');
    }
    // Mermaid identifiers can't start with a digit — prefix with
    // `n` to avoid a parse error on numeric-leading ids.
    if out.starts_with(|c: char| c.is_ascii_digit()) {
        out.insert(0, 'n');
    }
    out
}

/// Escape a string for use inside a Mermaid label literal.
/// Double-quotes break the `"…"` delimiter; angle brackets are
/// interpreted as HTML. Newlines become Mermaid `<br/>` so multi-
/// line labels render correctly.
fn mermaid_label(raw: &str) -> String {
    raw.replace('"', "&quot;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('\n', "<br/>")
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    reason = "tests panic on broken invariants — that is the point of a test"
)]
mod tests {
    use super::*;
    use crate::store::query::entity_subgraph::{
        EntitySubgraph, EntitySubgraphEdge, EntitySubgraphNode,
    };

    #[test]
    fn mermaid_id_sanitizes_special_chars() {
        assert_eq!(mermaid_id("pricing-rule"), "pricing_rule");
        assert_eq!(mermaid_id("backend.api"), "backend_api");
        assert_eq!(mermaid_id("a/b/c"), "a_b_c");
        assert_eq!(mermaid_id(""), "_", "empty falls back");
    }

    #[test]
    fn mermaid_id_prefixes_numeric_leading() {
        // Mermaid identifiers can't start with a digit.
        assert_eq!(mermaid_id("123abc"), "n123abc");
    }

    #[test]
    fn mermaid_label_escapes_html_and_newlines() {
        assert_eq!(mermaid_label(r#"Hello "world""#), "Hello &quot;world&quot;");
        assert_eq!(mermaid_label("<tag>"), "&lt;tag&gt;");
        assert_eq!(mermaid_label("line1\nline2"), "line1<br/>line2");
    }

    #[test]
    fn render_entity_mermaid_emits_graph_lr_header() {
        let g = EntitySubgraph {
            centre: "pricing-rule".to_string(),
            depth: 1,
            nodes: vec![
                EntitySubgraphNode {
                    id: "pricing-rule".to_string(),
                    display: "Pricing Rule".to_string(),
                    mention_count: 34,
                    is_god_node: true,
                },
                EntitySubgraphNode {
                    id: "backend-api".to_string(),
                    display: "Backend API".to_string(),
                    mention_count: 12,
                    is_god_node: false,
                },
            ],
            edges: vec![EntitySubgraphEdge {
                source: "backend-api".to_string(),
                target: "pricing-rule".to_string(),
                type_: "depends_on".to_string(),
                weight: 1.0,
                frequency: 0,
                edge_source: "frontmatter".to_string(),
            }],
        };
        let out = render_entity_mermaid(&g);
        assert!(out.starts_with("graph LR\n"));
        // God-node marker present.
        assert!(out.contains("★ Pricing Rule"), "got: {out}");
        // Non-god label rendered plain.
        assert!(out.contains("backend_api[\"Backend API\"]"));
        // Edge with type label.
        assert!(out.contains(r#"backend_api -- "depends_on" --> pricing_rule"#));
    }

    #[test]
    fn render_entity_mermaid_falls_back_to_id_when_display_empty() {
        let g = EntitySubgraph {
            centre: "x".to_string(),
            depth: 1,
            nodes: vec![EntitySubgraphNode {
                id: "x".to_string(),
                display: String::new(),
                mention_count: 0,
                is_god_node: false,
            }],
            edges: Vec::new(),
        };
        let out = render_entity_mermaid(&g);
        assert!(out.contains(r#"x["x"]"#), "got: {out}");
    }
}
