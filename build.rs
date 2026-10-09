// The single-char variable names in the SHA-256 compression
// function and the bare `0xABCDEF12` constants in the K-table
// are intentional — they match FIPS 180-4 §6.2 and the wider
// implementation literature exactly so a reader cross-checking
// against the spec doesn't have to translate names. The 16 KiB
// `large_stack_arrays` threshold is geared at runtime code; in a
// build script the I/O buffer for SHA-256 streaming is allowed
// to be larger.
#![allow(clippy::similar_names)]
#![allow(clippy::single_char_lifetime_names)]
#![allow(clippy::many_single_char_names)]
#![allow(clippy::unreadable_literal)]
#![allow(clippy::large_stack_arrays)]

//! Roadmap issue #28 v6 (v0.4.0): build-time sentence-transformer
//! download.
//!
//! Runs only when `--features embeddings` is enabled (we gate on
//! the `CARGO_FEATURE_EMBEDDINGS` env var Cargo sets for us). The
//! goal is to make `cargo install doc-linter --features embeddings`
//! work out of the box without forcing operators to chase
//! HuggingFace URLs.
//!
//! ## What we ship
//!
//! [`bge-small-en-v1.5`](https://huggingface.co/BAAI/bge-small-en-v1.5)
//! via Xenova's pre-quantized ONNX export. 33M params, 384-dim,
//! MTEB ~62 — currently best-in-class on the small / 384-dim tier
//! (the dim the v3 schema is locked to). Quantized weights ship as
//! ~34 MB which is a 4× reduction over the f32 export and runs
//! comfortably on consumer CPUs via `tract`.
//!
//! ## Where files land
//!
//! Cached under the user's standard cache directory:
//!   - `$XDG_CACHE_HOME/doc-linter/models/bge-small-en-v1.5/`
//!   - `$HOME/.cache/doc-linter/models/bge-small-en-v1.5/` on Linux/macOS without XDG
//!   - `%LOCALAPPDATA%\doc-linter\models\bge-small-en-v1.5\` on Windows
//!
//! Re-runs of `cargo build` are no-ops when the files already
//! exist and SHA-256 match.
//!
//! ## Integrity
//!
//! Every downloaded byte is SHA-256-verified against pins captured
//! at PR-merge time. A mismatch fails the build with a clear
//! message — this is the security boundary for "the model that
//! arrived is the model we expected". The pins are updated in
//! lockstep when we bump Xenova model revisions.
//!
//! ## Opt-out
//!
//! Set `DOC_LINTER_SKIP_MODEL_DOWNLOAD=1` to skip the download
//! entirely (air-gapped builds, vendored mirrors, CI that
//! supplies its own model via `DOC_LINTER_EMBED_MODEL`). The
//! runtime then falls back to whatever `DOC_LINTER_EMBED_MODEL` /
//! `DOC_LINTER_EMBED_TOKENIZER` point at, or to `NoOpEmbedder`.
//!
//! ## Why this isn't done at runtime
//!
//! Doing the download at first `embed()` call would stall an
//! agent-facing `cmd_check` on a multi-megabyte network round-trip.
//! Doing it at build time amortises the cost to "once per machine
//! when the operator opts into the feature" — they ran
//! `cargo install --features embeddings` deliberately and a
//! one-minute network blip in that step is expected.

use std::env;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Roadmap issue #28 v6: the model + tokenizer pair we auto-fetch
/// when the operator builds with `--features embeddings`. Switching
/// to a different model in a future release means bumping all four
/// constants below (URLs + SHA-256 pins) together; the runtime
/// dimensionality must still match the `EMBED_DIM = 384` constant
/// in `store::populate_embeddings`.
const MODEL_NAME: &str = "bge-small-en-v1.5";

/// Quantized ONNX (~34 MB) via Xenova's pre-converted export of
/// BAAI/bge-small-en-v1.5. Quantized weights cut size 4× off the
/// f32 export and remain accuracy-competitive (well within MTEB
/// noise floor) for sentence-similarity workloads.
const MODEL_URL: &str =
    "https://huggingface.co/Xenova/bge-small-en-v1.5/resolve/main/onnx/model_quantized.onnx";

/// SHA-256 of the model file captured at v0.4.0 PR-merge time.
/// Pinned so a silent upstream replacement can't substitute a
/// different model under our nose.
const MODEL_SHA256: &str = "6c9c6101a956d62dfb5e7190c538226c0c5bb9cb27b651234b6df063ee7dbfe4";

