//! Roadmap issue #33 (v0.3.0): SCIP ingest cache.
//!
//! Persists a tiny `.doc-lint/ingest-cache.json` after each successful
//! SCIP ingest capturing the inputs that determined the ingest content:
//! the SCIP file's mtime + byte length, the schema generation
//! identifier the ingest wrote against, and the resulting Function /
//! Type counts.
//!
//! ## v1 scope
//!
//! The cache file is **informational** for v0.3.0 — the SCIP ingest
//! still runs every `cmd_check` because the graph schema is wiped on
//! every run (see `schema::reset_schema`). When the cache hits
//! (same scip mtime + matching schema version), `cmd_check` emits a
//! stderr line saying so. The actual skip-path requires splitting
//! `reset_schema` so the code-graph tables can be preserved across
//! runs — that's a v0.4.0 follow-up that this cache scaffolds.
//!
//! `--rebuild` suppresses the cache-hit line (and, in v0.4.0, will
//! force a full re-ingest).
//!
//! ## Change detection
//!
//! v0.4.0 (issue #33 v1): mtime + byte length + SHA-256 over the SCIP
//! bytes. The hash is the authority — mtime+len stay so a fast
//! pre-check can short-circuit a miss without re-reading the file.
//! Same-second, same-length edits (which the v0.3.0 mtime-only cache
//! acknowledged as an accepted collision) now invalidate cleanly.
//!
//! The `schema_version` bump from 1 → 2 auto-invalidates any v0.3.0
//! cache file in the wild (the missing `scip_content_sha256` field
//! would have deserialised to default and silently produced a false
//! hit otherwise).
//!
//! ## File format
//!
//! ```json
//! {
//!   "schema_version": 2,
//!   "scip_mtime_secs": 1715000000,
//!   "scip_byte_len": 12345,
//!   "scip_content_sha256": "9f86d081884c7d659a2feaa0c55ad015a3bf4f1b...",
//!   "ingested_at": "2026-05-19T01:23:45Z",
//!   "function_count": 12345,
//!   "type_count": 678
//! }
//! ```

use std::fs;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// Bump this whenever the graph schema gains/loses a column or table
/// that the SCIP ingest writes. A mismatch with the cache forces a
/// full rebuild. The cache file's [`IngestCache::schema_version`]
/// reflects the version at the time of the last successful ingest.
///
/// Bumped to `2` for v0.4.0: aggregates schema-impacting changes that
/// shipped this release —
///   * #33 v1 added `scip_content_sha256`. Old caches lack the field
///     and would silently default to the empty string, producing false
///     hits.
///   * #28 v3 added FLOAT[384] embedding columns to Doc / Entity /
///     Function / Type tables. Cache-hit rederive doesn't recreate
///     these tables, so a v0.3.0 cache against an upgraded DB would
///     leave the columns absent and the post-pass populate_embeddings
///     would fail with "unknown property". One forced re-ingest after
///     upgrade lays the columns down.
///   * #32 v4+ added METHOD_OF / USES_TYPE / IMPLEMENTS / EXTENDS rel
///     tables. The cache hit path doesn't recreate code-graph tables,
///     so the bump forces one full re-ingest after upgrade.
///
/// Bumped to `3` for Java support: scip-java crate names now resolve to
/// the Maven artifact and `Type#method()` members get METHOD_OF edges, so
/// graphs built from an unchanged `code.scip` by an older binary are stale.
///
/// Bumped to `4` for the `Field` node and `REFERENCES` rel tables, which
/// the cache-hit path doesn't create.
///
/// Bumped to `5`: calls to a second or later overload (`foo(+1).`) are
/// CALLS edges; graphs built before dropped them.
///
/// Bumped to `6`: compiler-generated Java members (Lombok, records,
/// enums) carry `Function.generated` and are no longer call-graph
/// callers.
///
/// Bumped to `7`: static methods and constructors (SCIP `StaticMethod`,
/// `Constructor`, …) are Function rows; graphs built before dropped them.
///
/// Bumped to `8`: CALLS carries `dispatch`, and a call through an
/// interface method also reaches its implementations (#268).
///
/// Bumped to `9`: scip-python import statements become IMPORTS edges
/// (#227); graphs built before had none for Python.
pub const SCHEMA_VERSION: u32 = 9;

