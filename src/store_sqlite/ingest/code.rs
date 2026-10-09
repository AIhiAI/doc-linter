//! SQLite twin of `store::code_ingest`: SCIP facts -> Function /
//! Type / Field rows and their edges. Dedupe, mention scanning, kind
//! labels and god-node classification are the the store module's own
//! functions; only the writes and the derive queries are SQL.

use std::collections::{HashMap, HashSet};
use std::path::Path;

use anyhow::{Context, Result};

use super::{insert_edges, insert_pairs, int, s, upsert};
use crate::config::LintConfig;
use crate::disambiguation::TermIndex;
use crate::scip_ingest::{FunctionFact, FunctionKind, ScipFacts};
use crate::store::code_ingest::{
    build_mention_pairs, classify_god_nodes, dedupe_function_facts, method_parent_prefix,
    partition_mention_pairs, type_kind_label, MentionPairs,
};
use crate::store::projections::ScipIngestStats;
use crate::store::symbols::{
    build_symbol_stop_list, crate_name_from_readme_path, symbol_descriptor_prefix,
};
use crate::store::{StoreRead, Value};
use crate::store_sqlite::SqliteDb;

/// Tables wiped before a SCIP (re)ingest: a detaching delete on
/// Function plus the Type / Field rows a Full reset would have dropped.
const SCIP_TABLES: &[&str] = &[
    "Function",
    "Type",
    "Field",
    "CALLS",
    "METHOD_OF",
    "USES_TYPE",
    "REFERENCES",
    "IMPLEMENTS",
    "EXTENDS",
    "FUNCTION_DEFINED_IN",
    "FUNCTION_MENTIONS",
    "FUNCTION_BELONGS_TO",
    "TYPE_DEFINED_IN",
    "TYPE_MENTIONS",
    "TYPE_BELONGS_TO",
    "ENTITY_CALLS",
    "DEFINED_IN_FILE",
    "TEST_FOR",
    "ENDPOINT_HANDLED_BY",
];

/// Wipe and repopulate Function / Type / Field and every edge derived
/// from SCIP facts, in one transaction.
pub fn ingest_scip(
    db: &SqliteDb,
    facts: &ScipFacts,
    term_index: &TermIndex,
    config: &LintConfig,
    root: &Path,
) -> Result<ScipIngestStats> {
    db.transaction(|db| {
        for t in SCIP_TABLES {
            db.execute_batch(&format!("DELETE FROM \"{t}\";"))?;
        }
        ingest_rows(db, facts, term_index, config, root)
    })
}

