//! Gap-010 phase 1: per-repo identity. Writes a `Repo` node row
//! for the current ingest's `--root` so the graph can eventually
//! distinguish "node from THIS repo" from "node from a sibling".
//! Per-node `repo_id` columns on Doc / Entity / Function / Type /
//! File / Module / Endpoint land in phase 2; this module just
//! seeds the Repo table so list_repos and the future cross-repo
//! tools have a row to reference.
//!
//! Repo id derivation: canonical absolute path of `--root`. This
//! is unique per filesystem location and stable across runs. The
//! basename is exposed as `name` for human-readable display.
//! `cross_repo_roots` (when set) gets one Repo row per entry — so
//! a multi-root ingest produces multiple Repo rows.
//!
//! Separate from RepoMeta (which carries the singleton
//! `domain` + `problem_statement` config tuple) by design — Repo
//! identifies *where* the corpus lives, RepoMeta describes *what*
//! it is about. They will diverge further as gap-010 phase 2
//! introduces `repo_id` foreign keys.

use std::path::Path;

use anyhow::{Context, Result};

/// One ingested repo identity. Returned by [`ingest_repos`] so
/// the caller can wire `repo_id` into subsequent per-node ingest
/// passes once phase 2 lands.
#[derive(Debug, Clone)]
pub struct IngestedRepo {
    pub id: String,
    pub root_path: String,
    pub name: String,
}

pub(crate) fn make_repo(path: &Path) -> Result<IngestedRepo> {
    let canon = path
        .canonicalize()
        .with_context(|| format!("canonicalize {}", path.display()))?;
    let root_path = canon.to_string_lossy().to_string();
    let name = canon
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("repo")
        .to_string();
    Ok(IngestedRepo {
        id: root_path.clone(),
        root_path,
        name,
    })
}
