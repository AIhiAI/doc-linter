//! Embedding population for the SQLite store: the twin of
//! `store::populate_embeddings`. Vectors never go in table columns;
//! they are added to one [`VectorIndex`] under the key
//! `table_tag << 56 | rowid`, saved at `<db>.vec` and swapped in by
//! [`super::build_and_swap`] (see `meta.vec_file`).
//!
//! The `embed_cache` table (text fingerprint -> vector) survives rebuilds,
//! so a `check` without `--embeddings` restores the index from the cache
//! exactly like the old rehydrate pass.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use rayon::prelude::*;

use super::vector::VectorIndex;
use super::SqliteDb;
use crate::embeddings::Embedder;
use crate::store::populate_embeddings::{
    fingerprint, EmbeddingStats, EMBED_DIM, SECTION_EMBED_CHARS,
};
use crate::store::{StoreRead, Value};

const EMBED_BATCH_SIZE: usize = 32;

/// (key tag, table, pk column, text column, hash column, max chars).
type Spec = (
    u64,
    &'static str,
    &'static str,
    &'static str,
    &'static str,
    Option<usize>,
);
const SPECS: &[Spec] = &[
    (1, "Doc", "id", "summary", "summary_embed_hash", None),
    (2, "Entity", "id", "display", "display_embed_hash", None),
    (
        3,
        "Function",
        "symbol",
        "doc_comment",
        "doc_comment_embed_hash",
        None,
    ),
    (
        4,
        "Type",
        "symbol",
        "doc_comment",
        "doc_comment_embed_hash",
        None,
    ),
    (
        5,
        "Section",
        "id",
        "text",
        "text_embed_hash",
        Some(SECTION_EMBED_CHARS),
    ),
];

/// Index key for a row of a table with the given tag.
pub fn key(tag: u64, rowid: i64) -> u64 {
    (tag << 56) | (rowid as u64 & ((1 << 56) - 1))
}

/// Inverse of [`key`]: `(table name, rowid)`.
pub fn split_key(k: u64) -> Option<(&'static str, i64)> {
    let table = SPECS.iter().find(|s| s.0 == k >> 56)?.1;
    Some((table, (k & ((1 << 56) - 1)) as i64))
}

/// Where `build` leaves the index of a staging db.
pub fn staging_path(db_file: &Path) -> PathBuf {
    PathBuf::from(format!("{}.vec", db_file.display()))
}

fn db_file(db: &SqliteDb) -> Result<PathBuf> {
    let rows = db.query(
        "SELECT file FROM pragma_database_list WHERE name = 'main'",
        &[],
    )?;
    Ok(PathBuf::from(rows[0][0].str_or_empty()))
}

/// `meta.vec_file`: the index file name beside the db, if any.
pub fn vec_file(db: &SqliteDb) -> Result<Option<String>> {
    Ok(db
        .query("SELECT value FROM meta WHERE key = 'vec_file'", &[])?
        .first()
        .map(|r| r[0].str_or_empty()))
}

pub fn set_vec_file(db: &SqliteDb, name: Option<&str>) -> Result<()> {
    match name {
        Some(n) => db.write_rows(
            "INSERT INTO meta(key, value) VALUES('vec_file', ?) \
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            [vec![Value::Str(n.to_string())]],
        ),
        None => db.write_rows(
            "DELETE FROM meta WHERE key = ? ",
            [vec![Value::Str("vec_file".into())]],
        ),
    }
    .map(|_| ())
}

/// Load the index a reader should use: the file `meta.vec_file` names
/// (falling back to `<db>.vec`), or `None` when the graph has no vectors.
pub fn load_index<V: VectorIndex>(db: &SqliteDb) -> Result<Option<V>> {
    let file = db_file(db)?;
    let path = match vec_file(db)? {
        Some(name) => file.with_file_name(name),
        None => staging_path(&file),
    };
    if !path.exists() {
        return Ok(None);
    }
    V::load(&path, EMBED_DIM).map(Some)
}

fn to_blob(v: &[f32]) -> Vec<u8> {
    v.iter().flat_map(|x| x.to_le_bytes()).collect()
}

fn from_blob(b: &[u8]) -> Vec<f32> {
    b.chunks_exact(4)
        .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
        .collect()
}

