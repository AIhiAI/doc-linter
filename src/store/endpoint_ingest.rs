//! Phase 2 of roadmap-43: Endpoint ingest into the [[entity-doc-graph]]
//! graph schema.
//!
//! `ingest_endpoints` wipes and repopulates `Endpoint`,
//! `ENDPOINT_HANDLED_BY`, and `ENDPOINT_TOUCHES_ENTITY`, resolving each
//! `EndpointFact`'s `handler_local` to a full SCIP symbol via the
//! [`FunctionIndex`] cheap lookup table. The TOUCHES_ENTITY edges are
//! derived by walking
//!   `Endpoint -[:HANDLED_BY]-> Function -[:MENTIONS]-> Entity`
//! in a single SQL hop and materialising the result as direct edges.

use std::collections::BTreeMap;

use super::symbols::{strip_scip_descriptor_decoration, strip_scip_package_prefix};

/// One row of the SCIP-derived function index used to resolve a
/// handler's local name (`get_item`) to its full SCIP symbol
/// (`rust-analyzer cargo example-api 0.1.0 items/handlers/get_item().`).
/// Built once per ingest pass and consumed read-only.
#[derive(Debug, Clone)]
pub struct FunctionIndexEntry {
    pub symbol: String,
    pub file: String,
    pub crate_name: String,
    /// Roadmap-49 phase 2: 1-based source line of the function
    /// definition, copied from the SCIP `FunctionFact`. Lets the
    /// `migrate-endpoint-markers` tool jump from a resolved handler
    /// symbol straight to the file:line where the doc-comment should
    /// be stamped, no second SCIP lookup required.
    pub line: u32,
}

/// Cheap lookup map: local-fn-name → every SCIP symbol that name
/// maps to. The endpoint ingest disambiguates by source-file colocation
/// and falls back to the same-crate match.
#[derive(Debug, Default, Clone)]
pub struct FunctionIndex {
    pub by_local_name: BTreeMap<String, Vec<FunctionIndexEntry>>,
}

impl FunctionIndex {
    /// Build from the raw `ScipFacts` set produced by `parse_scip`.
    /// The "local name" is the trailing identifier in the SCIP
    /// descriptor path — for `outlets/handlers/get_outlet().` that's
    /// `get_outlet`.
    pub fn build(facts: &crate::scip_ingest::ScipFacts) -> Self {
        let mut by_local_name: BTreeMap<String, Vec<FunctionIndexEntry>> = BTreeMap::new();
        for f in &facts.functions {
            let Some(name) = local_name_from_symbol(&f.symbol) else {
                continue;
            };
            by_local_name
                .entry(name)
                .or_default()
                .push(FunctionIndexEntry {
                    symbol: f.symbol.clone(),
                    file: f.file.clone(),
                    crate_name: f.crate_name.clone(),
                    line: f.line,
                });
        }
        FunctionIndex { by_local_name }
    }
}