fn ingest_rows(
    db: &SqliteDb,
    facts: &ScipFacts,
    term_index: &TermIndex,
    config: &LintConfig,
    root: &Path,
) -> Result<ScipIngestStats> {
    let symbol_stop = build_symbol_stop_list(config);
    let crate_doc_ids = crate_doc_id_map(db)?;
    let repo_id = root.canonicalize().map_or_else(
        |_| root.to_string_lossy().to_string(),
        |p| p.to_string_lossy().to_string(),
    );
    let (deduped, mut stats) = dedupe_function_facts(facts, config, root);

    // Function and Type share their metadata columns.
    const COMMON: [&str; 10] = [
        "symbol",
        "crate",
        "file",
        "line",
        "doc_comment",
        "language",
        "signature",
        "body_excerpt",
        "last_touched",
        "repo_id",
    ];
    let common = |f: &FunctionFact| {
        vec![
            s(&f.symbol),
            s(&f.crate_name),
            s(&f.file),
            int(f.line),
            s(&f.doc_comment),
            s(&f.language),
            s(&f.signature),
            s(&f.body_excerpt),
            s(&f.last_touched),
            s(&repo_id),
        ]
    };
    let fn_cols: Vec<&str> = COMMON.iter().copied().chain(["generated"]).collect();
    stats.functions += upsert(
        db,
        "Function",
        "symbol",
        &fn_cols,
        deduped
            .iter()
            .filter(|f| type_kind_label(f.kind).is_none())
            .map(|f| {
                let mut r = common(f);
                r.push(Value::Bool(f.generated));
                r
            }),
    )
    .context("insert Function rows")?;
    let type_cols: Vec<&str> = COMMON.iter().copied().chain(["kind"]).collect();
    stats.types += upsert(
        db,
        "Type",
        "symbol",
        &type_cols,
        deduped.iter().filter_map(|f| {
            let kind = type_kind_label(f.kind)?;
            let mut r = common(f);
            r.push(s(kind));
            Some(r)
        }),
    )
    .context("insert Type rows")?;

    insert_method_of(db, &deduped, &mut stats)?;

    stats.uses_type_edges += insert_pairs(
        db,
        "USES_TYPE",
        ("Function", "symbol"),
        ("Type", "symbol"),
        facts
            .uses_types
            .iter()
            .map(|f| (f.function_symbol.clone(), f.type_symbol.clone())),
    )?;

    // Fields (same file exclusions as functions) and REFERENCES edges.
    let mut seen: HashSet<&str> = HashSet::new();
    let mut excluded: HashMap<&str, bool> = HashMap::new();
    let mut field_rows = Vec::new();
    for f in &facts.fields {
        let skip = *excluded.entry(f.file.as_str()).or_insert_with(|| {
            crate::store::code_ingest::is_excluded_code_file(config, root, &f.file)
        });
        if skip || !seen.insert(f.symbol.as_str()) {
            continue;
        }
        field_rows.push(vec![
            s(&f.symbol),
            s(&f.name),
            s(&f.owner),
            s(&f.file),
            int(f.line),
            s(&f.doc_comment),
            s(&f.language),
        ]);
    }
    stats.fields += upsert(
        db,
        "Field",
        "symbol",
        &[
            "symbol",
            "name",
            "owner",
            "file",
            "line",
            "doc_comment",
            "language",
        ],
        field_rows,
    )?;
    stats.references_edges += insert_edges(
        db,
        "REFERENCES",
        ("Function", "symbol"),
        ("Field", "symbol"),
        &["source_file", "source_line"],
        facts.references.iter().map(|r| {
            vec![
                s(&r.function_symbol),
                s(&r.field_symbol),
                s(&r.source_file),
                int(r.source_line),
            ]
        }),
    )?;

    stats.implements_edges += insert_pairs(
        db,
        "IMPLEMENTS",
        ("Type", "symbol"),
        ("Type", "symbol"),
        facts
            .implements
            .iter()
            .map(|f| (f.from_type.clone(), f.to_type.clone())),
    )?;
    stats.extends_edges += insert_pairs(
        db,
        "EXTENDS",
        ("Type", "symbol"),
        ("Type", "symbol"),
        facts
            .extends
            .iter()
            .map(|f| (f.from_type.clone(), f.to_type.clone())),
    )?;

    cross_bucket(
        db,
        &deduped,
        &crate_doc_ids,
        term_index,
        config,
        &symbol_stop,
        &mut stats,
    )?;

    // CALLS, deduped on (caller, callee, file, line) in sorted order.
    let mut calls: Vec<_> = facts.calls.iter().collect();
    calls.sort_by(|a, b| {
        (
            &a.caller_symbol,
            &a.callee_symbol,
            &a.source_file,
            a.source_line,
        )
            .cmp(&(
                &b.caller_symbol,
                &b.callee_symbol,
                &b.source_file,
                b.source_line,
            ))
    });
    let mut seen_calls = HashSet::new();
    stats.calls_edges += insert_edges(
        db,
        "CALLS",
        ("Function", "symbol"),
        ("Function", "symbol"),
        &["source_file", "source_line", "dispatch"],
        calls
            .into_iter()
            .filter(|c| {
                seen_calls.insert((
                    c.caller_symbol.as_str(),
                    c.callee_symbol.as_str(),
                    c.source_file.as_str(),
                    c.source_line,
                ))
            })
            .map(|c| {
                vec![
                    s(&c.caller_symbol),
                    s(&c.callee_symbol),
                    s(&c.source_file),
                    int(c.source_line),
                    Value::Bool(c.dispatch),
                ]
            }),
    )?;

    derive_entity_calls(db, &mut stats)?;
    derive_god_nodes(db, &mut stats)?;
    Ok(stats)
}

