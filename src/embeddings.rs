//! Roadmap issue #28 (v0.4.0): semantic search over Doc / Function /
//! Entity text columns.
//!
//! ## v1 / v2 scope (this module)
//!
//! Defines the [`Embedder`] trait + a [`NoOpEmbedder`] that always
//! returns an empty vector (v1) and a real
//! [`OnnxEmbedder`] backed by [`tract_onnx`] + [`tokenizers`] gated
//! behind the `embeddings` Cargo feature (v2). Nothing in the CLI
//! consumes either yet — v3 wires the embedder into the ingest
//! path; v4 swaps the BM25 backend on `query similar
//! --backend embedding`.
//!
//! The `query similar` output already advertises an `algorithm`
//! field that is `"bm25"` today and reserved to flip to
//! `"embedding"` once the backend is real — see `src/query.rs:1597`.
//!
//! ## Why a trait, not an enum
//!
//! The roadmap keeps two backends in scope (ONNX + a Python
//! sidecar). An enum would force every call site to match on
//! variants that need different runtimes; a trait keeps the
//! corpus-loader code agnostic. The cost — one dynamic dispatch
//! per embed — is dwarfed by the ONNX inference itself.
//!
//! ## Failure mode
//!
//! `embed` returns `anyhow::Result<Vec<f32>>` rather than a bare
//! `Vec<f32>` because real backends can fail (model file missing,
//! tensor-shape mismatch, runtime not initialised). The NoOp impl
//! returns `Ok(Vec::new())` to signal "no embedding available" —
//! callers MUST treat the empty vector as the explicit "embeddings
//! disabled" sentinel and fall back to BM25 rather than treating it
//! as a zero vector for cosine-similarity purposes.

use anyhow::Result;

/// Small Embedder surface — what the corpus loaders call when
/// building the embedding column for a Doc / Function / Entity row.
///
/// `Send + Sync` so a single shared embedder can sit behind an
/// `Arc` and be used across the parallel corpus iteration paths.
pub trait Embedder: Send + Sync {
    /// Embed a single text fragment. Returns the embedding as a
    /// `Vec<f32>` of length [`Embedder::dim`], or `Ok(Vec::new())`
    /// when the backend is the [`NoOpEmbedder`] sentinel.
    fn embed(&self, text: &str) -> Result<Vec<f32>>;

    /// Embed many text fragments in a single forward pass when the
    /// backend supports it. The default impl falls back to a
    /// per-input loop over [`Embedder::embed`] so backends like
    /// [`NoOpEmbedder`] need no extra code. [`OnnxEmbedder`]
    /// overrides this to fold the inputs into one batched tract
    /// run, which is roughly an order of magnitude faster per row
    /// on CPU because tokenization + tensor setup + Python-style
    /// graph dispatch overhead is amortised across the batch.
    ///
    /// Returns `Err` only on a whole-batch failure (model run
    /// blew up). Individual empty-vector outputs follow the same
    /// "no signal" convention as [`Embedder::embed`].
    fn embed_batch(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>> {
        texts.iter().map(|t| self.embed(t)).collect()
    }

    /// Embedding dimensionality (e.g. 384 for `all-MiniLM-L6-v2`).
    /// `0` for the NoOp backend.
    fn dim(&self) -> usize;

    /// Stable identifier persisted alongside the embedding column
    /// so a model swap forces a re-embed instead of silently mixing
    /// vector spaces. `"noop"` for the placeholder.
    fn name(&self) -> &'static str;

    /// `true` when this backend produces real vectors. Convenience
    /// for the corpus loaders so they can skip the column entirely
    /// when only NoOp is available — no need to call `embed` per
    /// row just to discover the empty result.
    fn is_available(&self) -> bool {
        self.dim() > 0
    }
}

/// Placeholder backend. Always returns an empty vector and reports
/// dim `0`. Used as the default factory result when no embedding
/// Cargo feature is enabled.
#[derive(Debug, Clone, Copy, Default)]
pub struct NoOpEmbedder;

impl Embedder for NoOpEmbedder {
    fn embed(&self, _text: &str) -> Result<Vec<f32>> {
        Ok(Vec::new())
    }

    fn dim(&self) -> usize {
        0
    }

    fn name(&self) -> &'static str {
        "noop"
    }
}

/// Factory used by the corpus loaders to obtain an embedder.
///
/// Returns [`NoOpEmbedder`] in default builds. With `--features
/// embeddings`, resolves a real [`OnnxEmbedder`] via a layered
/// fallback:
///
/// 1. Explicit runtime override — both [`ENV_MODEL_PATH`] and
///    [`ENV_TOKENIZER_PATH`] env vars set and point at existing
///    files. Operators with their own model (different size,
///    different language, custom fine-tune) plug in here.
/// 2. Build-bundled model — `build.rs` (issue #28 v6) downloads
///    `bge-small-en-v1.5` to the user cache directory and bakes
///    the absolute paths in via the
///    `DOC_LINTER_BUNDLED_EMBED_MODEL` /
///    `DOC_LINTER_BUNDLED_EMBED_TOKENIZER` compile-time env vars.
///    This is the "just works" path for `cargo install
///    --features embeddings`.
/// 3. Fallback to [`NoOpEmbedder`] — neither override nor bundle
///    resolved. The whole pipeline still runs; `query similar
///    --backend embedding` will error rather than silently
///    returning empty hits.
///
/// On layer 2 init failure (corrupted cache, missing files), a
/// one-line warning goes to stderr so operators can see why
/// semantic search isn't materialising before the fallback.
/// [`Embedder::name`] of the ONNX backend. Part of every embedding
/// fingerprint, so cached vectors can be matched without loading the model.
pub const ONNX_BACKEND_NAME: &str = "onnx";

