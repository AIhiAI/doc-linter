//! Round 3B / roadmap-43: SCIP ingest into the Function table + the
//! FUNCTION_DEFINED_IN, FUNCTION_MENTIONS, FUNCTION_BELONGS_TO edges
//! for the [[entity-doc-graph]] linter.
//!
//! The symbol-tokenization layer (`symbol_tokens`, descriptor-path
//! helpers, the camel/kebab/snake splitter) lives in the sibling
//! `symbols` module — adding a new tokenization rule lands there, not
//! here. This file owns only the the store write path.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::Path;

use crate::config::LintConfig;
use crate::disambiguation::TermIndex;
use crate::scip_ingest::{FunctionFact, ScipFacts};
use crate::store::projections::ScipIngestStats;

use super::symbols::{
    is_type_definition_kind, scan_doc_comment_for_entities, scan_symbol_for_entities,
    symbol_descriptor_prefix,
};

/// Dedupe SCIP facts by symbol and drop codegen-excluded files; the
/// returned stats carry the `codegen_excluded` count. Shared with the
/// SQLite writer (`store_sqlite::ingest::code`).
pub(crate) fn dedupe_function_facts<'a>(
    facts: &'a ScipFacts,
    config: &LintConfig,
    root: &Path,
) -> (Vec<&'a FunctionFact>, ScipIngestStats) {
    // Deduplicate by symbol — SCIP can emit the same SymbolInformation
    // across multiple Documents (once per file that references it). We
    // keep the first one we see (whichever document had the kind set).
    //
    // Phase 5c of roadmap-43: drop FunctionFacts whose source file
    // matches `coverage_codegen_exclude`. The exclusion is at INGEST
    // time — these symbols never get a Function node, never get a
    // FUNCTION_DEFINED_IN edge, never get a FUNCTION_MENTIONS edge.
    // The reach % denominator naturally shrinks; codegen sentinels
    // (frb_generated.rs, *_generated.rs, build.rs) stop bloating the
    // dark count and authored doc-comments would not be wiped on the
    // next codegen anyway.
    let mut seen_symbols: HashSet<String> = HashSet::new();
    let mut deduped: Vec<&FunctionFact> = Vec::with_capacity(facts.functions.len());
    let mut stats = ScipIngestStats::default();
    // Per-file header-sniff cache. SCIP can emit hundreds of
    // FunctionFacts pointing at the same source file (one per
    // public symbol); without this cache we'd re-open + re-read
    // each codegen file once per fact. Map: repo-relative path
    // → is_codegen.
    let mut codegen_cache: HashMap<String, bool> = HashMap::new();
    for f in &facts.functions {
        let is_codegen = if let Some(&cached) = codegen_cache.get(&f.file) {
            cached
        } else {
            let v = is_excluded_code_file(config, root, &f.file);
            codegen_cache.insert(f.file.clone(), v);
            v
        };
        if is_codegen {
            // Count once per fact even if duplicates would have
            // collapsed in the dedup pass — the metric is "facts
            // dropped", not "unique symbols dropped".
            stats.codegen_excluded += 1;
            continue;
        }
        if seen_symbols.insert(f.symbol.clone()) {
            deduped.push(f);
        }
    }
    (deduped, stats)
}

/// Stable kind label for the new `Type` node table. Returns `None`
/// when the FunctionFact's kind isn't a type definition — those rows
/// stay function-only. The labels match the issue #32 spec
/// (`struct` / `enum` / `trait` / `module` / `type_alias`) so that
/// `MATCH (t:Type {kind: 'struct'})` reads naturally.
pub(crate) fn type_kind_label(kind: crate::scip_ingest::FunctionKind) -> Option<&'static str> {
    use crate::scip_ingest::FunctionKind;
    match kind {
        FunctionKind::Struct => Some("struct"),
        FunctionKind::Enum => Some("enum"),
        FunctionKind::Trait => Some("trait"),
        FunctionKind::Module => Some("module"),
        FunctionKind::TypeAlias => Some("type_alias"),
        FunctionKind::Function | FunctionKind::Method | FunctionKind::Other => None,
    }
}

