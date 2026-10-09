//! SQLite twin of `store::ingest`: the corpus walk is shared
//! (`build_plan`); only the drain of the plan differs.

use std::path::{Path, PathBuf};

use anyhow::Result;

use super::{insert_edges, int, reset, s, upsert, write_list};
use crate::config::LintConfig;
use crate::ontology::Ontology;
use crate::store::ingest::{build_plan, IngestPlan};
use crate::store::{EdgeType, ResetMode, Value};
use crate::store_sqlite::SqliteDb;

/// Walk the corpus and write Doc / Entity / Section / Doc-Doc / COVERS /
/// RELATES_TO rows. Counterpart of `store::ingest_with_mode`.
pub fn ingest_with_mode(
    db: &SqliteDb,
    root: &Path,
    config: &LintConfig,
    files: &[PathBuf],
    ontology: &Ontology,
    mode: ResetMode,
) -> Result<()> {
    let plan = build_plan(root, config, files, ontology)?;
    db.transaction(|db| {
        reset(db, mode)?;
        write_plan(db, &plan)
    })
}

fn write_plan(db: &SqliteDb, plan: &IngestPlan) -> Result<()> {
    upsert(
        db,
        "Doc",
        "id",
        &[
            "id",
            "path",
            "role",
            "kind",
            "lifecycle",
            "bounded_context",
            "title",
            "summary",
            "status",
            "updated",
            "repo_id",
            "phase",
        ],
        plan.docs.iter().map(|d| {
            vec![
                s(&d.id),
                s(&d.path),
                s(&d.role),
                s(&d.kind),
                s(&d.lifecycle),
                s(&d.bounded_context),
                s(&d.title),
                s(&d.summary),
                s(&d.status),
                s(&d.updated),
                s(&d.repo_id),
                s(&d.phase),
            ]
        }),
    )?;
    let owners = |f: fn(&crate::store::ingest::DocNode) -> &Vec<String>| {
        plan.docs
            .iter()
            .map(move |d| (d.id.clone(), f(d).clone()))
            .collect::<Vec<_>>()
    };
    write_list(db, "doc_tags", "doc_id", owners(|d| &d.tags))?;
    write_list(db, "doc_covers", "doc_id", owners(|d| &d.covers))?;
    write_list(db, "doc_attributes", "doc_id", owners(|d| &d.attributes))?;

    // `mention_count` / `is_god_node` start at 0 and are set by the
    // post-SCIP god-node pass.
    upsert(
        db,
        "Entity",
        "id",
        &[
            "id",
            "display",
            "description",
            "mention_count",
            "is_god_node",
            "entity_class",
            "repo_id",
        ],
        plan.entities.iter().map(|e| {
            vec![
                s(&e.id),
                s(&e.display),
                s(&e.description),
                Value::Int(0),
                Value::Bool(false),
                s(e.entity_class.clone().unwrap_or_default()),
                s(&plan.repo_id),
            ]
        }),
    )?;
    let eowners = |f: fn(&crate::store::ingest::EntityNode) -> &Vec<String>| {
        plan.entities
            .iter()
            .map(move |e| (e.id.clone(), f(e).clone()))
            .collect::<Vec<_>>()
    };
    write_list(db, "entity_synonyms", "entity_id", eowners(|e| &e.synonyms))?;
    write_list(
        db,
        "entity_bounded_contexts",
        "entity_id",
        eowners(|e| &e.bounded_contexts),
    )?;
    write_list(
        db,
        "entity_scanner_coverage",
        "entity_id",
        eowners(|e| &e.scanner_coverage),
    )?;
    write_list(
        db,
        "entity_source_modules",
        "entity_id",
        eowners(|e| &e.source_modules),
    )?;
    write_list(
        db,
        "entity_attributes",
        "entity_id",
        eowners(|e| &e.attributes),
    )?;

    for kind in [
        crate::graph::EdgeKind::Wikilink,
        crate::graph::EdgeKind::MdLink,
        crate::graph::EdgeKind::DependsOn,
        crate::graph::EdgeKind::InformedBy,
        crate::graph::EdgeKind::Supersedes,
        crate::graph::EdgeKind::CrateRef,
    ] {
        insert_edges(
            db,
            kind.label(),
            ("Doc", "id"),
            ("Doc", "id"),
            &["line"],
            plan.edges
                .iter()
                .filter(|e| e.kind == kind)
                .map(|e| vec![s(&e.from_id), s(&e.to_id), int(e.line)]),
        )?;
    }

    insert_edges(
        db,
        "COVERS",
        ("Doc", "id"),
        ("Entity", "id"),
        &["line", "inferred"],
        plan.covers_edges.iter().map(|c| {
            vec![
                s(&c.doc_id),
                s(&c.entity_id),
                int(0),
                Value::Bool(c.inferred),
            ]
        }),
    )?;

    // Section rows need their Doc: MATCH (d:Doc) ... CREATE in the store.
    let known: std::collections::HashSet<&str> = plan.docs.iter().map(|d| d.id.as_str()).collect();
    let sections: Vec<_> = plan
        .sections
        .iter()
        .filter(|(doc, _)| known.contains(doc.as_str()))
        .collect();
    upsert(
        db,
        "Section",
        "id",
        &["id", "doc_id", "heading", "anchor", "level", "line", "text"],
        sections.iter().map(|(doc, sec)| {
            vec![
                s(format!("{doc}#{}", sec.anchor)),
                s(doc),
                s(&sec.heading),
                s(&sec.anchor),
                int(sec.level),
                int(sec.line),
                s(&sec.text),
            ]
        }),
    )?;
    insert_edges(
        db,
        "SECTION_OF",
        ("Section", "id"),
        ("Doc", "id"),
        &[],
        sections
            .iter()
            .map(|(doc, sec)| vec![s(format!("{doc}#{}", sec.anchor)), s(doc)]),
    )?;

    // Frontmatter-authored RELATES_TO: weight 1.0, frequency 0.
    insert_edges(
        db,
        "RELATES_TO",
        ("Entity", "id"),
        ("Entity", "id"),
        &["type", "weight", "frequency", "source"],
        plan.relates_to_edges.iter().map(|r| {
            vec![
                s(&r.from_id),
                s(&r.to_id),
                s(&r.relation_type),
                Value::Float(1.0),
                int(0),
                s("frontmatter"),
            ]
        }),
    )?;
    Ok(())
}