pub fn default_embedder() -> Box<dyn Embedder> {
    #[cfg(feature = "embeddings")]
    {
        // Layer 1: explicit env var override.
        if std::env::var_os(ENV_MODEL_PATH).is_some()
            || std::env::var_os(ENV_TOKENIZER_PATH).is_some()
        {
            match OnnxEmbedder::try_from_env() {
                Ok(e) => return Box::new(e),
                Err(err) => {
                    eprintln!(
                        "doc-linter: {ENV_MODEL_PATH} / {ENV_TOKENIZER_PATH} are set but \
                         OnnxEmbedder init failed; falling back to bundled / NoOp: {err:#}"
                    );
                }
            }
        }
        // Layer 2: build.rs-baked bundle paths. `option_env!()`
        // returns None when build.rs didn't emit the var (default
        // builds, or feature builds with the skip env var set).
        let bundled_model = option_env!("DOC_LINTER_BUNDLED_EMBED_MODEL");
        let bundled_tok = option_env!("DOC_LINTER_BUNDLED_EMBED_TOKENIZER");
        if let (Some(m), Some(t)) = (bundled_model, bundled_tok) {
            match OnnxEmbedder::try_new(m, t) {
                Ok(e) => return Box::new(e),
                Err(err) => {
                    eprintln!(
                        "doc-linter: bundled embedder init failed (cache may be stale); \
                         falling back to NoOp: {err:#}"
                    );
                }
            }
        }
    }
    Box::new(NoOpEmbedder)
}

/// Roadmap issue #28 v2 (v0.4.0): environment variable holding the
/// absolute path to the ONNX model file. Read by
/// [`OnnxEmbedder::try_from_env`] and surfaced in error messages.
#[cfg(feature = "embeddings")]
pub const ENV_MODEL_PATH: &str = "DOC_LINTER_EMBED_MODEL";

/// Roadmap issue #28 v2 (v0.4.0): environment variable holding the
/// absolute path to the HuggingFace `tokenizer.json` file the model
/// was trained with. Required because the same model shape can be
/// driven by different tokenizers (BPE / WordPiece / Unigram).
#[cfg(feature = "embeddings")]
pub const ENV_TOKENIZER_PATH: &str = "DOC_LINTER_EMBED_TOKENIZER";

/// Hard ceiling on tokenized input length, matching the positional-
/// embedding shape of every sentence-transformer encoder we ship
/// (bge-small / all-MiniLM, both BERT-family with 512 positions).
/// Inputs longer than this are truncated at the tokenizer rather
/// than failing at the slice op inside the forward pass.
#[cfg(feature = "embeddings")]
const MAX_SEQ_LEN: usize = 512;

/// Roadmap issue #28 v2 (v0.4.0): ONNX backend powered by
/// [`tract_onnx`] (pure-Rust ONNX inference, no C++ toolchain) and
/// [`tokenizers`] (pure-Rust HuggingFace tokenizers). Loads the
/// model + tokenizer at construction; each [`Embedder::embed`] call
/// tokenizes one input, runs forward inference, mean-pools over the
/// sequence dimension, and L2-normalises the result. Output is a
/// dense `Vec<f32>` of length [`Embedder::dim`].
///
/// The Embedder is `Send + Sync` because `tract`'s runnable model
/// and the HuggingFace tokenizer are both thread-safe; the corpus
/// loaders share one boxed instance across the parallel ingest
/// passes without locking.
///
/// Model bundling: the model bytes are NOT shipped with the crate
/// (an 80 MB+ git artifact would dwarf the source tree). Operators
/// download `model.onnx` + `tokenizer.json` for whichever
/// sentence-transformer they want — `all-MiniLM-L6-v2` is the
/// recommended default (384-dim, ~80 MB) — and point
/// [`ENV_MODEL_PATH`] + [`ENV_TOKENIZER_PATH`] at the files. v3's
/// ingest pipeline reads the same env vars.
///
/// Gated by `#[cfg(feature = "embeddings")]` so default builds
/// carry neither the type nor its tract/tokenizers dependency tree.
#[cfg(feature = "embeddings")]
pub struct OnnxEmbedder {
    /// Absolute path the model was loaded from. Surfaced in error
    /// messages so an operator with the wrong path doesn't have to
    /// chase the env var to find what went wrong.
    model_path: std::path::PathBuf,
    /// Embedding dimensionality. Determined at construction by
    /// inspecting the model's output tensor shape (NOT a caller-
    /// supplied constant) — the model file is the source of truth.
    dim: usize,
    /// Tokenizer matched to the model. Loaded from
    /// [`ENV_TOKENIZER_PATH`].
    tokenizer: tokenizers::Tokenizer,
    /// Loaded ONNX session, optimised + ready for inference.
    /// Wrapped in a typed alias for readability.
    session: TractSession,
}