/// Fill the vector index for every embeddable row and save it at
/// `<db>.vec`. `embedder = None` is cache-only (rows whose text
/// fingerprint is cached get their vector, nothing is computed).
/// `new_index` builds an empty index of the given dimension.
pub fn populate<V: VectorIndex>(
    db: &SqliteDb,
    embedder: Option<&dyn Embedder>,
    new_index: impl FnOnce(usize) -> Result<V>,
) -> Result<EmbeddingStats> {
    let backend = embedder.map_or(crate::embeddings::ONNX_BACKEND_NAME, |e| e.name());
    let mut stats = EmbeddingStats {
        backend,
        ..Default::default()
    };
    if let Some(e) = embedder {
        if !e.is_available() {
            return Ok(stats);
        }
        if e.dim() != EMBED_DIM {
            eprintln!(
                "doc-linter: embedder {} reports dim={}, expected {EMBED_DIM} — skipping embeddings",
                e.name(),
                e.dim()
            );
            return Ok(stats);
        }
    }

    let mut cache: HashMap<String, Vec<f32>> = HashMap::new();
    {
        // Blobs bypass `Value` (no bytes variant): read them raw.
        let mut st = db.0.prepare("SELECT fp, emb FROM embed_cache")?;
        let rows = st.query_map([], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, Vec<u8>>(1)?))
        })?;
        for row in rows {
            let (fp, blob) = row?;
            let v = from_blob(&blob);
            if v.len() == EMBED_DIM {
                cache.insert(fp, v);
            }
        }
    }
    let mut used: HashSet<String> = HashSet::new();
    let mut fresh: Vec<(String, Vec<f32>)> = Vec::new();
    let mut entries: Vec<(u64, Vec<f32>)> = Vec::new();
    let mut hashes: Vec<(&str, &str, i64, String)> = Vec::new();

    for (tag, table, pk, text_col, hash_col, max_chars) in SPECS {
        let rows = db.query(
            &format!("SELECT rowid, {pk}, {text_col} FROM \"{table}\""),
            &[],
        )?;
        let mut to_embed: Vec<(i64, String, String, String)> = Vec::new();
        for r in rows {
            stats.rows_visited += 1;
            let (rowid, pkv, mut text) = (
                r[0].as_i64().unwrap_or(0),
                r[1].str_or_empty(),
                r[2].str_or_empty(),
            );
            if text.trim().is_empty() {
                continue;
            }
            if let Some(max) = max_chars {
                if let Some((cut, _)) = text.char_indices().nth(*max) {
                    text.truncate(cut);
                }
            }
            let fp = fingerprint(backend, &text);
            if let Some(v) = cache.get(&fp) {
                used.insert(fp.clone());
                entries.push((key(*tag, rowid), v.clone()));
                hashes.push((table, hash_col, rowid, fp));
                stats.rows_skipped_unchanged += 1;
            } else if embedder.is_some() {
                to_embed.push((rowid, pkv, text, fp));
            }
        }
        let Some(embedder) = embedder else { continue };
        to_embed.sort_by_key(|(_, _, text, _)| text.len());
        let chunks: Vec<&[(i64, String, String, String)]> =
            to_embed.chunks(EMBED_BATCH_SIZE).collect();
        let embedded: Vec<Vec<Vec<f32>>> = chunks
            .par_iter()
            .map(|chunk| {
                let texts: Vec<&str> = chunk.iter().map(|c| c.2.as_str()).collect();
                embedder.embed_batch(&texts).unwrap_or_else(|e| {
                    eprintln!("doc-linter: {table} batched embed failed ({e:#}); per-row fallback");
                    chunk
                        .iter()
                        .map(|c| embedder.embed(&c.2).unwrap_or_default())
                        .collect()
                })
            })
            .collect();
        let mut written = 0;
        for (chunk, vecs) in chunks.iter().zip(embedded) {
            if vecs.len() != chunk.len() {
                eprintln!("doc-linter: {table} embed_batch size mismatch — skipping chunk");
                continue;
            }
            for ((rowid, pkv, _, fp), v) in chunk.iter().zip(vecs) {
                if v.len() != EMBED_DIM {
                    if !v.is_empty() {
                        eprintln!("doc-linter: skip {table} {pkv}: embed dim {}", v.len());
                    }
                    continue;
                }
                used.insert(fp.clone());
                fresh.push((fp.clone(), v.clone()));
                entries.push((key(*tag, *rowid), v));
                hashes.push((table, hash_col, *rowid, fp.clone()));
                stats.rows_written += 1;
                written += 1;
            }
        }
        match *table {
            "Doc" => stats.doc_written = written,
            "Entity" => stats.entity_written = written,
            "Function" => stats.function_written = written,
            "Type" => stats.type_written = written,
            _ => stats.section_written = written,
        }
    }

    db.transaction(|db| {
        let mut put = db.0.prepare_cached(
            "INSERT INTO embed_cache(fp, emb) VALUES (?1, ?2) ON CONFLICT(fp) DO NOTHING",
        )?;
        for (fp, v) in &fresh {
            put.execute(rusqlite::params![fp, to_blob(v)])?;
        }
        let stale: Vec<Vec<Value>> = cache
            .keys()
            .filter(|fp| !used.contains(*fp))
            .map(|fp| vec![Value::Str(fp.clone())])
            .collect();
        db.write_rows("DELETE FROM embed_cache WHERE fp = ?", stale)?;
        for (table, hash_col, rowid, fp) in &hashes {
            db.write_rows(
                &format!("UPDATE \"{table}\" SET {hash_col} = ?1 WHERE rowid = ?2"),
                [vec![Value::Str(fp.clone()), Value::Int(*rowid)]],
            )?;
        }
        Ok(())
    })?;

    if entries.is_empty() {
        let _ = std::fs::remove_file(staging_path(&db_file(db)?));
        return Ok(stats);
    }
    let mut index = new_index(EMBED_DIM)?;
    for (k, v) in &entries {
        index.add(*k, v)?;
    }
    index
        .save(&staging_path(&db_file(db)?))
        .context("save vector index")?;
    Ok(stats)
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "tests panic on broken invariants — that is the point of a test"
)]
mod tests {
    use super::*;
    use crate::store::Store;
    use crate::store_sqlite::vector::ExactIndex;
    use crate::store_sqlite::{build_and_swap, graph_path, open_ro};