/// Roadmap issue #32 v4 (v0.4.0): the descriptor prefix of a method's
/// enclosing type. Drops the leaf segment from the method's descriptor
/// prefix — `model/PricingRule/validate` → `model/PricingRule`. Returns
/// `None` when the prefix is empty, has only one segment (the method
/// lives at the module root, no enclosing type), or `symbol_descriptor_prefix`
/// itself rejected the symbol.
///
/// A member written `Type#method().` (scip-java, scip-typescript) keeps
/// the `#` inside its last segment, so the parent is everything before
/// the last `#` — `org/x/Loans#submit` → `org/x/Loans`, the prefix the
/// `Loans#` type row carries.
///
/// Free function rather than a method on FunctionFact so the unit tests
/// can pin the string contract without spinning up the full ingest.
pub(crate) fn method_parent_prefix(symbol: &str) -> Option<String> {
    let prefix = symbol_descriptor_prefix(symbol)?;
    let leaf_start = prefix.rfind('/').map_or(0, |i| i + 1);
    if let Some(hash) = prefix[leaf_start..].rfind('#') {
        return Some(prefix[..leaf_start + hash].to_string());
    }
    let (parent, _leaf) = prefix.rsplit_once('/')?;
    if parent.is_empty() {
        None
    } else {
        Some(parent.to_string())
    }
}

/// True when code in `file` (repo-relative) stays out of the graph: it
/// sits in a skip dir (a nested submodule or vendored tree is another
/// repo's, even when a C# indexer reaches it through project
/// references), it matches `coverage_codegen_exclude`, or it is generated
/// output (see [`is_generated_path`]) unless `include_generated` is set.
pub(crate) fn is_excluded_code_file(config: &LintConfig, root: &Path, file: &str) -> bool {
    let rel = Path::new(file);
    let in_skip_dir = rel.parent().is_some_and(|p| {
        p.components()
            .any(|c| config.skip_dirs.iter().any(|d| c.as_os_str() == d.as_str()))
    });
    in_skip_dir
        || config.is_codegen_excluded_with_header(rel, &root.join(rel))
        || config.is_generated_path(root, file)
}

/// Accumulate every (function, entity) mention pair the three Phase-1
/// passes produce. High-confidence (doc-comment) wins ties; low
/// (symbol-path + type-propagation) fills in where doc-comment scan
/// found nothing.
///
/// Sub-features (mirroring roadmap-43):
///   1a: scan the SCIP symbol path (function name + module
///       segments) — every Function fact, low-confidence.
///   1b: when a symbol contains an `impl#[...]` marker, the
///       bracketed target type's tokens propagate to that method
///       — covered by 1a because `symbol_tokens` descends into the
///       bracket payload.
///   1c: when a struct/enum/trait fact reaches an entity by name,
///       that entity propagates to every method whose symbol
///       descends from the type's path.
pub(crate) fn build_mention_pairs(
    deduped: &[&FunctionFact],
    term_index: &TermIndex<'_>,
    symbol_stop: &HashSet<String>,
    self_entity: Option<&str>,
) -> HashMap<(String, String), &'static str> {
    let mut pair_confidence: HashMap<(String, String), &'static str> = HashMap::new();

    // 4a. Doc-comment scan — high confidence.
    for f in deduped {
        if f.doc_comment.is_empty() {
            continue;
        }
        for ent_id in scan_doc_comment_for_entities(&f.doc_comment, term_index) {
            pair_confidence.insert((f.symbol.clone(), ent_id), "high");
        }
    }

    // 4b. Symbol-path scan — low confidence (1a + 1b combined since
    //     `symbol_tokens` walks the impl-marker payload).
    for f in deduped {
        for ent_id in scan_symbol_for_entities(&f.symbol, term_index, symbol_stop) {
            pair_confidence
                .entry((f.symbol.clone(), ent_id))
                .or_insert("low");
        }
    }

    // 4c. Type → method propagation. Build a map from the SCIP path
    //     prefix of each struct/enum/trait/type-alias fact to the
    //     entities its own name resolved against; then sweep every
    //     fact whose symbol descends from that prefix and inherit.
    let mut type_entities: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for f in deduped {
        if !is_type_definition_kind(f.kind) {
            continue;
        }
        let hits = scan_symbol_for_entities(&f.symbol, term_index, symbol_stop);
        if hits.is_empty() {
            continue;
        }
        let Some(prefix) = symbol_descriptor_prefix(&f.symbol) else {
            continue;
        };
        type_entities.entry(prefix).or_default().extend(hits);
    }
    // A symbol descends from a type when the type's prefix is one of
    // its own prefix's proper `/`-ancestors (`symbol_descends_from`), so
    // look those few ancestors up instead of testing every type against
    // every symbol (66 concepts × 35k functions took minutes, #270).
    if !type_entities.is_empty() {
        for f in deduped {
            let Some(prefix) = symbol_descriptor_prefix(&f.symbol) else {
                continue;
            };
            for (i, _) in prefix.match_indices('/') {
                for ent_id in type_entities.get(&prefix[..i]).into_iter().flatten() {
                    pair_confidence
                        .entry((f.symbol.clone(), ent_id.clone()))
                        .or_insert("low");
                }
            }
        }
    }

    // Roadmap issue #23: demote project-name self-matches. The
    // project's own entity (e.g. `myapp` for the myapp
    // repo) shows up in casual prose throughout doc-comments —
    // PEP-257 sentences regularly name the product. Without this
    // pass every Function lands a `high`-confidence edge against
    // the self-entity, drowning the actual matches.
    //
    // Two-stage demote:
    //   1. For every function that ALSO matches a different
    //      entity, drop the self-entity edge entirely (the more-
    //      specific entity is the real signal).
    //   2. For every function whose only match is the self-entity,
    //      force confidence to "low" (the symbol-path bucket).
    if let Some(self_id) = self_entity {
        let mut symbols_with_other_match: HashSet<String> = HashSet::new();
        for (sym, ent) in pair_confidence.keys() {
            if ent != self_id {
                symbols_with_other_match.insert(sym.clone());
            }
        }
        let keys: Vec<(String, String)> = pair_confidence.keys().cloned().collect();
        for (sym, ent) in keys {
            if ent != self_id {
                continue;
            }
            if symbols_with_other_match.contains(&sym) {
                pair_confidence.remove(&(sym, ent));
            } else {
                pair_confidence.insert((sym, ent), "low");
            }
        }
    }

    pair_confidence
}