/// Concrete tract type for the optimised, type-tagged runnable
/// model — pulled into a type alias to keep the `OnnxEmbedder`
/// struct legible. Both the type plan and the runnable model are
/// `Send + Sync`, which is what lets `OnnxEmbedder` ride behind an
/// `Arc<dyn Embedder>` across the corpus loaders.
#[cfg(feature = "embeddings")]
type TractSession = tract_onnx::prelude::SimplePlan<
    tract_onnx::prelude::TypedFact,
    Box<dyn tract_onnx::prelude::TypedOp>,
    tract_onnx::prelude::Graph<
        tract_onnx::prelude::TypedFact,
        Box<dyn tract_onnx::prelude::TypedOp>,
    >,
>;

#[cfg(feature = "embeddings")]
impl std::fmt::Debug for OnnxEmbedder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OnnxEmbedder")
            .field("model_path", &self.model_path)
            .field("dim", &self.dim)
            .finish_non_exhaustive()
    }
}

#[cfg(feature = "embeddings")]
impl OnnxEmbedder {
    /// Construct from the [`ENV_MODEL_PATH`] + [`ENV_TOKENIZER_PATH`]
    /// environment variables. Returns `Err` when either var is unset,
    /// either file is missing, the tokenizer can't be parsed, or the
    /// model can't be loaded. Callers (`default_embedder`) treat any
    /// error as a fall-back-to-NoOp signal.
    pub fn try_from_env() -> anyhow::Result<Self> {
        let model_var = ENV_MODEL_PATH;
        let tokenizer_var = ENV_TOKENIZER_PATH;
        let model = std::env::var(model_var).map_err(|_| {
            anyhow::anyhow!("env var {model_var} is not set; cannot construct OnnxEmbedder")
        })?;
        let tokenizer = std::env::var(tokenizer_var).map_err(|_| {
            anyhow::anyhow!("env var {tokenizer_var} is not set; cannot construct OnnxEmbedder")
        })?;
        Self::try_new(model, tokenizer)
    }

    /// Construct from explicit paths. Loads the tokenizer + model
    /// eagerly (cheaper than lazy-loading because the model has to
    /// be optimised exactly once anyway). Embedding dimensionality
    /// is read off the model's output tensor shape.
    pub fn try_new(
        model_path: impl Into<std::path::PathBuf>,
        tokenizer_path: impl AsRef<std::path::Path>,
    ) -> anyhow::Result<Self> {
        use anyhow::Context;
        use tract_onnx::prelude::*;

        let model_path = model_path.into();
        let tokenizer_path = tokenizer_path.as_ref();

        if !model_path.exists() {
            anyhow::bail!(
                "ONNX model file not found at {} — set {} to a valid path",
                model_path.display(),
                ENV_MODEL_PATH
            );
        }
        if !tokenizer_path.exists() {
            anyhow::bail!(
                "tokenizer.json not found at {} — set {} to a valid path",
                tokenizer_path.display(),
                ENV_TOKENIZER_PATH
            );
        }

        let mut tokenizer = tokenizers::Tokenizer::from_file(tokenizer_path).map_err(|e| {
            anyhow::anyhow!("load tokenizer from {}: {e}", tokenizer_path.display())
        })?;

        // Force truncation at the model's positional-embedding ceiling
        // (512 for BERT-family encoders, which covers bge-small /
        // all-MiniLM). Before this, long doc-comments fell through
        // the slice-failure path in single-row `embed()`: the
        // tokenizer happily produced a >512-id encoding, the forward
        // pass blew up on the `[1, seq]` reshape, and we logged six
        // skipped rows per self-host run after wasting the tokenize
        // + partial dispatch cost. Configuring truncation up-front
        // both bounds the per-row cost and stops silently dropping
        // the long inputs — they now embed successfully, just on the
        // first 512 tokens.
        tokenizer
            .with_truncation(Some(tokenizers::TruncationParams {
                max_length: MAX_SEQ_LEN,
                strategy: tokenizers::TruncationStrategy::LongestFirst,
                stride: 0,
                direction: tokenizers::TruncationDirection::Right,
            }))
            .map_err(|e| anyhow::anyhow!("configure tokenizer truncation: {e}"))?;

        // Batch padding to the longest input in each batch. This
        // only takes effect for `encode_batch` (the single-input
        // `encode` path leaves encodings unpadded), so single-row
        // `embed()` still pays only its own seq_len cost — only
        // batched `embed_batch` calls eat the longest-in-batch
        // tensor shape. `pad_id: 0` matches the [PAD] convention
        // every bge / MiniLM tokenizer.json declares; the attention
        // mask zeros out padded positions during mean-pool so they
        // don't contaminate the result.
        tokenizer.with_padding(Some(tokenizers::PaddingParams {
            strategy: tokenizers::PaddingStrategy::BatchLongest,
            direction: tokenizers::PaddingDirection::Right,
            pad_to_multiple_of: None,
            pad_id: 0,
            pad_type_id: 0,
            pad_token: "[PAD]".into(),
        }));

        // Load the model. We don't fix the input shape at this
        // point — tract's `into_optimized` can fold constants /
        // shape inference across the full graph once we know the
        // input dimensions per call. For sentence-transformer
        // models the per-call seq_len differs; using
        // `into_optimized` with the natural symbolic shape works.
        let model = tract_onnx::onnx()
            .model_for_path(&model_path)
            .with_context(|| format!("load ONNX model from {}", model_path.display()))?;
        let model = model
            .into_optimized()
            .with_context(|| format!("optimise ONNX model {}", model_path.display()))?;
        let session = model
            .into_runnable()
            .with_context(|| format!("compile ONNX model {} to runnable", model_path.display()))?;

        // Infer the embedding dimensionality from the model's
        // primary output tensor. Sentence-transformer ONNX exports
        // typically expose `last_hidden_state` with shape
        // `[batch, seq, hidden]`; the hidden dim is what we want.
        // Fall back to the last dim of whatever output index 0
        // declares; bail if it's not statically resolvable.
        let dim = infer_embedding_dim(&session)
            .context("infer embedding dimensionality from model output")?;
        if dim == 0 {
            anyhow::bail!(
                "ONNX model at {} declared embedding dim 0 — model is malformed or output shape is not what we expect",
                model_path.display()
            );
        }

        Ok(Self {
            model_path,
            dim,
            tokenizer,
            session,
        })
    }

