//! Roadmap issue #27 (v0.3.0): Migration node — makes ontology
//! version bumps queryable.
//!
//! Walks the ingest file list, picks out docs with `role:
//! ontology-migration`, and emits one Migration row per file
//! populated from frontmatter (`from_version`, `to_version`,
//! `applied`, `applied_at`, plus the standard `title` / `summary`).
//!
//! v1 surface: nodes only. The `ADDS` / `REMOVES` edges the issue
//! sketches (Migration → Entity, Migration → AxisValue) need a
//! body-parse pass that I'm deferring — they require the
//! migration narrative to declare what it changed in a
//! machine-readable form, and a follow-up issue should either
//! standardise that frontmatter shape or write a tree-sitter
//! markdown parser.
//!
//! ## Why a separate ingest pass
//!
//! Migration docs are already captured as ordinary `Doc` rows by
//! `ingest::ingest` (their `role` is just one of the recognised
//! ontology roles). This pass enriches them into typed
//! `Migration` rows so the SQL surface gets `MATCH
//! (m:Migration) ORDER BY m.to_version` instead of needing to
//! filter `Doc.role = 'ontology-migration'`. The Migration node
//! coexists with the Doc node for the same source file — they
//! share an `id`.

/// Stats returned to `cmd_check` for the stderr one-liner.
#[derive(Debug, Default, Clone, Copy)]
pub struct MigrationIngestStats {
    /// Total Migration rows inserted.
    pub migrations: usize,
    /// How many of those carried `applied: true` (rolled-out
    /// migrations; the rest are pending/proposed).
    pub applied: usize,
}

/// Read an `i64` field from `Frontmatter.extra` — returns `0`
/// when missing / unparseable, which is also the default for
/// migrations that don't carry a version yet (proposals,
/// drafts).
pub(crate) fn read_i64(meta: &crate::parser::Frontmatter, key: &str) -> i64 {
    meta.extra
        .get(key)
        .and_then(|v| {
            // YAML can deserialize integers as i64 directly, but
            // some authors write strings ("1", "2"). Accept both.
            if let Some(n) = v.as_i64() {
                Some(n)
            } else if let Some(s) = v.as_str() {
                s.parse::<i64>().ok()
            } else {
                None
            }
        })
        .unwrap_or(0)
}

/// Read a `bool` field from `Frontmatter.extra` — returns
/// `false` when missing / unparseable.
pub(crate) fn read_bool(meta: &crate::parser::Frontmatter, key: &str) -> bool {
    meta.extra
        .get(key)
        .and_then(|v| {
            if let Some(b) = v.as_bool() {
                Some(b)
            } else {
                v.as_str().map(|s| s.eq_ignore_ascii_case("true"))
            }
        })
        .unwrap_or(false)
}