/// Roadmap issue #32 v9 (v0.4.0): shape of `partition_mention_pairs`
/// output — function-side pairs in `.0`, type-side pairs in `.1`.
/// Both halves share the (symbol, entity-id) → confidence shape
/// `build_mention_pairs` produces.
pub(crate) type MentionPairs = HashMap<(String, String), &'static str>;

/// Roadmap issue #32 v9 (v0.4.0): split the shared
/// `build_mention_pairs` output into function-side and type-side
/// halves. Routing key is the symbol's kind looked up in a fresh
/// `symbol → kind` map built from deduped (the pair map only
/// carries symbol strings, not the originating fact). Symbols not
/// in `deduped` — uncommon, would mean the propagator emitted a
/// stale prefix — bias toward function so we don't lose edges.
pub(crate) fn partition_mention_pairs(
    deduped: &[&FunctionFact],
    pair_confidence: MentionPairs,
) -> (MentionPairs, MentionPairs) {
    let mut symbol_kind: HashMap<&str, crate::scip_ingest::FunctionKind> =
        HashMap::with_capacity(deduped.len());
    for f in deduped {
        symbol_kind.insert(f.symbol.as_str(), f.kind);
    }
    let mut function_pairs: MentionPairs = HashMap::new();
    let mut type_pairs: MentionPairs = HashMap::new();
    for (key, conf) in pair_confidence {
        let is_type = symbol_kind
            .get(key.0.as_str())
            .copied()
            .is_some_and(|k| type_kind_label(k).is_some());
        if is_type {
            type_pairs.insert(key, conf);
        } else {
            function_pairs.insert(key, conf);
        }
    }
    (function_pairs, type_pairs)
}

/// Bug #150: entity_class values that are excluded from god-node
/// ranking. Language and infrastructure entities (e.g. `python`,
/// `kubernetes`) accumulate `FUNCTION_MENTIONS` from every function
/// in a repo and would otherwise dominate the architectural
/// ranking — saturating mean+stddev and flagging genuine domain
/// entities as below-threshold. Mention count is still recorded
/// for these entities; only the `is_god_node` flag is forced false.
///
/// Kept here rather than re-derived from the schema so the
/// well-known classifier list lives next to the consumer that
/// gates on it.
const NON_RANKING_ENTITY_CLASSES: &[&str] = &["language", "infrastructure"];