    /// Path the model was loaded from. Read by tests and the
    /// stderr-on-fallback path.
    pub fn model_path(&self) -> &std::path::Path {
        &self.model_path
    }
}

#[cfg(feature = "embeddings")]
impl Embedder for OnnxEmbedder {
    fn embed(&self, text: &str) -> Result<Vec<f32>> {
        use anyhow::Context;
        use tract_onnx::prelude::*;

        // 1. Tokenize. The encode call adds the model-specific
        //    special tokens (CLS / SEP for BERT-family,
        //    sentence-piece markers for T5, etc.) when the
        //    tokenizer config says to.
        let encoded = self
            .tokenizer
            .encode(text, true)
            .map_err(|e| anyhow::anyhow!("tokenize {text:?}: {e}"))?;

        let ids: Vec<i64> = encoded.get_ids().iter().map(|&id| i64::from(id)).collect();
        let mask: Vec<i64> = encoded
            .get_attention_mask()
            .iter()
            .map(|&m| i64::from(m))
            .collect();
        let token_type_ids: Vec<i64> = encoded
            .get_type_ids()
            .iter()
            .map(|&t| i64::from(t))
            .collect();

        if ids.is_empty() {
            // Empty input → empty embedding. Callers treat the
            // empty vec as "no signal" the same way they treat the
            // NoOp backend; cosine-similarity consumers MUST guard
            // for this rather than divide by zero.
            return Ok(Vec::new());
        }

        let seq_len = ids.len();

        // 2. Build the three input tensors (input_ids,
        //    attention_mask, token_type_ids) with shape [1, seq_len].
        let ids_tensor: Tensor = tract_ndarray::Array2::from_shape_vec((1, seq_len), ids)
            .context("ids tensor")?
            .into();
        let mask_tensor: Tensor = tract_ndarray::Array2::from_shape_vec((1, seq_len), mask.clone())
            .context("mask tensor")?
            .into();
        let token_type_tensor: Tensor =
            tract_ndarray::Array2::from_shape_vec((1, seq_len), token_type_ids)
                .context("token_type_ids tensor")?
                .into();

        // 3. Run forward. tract's runnable model accepts inputs by
        //    position. Sentence-transformer ONNX exports declare
        //    inputs in (input_ids, attention_mask, token_type_ids)
        //    order; if a model declares a different order we'd
        //    need a per-model adapter — punt that to v0.5.0.
        let inputs: TVec<TValue> = tvec!(
            ids_tensor.into(),
            mask_tensor.into(),
            token_type_tensor.into(),
        );
        let outputs = self
            .session
            .run(inputs)
            .context("ONNX forward pass failed")?;

        // 4. Mean-pool over the sequence dimension using the
        //    attention mask. Output[0] is `last_hidden_state` with
        //    shape [1, seq, hidden]. We pool only over the real
        //    (non-padding) positions so short inputs aren't
        //    diluted by zero-vectors.
        let hidden = outputs
            .first()
            .ok_or_else(|| anyhow::anyhow!("ONNX session returned no outputs"))?
            .to_array_view::<f32>()
            .context("output tensor isn't f32")?;
        let hidden_shape = hidden.shape();
        if hidden_shape.len() != 3 || hidden_shape[0] != 1 {
            anyhow::bail!("expected [1, seq, hidden] output, got shape {hidden_shape:?}");
        }
        let hidden_dim = hidden_shape[2];
        let mut pooled = vec![0.0_f32; hidden_dim];
        let mut denom = 0.0_f32;
        for (pos, &m) in mask.iter().enumerate() {
            if m == 0 {
                continue;
            }
            denom += 1.0;
            for h in 0..hidden_dim {
                pooled[h] += hidden[[0, pos, h]];
            }
        }
        if denom > 0.0 {
            for h in &mut pooled {
                *h /= denom;
            }
        }

        // 5. L2-normalise so cosine similarity ↔ dot product.
        l2_normalize_in_place(&mut pooled);
        Ok(pooled)
    }

