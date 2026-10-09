//! Doc-side read helpers — list/get/outbound/inbound/shortest_path
//! over the [[entity-doc-graph]] SQLite store, plus the raw `sql`
//! passthrough.

/// Per iter 104: extract the Entity.id that a Doc.id refers to.
/// doc-linter's convention: ontology-entity Doc nodes have id
/// `entity-X`, and the corresponding Entity node has id `X`. Returns
/// `None` for any Doc.id NOT following that convention (research-*,
/// audit-*, interrogation-*, etc.) — they don't represent entities.
/// Pure function — testable without a DB.
pub fn entity_id_from_entity_doc(doc_id: &str) -> Option<&str> {
    let stripped = doc_id.strip_prefix("entity-")?;
    if stripped.is_empty() {
        return None;
    }
    Some(stripped)
}

#[cfg(test)]
mod entity_id_tests {
    use super::entity_id_from_entity_doc;

    #[test]
    fn strips_entity_prefix_to_entity_id() {
        assert_eq!(entity_id_from_entity_doc("entity-cursor"), Some("cursor"));
        assert_eq!(
            entity_id_from_entity_doc("entity-retrieval-primitive"),
            Some("retrieval-primitive")
        );
        assert_eq!(
            entity_id_from_entity_doc("entity-workflow-companion"),
            Some("workflow-companion")
        );
    }

    #[test]
    fn returns_none_for_non_entity_docs() {
        // research absorptions are NOT entity-shaped — their id starts
        // with `research-`, not `entity-`.
        assert_eq!(entity_id_from_entity_doc("research-cursor"), None);
        assert_eq!(entity_id_from_entity_doc("user-probe-018"), None);
        assert_eq!(entity_id_from_entity_doc("interrogation-019"), None);
        assert_eq!(
            entity_id_from_entity_doc("doc-design-prior-art-landscape"),
            None
        );
    }

    #[test]
    fn returns_none_for_bare_entity_prefix() {
        // `entity-` with no suffix isn't a real id — guard against
        // empty Entity.id lookups.
        assert_eq!(entity_id_from_entity_doc("entity-"), None);
    }

    #[test]
    fn returns_none_for_empty_input() {
        assert_eq!(entity_id_from_entity_doc(""), None);
    }
}