/// Pure god-node decision: given `(id, mention_count, entity_class)`
/// rows, return `(id, mention_count, is_god_node)` decisions.
///
/// Threshold rule: `mention_count > mean + 2*stddev` over the rankable
/// population (population stddev — divide by N — since this is a
/// descriptive statistic on the whole corpus, not a sample).
///
/// Bug #150 carve-out: entities whose `entity_class` is in
/// [`NON_RANKING_ENTITY_CLASSES`] are excluded both from the
/// threshold statistics and from `is_god_node` assignment. Their
/// `mention_count` is still returned unchanged so it lands on disk.
pub(crate) fn classify_god_nodes(rows: &[(String, i64, String)]) -> Vec<(String, i64, bool)> {
    let rankable: Vec<&(String, i64, String)> = rows
        .iter()
        .filter(|(_, _, class)| !NON_RANKING_ENTITY_CLASSES.contains(&class.as_str()))
        .collect();
    let threshold = if rankable.is_empty() {
        f64::INFINITY
    } else {
        let n = rankable.len() as f64;
        let mean = rankable.iter().map(|c| c.1 as f64).sum::<f64>() / n;
        let variance = rankable
            .iter()
            .map(|c| {
                let d = c.1 as f64 - mean;
                d * d
            })
            .sum::<f64>()
            / n;
        let stddev = variance.sqrt();
        2.0f64.mul_add(stddev, mean)
    };
    rows.iter()
        .map(|(id, count, class)| {
            let is_rankable = !NON_RANKING_ENTITY_CLASSES.contains(&class.as_str());
            let is_god = is_rankable && (*count as f64) > threshold && *count > 0;
            (id.clone(), *count, is_god)
        })
        .collect()
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "tests panic on broken invariants — that is the point of a test"
)]
mod tests {
    use super::{classify_god_nodes, method_parent_prefix};