    /// Batched forward pass. One tokenization + one tract dispatch
    /// covers up to `texts.len()` inputs; the per-row cost in the
    /// populate-embeddings hot path drops by roughly the batch size
    /// on CPU because tract's per-call setup (constant folding,
    /// shape resolution, working buffer alloc) is amortised across
    /// the batch instead of paid per row.
    ///
    /// Padding is handled by the tokenizer's `BatchLongest` config
    /// applied in [`Self::try_new`]: every encoding in the returned
    /// batch is padded to the longest input's post-truncation length,
    /// so the resulting tensors have a uniform shape. The attention
    /// mask carries the original length information into mean-pool,
    /// so padded positions don't dilute the result.
    fn embed_batch(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>> {
        use anyhow::Context;
        use tract_onnx::prelude::*;

        if texts.is_empty() {
            return Ok(Vec::new());
        }

        let encodings = self
            .tokenizer
            .encode_batch(texts.to_vec(), true)
            .map_err(|e| anyhow::anyhow!("tokenize batch of {}: {e}", texts.len()))?;

        let batch_size = encodings.len();
        if batch_size == 0 {
            return Ok(Vec::new());
        }
        // After encode_batch with BatchLongest padding every
        // encoding in the batch has the same id-vec length — the
        // longest input's post-truncation length. We trust that
        // invariant rather than re-checking per row; the
        // debug_assert below catches a tokenizer config drift in
        // tests without paying for the check in release.
        let seq_len = encodings[0].get_ids().len();
        if seq_len == 0 {
            // All inputs were empty strings — return the NoOp
            // sentinel for each so callers can skip writes
            // uniformly with the single-input path.
            return Ok(vec![Vec::new(); batch_size]);
        }

        let mut ids_flat: Vec<i64> = Vec::with_capacity(batch_size * seq_len);
        let mut mask_flat: Vec<i64> = Vec::with_capacity(batch_size * seq_len);
        let mut tt_flat: Vec<i64> = Vec::with_capacity(batch_size * seq_len);
        for enc in &encodings {
            debug_assert_eq!(enc.get_ids().len(), seq_len);
            ids_flat.extend(enc.get_ids().iter().map(|&id| i64::from(id)));
            mask_flat.extend(enc.get_attention_mask().iter().map(|&m| i64::from(m)));
            tt_flat.extend(enc.get_type_ids().iter().map(|&t| i64::from(t)));
        }

        let ids_tensor: Tensor =
            tract_ndarray::Array2::from_shape_vec((batch_size, seq_len), ids_flat)
                .context("batch ids tensor")?
                .into();
        let mask_tensor: Tensor =
            tract_ndarray::Array2::from_shape_vec((batch_size, seq_len), mask_flat.clone())
                .context("batch mask tensor")?
                .into();
        let tt_tensor: Tensor =
            tract_ndarray::Array2::from_shape_vec((batch_size, seq_len), tt_flat)
                .context("batch token_type_ids tensor")?
                .into();

        let inputs: TVec<TValue> = tvec!(ids_tensor.into(), mask_tensor.into(), tt_tensor.into(),);
        let outputs = self
            .session
            .run(inputs)
            .context("ONNX batched forward pass failed")?;

        let hidden = outputs
            .first()
            .ok_or_else(|| anyhow::anyhow!("ONNX session returned no outputs"))?
            .to_array_view::<f32>()
            .context("output tensor isn't f32")?;
        let shape = hidden.shape();
        if shape.len() != 3 || shape[0] != batch_size {
            anyhow::bail!("expected [{batch_size}, seq, hidden] output, got shape {shape:?}");
        }
        let hidden_dim = shape[2];

        let mut results: Vec<Vec<f32>> = Vec::with_capacity(batch_size);
        for b in 0..batch_size {
            let mut pooled = vec![0.0_f32; hidden_dim];
            let mut denom = 0.0_f32;
            for pos in 0..seq_len {
                let m = mask_flat[b * seq_len + pos];
                if m == 0 {
                    continue;
                }
                denom += 1.0;
                for h in 0..hidden_dim {
                    pooled[h] += hidden[[b, pos, h]];
                }
            }
            if denom > 0.0 {
                for h in &mut pooled {
                    *h /= denom;
                }
            }
            l2_normalize_in_place(&mut pooled);
            results.push(pooled);
        }
        Ok(results)
    }

    fn dim(&self) -> usize {
        self.dim
    }

    fn name(&self) -> &'static str {
        // Stable identifier persisted alongside embeddings — see
        // trait doc. v0.5.0 may switch this to a hash of the model
        // path so two physically-different MiniLM exports don't
        // collide in the vector index.
        ONNX_BACKEND_NAME
    }
}

/// Roadmap issue #28 v2: derive embedding dim from a tract session's
/// primary output shape. The convention is `[batch, seq, hidden]`
/// for sentence-transformer encoders; we return the trailing
/// dimension. Returns `Err` when the dim isn't a static integer
/// (e.g. fully-symbolic output).
#[cfg(feature = "embeddings")]
fn infer_embedding_dim(session: &TractSession) -> anyhow::Result<usize> {
    // tract exposes `to_usize` on `TDim` via the `DimLike` trait —
    // pulling the trait into scope rather than re-using
    // `prelude::*` keeps the dependency surface explicit.
    use tract_onnx::tract_hir::internal::DimLike;

    let model = session.model();
    let output_id = *model
        .outputs
        .first()
        .ok_or_else(|| anyhow::anyhow!("model has no outputs"))?;
    let fact = model
        .outlet_fact(output_id)
        .map_err(|e| anyhow::anyhow!("read output fact: {e}"))?;
    let shape = &fact.shape;
    let last = shape
        .dims()
        .last()
        .ok_or_else(|| anyhow::anyhow!("output tensor has zero dimensions"))?;
    let dim = last
        .to_usize()
        .map_err(|e| anyhow::anyhow!("trailing output dim isn't static: {e}"))?;
    Ok(dim)
}