/// Pull the trailing identifier out of a SCIP symbol descriptor path during
/// [[entity-doc-graph]] code-graph ingest, stripping the `().`/`#`/`.`
/// decoration. A member of a type (`Type#method().` — Java, TypeScript)
/// yields the member name, and a Java overload's `(+N)` disambiguator is
/// dropped. Returns `None` for malformed inputs (mirrors
/// `symbol_descriptor_prefix`'s tolerance).
fn local_name_from_symbol(symbol: &str) -> Option<String> {
    let descriptor = strip_scip_package_prefix(symbol)?;
    let last = descriptor
        .split('/')
        .filter(|s| !s.is_empty())
        .next_back()?;
    let last = last
        .split('#')
        .filter(|s| !s.is_empty())
        .next_back()
        .unwrap_or(last);
    let last = match last.find("(+") {
        Some(i) if last.ends_with(").") => &last[..i],
        _ => last,
    };
    let trimmed = strip_scip_descriptor_decoration(last);
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

/// Resolve a handler local name to a full SCIP symbol during
/// [[entity-doc-graph]] endpoint ingest. Returns the empty string when no
/// symbol could be determined (which the caller treats as "no
/// ENDPOINT_HANDLED_BY edge will be inserted").
///
/// Strategy:
///   1. Drop early if the EndpointFact has no `handler_local` ident.
///   2. Look up every symbol matching the local name.
///   3. If exactly one — use it.
///   4. If multiple — prefer the one whose `file` field matches the
///      endpoint's `source_file` exactly. If that resolves to one,
///      use it.
///   5. Otherwise drop the symbol — best-effort.
pub(crate) fn resolve_handler_symbol(
    fact: &crate::endpoint_extract::EndpointFact,
    index: &FunctionIndex,
) -> String {
    let Some(local) = fact.handler_local.as_deref() else {
        return String::new();
    };
    let Some(candidates) = index.by_local_name.get(local) else {
        return String::new();
    };
    if candidates.is_empty() {
        return String::new();
    }
    if candidates.len() == 1 {
        return candidates[0].symbol.clone();
    }
    // Multiple matches — prefer same-file.
    let same_file: Vec<&FunctionIndexEntry> = candidates
        .iter()
        .filter(|e| e.file == fact.source_file)
        .collect();
    if same_file.len() == 1 {
        return same_file[0].symbol.clone();
    }
    // JAX-RS overloads in one file (`delete(Long)` + `delete(String)`): the
    // handler always follows its annotations, so take the first definition
    // at or below the verb line. Nearest-either-way would pick the previous
    // overload when the `@Operation` block above the handler is long.
    if fact.kind == crate::endpoint_extract::EndpointKind::Jaxrs && same_file.len() > 1 {
        if let Some(e) = same_file
            .iter()
            .filter(|e| e.line >= fact.source_line)
            .min_by_key(|e| e.line)
        {
            return e.symbol.clone();
        }
    }
    // Same-crate fallback. Derive the crate from `source_file`'s
    // `crates/<name>/...` prefix.
    if let Some(crate_name) = source_file_crate(&fact.source_file) {
        let same_crate: Vec<&FunctionIndexEntry> = candidates
            .iter()
            .filter(|e| e.crate_name == crate_name)
            .collect();
        if same_crate.len() == 1 {
            return same_crate[0].symbol.clone();
        }
    }
    String::new()
}

/// Recovers the crate name from a `crates/<name>/...` path so the
/// [[entity-doc-graph]] endpoint ingest can attribute handler functions
/// to the right crate node.
fn source_file_crate(rel_path: &str) -> Option<String> {
    let p = rel_path.replace('\\', "/");
    let mut parts = p.split('/');
    let first = parts.next()?;
    if first != "crates" {
        return None;
    }
    let name = parts.next()?;
    if name.is_empty() {
        None
    } else {
        Some(name.to_string())
    }
}

#[cfg(test)]
mod local_name_tests {
    use super::local_name_from_symbol;

    #[test]
    fn jaxrs_overloads_in_one_file_resolve_to_the_next_definition() {
        use super::{resolve_handler_symbol, FunctionIndex, FunctionIndexEntry};
        use crate::endpoint_extract::{EndpointFact, EndpointKind};
        let entry = |symbol: &str, line| FunctionIndexEntry {
            symbol: symbol.to_string(),
            file: "Clients.java".to_string(),
            crate_name: "fineract-provider".to_string(),
            line,
        };
        let mut index = FunctionIndex::default();
        index.by_local_name.insert(
            "delete".to_string(),
            vec![
                entry("Clients#delete().", 214),
                entry("Clients#delete(+1).", 414),
                entry("Clients#delete(+2).", 240),
            ],
        );
        let fact = |line| EndpointFact {
            id: "jaxrs:DELETE:/v1/clients/external-id/{externalId}".to_string(),
            kind: EndpointKind::Jaxrs,
            method: "DELETE".to_string(),
            path: "/v1/clients/external-id/{externalId}".to_string(),
            handler_local: Some("delete".to_string()),
            source_file: "Clients.java".to_string(),
            source_line: line,
        };
        assert_eq!(
            resolve_handler_symbol(&fact(410), &index),
            "Clients#delete(+1)."
        );
        assert_eq!(
            resolve_handler_symbol(&fact(211), &index),
            "Clients#delete()."
        );
        assert_eq!(
            resolve_handler_symbol(&fact(218), &index),
            "Clients#delete(+2).",
            "long annotation block: the overload above is closer but precedes the verb"
        );
        assert_eq!(
            resolve_handler_symbol(&fact(500), &index),
            "",
            "no definition below the verb stays unresolved"
        );
    }

    #[test]
    fn member_and_overload_symbols_yield_the_method_name() {
        let java = "semanticdb maven maven/org.apache.fineract/fineract-loan 1.0 \
                    org/apache/fineract/LoansApiResource#retrieveAll().";
        assert_eq!(local_name_from_symbol(java).as_deref(), Some("retrieveAll"));
        let overload = "semanticdb maven . 1.0 org/x/Loans#submit(+1).";
        assert_eq!(local_name_from_symbol(overload).as_deref(), Some("submit"));
        let ty = "semanticdb maven . 1.0 org/x/Loans#";
        assert_eq!(local_name_from_symbol(ty).as_deref(), Some("Loans"));
        let rust = "rust-analyzer cargo example-api 0.1.0 items/handlers/get_item().";
        assert_eq!(local_name_from_symbol(rust).as_deref(), Some("get_item"));
    }
}
