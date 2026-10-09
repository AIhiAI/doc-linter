//! Roadmap issue #16 (v0.3.0): write SCIP-derived `IMPORTS`
//! edges into the SQLite graph.
//!
//! `parse_scip` populates `ScipFacts.imports` with one
//! `ImportFact` per Import-role occurrence. This pass aggregates
//! them into per-pair `(importer, imported)` counts and emits
//! one `File-[:IMPORTS]->File` edge per pair where both sides
//! are present in the File table (#17). The MATCH-on-both
//! pattern silently drops edges whose imported file isn't in
//! the corpus — typical for stdlib / cross-crate imports — same
//! convention as CALLS / FUNCTION_MENTIONS.

use std::collections::HashMap;

use crate::scip_ingest::ScipFacts;

/// Stats returned to `cmd_check`'s stderr line.
#[derive(Debug, Default, Clone, Copy)]
pub struct ImportsIngestStats {
    /// Unique (importer, imported) pairs after aggregation.
    pub pairs: usize,
    /// Successful IMPORTS edge inserts (= pairs where both
    /// sides resolved to File rows). Bounded above by `pairs`.
    pub edges: usize,
}

/// Distinct `(importer, imported)` file pairs with their import counts,
/// sorted. Shared with the SQLite writer.
pub(crate) fn import_counts(facts: &ScipFacts) -> Vec<((String, String), i64)> {
    let mut counts: HashMap<(String, String), i64> = HashMap::new();
    for fact in &facts.imports {
        if fact.importer_file.is_empty() || fact.imported_file.is_empty() {
            continue;
        }
        let key = (fact.importer_file.clone(), fact.imported_file.clone());
        *counts.entry(key).or_insert(0) += 1;
    }
    let mut sorted: Vec<((String, String), i64)> = counts.into_iter().collect();
    sorted.sort_by(|a, b| a.0.cmp(&b.0));
    sorted
}
