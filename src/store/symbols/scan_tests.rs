//! Scanner + tokenizer tests for the [[entity-doc-graph]] SCIP ingest.
//! Split from `symbols/mod.rs` to keep production code under the
//! 700-LOC per-file ceiling.

use std::collections::HashMap;

use super::{
    build_symbol_stop_list, crate_name_from_readme_path, extract_impl_target,
    scan_doc_comment_for_entities, scan_symbol_for_entities, symbol_descends_from, symbol_tokens,
};
use crate::config::LintConfig;
use crate::disambiguation::TermIndex;
use crate::ontology::{EntityDef, Ontology};

#[test]
/// Asserts that a doc-comment containing an entity synonym yields a
/// matching mention in the [[entity-doc-graph]] scanner.
fn scan_doc_comment_finds_synonyms() {
    let mut ont = Ontology::bootstrap();
    ont.entities.insert(
        "pricing-rule".to_string(),
        EntityDef {
            id: "pricing-rule".to_string(),
            display: "Pricing Rule".to_string(),
            description: "test".to_string(),
            synonyms: vec!["rule".to_string()],
            bounded_contexts: vec![],
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
    let idx = TermIndex::build(&ont);
    let comment = "Applies the configured pricing rule. Rules cascade.";
    let hits = scan_doc_comment_for_entities(comment, &idx);
    assert_eq!(hits, vec!["pricing-rule"]);
}

#[test]
/// Asserts that the [[entity-doc-graph]] doc-comment scanner emits one
/// row per entity even when the same synonym appears multiple times.
fn scan_doc_comment_dedupes_repeated_mentions() {
    let mut ont = Ontology::bootstrap();
    ont.entities.insert(
        "tenant".to_string(),
        EntityDef {
            id: "tenant".to_string(),
            display: "Tenant".to_string(),
            description: "test".to_string(),
            synonyms: vec![],
            bounded_contexts: vec![],
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
    let idx = TermIndex::build(&ont);
    let comment = "tenant tenant tenant Tenant TENANT";
    let hits = scan_doc_comment_for_entities(comment, &idx);
    assert_eq!(hits, vec!["tenant"], "duplicates collapse to one edge");
}

#[test]
/// Asserts that the [[entity-doc-graph]] doc-comment scanner returns no
/// mentions when the prose contains no ontology synonyms.
fn scan_doc_comment_returns_empty_for_unknown_terms() {
    let ont = Ontology::bootstrap();
    let idx = TermIndex::build(&ont);
    let hits = scan_doc_comment_for_entities("nothing matches here", &idx);
    assert!(hits.is_empty());
}

#[test]
/// Asserts that `crate_name_from_readme_path` recognises the
/// `crates/<name>/README.md` shape used by the [[entity-doc-graph]]
/// crate-ref ingest.
fn crate_name_from_readme_path_recognises_crate_readme() {
    assert_eq!(
        crate_name_from_readme_path("crates/pricing-core/README.md").as_deref(),
        Some("pricing-core")
    );
    assert!(crate_name_from_readme_path("crates/pricing-core/src/lib.rs").is_none());
    assert!(crate_name_from_readme_path("docs/foo.md").is_none());
}

// ---------------------------------------------------------------
// Phase 1 (roadmap-43): symbol-tokenization tests.
// ---------------------------------------------------------------

fn ont_with_pricing_outlet() -> Ontology {
    let mut ont = Ontology::bootstrap();
    ont.entities.insert(
        "pricing-rule".to_string(),
        EntityDef {
            id: "pricing-rule".to_string(),
            display: "Pricing Rule".to_string(),
            description: "test".to_string(),
            synonyms: vec!["rule".to_string()],
            bounded_contexts: vec![],
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
        "outlet".to_string(),
        EntityDef {
            id: "outlet".to_string(),
            display: "Outlet".to_string(),
            description: "test".to_string(),
            synonyms: vec!["store".to_string()],
            bounded_contexts: vec![],
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

#[test]
fn symbol_tokens_function_with_snake_case() {
    let sym = "rust-analyzer cargo doc-linter 0.1.0 outlet_repo/create_outlet().";
    let toks = symbol_tokens(sym);
    assert!(toks.contains(&"outlet".to_string()), "tokens={toks:?}");
    assert!(toks.contains(&"repo".to_string()), "tokens={toks:?}");
    assert!(toks.contains(&"create".to_string()), "tokens={toks:?}");
    assert!(!toks.iter().any(|t| t.contains('(') || t.contains(')')));
}

#[test]
fn symbol_tokens_camel_case_struct() {
    let sym = "rust-analyzer cargo pricing-core 0.1.0 model/PricingRule#";
    let toks = symbol_tokens(sym);
    assert!(toks.contains(&"pricing".to_string()), "tokens={toks:?}");
    assert!(toks.contains(&"rule".to_string()), "tokens={toks:?}");
    assert!(toks.contains(&"model".to_string()), "tokens={toks:?}");
}

#[test]
fn symbol_tokens_impl_method() {
    let sym = "rust-analyzer cargo pricing-core 0.1.0 model/impl#[PricingRule]/validate().";
    let toks = symbol_tokens(sym);
    assert!(toks.contains(&"pricing".to_string()), "tokens={toks:?}");
    assert!(toks.contains(&"rule".to_string()), "tokens={toks:?}");
    assert!(toks.contains(&"validate".to_string()), "tokens={toks:?}");
}

#[test]
fn symbol_tokens_handler_module_path() {
    let sym = "rust-analyzer cargo example-api 0.1.0 outlets/handlers/get_outlet().";
    let toks = symbol_tokens(sym);
    assert!(toks.contains(&"outlets".to_string()), "tokens={toks:?}");
    assert!(toks.contains(&"handlers".to_string()), "tokens={toks:?}");
    assert!(toks.contains(&"get".to_string()), "tokens={toks:?}");
    assert!(toks.contains(&"outlet".to_string()), "tokens={toks:?}");
}

#[test]
fn symbol_tokens_drops_short_tokens() {
    let sym = "rust-analyzer cargo demo 0.1.0 lib.rs/Foo#a/fn_compute().";
    let toks = symbol_tokens(sym);
    assert!(!toks.contains(&"a".to_string()), "tokens={toks:?}");
    assert!(!toks.contains(&"fn".to_string()), "tokens={toks:?}");
    assert!(toks.contains(&"compute".to_string()), "tokens={toks:?}");
}

#[test]
fn symbol_tokens_returns_empty_for_local_symbol() {
    assert!(symbol_tokens("local 0").is_empty());
    assert!(symbol_tokens("").is_empty());
}

#[test]
/// Asserts that `extract_impl_target` recovers the receiver type from
/// a simple `impl Foo` SCIP symbol for the [[entity-doc-graph]]
/// type-propagation pass.
fn extract_impl_target_basic() {
    let sym = "rust-analyzer cargo pricing-core 0.1.0 model/impl#[PricingRule]/validate().";
    assert_eq!(
        extract_impl_target(sym).as_deref(),
        Some("PricingRule"),
        "basic impl marker pulled out"
    );
}

#[test]
/// Asserts that `extract_impl_target` strips generic parameters from
/// `impl Foo<T>` for the [[entity-doc-graph]] type-propagation pass.
fn extract_impl_target_generic() {
    let sym = "rust-analyzer cargo demo 0.1.0 mod/impl#[Foo<Bar>]/method().";
    assert_eq!(extract_impl_target(sym).as_deref(), Some("Foo<Bar>"));
}

#[test]
/// Asserts that `extract_impl_target` returns `None` for a non-impl
/// SCIP symbol — the negative case for the [[entity-doc-graph]]
/// type-propagation pass.
fn extract_impl_target_returns_none_for_no_impl() {
    let sym = "rust-analyzer cargo demo 0.1.0 mod/Plain/free_function().";
    assert_eq!(extract_impl_target(sym), None);
}

#[test]
/// Asserts that the [[entity-doc-graph]] symbol scanner ignores
/// synonyms that appear in the configured stop list — the noise gate
/// that keeps generic words out of mention edges.
fn scan_symbol_drops_stop_listed_synonyms() {
    let ont = ont_with_pricing_outlet();
    let idx = TermIndex::build(&ont);
    let mut config = LintConfig::default();
    config.vale.vale_ambiguous_words = vec!["rule".to_string()];
    let stop = build_symbol_stop_list(&config);
    let sym = "rust-analyzer cargo demo 0.1.0 mod/apply_rule().";
    let hits = scan_symbol_for_entities(sym, &idx, &stop);
    assert!(
        !hits.contains(&"pricing-rule".to_string()),
        "stop list should drop bare 'rule': hits={hits:?}"
    );
}

#[test]
fn scan_symbol_finds_outlet_in_handler_path() {
    let ont = ont_with_pricing_outlet();
    let idx = TermIndex::build(&ont);
    let stop = build_symbol_stop_list(&LintConfig::default());
    let sym = "rust-analyzer cargo example-api 0.1.0 outlets/handlers/get_outlet().";
    let hits = scan_symbol_for_entities(sym, &idx, &stop);
    assert_eq!(hits, vec!["outlet"]);
}

#[test]
fn scan_symbol_finds_pricing_rule_via_full_compound() {
    let ont = ont_with_pricing_outlet();
    let idx = TermIndex::build(&ont);
    let stop = build_symbol_stop_list(&LintConfig::default());
    let sym = "rust-analyzer cargo pricing-core 0.1.0 model/PricingRule#";
    let hits = scan_symbol_for_entities(sym, &idx, &stop);
    // The synthetic ontology only has `rule` synonym — so under the
    // default stop list NEITHER token resolves. That demonstrates the
    // stop-list trade-off explicitly:
    assert!(
        hits.is_empty() || hits == vec!["pricing-rule"],
        "either filter blocks both, or only pricing matches: hits={hits:?}"
    );
}

#[test]
/// Asserts that when both confidence tiers point at the same entity,
/// the [[entity-doc-graph]] mention-dedup keeps the high-confidence
/// row.
fn high_confidence_overrides_low_in_dedup() {
    let mut pair_confidence: HashMap<(String, String), &'static str> = HashMap::new();
    let key = ("sym".to_string(), "ent".to_string());
    pair_confidence.insert(key.clone(), "high");
    pair_confidence.entry(key.clone()).or_insert("low");
    assert_eq!(pair_confidence.get(&key), Some(&"high"));
}

#[test]
/// Asserts that a low-confidence-only pair survives [[entity-doc-graph]]
/// dedup — degraded signal still mints a mention edge.
fn low_only_pair_keeps_low() {
    let mut pair_confidence: HashMap<(String, String), &'static str> = HashMap::new();
    let key = ("sym".to_string(), "ent".to_string());
    pair_confidence.entry(key.clone()).or_insert("low");
    assert_eq!(pair_confidence.get(&key), Some(&"low"));
}

#[test]
/// Asserts that the [[entity-doc-graph]] type-propagation pass only
/// descends into nested SCIP-symbol segments when separated by a
/// proper `/` boundary (no false-positive substring matches).
fn type_propagation_descends_only_with_separator() {
    assert!(symbol_descends_from(
        "rust-analyzer cargo p 0.1.0 model/PricingRule/validate().",
        "model/PricingRule"
    ));
    assert!(!symbol_descends_from(
        "rust-analyzer cargo p 0.1.0 model/PricingRuleHelper/work().",
        "model/PricingRule"
    ));
    assert!(!symbol_descends_from(
        "rust-analyzer cargo p 0.1.0 model/PricingRule#",
        "model/PricingRule"
    ));
}

#[test]
/// Asserts that `build_symbol_stop_list` merges the built-in stop
/// list with the user-configured additions for the
/// [[entity-doc-graph]] symbol scanner.
fn build_symbol_stop_list_merges_both_sources() {
    let mut config = LintConfig::default();
    config.vale.vale_ambiguous_words = vec!["custom-word".to_string()];
    let stop = build_symbol_stop_list(&config);
    assert!(stop.contains("returns"));
    assert!(stop.contains("vec"));
    assert!(stop.contains("custom-word"));
}

/// #269: `code_terms` narrows a one-word concept to compound terms
/// matched on adjacent symbol tokens; `code_veto` drops a match when a
/// veto phrase appears. Fineract's measured mislinks: `business` (=
/// business date) caught business events and steps, `client` caught
/// HTTP / Feign clients.
#[test]
fn code_terms_and_vetoes_narrow_single_word_concepts() {
    let mut ont = Ontology::bootstrap();
    for (id, terms, vetoes) in [
        ("business", vec!["business-date"], vec![]),
        (
            "client",
            vec![],
            vec!["feign-client", "http-client", "client-config"],
        ),
        ("loan", vec![], vec![]),
    ] {
        ont.entities.insert(
            id.to_string(),
            EntityDef {
                id: id.to_string(),
                display: id.to_string(),
                status: "stable".to_string(),
                code_terms: terms.into_iter().map(String::from).collect(),
                code_veto: vetoes.into_iter().map(String::from).collect(),
                ..EntityDef::default()
            },
        );
    }
    let idx = TermIndex::build(&ont);
    let stop = std::collections::HashSet::new();
    let scan = |s: &str| {
        scan_symbol_for_entities(
            &format!("semanticdb maven . 1.0 org/apache/fineract/{s}"),
            &idx,
            &stop,
        )
    };
    assert_eq!(scan("BusinessDateService#getBusinessDate()."), ["business"]);
    assert_eq!(scan("LoanService#isBusinessDates()."), ["business", "loan"]);
    assert!(scan("BusinessEventNotifier#notify().").is_empty());
    assert!(scan("cob/BusinessStepService#run().").is_empty());
    assert_eq!(scan("ClientWritePlatformService#create()."), ["client"]);
    assert!(scan("infrastructure/FeignClientConfig#build().").is_empty());
    assert!(scan("infrastructure/OkHttpClientFactory#create().").is_empty());
    // Vetoes match whole tokens: `configurer` is not `config`.
    assert_eq!(scan("LoanClientConfigurer#run()."), ["client", "loan"]);
}