/// On-disk cache snapshot. Versioned via `schema_version` so a
/// schema bump invalidates older cache files automatically — any
/// missing/unknown field deserialises as default and a `cache_hit`
/// comparison fails.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IngestCache {
    pub schema_version: u32,
    pub scip_mtime_secs: u64,
    pub scip_byte_len: u64,
    /// Lowercase hex SHA-256 of the SCIP file bytes at ingest time.
    /// v0.4.0 (#33 v1): mtime+len alone allowed a same-second,
    /// same-length collision to look like a hit; the hash closes it.
    #[serde(default)]
    pub scip_content_sha256: String,
    pub ingested_at: String,
    pub function_count: u64,
    pub type_count: u64,
}

/// Path the cache file lives at — `<root>/.doc-lint/ingest-cache.json`.
pub fn cache_path(root: &Path) -> PathBuf {
    root.join(".doc-lint").join("ingest-cache.json")
}

/// Build a fresh cache snapshot from the current SCIP file + ingest
/// stats. Returns `None` when the SCIP file can't be stat'd or read —
/// the caller treats that as "skip the cache write" rather than
/// failing the ingest.
pub fn build_snapshot(
    scip_path: &Path,
    function_count: u64,
    type_count: u64,
) -> Option<IngestCache> {
    let meta = fs::metadata(scip_path).ok()?;
    let mtime = meta.modified().ok()?;
    let secs = mtime.duration_since(SystemTime::UNIX_EPOCH).ok()?.as_secs();
    let sha = sha256_hex(scip_path)?;
    Some(IngestCache {
        schema_version: SCHEMA_VERSION,
        scip_mtime_secs: secs,
        scip_byte_len: meta.len(),
        scip_content_sha256: sha,
        ingested_at: chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string(),
        function_count,
        type_count,
    })
}

/// Lowercase hex SHA-256 of the given file. `None` on any read error
/// so callers can fall back to "skip the cache write" without
/// panicking the ingest.
fn sha256_hex(path: &Path) -> Option<String> {
    let bytes = fs::read(path).ok()?;
    let mut hasher = Sha256::new();
    hasher.update(&bytes);
    Some(format!("{:x}", hasher.finalize()))
}

/// Persist the cache to disk. Returns the path written for the
/// caller's stderr summary.
pub fn write(root: &Path, snapshot: &IngestCache) -> Result<PathBuf> {
    write_at(&cache_path(root), snapshot)
}

/// Cache file of the SQLite store. Kept apart from the the store one: the two
/// engines hold separate graphs, so one engine's "unchanged SCIP file"
/// says nothing about the other's preserved code rows.
pub fn sqlite_cache_path(root: &Path) -> PathBuf {
    root.join(".doc-lint").join("ingest-cache.sqlite.json")
}

/// [`write`] to an explicit cache file.
pub fn write_at(path: &Path, snapshot: &IngestCache) -> Result<PathBuf> {
    let path = path.to_path_buf();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
    }
    let json = serde_json::to_string_pretty(snapshot).context("serialize ingest cache")?;
    fs::write(&path, json).with_context(|| format!("write {}", path.display()))?;
    Ok(path)
}

/// Read the cache file if it exists. Returns `None` on any read /
/// parse error — the caller treats a malformed cache as a miss and
/// will overwrite it after the next successful ingest.
pub fn read(root: &Path) -> Option<IngestCache> {
    read_at(&cache_path(root))
}

/// [`read`] from an explicit cache file.
pub fn read_at(path: &Path) -> Option<IngestCache> {
    let body = fs::read_to_string(path).ok()?;
    serde_json::from_str(&body).ok()
}

