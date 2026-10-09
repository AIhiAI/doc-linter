//! Roadmap issue #28 v3 (v0.4.0): post-ingest pass that fills the
//! fixed-dim `FLOAT[384]` embedding columns on `Doc`,
//! `Entity`, `Function`, and `Type` nodes. Runs after the existing
//! ingest writers (`ingest`, `ingest_scip`, the rederive path) so
//! the basic CREATE statements stay untouched — the embedding
//! pipeline is a strictly additive layer.
//!
//! ## When it runs
//!
//! `cmd_check` calls [`populate_embeddings`] once, near the end of
//! the check pipeline. The pass:
//!   1. Constructs an `Embedder` via [`crate::embeddings::default_embedder`].
//!   2. Short-circuits when `is_available() == false` (NoOp or
//!      missing model) — the column stays NULL, which is what
//!      every existing query expects.
//!   3. Otherwise scans each text column (Doc.summary,
//!      Entity.display, Function.doc_comment, Type.doc_comment),
//!      computes the embedding, and writes it back via
//!      `MATCH ... SET <col>_embedding = $vec`.
//!
//! ## Why per-table SET rather than fold into CREATE
//!
//! Keeps the ingest writers free of `Embedder` awareness — the
//! pre-#28 code paths (ingest.rs, code_ingest.rs, the rederive
//! path) continue to work unchanged. The cost is one extra round-
//! trip per row, dwarfed by the inference cost itself (~5ms per
//! input for MiniLM-L6-v2 on CPU). Re-running the populate pass
//! is idempotent: SET overwrites whatever was there.
//!
//! ## Dim mismatch
//!
//! The schema declares `FLOAT[384]`. The OnnxEmbedder reports its
//! actual dim from the loaded model. If the loaded model produces
//! something other than 384 (e.g. bge-base at 768), [`populate_embeddings`]
//! logs a one-line warning to stderr and skips the write — the
//! column stays NULL and the schema isn't violated. A v0.5.0
//! follow-up will parameterise the column shape so other-dim
//! models work natively.

use sha2::{Digest, Sha256};

/// Roadmap issue #28 v3 (v0.4.0): hardcoded column dimensionality
/// matching `all-MiniLM-L6-v2` (the recommended default model).
/// Other-dim models are caught by the `dim != EMBED_DIM` guard in
/// [`populate_embeddings`] and skip the write; the user sees a
/// stderr warning rather than a confusing schema mismatch.
pub const EMBED_DIM: usize = 384;

/// Section text fed to the embedder (~150-200 tokens). Measured on 120
/// ~1500-char sections: embedding the full 512-token window cost ~2.8 s
/// per section on 4 threads.
pub(crate) const SECTION_EMBED_CHARS: usize = 800;

/// Roadmap issue #28 v3 (v0.4.0): post-ingest stats from
/// [`populate_embeddings`]. Surfaced via `cmd_check`'s stderr
/// summary so operators can see at a glance whether the embedder
/// kicked in.
#[derive(Debug, Default, Clone, Copy)]
pub struct EmbeddingStats {
    /// Total rows visited across Doc / Entity / Function / Type / Section.
    /// Equals the sum of the per-table `*_visited` counters below.
    pub rows_visited: usize,
    /// Rows for which an embedding was successfully computed and
    /// written. Bounded above by `rows_visited`; the difference is
    /// rows whose text column was empty (no signal to embed) or
    /// whose embed call returned an error.
    pub rows_written: usize,
    /// Rows whose text + model fingerprint matched the hash already
    /// stored alongside the embedding column, so the existing
    /// vector was reused and no forward pass was run. Critical for
    /// the warm path: on an unchanged repo the entire embedding
    /// pass collapses to a single read query plus the hash compare.
    pub rows_skipped_unchanged: usize,
    /// Per-table breakdown of `rows_written` so the stderr summary
    /// can spell out which surfaces got coverage.
    pub doc_written: usize,
    /// See [`Self::doc_written`].
    pub entity_written: usize,
    /// See [`Self::doc_written`].
    pub function_written: usize,
    /// See [`Self::doc_written`].
    pub type_written: usize,
    /// See [`Self::doc_written`].
    pub section_written: usize,
    /// Backend name reported by `Embedder::name()` — captured so
    /// the stderr summary can name the model that wrote the
    /// vectors (helps when comparing two ingest runs against
    /// different model files).
    pub backend: &'static str,
}

/// Fingerprint of a row's text plus the backend that would produce
/// the vector. Stored in `<text_col>_embed_hash` alongside the
/// embedding so a re-run can read-then-skip identical rows without
/// running the forward pass. Truncated to 32 hex chars (128 bits) —
/// collisions at that width on a corpus of ~10k rows are
/// astronomically unlikely, and a shorter string keeps the column
/// cheap in storage and easy on the eye when debugging.
///
/// Backend name is part of the input so swapping `onnx` for a
/// hypothetical future `cohere` backend forces every cached vector
/// to recompute instead of silently mixing vector spaces.
pub(crate) fn fingerprint(backend: &str, text: &str) -> String {
    let mut h = Sha256::new();
    h.update(backend.as_bytes());
    h.update(b":");
    h.update(text.as_bytes());
    let digest = h.finalize();
    let mut out = String::with_capacity(32);
    for b in digest.iter().take(16) {
        use std::fmt::Write;
        let _ = write!(out, "{b:02x}");
    }
    out
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "tests panic on broken invariants — that is the point of a test"
)]
mod tests {}