/// Pre-built HuggingFace tokenizer config matched to the model
/// above. Required because the same BERT topology can be driven
/// by different vocab / normalizer combos; the `tokenizer.json`
/// is the authoritative pairing.
const TOKENIZER_URL: &str =
    "https://huggingface.co/Xenova/bge-small-en-v1.5/resolve/main/tokenizer.json";

/// SHA-256 pin for the tokenizer JSON.
const TOKENIZER_SHA256: &str = "d241a60d5e8f04cc1b2b3e9ef7a4921b27bf526d9f6050ab90f9267a1f9e5c66";

/// Generate `$OUT_DIR/saved_sql_table.rs`: `(name, include_str!)` for every
/// `src/store_sqlite/saved_sql/*.sql` (dependency-free embed).
fn gen_saved_sql_table() {
    let dir = std::path::Path::new(&env::var("CARGO_MANIFEST_DIR").unwrap_or_default())
        .join("src/store_sqlite/saved_sql");
    println!("cargo:rerun-if-changed={}", dir.display());
    let mut names: Vec<String> = std::fs::read_dir(&dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| e.file_name().to_str().map(str::to_owned))
        .filter(|n| n.ends_with(".sql"))
        .collect();
    names.sort();
    let mut out = String::from("pub static SAVED_SQL: &[(&str, &str)] = &[\n");
    for n in &names {
        let stem = n.trim_end_matches(".sql");
        out.push_str(&format!(
            "    ({stem:?}, include_str!({:?})),\n",
            dir.join(n).display().to_string()
        ));
    }
    out.push_str("];\n");
    let dest =
        std::path::Path::new(&env::var("OUT_DIR").unwrap_or_default()).join("saved_sql_table.rs");
    let _ = std::fs::write(dest, out);
}

fn main() {
    gen_saved_sql_table();
    // Always tell Cargo to rerun build.rs when env knobs change.
    // The `if-changed` directives ensure that toggling the
    // embeddings feature, the opt-out var, or the user's model
    // override doesn't require a clean rebuild.
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-env-changed=CARGO_FEATURE_EMBEDDINGS");
    println!("cargo:rerun-if-env-changed=DOC_LINTER_SKIP_MODEL_DOWNLOAD");
    println!("cargo:rerun-if-env-changed=DOC_LINTER_EMBED_MODEL");
    println!("cargo:rerun-if-env-changed=DOC_LINTER_EMBED_TOKENIZER");

    if env::var_os("CARGO_FEATURE_EMBEDDINGS").is_none() {
        // Default build: nothing to do. The whole module is
        // feature-gated; emitting no env vars means the runtime
        // factory falls back to NoOpEmbedder, which is exactly
        // the v0.3.0 behaviour.
        return;
    }

    if env::var_os("DOC_LINTER_SKIP_MODEL_DOWNLOAD").is_some() {
        // Opt-out path: skip the download entirely. The runtime
        // still honours `DOC_LINTER_EMBED_MODEL` at startup, so a
        // vendored mirror or air-gapped CI can plug its own files
        // in via env vars.
        println!(
            "cargo:warning=doc-linter: DOC_LINTER_SKIP_MODEL_DOWNLOAD is set; \
             skipping bundled model download. Set DOC_LINTER_EMBED_MODEL + \
             DOC_LINTER_EMBED_TOKENIZER at runtime, or unset the skip var \
             and rebuild to fetch bge-small-en-v1.5."
        );
        return;
    }

    let Some(cache_root) = pick_cache_dir() else {
        println!(
            "cargo:warning=doc-linter: couldn't resolve a user cache directory \
             (no XDG_CACHE_HOME, HOME, or LOCALAPPDATA); skipping model download. \
             Override at runtime with DOC_LINTER_EMBED_MODEL + DOC_LINTER_EMBED_TOKENIZER."
        );
        return;
    };
    let model_dir = cache_root
        .join("doc-linter")
        .join("models")
        .join(MODEL_NAME);
    if let Err(e) = fs::create_dir_all(&model_dir) {
        println!(
            "cargo:warning=doc-linter: failed to create {}: {e}. Skipping download.",
            model_dir.display()
        );
        return;
    }

    let model_path = model_dir.join("model.onnx");
    let tokenizer_path = model_dir.join("tokenizer.json");

    if let Err(e) = ensure_file(&model_path, MODEL_URL, MODEL_SHA256) {
        println!(
            "cargo:warning=doc-linter: model download failed ({e}); the binary will \
             still build, but `default_embedder()` will fall back to NoOpEmbedder \
             unless DOC_LINTER_EMBED_MODEL is set at runtime."
        );
        return;
    }
    if let Err(e) = ensure_file(&tokenizer_path, TOKENIZER_URL, TOKENIZER_SHA256) {
        println!(
            "cargo:warning=doc-linter: tokenizer download failed ({e}); the binary will \
             still build, but `default_embedder()` will fall back to NoOpEmbedder \
             unless DOC_LINTER_EMBED_TOKENIZER is set at runtime."
        );
        return;
    }

    // Bake the paths in as compile-time env vars. The runtime
    // `default_embedder` reads these via `option_env!()` and uses
    // them iff the user hasn't supplied their own override.
    println!(
        "cargo:rustc-env=DOC_LINTER_BUNDLED_EMBED_MODEL={}",
        model_path.display()
    );
    println!(
        "cargo:rustc-env=DOC_LINTER_BUNDLED_EMBED_TOKENIZER={}",
        tokenizer_path.display()
    );
}