/// Roadmap issue #33 v2: convenience for the cache-aware dispatch
/// path. Returns true when the SCIP ingest can be skipped — `rebuild`
/// not set, cache file present, scip file matches.
pub fn should_skip_ingest(root: &Path, scip_path: &Path, rebuild: bool) -> bool {
    if rebuild {
        return false;
    }
    let Some(cached) = read(root) else {
        return false;
    };
    cache_hit(scip_path, &cached)
}

/// Returns true when the cached snapshot matches what would be
/// produced by a fresh ingest of the SCIP file — same schema
/// version, same scip mtime + byte length, and same SHA-256 over
/// the file bytes.
///
/// mtime + len are cheap stat-only checks; if either differs we
/// avoid the hash entirely. The hash is the authority — two files
/// with the same length and same-second mtime but different
/// contents (the corner case the v0.3.0 cache explicitly accepted)
/// now miss cleanly.
///
/// An empty `scip_content_sha256` in the cache (only possible if a
/// caller hand-constructs a snapshot without going through
/// [`build_snapshot`]) is treated as a miss so the next ingest writes
/// a populated entry.
pub fn cache_hit(scip_path: &Path, cached: &IngestCache) -> bool {
    if cached.schema_version != SCHEMA_VERSION {
        return false;
    }
    if cached.scip_content_sha256.is_empty() {
        return false;
    }
    let Ok(meta) = fs::metadata(scip_path) else {
        return false;
    };
    if meta.len() != cached.scip_byte_len {
        return false;
    }
    let Ok(mtime) = meta.modified() else {
        return false;
    };
    let Ok(elapsed) = mtime.duration_since(SystemTime::UNIX_EPOCH) else {
        return false;
    };
    if elapsed.as_secs() != cached.scip_mtime_secs {
        return false;
    }
    match sha256_hex(scip_path) {
        Some(hex) => hex == cached.scip_content_sha256,
        None => false,
    }
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

    fn unique_tmpdir(label: &str) -> PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("doc-linter-cache-{label}-{nanos}"));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn round_trip_preserves_fields() {
        let dir = unique_tmpdir("round-trip");
        let snap = IngestCache {
            schema_version: SCHEMA_VERSION,
            scip_mtime_secs: 1_715_000_000,
            scip_byte_len: 4096,
            scip_content_sha256: "a".repeat(64),
            ingested_at: "2026-05-19T01:23:45Z".to_string(),
            function_count: 42,
            type_count: 7,
        };
        write(&dir, &snap).unwrap();
        let back = read(&dir).expect("cache present");
        assert_eq!(back.schema_version, snap.schema_version);
        assert_eq!(back.scip_mtime_secs, snap.scip_mtime_secs);
        assert_eq!(back.scip_byte_len, snap.scip_byte_len);
        assert_eq!(back.scip_content_sha256, snap.scip_content_sha256);
        assert_eq!(back.function_count, snap.function_count);
        assert_eq!(back.type_count, snap.type_count);
    }

    #[test]
    fn build_snapshot_populates_sha256() {
        let dir = unique_tmpdir("sha-build");
        let scip_path = dir.join("code.scip");
        fs::write(&scip_path, b"hello").unwrap();
        let snap = build_snapshot(&scip_path, 1, 0).unwrap();
        // Known SHA-256("hello") for the regression-test bite.
        assert_eq!(
            snap.scip_content_sha256,
            "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824"
        );
    }

    #[test]
    fn cache_miss_when_sha256_field_empty() {
        let dir = unique_tmpdir("empty-sha");
        let scip_path = dir.join("code.scip");
        fs::write(&scip_path, b"hello").unwrap();
        let mut snap = build_snapshot(&scip_path, 1, 0).unwrap();
        snap.scip_content_sha256.clear();
        assert!(
            !cache_hit(&scip_path, &snap),
            "empty sha256 (legacy/half-constructed cache) → miss"
        );
    }

    #[test]
    fn cache_miss_on_same_len_same_mtime_different_bytes() {
        // The exact corner case the v0.3.0 mtime+len cache accepted.
        // With the content hash this must miss.
        let dir = unique_tmpdir("collision");
        let scip_path = dir.join("code.scip");
        fs::write(&scip_path, b"AAAAAAAA").unwrap();
        let snap = build_snapshot(&scip_path, 1, 0).unwrap();
        // Overwrite atomically preserving length; mtime-second may or
        // may not tick depending on filesystem granularity, but len
        // is identical and we want the hash to be the deciding factor
        // even when mtime hasn't moved. Force the mtime field on the
        // cached snapshot to whatever the new file reports so the
        // pre-hash checks pass, exposing the hash check.
        fs::write(&scip_path, b"BBBBBBBB").unwrap();
        let new_meta = fs::metadata(&scip_path).unwrap();
        let new_secs = new_meta
            .modified()
            .unwrap()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        let forced = IngestCache {
            scip_mtime_secs: new_secs,
            scip_byte_len: new_meta.len(),
            ..snap
        };
        assert!(
            !cache_hit(&scip_path, &forced),
            "same len + forced same mtime but different bytes → miss"
        );
    }

    #[test]
    fn cache_miss_on_schema_bump() {
        let dir = unique_tmpdir("schema-bump");
        let scip_path = dir.join("code.scip");
        fs::write(&scip_path, b"hello").unwrap();
        let mut snap = build_snapshot(&scip_path, 1, 1).unwrap();
        snap.schema_version = SCHEMA_VERSION + 99;
        assert!(!cache_hit(&scip_path, &snap), "bumped version → miss");
    }

    #[test]
    fn cache_hit_on_unchanged_file() {
        let dir = unique_tmpdir("unchanged");
        let scip_path = dir.join("code.scip");
        fs::write(&scip_path, b"hello world").unwrap();
        let snap = build_snapshot(&scip_path, 9, 2).unwrap();
        assert!(cache_hit(&scip_path, &snap), "unchanged file → hit");
    }

    #[test]
    fn cache_miss_on_size_change() {
        let dir = unique_tmpdir("size-change");
        let scip_path = dir.join("code.scip");
        fs::write(&scip_path, b"short").unwrap();
        let snap = build_snapshot(&scip_path, 1, 0).unwrap();
        // Rewrite with different size — even on identical mtime the
        // byte-length check forces a miss.
        fs::write(&scip_path, b"meaningfully longer content").unwrap();
        assert!(!cache_hit(&scip_path, &snap), "size changed → miss");
    }

    #[test]
    fn missing_scip_file_is_miss() {
        let dir = unique_tmpdir("missing");
        let snap = IngestCache {
            schema_version: SCHEMA_VERSION,
            scip_mtime_secs: 1,
            scip_byte_len: 1,
            scip_content_sha256: "a".repeat(64),
            ingested_at: "2026-05-19T00:00:00Z".to_string(),
            function_count: 0,
            type_count: 0,
        };
        assert!(!cache_hit(&dir.join("never-existed.scip"), &snap));
    }

    #[test]
    fn legacy_v1_cache_file_misses_after_schema_bump() {
        // A v0.3.0 cache file on disk (schema_version = 1, no
        // scip_content_sha256) must miss after the v0.4.0 bump so
        // the next ingest writes a fresh v2 entry instead of
        // silently reusing partial data.
        let dir = unique_tmpdir("legacy-v1");
        fs::create_dir_all(dir.join(".doc-lint")).unwrap();
        let legacy = r#"{
            "schema_version": 1,
            "scip_mtime_secs": 1715000000,
            "scip_byte_len": 4096,
            "ingested_at": "2026-05-19T01:23:45Z",
            "function_count": 42,
            "type_count": 7
        }"#;
        fs::write(cache_path(&dir), legacy).unwrap();
        let scip_path = dir.join("code.scip");
        fs::write(&scip_path, b"whatever").unwrap();
        let cached = read(&dir).expect("legacy cache parses with serde default");
        assert_eq!(cached.schema_version, 1);
        assert_eq!(cached.scip_content_sha256, "");
        assert!(!cache_hit(&scip_path, &cached), "schema bump → miss");
    }
}