/// L2-normalise a vector in place. Cosine similarity over the
/// resulting unit vectors collapses to a dot product, which is
/// what the v0.4.0 #28 v4 `query similar --backend embedding`
/// reads.
#[cfg(feature = "embeddings")]
fn l2_normalize_in_place(v: &mut [f32]) {
    let norm: f32 = v.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm > 0.0 {
        for x in v.iter_mut() {
            *x /= norm;
        }
    }
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::float_cmp,
    reason = "tests panic on broken invariants — that is the point of a test"
)]
mod tests {
    use super::*;

    #[test]
    fn noop_returns_empty_vector_for_any_text() {
        let e = NoOpEmbedder;
        assert!(e.embed("anything").unwrap().is_empty());
        assert!(e.embed("").unwrap().is_empty());
        assert!(e
            .embed("a very long input string ".repeat(100).as_str())
            .unwrap()
            .is_empty());
    }

    #[test]
    fn noop_reports_dim_zero_and_unavailable() {
        let e = NoOpEmbedder;
        assert_eq!(e.dim(), 0);
        assert!(
            !e.is_available(),
            "NoOp must be is_available()==false so corpus loaders skip the column"
        );
        assert_eq!(e.name(), "noop");
    }

    #[test]
    fn default_factory_returns_noop_without_feature_or_env() {
        // Default builds (no `embeddings` feature) MUST return
        // NoOp — the corpus loaders rely on `is_available() ==
        // false` to skip the embedding column entirely.
        //
        // Feature-on builds fall back to NoOp only when ALL three
        // resolution layers fail: no layer-1 env override, no
        // layer-2 bundled paths from build.rs (#28 v6), and no
        // layer-3 inline default. Skip the assertion when any of
        // those would legitimately return ONNX so the test still
        // asserts something meaningful on minimal builds without
        // false-failing the bundled-model build.
        #[cfg(feature = "embeddings")]
        {
            if std::env::var_os(ENV_MODEL_PATH).is_some()
                || std::env::var_os(ENV_TOKENIZER_PATH).is_some()
            {
                return;
            }
            if option_env!("DOC_LINTER_BUNDLED_EMBED_MODEL").is_some()
                && option_env!("DOC_LINTER_BUNDLED_EMBED_TOKENIZER").is_some()
            {
                return;
            }
        }
        let e = default_embedder();
        assert_eq!(e.name(), "noop");
        assert_eq!(e.dim(), 0);
        assert!(!e.is_available());
    }

    /// The Embedder trait MUST stay object-safe so the factory can
    /// return `Box<dyn Embedder>` (and so a shared `Arc<dyn Embedder>`
    /// can sit behind the corpus loaders). This compile-time check
    /// fails to compile if any trait method becomes generic over a
    /// type parameter or uses `Self` in a non-receiver position.
    #[test]
    fn embedder_trait_is_object_safe() {
        fn assert_object_safe(_: &dyn Embedder) {}
        let e = NoOpEmbedder;
        assert_object_safe(&e);
    }

    /// Default `embed_batch` impl falls back to a per-input loop
    /// over `embed`, so backends like [`NoOpEmbedder`] that don't
    /// override it still satisfy the batch contract: one output
    /// per input, in matching order. Critical for the populate-
    /// embeddings batched loop, which zips inputs back to pks by
    /// position.
    #[test]
    fn default_embed_batch_loops_over_embed_in_order() {
        // Stub embedder whose vector encodes the input's first
        // character so the test can prove order is preserved
        // (a permuted output would scramble char->index).
        struct CharIdEmbedder;
        impl Embedder for CharIdEmbedder {
            fn embed(&self, text: &str) -> Result<Vec<f32>> {
                let c = text.chars().next().unwrap_or('\0') as u32 as f32;
                Ok(vec![c, c, c])
            }
            fn dim(&self) -> usize {
                3
            }
            fn name(&self) -> &'static str {
                "char-id-stub"
            }
        }

        let e = CharIdEmbedder;
        let out = e.embed_batch(&["alpha", "beta", "gamma"]).unwrap();
        assert_eq!(out.len(), 3);
        assert_eq!(out[0][0], 'a' as u32 as f32);
        assert_eq!(out[1][0], 'b' as u32 as f32);
        assert_eq!(out[2][0], 'g' as u32 as f32);

        // Empty slice ⇒ empty output, not an error.
        let empty = e.embed_batch(&[]).unwrap();
        assert!(empty.is_empty());