/// SCIP cache-hit twin of `rederive_cross_bucket_edges_from_the store`: the
/// Function / Type rows and everything between code nodes were kept; the
/// vault was rebuilt, so redraw only the edges that point into it
/// (DEFINED_IN, MENTIONS, BELONGS_TO), then the entity-level derivations
/// that read them (ENTITY_CALLS, mention counts, god nodes).
/// ENDPOINT_TOUCHES_ENTITY is refilled by the endpoint pass that follows.
pub fn rederive_cross_bucket(
    db: &SqliteDb,
    term_index: &TermIndex,
    config: &LintConfig,
) -> Result<ScipIngestStats> {
    db.transaction(|db| {
        let facts = read_function_facts(db)?;
        let deduped: Vec<&FunctionFact> = facts.iter().collect();
        let types = deduped
            .iter()
            .filter(|f| type_kind_label(f.kind).is_some())
            .count();
        let mut stats = ScipIngestStats {
            functions: deduped.len() - types,
            types,
            ..ScipIngestStats::default()
        };
        let crate_doc_ids = crate_doc_id_map(db)?;
        let symbol_stop = build_symbol_stop_list(config);
        cross_bucket(
            db,
            &deduped,
            &crate_doc_ids,
            term_index,
            config,
            &symbol_stop,
            &mut stats,
        )?;
        derive_entity_calls(db, &mut stats)?;
        derive_god_nodes(db, &mut stats)?;
        Ok(stats)
    })
}

/// Function and Type rows read back as facts (`kind` recovered for types,
/// every function row is callable), as `read_function_facts_from_the store`.
pub fn read_function_facts(db: &SqliteDb) -> Result<Vec<FunctionFact>> {
    let mut out = Vec::new();
    for r in db.query(
        "SELECT symbol, crate, file, line, doc_comment, language, signature, \
         body_excerpt, last_touched, generated FROM Function ORDER BY symbol",
        &[],
    )? {
        out.push(FunctionFact {
            symbol: r[0].str_or_empty(),
            crate_name: r[1].str_or_empty(),
            file: r[2].str_or_empty(),
            line: r[3].as_u64_or_zero() as u32,
            doc_comment: r[4].str_or_empty(),
            kind: FunctionKind::Function,
            language: r[5].str_or_empty(),
            signature: r[6].str_or_empty(),
            body_excerpt: r[7].str_or_empty(),
            last_touched: r[8].str_or_empty(),
            generated: r[9].as_i64() == Some(1),
        });
    }
    for r in db.query(
        "SELECT symbol, kind, crate, file, line, doc_comment, language, signature, \
         body_excerpt, last_touched FROM Type ORDER BY symbol",
        &[],
    )? {
        out.push(FunctionFact {
            symbol: r[0].str_or_empty(),
            kind: match r[1].str_or_empty().as_str() {
                "struct" => FunctionKind::Struct,
                "enum" => FunctionKind::Enum,
                "trait" => FunctionKind::Trait,
                "module" => FunctionKind::Module,
                "type_alias" => FunctionKind::TypeAlias,
                _ => FunctionKind::Other,
            },
            crate_name: r[2].str_or_empty(),
            file: r[3].str_or_empty(),
            line: r[4].as_u64_or_zero() as u32,
            doc_comment: r[5].str_or_empty(),
            language: r[6].str_or_empty(),
            signature: r[7].str_or_empty(),
            body_excerpt: r[8].str_or_empty(),
            last_touched: r[9].str_or_empty(),
            generated: false,
        });
    }
    out.retain(|f| !f.symbol.is_empty());
    Ok(out)
}

