//! Vector index seam. Keys are the owning row's integer `rowid`; vectors
//! never live in the SQLite file (docs/design/store-trait.md).
//!
//! The product path is [`UsearchIndex`] (HNSW, feature `vector-usearch`).
//! [`ExactIndex`] is a brute-force reference used only to measure recall
//! in tests; it is not meant to serve queries.

use std::path::Path;

use anyhow::Result;

pub trait VectorIndex: Sized {
    /// Insert `vector` under `key` (a rowid).
    fn add(&mut self, key: u64, vector: &[f32]) -> Result<()>;
    /// Up to `k` nearest keys by cosine distance, nearest first.
    fn search(&self, query: &[f32], k: usize) -> Result<Vec<(u64, f32)>>;
    fn save(&self, path: &Path) -> Result<()>;
    fn load(path: &Path, dim: usize) -> Result<Self>;
}

fn cosine_dist(a: &[f32], b: &[f32]) -> f32 {
    let (mut dot, mut na, mut nb) = (0f32, 0f32, 0f32);
    for (x, y) in a.iter().zip(b) {
        dot += x * y;
        na += x * x;
        nb += y * y;
    }
    1.0 - dot / (na.sqrt() * nb.sqrt()).max(f32::MIN_POSITIVE)
}

/// Exact reference index (O(n) per query). Test oracle only.
pub struct ExactIndex {
    dim: usize,
    rows: Vec<(u64, Vec<f32>)>,
}

impl ExactIndex {
    pub fn new(dim: usize) -> Self {
        Self {
            dim,
            rows: Vec::new(),
        }
    }
}

impl VectorIndex for ExactIndex {
    fn add(&mut self, key: u64, vector: &[f32]) -> Result<()> {
        anyhow::ensure!(vector.len() == self.dim, "expected {} dims", self.dim);
        self.rows.push((key, vector.to_vec()));
        Ok(())
    }

    fn search(&self, query: &[f32], k: usize) -> Result<Vec<(u64, f32)>> {
        let mut all: Vec<(u64, f32)> = self
            .rows
            .iter()
            .map(|(key, v)| (*key, cosine_dist(query, v)))
            .collect();
        all.sort_by(|a, b| a.1.total_cmp(&b.1));
        all.truncate(k);
        Ok(all)
    }

    fn save(&self, path: &Path) -> Result<()> {
        let mut out = Vec::with_capacity(self.rows.len() * (8 + 4 * self.dim));
        for (key, v) in &self.rows {
            out.extend(key.to_le_bytes());
            v.iter().for_each(|x| out.extend(x.to_le_bytes()));
        }
        Ok(std::fs::write(path, out)?)
    }

    fn load(path: &Path, dim: usize) -> Result<Self> {
        let bytes = std::fs::read(path)?;
        let stride = 8 + 4 * dim;
        anyhow::ensure!(bytes.len() % stride == 0, "corrupt index file");
        let rows = bytes
            .chunks(stride)
            .map(|c| {
                let key = u64::from_le_bytes(c[..8].try_into().unwrap_or_default());
                let v = c[8..]
                    .chunks(4)
                    .map(|f| f32::from_le_bytes(f.try_into().unwrap_or_default()))
                    .collect();
                (key, v)
            })
            .collect();
        Ok(Self { dim, rows })
    }
}

#[cfg(feature = "vector-usearch")]
pub use usearch_impl::UsearchIndex;

/// The index a reader loads for similarity search: HNSW with
/// `vector-usearch`, otherwise the (empty-in-practice) exact index —
/// without the feature `check --embeddings` writes no index file.
#[cfg(feature = "vector-usearch")]
pub type Index = UsearchIndex;
#[cfg(not(feature = "vector-usearch"))]
pub type Index = ExactIndex;

#[cfg(feature = "vector-usearch")]
mod usearch_impl {
    use super::{Path, Result, VectorIndex};
    use anyhow::anyhow;
    use usearch::{Index, IndexOptions, MetricKind, ScalarKind};

    /// HNSW cosine index, f32 storage.
    pub struct UsearchIndex(Index);

    fn options(dim: usize) -> IndexOptions {
        IndexOptions {
            dimensions: dim,
            metric: MetricKind::Cos,
            quantization: ScalarKind::F32,
            ..Default::default()
        }
    }

    impl UsearchIndex {
        pub fn new(dim: usize) -> Result<Self> {
            Index::new(&options(dim))
                .map(Self)
                .map_err(|e| anyhow!("{e}"))
        }
    }

    impl VectorIndex for UsearchIndex {
        fn add(&mut self, key: u64, vector: &[f32]) -> Result<()> {
            // usearch needs capacity reserved ahead of inserts.
            if self.0.size() >= self.0.capacity() {
                let cap = (self.0.capacity() * 2).max(1024);
                self.0.reserve(cap).map_err(|e| anyhow!("{e}"))?;
            }
            self.0.add(key, vector).map_err(|e| anyhow!("{e}"))
        }

        fn search(&self, query: &[f32], k: usize) -> Result<Vec<(u64, f32)>> {
            let m = self.0.search(query, k).map_err(|e| anyhow!("{e}"))?;
            Ok(m.keys.into_iter().zip(m.distances).collect())
        }

        fn save(&self, path: &Path) -> Result<()> {
            let p = path.to_str().ok_or_else(|| anyhow!("non-utf8 path"))?;
            self.0.save(p).map_err(|e| anyhow!("{e}"))
        }

        fn load(path: &Path, dim: usize) -> Result<Self> {
            let idx = Self::new(dim)?;
            let p = path.to_str().ok_or_else(|| anyhow!("non-utf8 path"))?;
            idx.0.load(p).map_err(|e| anyhow!("{e}"))?;
            Ok(idx)
        }
    }
}