    /// One-hot-ish vectors: same text, same vector; different text, orthogonal.
    struct Fake;
    impl Embedder for Fake {
        fn embed(&self, text: &str) -> Result<Vec<f32>> {
            let slot = text.bytes().fold(7usize, |a, b| a * 31 + usize::from(b)) % EMBED_DIM;
            let mut v = vec![0.0; EMBED_DIM];
            v[slot] = 1.0;
            Ok(v)
        }
        fn dim(&self) -> usize {
            EMBED_DIM
        }
        fn name(&self) -> &'static str {
            crate::embeddings::ONNX_BACKEND_NAME
        }
    }

    fn tmp() -> PathBuf {
        let d = std::env::temp_dir().join(format!(
            "doc-linter-vec-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn vec_files(root: &Path) -> Vec<String> {
        std::fs::read_dir(root.join(".doc-lint"))
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().to_string())
            .filter(|n| n.starts_with("graph.vec."))
            .collect()
    }

    #[test]
    fn index_is_filled_swapped_restored_from_cache_and_kept_on_failure() {
        let root = tmp();
        build_and_swap(&root, |db| {
            for (id, summary) in [("a", "alpha doc"), ("b", "beta doc"), ("c", "")] {
                db.exec(
                    "INSERT INTO Doc(id, summary) VALUES ($i, $s)",
                    &[
                        ("i", Value::Str(id.into())),
                        ("s", Value::Str(summary.into())),
                    ],
                )?;
            }
            let st = populate(db, Some(&Fake), |d| Ok(ExactIndex::new(d)))?;
            assert_eq!((st.rows_written, st.doc_written), (2, 2));
            Ok(())
        })
        .unwrap();
        let first = vec_files(&root);
        assert_eq!(first.len(), 1, "{first:?}");

        let db = open_ro(&root).unwrap();
        let ix: ExactIndex = load_index(&db).unwrap().expect("index beside the db");
        let rowid = db
            .query("SELECT rowid FROM Doc WHERE id = 'b'", &[])
            .unwrap()[0][0]
            .as_i64()
            .unwrap();
        let hit = ix.search(&Fake.embed("beta doc").unwrap(), 1).unwrap();
        assert_eq!(hit[0].0, key(1, rowid));
        assert_eq!(split_key(hit[0].0), Some(("Doc", rowid)));
        assert!(hit[0].1 < 1e-5, "exact match has ~0 cosine distance");

        // A rebuild without an embedder restores the index from the cache.
        build_and_swap(&root, |db| {
            let st = populate(db, None, |d| Ok(ExactIndex::new(d)))?;
            assert_eq!((st.rows_written, st.rows_skipped_unchanged), (0, 2));
            Ok(())
        })
        .unwrap();
        let second = vec_files(&root);
        assert_eq!(second.len(), 1, "old index removed: {second:?}");
        assert_ne!(first, second, "new index has a new name");
        let ix: ExactIndex = load_index(&open_ro(&root).unwrap()).unwrap().unwrap();
        assert_eq!(
            ix.search(&Fake.embed("alpha doc").unwrap(), 1)
                .unwrap()
                .len(),
            1
        );

        // A failed build leaves the live db and its index untouched.
        let failed: Result<()> = build_and_swap(&root, |db| {
            populate(db, Some(&Fake), |d| Ok(ExactIndex::new(d)))?;
            anyhow::bail!("boom")
        });
        assert!(failed.is_err());
        assert_eq!(vec_files(&root), second);
        assert!(graph_path(&root).exists());
        let leftovers = std::fs::read_dir(root.join(".doc-lint"))
            .unwrap()
            .flatten()
            .filter(|e| e.file_name().to_string_lossy().contains(".build-"))
            .count();
        assert_eq!(leftovers, 0);

        // A text edit changes the fingerprint: the stale vector is pruned.
        build_and_swap(&root, |db| {
            db.exec(
                "UPDATE Doc SET summary = 'alpha changed' WHERE id = 'a'",
                &[],
            )?;
            let st = populate(db, None, |d| Ok(ExactIndex::new(d)))?;
            assert_eq!(st.rows_skipped_unchanged, 1, "only b is still cached");
            let n = db.query("SELECT count(*) FROM embed_cache", &[])?[0][0].as_i64();
            assert_eq!(n, Some(1));
            Ok(())
        })
        .unwrap();
        std::fs::remove_dir_all(&root).ok();
    }
}