        // NoOp's empty-vec sentinel survives the loop unchanged
        // — populate_embeddings relies on this to skip writes.
        let noop = NoOpEmbedder;
        let noop_out = noop.embed_batch(&["x", "y"]).unwrap();
        assert_eq!(noop_out, vec![Vec::<f32>::new(), Vec::<f32>::new()]);
    }

    /// Roadmap issue #28 v2 (v0.4.0): opt-in batched-inference
    /// happy path. Mirrors `onnx_embedder_loads_real_model_when_env_set`
    /// but exercises `embed_batch` directly so a regression in the
    /// batched tensor packing surfaces as a test failure rather than
    /// a quiet correctness drift in the populate pass. Asserts:
    /// - One output per input, in matching order.
    /// - Each output is the model's declared dim.
    /// - Vectors produced via `embed_batch` agree (modulo tiny FP
    ///   drift) with those produced via per-row `embed` — the batch
    ///   path is a perf optimisation, not a semantics change.
    #[cfg(feature = "embeddings")]
    #[test]
    fn onnx_embedder_embed_batch_matches_single_embed() {
        let model_var = std::env::var(ENV_MODEL_PATH).ok();
        let tok_var = std::env::var(ENV_TOKENIZER_PATH).ok();
        let (Some(model), Some(tok)) = (model_var, tok_var) else {
            return;
        };
        let e = OnnxEmbedder::try_new(&model, &tok)
            .expect("real model + tokenizer should load via try_new");

        let inputs = ["the cat sat on the mat", "a feline rests on a rug", "rust"];
        let batched = e
            .embed_batch(&inputs)
            .expect("batched embed of three inputs");
        assert_eq!(batched.len(), inputs.len());
        for v in &batched {
            assert_eq!(v.len(), e.dim());
        }

        for (i, text) in inputs.iter().enumerate() {
            let single = e.embed(text).unwrap();
            // Cosine sim of two equivalent vectors should be ~1.
            // Allow a small epsilon — tract reorders ops between
            // batched and single dispatch, which can perturb the
            // last few f32 mantissa bits.
            let dot: f32 = single
                .iter()
                .zip(batched[i].iter())
                .map(|(a, b)| a * b)
                .sum();
            assert!(
                dot > 0.999,
                "batched[{i}] and single embed should be ~equal (cosine ~1), got dot={dot}"
            );
        }
    }

    /// Roadmap issue #28 v2 (v0.4.0): inputs longer than the
    /// model's positional-embedding ceiling no longer fall through
    /// the slice-failure path. With truncation configured at the
    /// tokenizer level the long input is truncated to 512 tokens
    /// and embeds successfully, instead of erroring out after a
    /// wasted partial forward pass. Opt-in (needs the real model).
    #[cfg(feature = "embeddings")]
    #[test]
    fn onnx_embedder_truncates_long_inputs_instead_of_failing() {
        let model_var = std::env::var(ENV_MODEL_PATH).ok();
        let tok_var = std::env::var(ENV_TOKENIZER_PATH).ok();
        let (Some(model), Some(tok)) = (model_var, tok_var) else {
            return;
        };
        let e = OnnxEmbedder::try_new(&model, &tok)
            .expect("real model + tokenizer should load via try_new");

        // ~4000 words ⇒ well past 512 tokens for any tokenizer
        // we'd plug in. Pre-truncation config this would have
        // panicked the slice op inside `embed()`.
        let long_input = "lorem ipsum dolor sit amet ".repeat(800);
        let v = e
            .embed(&long_input)
            .expect("long input should truncate and embed, not error");
        assert_eq!(v.len(), e.dim());

        // Same for the batched path.
        let batched = e
            .embed_batch(&[long_input.as_str(), "short"])
            .expect("mixed-length batch should embed cleanly");
        assert_eq!(batched.len(), 2);
        assert_eq!(batched[0].len(), e.dim());
        assert_eq!(batched[1].len(), e.dim());
    }

    /// Roadmap issue #28 v2 (v0.4.0): `OnnxEmbedder::try_new`
    /// surfaces clear errors with the relevant env var name when
    /// the model or tokenizer path doesn't exist. Doesn't exercise
    /// the happy path because that needs a real model fixture —
    /// see `onnx_embedder_loads_real_model_when_env_set` for the
    /// opt-in end-to-end run.
    #[cfg(feature = "embeddings")]
    #[test]
    fn onnx_embedder_try_new_reports_missing_paths() {
        let bogus_model = std::path::PathBuf::from("/definitely/not/a/real/path/model.onnx");
        let bogus_tok = std::path::PathBuf::from("/definitely/not/a/real/path/tokenizer.json");
        let err = OnnxEmbedder::try_new(&bogus_model, &bogus_tok).unwrap_err();
        assert!(
            err.to_string().contains("DOC_LINTER_EMBED_MODEL"),
            "missing-model error should mention the env var: {err}"
        );

        // A real model path + missing tokenizer surfaces the
        // tokenizer env var instead.
        let real_path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml");
        let err = OnnxEmbedder::try_new(&real_path, &bogus_tok).unwrap_err();
        assert!(
            err.to_string().contains("DOC_LINTER_EMBED_TOKENIZER"),
            "missing-tokenizer error should mention the env var: {err}"
        );
    }

    /// Roadmap issue #28 v2 (v0.4.0): `try_from_env` returns an
    /// error (rather than panicking) when the env vars are unset,
    /// so `default_embedder`'s fallback path can detect and
    /// degrade cleanly. Run with both vars unset; if the host
    /// happens to have them set, skip — the assertion is about
    /// missing-var behaviour, not real loading.
    #[cfg(feature = "embeddings")]
    #[test]
    fn onnx_embedder_try_from_env_errors_when_vars_unset() {
        if std::env::var_os(ENV_MODEL_PATH).is_some()
            || std::env::var_os(ENV_TOKENIZER_PATH).is_some()
        {
            // Don't override the host's env. The opt-in
            // happy-path test below covers the with-vars case.
            return;
        }
        let err = OnnxEmbedder::try_from_env().unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains(ENV_MODEL_PATH) || msg.contains(ENV_TOKENIZER_PATH),
            "missing-env error should name the env var: {msg}"
        );
    }

    /// Roadmap issue #28 v2 (v0.4.0): opt-in end-to-end embedding
    /// round-trip. Only runs when both env vars are set to real
    /// files on disk — in CI / developer machines without the
    /// model downloaded the test silently passes (the missing-env
    /// case is covered above). When it does run, it asserts:
    /// - The embedder reports the model's declared dim.
    /// - `embed()` returns a vector of that length.
    /// - Semantically similar inputs have higher cosine similarity
    ///   than dissimilar ones. This is the property that makes the
    ///   v4 `query similar --backend embedding` actually work.
    #[cfg(feature = "embeddings")]
    #[test]
    fn onnx_embedder_loads_real_model_when_env_set() {
        let model_var = std::env::var(ENV_MODEL_PATH).ok();
        let tok_var = std::env::var(ENV_TOKENIZER_PATH).ok();
        let (Some(model), Some(tok)) = (model_var, tok_var) else {
            return;
        };
        let e = OnnxEmbedder::try_new(&model, &tok)
            .expect("real model + tokenizer should load via try_new");
        assert!(e.dim() > 0, "real model reports non-zero dim");
        assert!(e.is_available());
        assert_eq!(e.name(), "onnx");

        let v_a = e.embed("the cat sat on the mat").expect("embed text A");
        let v_b = e.embed("a feline rests on a rug").expect("embed text B");
        let v_c = e.embed("rust compiler error E0382").expect("embed text C");
        assert_eq!(v_a.len(), e.dim());
        assert_eq!(v_b.len(), e.dim());
        assert_eq!(v_c.len(), e.dim());

        // Cosine similarity of two L2-normalised vectors is their
        // dot product. We expect the cat/feline pair to be closer
        // than the cat/rust-error pair.
        let sim_ab: f32 = v_a.iter().zip(v_b.iter()).map(|(a, b)| a * b).sum();
        let sim_ac: f32 = v_a.iter().zip(v_c.iter()).map(|(a, c)| a * c).sum();
        assert!(
            sim_ab > sim_ac,
            "semantically similar pair should rank higher: ab={sim_ab}, ac={sim_ac}"
        );
    }

    /// Roadmap issue #28 v6 (v0.4.0): when the build.rs auto-
    /// download succeeded AND the host hasn't set the layer-1
    /// override env vars, `default_embedder()` should resolve to
    /// a working ONNX backend via the layer-2 bundled path.
    /// Asserts the bundled paths got baked in at compile time and
    /// that the resulting embedder reports `name() == "onnx"`.
    ///
    /// Skipped when:
    ///   - build.rs opted out of the download (`DOC_LINTER_SKIP_MODEL_DOWNLOAD=1`
    ///     or the download failed — build emits a warning, no
    ///     env vars get baked in).
    ///   - The layer-1 override env vars are set; the bundled
    ///     path doesn't get exercised in that case, and `unsafe`
    ///     env-var mutation isn't available to force the issue
    ///     (this crate forbids `unsafe_code`).
    #[cfg(feature = "embeddings")]
    #[test]
    fn bundled_embedder_loads_via_default_factory() {
        let bundled_model = option_env!("DOC_LINTER_BUNDLED_EMBED_MODEL");
        let bundled_tok = option_env!("DOC_LINTER_BUNDLED_EMBED_TOKENIZER");
        if bundled_model.is_none() || bundled_tok.is_none() {
            return;
        }
        if std::env::var_os(ENV_MODEL_PATH).is_some()
            || std::env::var_os(ENV_TOKENIZER_PATH).is_some()
        {
            // Host has the layer-1 override set; can't observe
            // the layer-2 path without `unsafe` env mutation.
            // The `onnx_embedder_loads_real_model_when_env_set`
            // test still covers correctness against whatever
            // model the override points at.
            return;
        }
        let e = default_embedder();
        assert_eq!(
            e.name(),
            "onnx",
            "with bundled model present, default_embedder() picks the ONNX backend; \
             got {:?} — bundled paths: model={bundled_model:?}, tok={bundled_tok:?}",
            e.name(),
        );
        assert!(
            e.is_available(),
            "bundled OnnxEmbedder must report is_available()==true"
        );
        assert!(e.dim() > 0, "bundled model declares non-zero dim");
    }

    /// `l2_normalize_in_place` produces a unit vector and is a
    /// no-op on the zero vector (avoids divide-by-zero — important
    /// because the empty-input embed() path can produce all-zero
    /// pooled vectors before normalisation).
    #[cfg(feature = "embeddings")]
    #[test]
    fn l2_normalize_produces_unit_vector_and_handles_zero() {
        let mut v = vec![3.0_f32, 4.0_f32];
        l2_normalize_in_place(&mut v);
        let norm: f32 = v.iter().map(|x| x * x).sum::<f32>().sqrt();
        assert!(
            (norm - 1.0).abs() < 1e-6,
            "L2 norm should be 1.0, got {norm}"
        );

        let mut z = vec![0.0_f32, 0.0_f32, 0.0_f32];
        l2_normalize_in_place(&mut z);
        assert_eq!(z, vec![0.0_f32, 0.0_f32, 0.0_f32], "zero vector unchanged");
    }
}