/// DEFINED_IN, MENTIONS and BELONGS_TO edges: the ones that point at
/// vault nodes, so the cache-hit path can redraw them alone.
fn cross_bucket(
    db: &SqliteDb,
    deduped: &[&FunctionFact],
    crate_doc_ids: &HashMap<String, String>,
    term_index: &TermIndex,
    config: &LintConfig,
    symbol_stop: &HashSet<String>,
    stats: &mut ScipIngestStats,
) -> Result<()> {
    let defined_in = |want_type: bool| {
        deduped
            .iter()
            .filter(|f| type_kind_label(f.kind).is_some() == want_type && !f.crate_name.is_empty())
            .filter_map(|f| Some((f.symbol.clone(), crate_doc_ids.get(&f.crate_name)?.clone())))
            .collect::<Vec<_>>()
    };
    stats.defined_in_edges += insert_pairs(
        db,
        "FUNCTION_DEFINED_IN",
        ("Function", "symbol"),
        ("Doc", "id"),
        defined_in(false),
    )?;
    stats.type_defined_in_edges += insert_pairs(
        db,
        "TYPE_DEFINED_IN",
        ("Type", "symbol"),
        ("Doc", "id"),
        defined_in(true),
    )?;

    let self_entity = config.ontology.homepage_root_entity.as_deref();
    let pairs = build_mention_pairs(deduped, term_index, symbol_stop, self_entity);
    let (function_pairs, type_pairs) = partition_mention_pairs(deduped, pairs);
    let (hi, lo) = insert_mentions(db, "FUNCTION_MENTIONS", "Function", function_pairs)?;
    stats.mentions_edges_high += hi;
    stats.mentions_edges_low += lo;
    stats.mentions_edges += hi + lo;
    let (hi, lo) = insert_mentions(db, "TYPE_MENTIONS", "Type", type_pairs)?;
    stats.type_mentions_edges_high += hi;
    stats.type_mentions_edges_low += lo;
    stats.type_mentions_edges += hi + lo;

    // BELONGS_TO: entity `source_modules` globs against the symbol's file.
    let mut entity_modules: Vec<(String, globset::GlobSet)> = Vec::new();
    let mut patterns: HashMap<String, Vec<String>> = HashMap::new();
    for r in db.query("SELECT entity_id, value FROM entity_source_modules", &[])? {
        patterns
            .entry(r[0].str_or_empty())
            .or_default()
            .push(r[1].str_or_empty());
    }
    let mut ids: Vec<_> = patterns.keys().cloned().collect();
    ids.sort();
    for id in ids {
        let mut b = globset::GlobSetBuilder::new();
        for p in &patterns[&id] {
            if let Ok(g) = globset::Glob::new(p) {
                b.add(g);
            }
        }
        if let Ok(set) = b.build() {
            entity_modules.push((id, set));
        }
    }
    if !entity_modules.is_empty() {
        let belongs = |want_type: bool| {
            let mut rows = Vec::new();
            for f in deduped
                .iter()
                .filter(|f| type_kind_label(f.kind).is_some() == want_type)
            {
                for (ent, set) in &entity_modules {
                    if set.is_match(Path::new(&f.file)) {
                        rows.push((f.symbol.clone(), ent.clone()));
                    }
                }
            }
            rows
        };
        stats.belongs_to_edges += insert_pairs(
            db,
            "FUNCTION_BELONGS_TO",
            ("Function", "symbol"),
            ("Entity", "id"),
            belongs(false),
        )?;
        stats.type_belongs_to_edges += insert_pairs(
            db,
            "TYPE_BELONGS_TO",
            ("Type", "symbol"),
            ("Entity", "id"),
            belongs(true),
        )?;
    }
    Ok(())
}

/// Insert `high` then `low` mention edges; returns (high, low) created.
fn insert_mentions(
    db: &SqliteDb,
    rel: &str,
    from_table: &str,
    pairs: MentionPairs,
) -> Result<(usize, usize)> {
    let mut sorted: Vec<_> = pairs.into_iter().collect();
    sorted.sort_by(|a, b| a.0.cmp(&b.0));
    let mut out = [0, 0];
    for (i, conf) in ["high", "low"].into_iter().enumerate() {
        out[i] = insert_edges(
            db,
            rel,
            (from_table, "symbol"),
            ("Entity", "id"),
            &["confidence"],
            sorted
                .iter()
                .filter(|(_, c)| *c == conf)
                .map(|((sym, ent), _)| vec![s(sym), s(ent), s(conf)]),
        )?;
    }
    Ok(out.into())
}

