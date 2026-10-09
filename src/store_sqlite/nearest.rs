//! Similarity search against the usearch index beside the SQLite graph
//! (the `embedding` backend of `query_similar` / `audit_doc_region`).

use std::collections::HashMap;

use anyhow::Result;

use super::vector::VectorIndex;
use super::vectors::{load_index, split_key};
use super::SqliteDb;
use crate::graph_read::{NearestRequest, NearestResult};
use crate::store::populate_embeddings::EMBED_DIM;
use crate::store::{StoreRead, Value};

impl SqliteDb {
    /// The reader's vector index, loaded on first use and cached for the
    /// life of the handle (a swapped graph means a new handle). `None`
    /// when the graph was built without embeddings.
    fn vector_index(&self) -> Result<Option<&super::vector::Index>> {
        if self.1.get().is_none() {
            let loaded = load_index::<super::vector::Index>(self)?;
            let _ = self.1.set(loaded);
        }
        Ok(self.1.get().and_then(Option::as_ref))
    }
}

/// Nearest embedded rows of one corpus: the `query_similar` /
/// `audit_doc_region` embedding backend on SQLite. The HNSW index returns
/// keys nearest first; keys of other tables and rows rejected by the
/// repo / tag filters are skipped, widening the search until `top` rows
/// survive or the index is exhausted. `corpus_size` counts the rows that
/// carry a vector (embed hash set) and pass the filters, as the the store
/// `emb IS NOT NULL` scan does. Score is cosine similarity (`1 - distance`),
/// the dot product of the normalised vectors the store ranks by.
pub fn nearest(db: &SqliteDb, req: &NearestRequest<'_>) -> Result<NearestResult> {
    let (tag, table, pk, body, hash) = match req.kind {
        "doc" => (1, "Doc", "id", "summary", "summary_embed_hash"),
        "entity" => (2, "Entity", "id", "description", "display_embed_hash"),
        "function" => (
            3,
            "Function",
            "symbol",
            "doc_comment",
            "doc_comment_embed_hash",
        ),
        "type" => (4, "Type", "symbol", "doc_comment", "doc_comment_embed_hash"),
        "section" => (5, "Section", "id", "text", "text_embed_hash"),
        other => anyhow::bail!("no embedding corpus `{other}`"),
    };
    anyhow::ensure!(
        req.query.len() == EMBED_DIM,
        "query vector has {} dimensions, the index has {EMBED_DIM}",
        req.query.len()
    );
    let mut cond = format!("t.{hash} IS NOT NULL AND t.{hash} <> ''");
    let mut params: Vec<(&str, Value)> = Vec::new();
    match req.kind {
        "doc" => {
            let (c, p) = crate::graph_read::doc_filter_sql(
                "t",
                req.source_repo,
                req.with_tag,
                req.without_tag,
            );
            if !c.is_empty() {
                cond += &format!(" AND {c}");
                params = p;
            }
        }
        "section" => {
            let (c, p) = crate::graph_read::doc_filter_sql(
                "d",
                req.source_repo,
                req.with_tag,
                req.without_tag,
            );
            let extra = if c.is_empty() {
                String::new()
            } else {
                format!(" AND {c}")
            };
            cond += &format!(
                " AND EXISTS(SELECT 1 FROM \"SECTION_OF\" so JOIN Doc d ON d.id = so.dst \
                 WHERE so.src = t.id{extra})"
            );
            params = p;
        }
        "type" => {}
        _ => {
            if let Some(r) = req.source_repo {
                cond += " AND t.repo_id = $repo_id";
                params.push(("repo_id", Value::Str(r.to_string())));
            }
        }
    }
    let corpus_size = db.query(
        &format!("SELECT count(*) FROM \"{table}\" t WHERE {cond}"),
        &params,
    )?[0][0]
        .as_u64_or_zero() as usize;
    let mut out = NearestResult {
        hits: Vec::new(),
        corpus_size,
    };
    let Some(index) = db.vector_index()? else {
        return Ok(out);
    };
    if corpus_size == 0 || req.top == 0 {
        return Ok(out);
    }
    let fetch_sql = format!(
        "SELECT t.rowid, t.{pk}, coalesce(t.{body}, '') FROM \"{table}\" t \
         WHERE t.rowid IN (SELECT value FROM json_each($ids)) AND {cond}"
    );
    let mut k = (req.top * 8).max(64);
    loop {
        let found = index.search(req.query, k)?;
        let exhausted = found.len() < k;
        let rowids: Vec<(i64, f32)> = found
            .iter()
            .filter(|(key, _)| key >> 56 == tag)
            .filter_map(|(key, dist)| Some((split_key(*key)?.1, *dist)))
            .collect();
        let ids = serde_json::to_string(&rowids.iter().map(|r| r.0).collect::<Vec<_>>())?;
        let mut p = params.clone();
        p.push(("ids", Value::Str(ids)));
        let rows: HashMap<i64, (String, String)> = db
            .query(&fetch_sql, &p)?
            .iter()
            .map(|r| {
                (
                    r[0].as_i64().unwrap_or(0),
                    (r[1].str_or_empty(), r[2].str_or_empty()),
                )
            })
            .collect();
        out.hits = rowids
            .iter()
            .filter_map(|(rowid, dist)| {
                let (id, b) = rows.get(rowid)?;
                Some((id.clone(), b.clone(), 1.0 - f64::from(*dist)))
            })
            .take(req.top)
            .collect();
        if out.hits.len() >= req.top || exhausted {
            return Ok(out);
        }
        k *= 4;
    }
}