/// Resolve the user's cache directory, honoring XDG on Linux,
/// `$HOME/Library/Caches` on macOS via the same XDG fallback, and
/// `%LOCALAPPDATA%` on Windows. Returns `None` when none of the
/// expected env vars are set — happens in stripped CI images
/// where build.rs gracefully degrades.
fn pick_cache_dir() -> Option<PathBuf> {
    if let Some(x) = env::var_os("XDG_CACHE_HOME") {
        let p = PathBuf::from(x);
        if !p.as_os_str().is_empty() {
            return Some(p);
        }
    }
    if let Some(h) = env::var_os("HOME") {
        let p = PathBuf::from(h);
        if !p.as_os_str().is_empty() {
            return Some(p.join(".cache"));
        }
    }
    if let Some(l) = env::var_os("LOCALAPPDATA") {
        let p = PathBuf::from(l);
        if !p.as_os_str().is_empty() {
            return Some(p);
        }
    }
    None
}

/// Ensure `path` exists and matches `expected_sha256`. Downloads
/// from `url` via system `curl` if missing; verifies SHA on
/// every build (so a partial-download corruption gets caught on
/// the next build rather than at first runtime use).
fn ensure_file(path: &Path, url: &str, expected_sha256: &str) -> Result<(), String> {
    if path.exists() {
        let actual = sha256_of(path)?;
        if actual.eq_ignore_ascii_case(expected_sha256) {
            return Ok(());
        }
        // Mismatch — re-download (the previous attempt might have
        // been truncated; or the upstream pin moved). Surface the
        // mismatch as a warning so operators see it in build
        // output, then attempt the fresh download.
        println!(
            "cargo:warning=doc-linter: SHA-256 mismatch on cached {} \
             (got {}, expected {}); re-downloading.",
            path.display(),
            actual,
            expected_sha256
        );
        let _ = fs::remove_file(path);
    }
    download_with_curl(url, path)?;
    let actual = sha256_of(path)?;
    if !actual.eq_ignore_ascii_case(expected_sha256) {
        let _ = fs::remove_file(path);
        return Err(format!(
            "SHA-256 mismatch after download: got {actual}, expected {expected_sha256}. \
             The cached file has been removed; re-run the build to retry, or check \
             whether MODEL_SHA256 / TOKENIZER_SHA256 need bumping for a new Xenova revision."
        ));
    }
    Ok(())
}

/// Shell out to system `curl` rather than pulling in a Rust HTTP
/// stack as a build-dependency. The standard install base for
/// doc-linter (Linux / macOS dev machines, CI runners) all ship
/// curl by default; missing-curl is surfaced as a clean error
/// that points at the opt-out env var.
fn download_with_curl(url: &str, dest: &Path) -> Result<(), String> {
    let status = Command::new("curl")
        .arg("--fail")
        .arg("--silent")
        .arg("--show-error")
        .arg("--location")
        .arg("--output")
        .arg(dest)
        .arg(url)
        .status()
        .map_err(|e| {
            format!(
                "spawn `curl`: {e}. Install curl, or set \
                 DOC_LINTER_SKIP_MODEL_DOWNLOAD=1 and supply \
                 DOC_LINTER_EMBED_MODEL at runtime."
            )
        })?;
    if !status.success() {
        return Err(format!(
            "curl exited with {status} fetching {url} into {}",
            dest.display()
        ));
    }
    Ok(())
}

/// Streaming SHA-256 implementation. Avoids pulling in a sha2
/// crate as a build-dep — build scripts pay the compile cost
/// for every reverse dep, so the ~150-line bespoke impl is the
/// cheaper choice for "one hash computed twice per build".
fn sha256_of(path: &Path) -> Result<String, String> {
    let mut file =
        fs::File::open(path).map_err(|e| format!("open {} for hashing: {e}", path.display()))?;
    let mut hasher = Sha256::new();
    let mut buf = [0u8; 64 * 1024];
    loop {
        let n = file
            .read(&mut buf)
            .map_err(|e| format!("read {} for hashing: {e}", path.display()))?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hasher.finalize_hex())
}