fn insert_method_of(
    db: &SqliteDb,
    deduped: &[&FunctionFact],
    stats: &mut ScipIngestStats,
) -> Result<()> {
    let mut type_by_prefix: HashMap<String, &str> = HashMap::new();
    for f in deduped {
        if !matches!(
            f.kind,
            FunctionKind::Struct
                | FunctionKind::Enum
                | FunctionKind::Trait
                | FunctionKind::TypeAlias
        ) {
            continue;
        }
        if let Some(prefix) = symbol_descriptor_prefix(&f.symbol) {
            type_by_prefix.entry(prefix).or_insert(f.symbol.as_str());
        }
    }
    let rows: Vec<(String, String)> = deduped
        .iter()
        .filter(|f| f.kind == FunctionKind::Method)
        .filter_map(|f| {
            let ty = type_by_prefix.get(method_parent_prefix(&f.symbol)?.as_str())?;
            Some((f.symbol.clone(), (*ty).to_string()))
        })
        .collect();
    stats.method_of_edges += insert_pairs(
        db,
        "METHOD_OF",
        ("Function", "symbol"),
        ("Type", "symbol"),
        rows,
    )?;
    Ok(())
}

/// `crate name -> Doc id`, from `crates/<name>/README.md` paths and
/// verbatim `crate-<name>` ids.
fn crate_doc_id_map(db: &SqliteDb) -> Result<HashMap<String, String>> {
    let mut m = HashMap::new();
    for r in db.query("SELECT id, path FROM Doc", &[])? {
        let (id, path) = (r[0].str_or_empty(), r[1].str_or_empty());
        if let Some(name) = crate_name_from_readme_path(&path) {
            m.entry(name).or_insert_with(|| id.clone());
        }
        if let Some(rest) = id.strip_prefix("crate-") {
            m.entry(rest.to_string()).or_insert_with(|| id.clone());
        }
    }
    Ok(m)
}

fn derive_entity_calls(db: &SqliteDb, stats: &mut ScipIngestStats) -> Result<()> {
    let mut pairs: Vec<(String, String, i64)> = db
        .query(
            "SELECT b1.dst, b2.dst, count(*) FROM CALLS c \
             JOIN FUNCTION_BELONGS_TO b1 ON b1.src = c.src \
             JOIN FUNCTION_BELONGS_TO b2 ON b2.src = c.dst \
             WHERE b1.dst <> b2.dst GROUP BY b1.dst, b2.dst",
            &[],
        )?
        .iter()
        .filter_map(|r| Some((r[0].str_or_empty(), r[1].str_or_empty(), r[2].as_i64()?)))
        .filter(|(a, b, n)| *n > 0 && !a.is_empty() && !b.is_empty())
        .collect();
    let max = pairs.iter().map(|p| p.2).max().unwrap_or(1).max(1);
    pairs.sort();
    stats.entity_calls_edges += insert_edges(
        db,
        "ENTITY_CALLS",
        ("Entity", "id"),
        ("Entity", "id"),
        &["frequency", "weight", "source"],
        pairs.iter().map(|(a, b, n)| {
            vec![
                s(a),
                s(b),
                int(*n),
                Value::Float(*n as f64 / max as f64),
                s("derived"),
            ]
        }),
    )?;
    Ok(())
}

fn derive_god_nodes(db: &SqliteDb, stats: &mut ScipIngestStats) -> Result<()> {
    let rows: Vec<(String, i64, String)> = db
        .query(
            "SELECT e.id, count(DISTINCT m.src), e.entity_class FROM Entity e \
             LEFT JOIN FUNCTION_MENTIONS m ON m.dst = e.id GROUP BY e.id",
            &[],
        )?
        .iter()
        .map(|r| {
            (
                r[0].str_or_empty(),
                r[1].as_i64().unwrap_or(0),
                r[2].str_or_empty(),
            )
        })
        .filter(|(id, _, _)| !id.is_empty())
        .collect();
    let decisions = classify_god_nodes(&rows);
    stats.entities_updated += db.write_rows(
        "UPDATE Entity SET mention_count = ?2, is_god_node = ?3 WHERE id = ?1",
        decisions
            .iter()
            .map(|(id, n, god)| vec![s(id), int(*n), Value::Bool(*god)]),
    )?;
    stats.god_nodes += decisions.iter().filter(|d| d.2).count();
    Ok(())
}