    /// Gitignored output, `generated/` dirs and `*.gen.*` files stay out of
    /// the graph; `include_generated = true` brings them back.
    #[test]
    fn generated_and_gitignored_paths_are_excluded() {
        let root = std::env::temp_dir().join(format!("dl-genpath-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("m/out")).unwrap();
        std::fs::create_dir_all(root.join("m/src")).unwrap();
        std::fs::write(root.join("m/src/Api.java"), "class A {}").unwrap();
        std::fs::write(root.join(".gitignore"), "out/\n").unwrap();
        let init = std::process::Command::new("git")
            .args(["init", "-q"])
            .current_dir(&root)
            .status()
            .unwrap();
        assert!(init.success());
        let mut cfg = crate::config::LintConfig::default();
        for (f, want) in [
            ("m/out/Api.java", true),
            ("m/generated/Api.java", true),
            ("m/Api.gen.ts", true),
            ("m/src/Api.java", false),
        ] {
            assert_eq!(super::is_excluded_code_file(&cfg, &root, f), want, "{f}");
        }
        cfg.coverage.include_generated = true;
        assert!(!super::is_excluded_code_file(&cfg, &root, "m/out/Api.java"));
        let _ = std::fs::remove_dir_all(&root);
    }

    fn row(id: &str, count: i64, class: &str) -> (String, i64, String) {
        (id.to_string(), count, class.to_string())
    }

    /// A type's concept reaches every symbol under the type's path (any
    /// depth) and no unrelated one; `PricingRuleSet` matches on its own
    /// `pricing` token. Pinned across the #270 rewrite of the type →
    /// method propagation from a pairwise scan to an ancestor lookup.
    #[test]
    fn build_mention_pairs_propagates_type_entities_to_descendants() {
        use crate::ontology::{EntityDef, Ontology};
        use crate::scip_ingest::{FunctionFact, FunctionKind};
        let mut ont = Ontology::bootstrap();
        ont.entities.insert(
            "pricing".to_string(),
            EntityDef {
                id: "pricing".to_string(),
                display: "Pricing".to_string(),
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
        let idx = crate::disambiguation::TermIndex::build(&ont);
        let fact = |symbol: &str, kind| FunctionFact {
            symbol: format!("rust-analyzer cargo core 1.0.0 {symbol}"),
            crate_name: String::new(),
            file: String::new(),
            line: 0,
            doc_comment: String::new(),
            kind,
            language: String::new(),
            signature: String::new(),
            body_excerpt: String::new(),
            last_touched: String::new(),
            generated: false,
        };
        let facts = [
            fact("src/lib.rs/PricingRule#", FunctionKind::Struct),
            fact("src/lib.rs/PricingRule/validate().", FunctionKind::Method),
            fact("src/lib.rs/PricingRule/inner/deep().", FunctionKind::Method),
            fact("src/lib.rs/Other/run().", FunctionKind::Method),
            fact("src/lib.rs/PricingRuleSet/size().", FunctionKind::Method),
        ];
        let refs: Vec<&FunctionFact> = facts.iter().collect();
        let pairs =
            super::build_mention_pairs(&refs, &idx, &std::collections::HashSet::new(), None);
        let mut got: Vec<&str> = pairs
            .keys()
            .filter(|(_, e)| e == "pricing")
            .map(|(s, _)| s.rsplit(' ').next().unwrap())
            .collect();
        got.sort_unstable();
        assert_eq!(
            got,
            [
                "src/lib.rs/PricingRule#",
                "src/lib.rs/PricingRule/inner/deep().",
                "src/lib.rs/PricingRule/validate().",
                "src/lib.rs/PricingRuleSet/size().",
            ]
        );
    }

    // --- Roadmap issue #32 v4 (v0.4.0): method_parent_prefix --------

    #[test]
    fn method_parent_prefix_splits_hash_members() {
        // scip-java: the method's parent prefix equals the class row's
        // descriptor prefix (`Loans#` → `org/x/Loans`), nested classes too.
        assert_eq!(
            method_parent_prefix("semanticdb maven . 1.0 org/x/Loans#submit(+1).").as_deref(),
            Some("org/x/Loans")
        );
        assert_eq!(
            super::symbol_descriptor_prefix("semanticdb maven . 1.0 org/x/Loans#").as_deref(),
            Some("org/x/Loans")
        );
        assert_eq!(
            method_parent_prefix("semanticdb maven . 1.0 org/x/Outer#Inner#run().").as_deref(),
            super::symbol_descriptor_prefix("semanticdb maven . 1.0 org/x/Outer#Inner#").as_deref()
        );
    }

    #[test]
    fn method_parent_prefix_strips_leaf_segment() {
        // The canonical case: a method on a struct/trait whose
        // descriptor prefix is `model/PricingRule/validate` collapses
        // to the enclosing type's prefix `model/PricingRule`.
        assert_eq!(
            method_parent_prefix("rust-analyzer cargo p 0.1.0 model/PricingRule/validate().")
                .as_deref(),
            Some("model/PricingRule")
        );
    }

    #[test]
    fn method_parent_prefix_handles_nested_module() {
        // Three segments deep — only the leaf is dropped.
        assert_eq!(
            method_parent_prefix("rust-analyzer cargo p 0.1.0 services/pricing/Engine/run().")
                .as_deref(),
            Some("services/pricing/Engine")
        );
    }

    #[test]
    fn method_parent_prefix_returns_none_for_root_level_function() {
        // Free function at module root — no `/` to strip, so there's
        // no enclosing type and METHOD_OF would have nothing to bind
        // against.
        assert_eq!(
            method_parent_prefix("rust-analyzer cargo p 0.1.0 free_fn()."),
            None
        );
    }

    #[test]
    fn method_parent_prefix_returns_none_for_malformed_symbol() {
        assert_eq!(method_parent_prefix("not-a-scip-symbol"), None);
        assert_eq!(method_parent_prefix(""), None);
    }

    /// Sanity: a domain entity whose count is far above the
    /// rest-of-population mean+2*stddev gets `is_god_node=true`.
    /// Uses a 10-entity fixture because mean+2*stddev with a single
    /// outlier on N=5 lands at exactly the outlier's value (formula
    /// is self-tight); N≥10 with proportionally similar spread is
    /// the smallest fixture that triggers god-node assignment.
    #[test]
    fn classify_flags_clear_outlier_as_god_node() {
        let rows = vec![
            row("api", 100, ""),
            row("auth", 1, ""),
            row("billing", 1, ""),
            row("scheduler", 1, ""),
            row("notifications", 1, ""),
            row("audit", 1, ""),
            row("rbac", 1, ""),
            row("search", 1, ""),
            row("cache", 1, ""),
            row("metrics", 1, ""),
        ];
        let out = classify_god_nodes(&rows);
        let api = out.iter().find(|(id, _, _)| id == "api").unwrap();
        assert!(api.2, "api should be a god node; got: {out:?}");
        for (id, _, is_god) in &out {
            if id != "api" {
                assert!(!is_god, "{id} should not be a god node; got: {out:?}");
            }
        }
    }

    /// Bug #150 regression: a `language`-class entity that would
    /// otherwise dominate the ranking is excluded from
    /// `is_god_node` even when its count blows past the threshold,
    /// AND its count is excluded from the threshold so genuine
    /// architectural entities still rank correctly. Counts mirror
    /// the observed example-app distribution from the issue
    /// (python=1017, the real architectural entity well behind
    /// it).
    #[test]
    fn classify_excludes_language_class_from_god_node() {
        let rows = vec![
            // python: language-class, would dominate without the gate.
            row("python", 1_017, "language"),
            // api: the real architectural god node — only stands out
            // once python's saturation is removed from the threshold.
            row("api", 240, ""),
            row("auth", 30, ""),
            row("billing", 20, ""),
            row("scheduler", 10, ""),
            row("notifications", 10, ""),
            row("audit", 10, ""),
            row("rbac", 10, ""),
            row("search", 10, ""),
            row("cache", 10, ""),
        ];
        let out = classify_god_nodes(&rows);

        let python = out.iter().find(|(id, _, _)| id == "python").unwrap();
        assert_eq!(
            python.1, 1_017,
            "mention count must still be persisted: {out:?}"
        );
        assert!(
            !python.2,
            "language-class entity must NEVER be a god node: {out:?}"
        );

        let api = out.iter().find(|(id, _, _)| id == "api").unwrap();
        assert!(
            api.2,
            "the real architectural god node should surface once \
             the language entity is excluded from the threshold: {out:?}",
        );
    }

    /// `infrastructure` is the second well-known carve-out class.
    /// Same exclusion as `language` — a `kubernetes` entity
    /// referenced from every deployment-related function would
    /// otherwise saturate ranking.
    #[test]
    fn classify_excludes_infrastructure_class_from_god_node() {
        let rows = vec![row("kubernetes", 500, "infrastructure"), row("api", 1, "")];
        let out = classify_god_nodes(&rows);
        let k8s = out.iter().find(|(id, _, _)| id == "kubernetes").unwrap();
        assert!(
            !k8s.2,
            "infrastructure-class entity must NEVER be a god node: {out:?}",
        );
        assert_eq!(k8s.1, 500, "count still persisted: {out:?}");
    }

    /// Edge case: when every entity is non-ranking the rankable
    /// population is empty; threshold is `+inf` so nothing is
    /// flagged. Avoids a NaN from dividing by zero.
    #[test]
    fn classify_handles_all_non_ranking_population() {
        let rows = vec![
            row("python", 1_017, "language"),
            row("kubernetes", 500, "infrastructure"),
        ];
        let out = classify_god_nodes(&rows);
        assert!(
            out.iter().all(|(_, _, is_god)| !is_god),
            "no god nodes when no rankable entities: {out:?}",
        );
    }

    /// An unrecognised `entity_class` string (e.g. `domain`,
    /// future taxonomy) participates in ranking — only the
    /// well-known carve-out classes are excluded.
    #[test]
    fn classify_treats_unknown_class_as_rankable() {
        let rows = vec![
            row("api", 100, "domain"),
            row("auth", 1, ""),
            row("billing", 1, "domain"),
            row("scheduler", 1, ""),
            row("notifications", 1, ""),
            row("audit", 1, ""),
            row("rbac", 1, ""),
            row("search", 1, ""),
            row("cache", 1, ""),
            row("metrics", 1, ""),
        ];
        let out = classify_god_nodes(&rows);
        let api = out.iter().find(|(id, _, _)| id == "api").unwrap();
        assert!(
            api.2,
            "entity_class=domain (or any non-carve-out value) is \
             still rankable: {out:?}",
        );
    }

    /// Zero-mention entities never become god nodes regardless of
    /// the threshold (count > 0 guard).
    #[test]
    fn classify_skips_zero_mention_entities() {
        let rows = vec![row("ghost", 0, ""), row("real", 0, "")];
        let out = classify_god_nodes(&rows);
        assert!(out.iter().all(|(_, _, is_god)| !is_god), "{out:?}");
    }
}