// ---------------------------------------------------------------------
// Minimal SHA-256 implementation (FIPS 180-4 §6.2). Kept inline so
// the build script has no third-party Rust dependencies — every
// reverse dep would otherwise pay the compile cost of pulling
// `sha2` + its `cpufeatures` / `block-buffer` graph just to run a
// hash that fires once per cargo build.
// ---------------------------------------------------------------------

struct Sha256 {
    state: [u32; 8],
    buf: [u8; 64],
    buf_len: usize,
    total_len: u64,
}

const K: [u32; 64] = [
    0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5,
    0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174,
    0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da,
    0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7, 0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967,
    0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85,
    0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070,
    0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
    0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2,
];

const H0: [u32; 8] = [
    0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab, 0x5be0cd19,
];

impl Sha256 {
    fn new() -> Self {
        Sha256 {
            state: H0,
            buf: [0u8; 64],
            buf_len: 0,
            total_len: 0,
        }
    }

    fn update(&mut self, mut data: &[u8]) {
        self.total_len = self.total_len.wrapping_add(data.len() as u64);
        if self.buf_len > 0 {
            let take = (64 - self.buf_len).min(data.len());
            self.buf[self.buf_len..self.buf_len + take].copy_from_slice(&data[..take]);
            self.buf_len += take;
            data = &data[take..];
            if self.buf_len == 64 {
                let block = self.buf;
                Self::compress(&mut self.state, &block);
                self.buf_len = 0;
            }
        }
        while data.len() >= 64 {
            let mut block = [0u8; 64];
            block.copy_from_slice(&data[..64]);
            Self::compress(&mut self.state, &block);
            data = &data[64..];
        }
        if !data.is_empty() {
            self.buf[..data.len()].copy_from_slice(data);
            self.buf_len = data.len();
        }
    }

    fn finalize_hex(mut self) -> String {
        let bit_len = self.total_len.wrapping_mul(8);
        // Append 0x80.
        self.buf[self.buf_len] = 0x80;
        self.buf_len += 1;
        if self.buf_len > 56 {
            // Pad current block and compress, then prepare new.
            for b in &mut self.buf[self.buf_len..] {
                *b = 0;
            }
            let block = self.buf;
            Self::compress(&mut self.state, &block);
            self.buf = [0u8; 64];
            self.buf_len = 0;
        }
        for b in &mut self.buf[self.buf_len..56] {
            *b = 0;
        }
        self.buf[56..].copy_from_slice(&bit_len.to_be_bytes());
        let block = self.buf;
        Self::compress(&mut self.state, &block);
        let mut out = String::with_capacity(64);
        for word in &self.state {
            out.push_str(&format!("{word:08x}"));
        }
        out
    }

    fn compress(state: &mut [u32; 8], block: &[u8; 64]) {
        let mut w = [0u32; 64];
        for (i, chunk) in block.chunks_exact(4).enumerate() {
            w[i] = u32::from_be_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]);
        }
        for i in 16..64 {
            let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
            let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
            w[i] = w[i - 16]
                .wrapping_add(s0)
                .wrapping_add(w[i - 7])
                .wrapping_add(s1);
        }
        let mut a = state[0];
        let mut b = state[1];
        let mut c = state[2];
        let mut d = state[3];
        let mut e = state[4];
        let mut f = state[5];
        let mut g = state[6];
        let mut h = state[7];
        for i in 0..64 {
            let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let ch = (e & f) ^ (!e & g);
            let temp1 = h
                .wrapping_add(s1)
                .wrapping_add(ch)
                .wrapping_add(K[i])
                .wrapping_add(w[i]);
            let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let maj = (a & b) ^ (a & c) ^ (b & c);
            let temp2 = s0.wrapping_add(maj);
            h = g;
            g = f;
            f = e;
            e = d.wrapping_add(temp1);
            d = c;
            c = b;
            b = a;
            a = temp1.wrapping_add(temp2);
        }
        state[0] = state[0].wrapping_add(a);
        state[1] = state[1].wrapping_add(b);
        state[2] = state[2].wrapping_add(c);
        state[3] = state[3].wrapping_add(d);
        state[4] = state[4].wrapping_add(e);
        state[5] = state[5].wrapping_add(f);
        state[6] = state[6].wrapping_add(g);
        state[7] = state[7].wrapping_add(h);
    }
}
