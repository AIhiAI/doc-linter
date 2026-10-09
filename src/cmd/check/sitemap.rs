//! Writes `<root>/docs/sitemap.json` by querying the
//! [[entity-doc-graph]] SQLite graph. Output shape is identical to the
//! legacy petgraph version (sorted by id) so AI agents reading the file
//! don't break on Round 2B. Idempotent — only rewrites when the
//! on-disk content differs.

use std::path::Path;

use anyhow::{Context, Result};
use serde::Serialize;

pub(crate) fn write_sitemap(root: &Path, db: &dyn doc_linter::graph_read::GraphRead) -> Result<()> {
    /// Serde shape for one doc row in the [[entity-doc-graph]]
    /// `sitemap.json` artefact — a stable AI-readable snapshot of every
    /// doc in the vault.
    #[derive(Serialize)]
    struct SitemapDoc<'a> {
        id: &'a str,
        path: &'a str,
        role: &'a str,
        #[serde(skip_serializing_if = "Option::is_none")]
        kind: Option<&'a str>,
        #[serde(skip_serializing_if = "Option::is_none")]
        lifecycle: Option<&'a str>,
        title: &'a str,
        summary: &'a str,
        status: &'a str,
        updated: &'a str,
        tags: &'a [String],
        #[serde(skip_serializing_if = "<[String]>::is_empty")]
        covers: &'a [String],
    }

    /// Top-level serde shape for the [[entity-doc-graph]] `sitemap.json`
    /// artefact — count and the ordered list of doc rows.
    #[derive(Serialize)]
    struct Sitemap<'a> {
        count: usize,
        docs: Vec<SitemapDoc<'a>>,
    }

    let rows = db.list_all_docs()?;
    let docs: Vec<SitemapDoc> = rows
        .iter()
        .map(|n| SitemapDoc {
            id: &n.id,
            path: &n.path,
            role: &n.role,
            kind: n.kind.as_deref(),
            lifecycle: n.lifecycle.as_deref(),
            title: &n.title,
            summary: &n.summary,
            status: &n.status,
            updated: &n.updated,
            tags: &n.tags,
            covers: &n.covers,
        })
        .collect();
    // list_all_docs ORDER BY d.id at the store layer, so we don't need
    // a Rust-side sort here. Belt-and-braces: assert ordering would catch
    // a future regression. (Skipped — the SQL ORDER BY is the contract.)

    let body = serde_json::to_string_pretty(&Sitemap {
        count: docs.len(),
        docs,
    })?;

    let target = root.join("docs/sitemap.json");
    let Some(parent) = target.parent() else {
        return Ok(());
    };
    if !parent.exists() {
        // No docs/ dir — nothing to do (keeps non-platform repos quiet).
        return Ok(());
    }

    let with_newline = format!("{body}\n");
    if let Ok(existing) = std::fs::read_to_string(&target) {
        if existing == with_newline {
            return Ok(());
        }
    }
    std::fs::write(&target, with_newline).with_context(|| format!("write {}", target.display()))?;
    Ok(())
}
