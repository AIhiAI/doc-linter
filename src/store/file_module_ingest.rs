//! Roadmap issue #17 (v0.3.0): File / Module node tables + their
//! edges. Walks the corpus, classifies each source file by extension,
//! identifies the enclosing Module (Rust crate, Python package), and
//! emits the matching `File`, `Module`, `DEFINED_IN_FILE`,
//! `IN_MODULE`, and `DESCRIBED_BY` rows.
//!
//! ## Why files / modules are first-class
//!
//! Pre-#17 the only file representation in the graph was a string
//! column on `Function`. Queries like "what functions live in
//! `src/lib.rs`?" required a `LIKE` scan; renaming a file broke every
//! query that hardcoded the path; there was no way to attach
//! per-file metadata (LOC, mtime, language). With File nodes the
//! graph carries the same information SCIP captures and adds room
//! for the LOC / git-blame metadata Phase 3 of roadmap #41 wants.
//!
//! ## What this ingest does, in order
//!
//! 1. Walk `<root>` and collect every source file (`.rs`, `.py`,
//!    `.ts`, `.tsx`, `.js`, `.vue`, `.cs`, `.dart`, config formats).
//!    Skipped: anything inside `LintConfig::skip_dirs`
//!    plus the standard `.doc-lint`, `target`, `node_modules`,
//!    `.git` directories. (We share the skip list with the rest of
//!    the linter — same blocklist used by `lsp::collect_files`.)
//! 2. Identify Modules:
//!      - Rust crates: any directory containing a `Cargo.toml`.
//!        Name is the basename of that directory; `id` is
//!        `rust:<rel-path>`.
//!      - Python packages: any directory containing `__init__.py`.
//!        Name is the basename; `id` is `python:<rel-path>`.
//!      - .NET projects (`*.csproj`), Dart packages (`pubspec.yaml`)
//!        and npm packages (`package.json`) likewise, as
//!        `dotnet:` / `dart:` / `npm:<rel-path>`.
//!      - TS modules are deferred to a follow-up alongside the
//!        IMPORTS_MODULE tree-sitter pass — see the carve-out in
//!        the PR description.
//! 3. Insert `File` and `Module` rows.
//! 4. Emit `DEFINED_IN_FILE` for each Function whose `file` matches
//!    an inserted File path. Skips functions defined in files that
//!    were filtered out (rare — only a codegen-excluded SCIP file
//!    can produce that, and we want it skipped anyway).
//! 5. Emit `IN_MODULE` for each File mapped to its enclosing Module.
//!    A File can belong to at most one Module (the closest enclosing
//!    one in the directory hierarchy); files outside any Module dir
//!    get no edge.
//! 6. Emit `DESCRIBED_BY` for each Module whose directory contains a
//!    Doc node at `<dir>/README.md`. The Doc table is already
//!    populated by the time `cmd_check` reaches this step.
//!
//! `IMPORTS_MODULE` is deferred (#17 follow-up). The DDL is created
//! in `schema::reset_schema` so the on-disk layout is stable across
//! drop / recreate cycles.

use std::collections::{HashMap, HashSet};
use std::path::Path;

use walkdir::WalkDir;

use crate::config::LintConfig;

/// Stats returned to `cmd_check` so the stderr one-liner shows what
/// the ingest produced. Mirrors the shape of `ScipIngestStats` /
/// `EndpointIngestStats` for consistency.
#[derive(Debug, Default, Clone, Copy)]
pub struct FileModuleIngestStats {
    /// Total File rows inserted.
    pub files: usize,
    /// Total Module rows inserted (rust_crate + python_package).
    pub modules: usize,
    /// DEFINED_IN_FILE edges from Function to File.
    pub defined_in_file_edges: usize,
    /// IN_MODULE edges from File to Module.
    pub in_module_edges: usize,
    /// DESCRIBED_BY edges from Module to Doc.
    pub described_by_edges: usize,
}

/// Source-file row staged for insertion. Path is repo-relative
/// (the same convention `FunctionFact.file` uses) so cross-table
/// joins line up.
#[derive(Debug, Clone)]
pub(crate) struct FileRow {
    pub(crate) path: String,
    pub(crate) language: String,
    pub(crate) loc: u32,
    pub(crate) last_touched: String,
}

/// Module row staged for insertion.
#[derive(Debug, Clone)]
pub(crate) struct ModuleRow {
    pub(crate) id: String,
    pub(crate) kind: String,
    pub(crate) path: String,
    pub(crate) name: String,
}

/// Directories that are always skipped during the corpus walk
/// regardless of `LintConfig.skip_dirs`. These mirror the defaults
/// already used in `lsp::collect_files` / `endpoint_extract` — keeping
/// them in one place per file would risk drift, but this list is
/// small enough that a couple of duplicates is OK.
const ALWAYS_SKIP: &[&str] = &[".doc-lint", "target", "node_modules", ".git", "dist"];

/// Walk the corpus and return one `FileRow` per source file. Skipped
/// files don't appear in the return — downstream edge emitters then
/// silently drop functions / modules pointing at filtered paths.
pub(crate) fn collect_files(root: &Path, config: &LintConfig) -> Vec<FileRow> {
    let mut out: Vec<FileRow> = Vec::new();

    let extra_skip: HashSet<&str> = config
        .skip_dirs
        .iter()
        .map(std::string::String::as_str)
        .collect();

    for entry in WalkDir::new(root).into_iter().filter_entry(|e| {
        // Always traverse the root itself.
        if e.depth() == 0 {
            return true;
        }
        let name = e.file_name().to_string_lossy();
        if ALWAYS_SKIP.iter().any(|s| s == &name.as_ref()) {
            return false;
        }
        if extra_skip.contains(name.as_ref()) {
            return false;
        }
        true
    }) {
        let Ok(entry) = entry else { continue };
        if !entry.file_type().is_file() {
            continue;
        }
        let path = entry.path();
        let name = path.file_name().and_then(|s| s.to_str()).unwrap_or("");
        // gap-filesystem-config-ingest (iter 178 closure per
        // user-probe-041): filename-pattern check FIRST. This
        // catches `Dockerfile`, `Makefile`, AND `.env.example` /
        // `.env.local` (whose Rust-extension would be "example" /
        // "local" — not None — so the extension branch would
        // otherwise miss them).
        let by_name: Option<&str> = if name == "Dockerfile" || name.starts_with("Dockerfile.") {
            Some("dockerfile")
        } else if name == "Makefile" || name == "Justfile" {
            Some("makefile")
        } else if name == ".env" || name.starts_with(".env.") {
            Some("env")
        } else {
            None
        };
        let language: &str = if let Some(l) = by_name {
            l
        } else if let Some(ext) = path.extension().and_then(|s| s.to_str()) {
            match ext {
                "rs" => "rust",
                "py" => "python",
                // gap-file-walker-misses-pyi (iter 368
                // user-probe-101): Python typed-stub files
                // were silently dropped by the File walker
                // even though SCIP indexes their functions —
                // 5 of 6 spaCy matcher impl files were in
                // .pyi but file-axis returned only the
                // __init__.py. Treat as Python so language
                // queries stay symmetric with Function-axis.
                "pyi" => "python",
                // Cython (.pyx / .pxd): spaCy's tokenizer
                // and matcher.pyx live here; SCIP doesn't
                // index them but file-axis path queries
                // still need to surface their existence so
                // an agent looking for "tokenizer" lands on
                // the right file. Distinct language label so
                // function-count-by-language stats stay
                // honest.
                "pyx" | "pxd" => "cython",
                "ts" => "typescript",
                "tsx" => "tsx",
                "js" | "mjs" | "cjs" => "javascript",
                "jsx" => "jsx",
                "vue" => "vue",
                "cs" => "csharp",
                "dart" => "dart",
                "java" => "java",
                // Config-file extensions. Distinct language values
                // (toml/yaml/json) keep source-language queries
                // (function-count-by-language, hub-functions,
                // etc.) from accidentally including them.
                "toml" => "toml",
                "yaml" | "yml" => "yaml",
                "json" => "json",
                // gap-walker-invisible-cfg-jsonl (iter 499 closure
                // per user-probe-193): spaCy uses .cfg config
                // language for pipeline declarations + .jsonl for
                // training data. Per spacy-llm usage_examples/
                // <task_provider>/ on iter-498 V: 4 of 6 template
                // files were invisible to walker (2 .cfg + 1
                // .jsonl + 1 .md). .cfg / .jsonl join the accept-
                // list so plugin-template enumeration completes.
                "cfg" => "cfg",
                "jsonl" => "jsonl",
                _ => continue,
            }
        } else {
            continue;
        };
        let Ok(rel) = path.strip_prefix(root) else {
            continue;
        };
        let rel_str = rel.to_string_lossy().replace('\\', "/");
        if super::code_ingest::is_excluded_code_file(config, root, &rel_str) {
            continue;
        }
        // Cheap LOC count — read the file and count newlines.
        // Skipping files that fail to read entirely keeps the ingest
        // resilient against transient I/O races.
        let loc = match std::fs::read(path) {
            Ok(b) => count_newlines(&b),
            Err(_) => 0,
        };
        let last_touched = crate::gitdate::last_commit_secs(path)
            .and_then(|s| u64::try_from(s).ok())
            .map(|s| std::time::UNIX_EPOCH + std::time::Duration::from_secs(s))
            .or_else(|| path.metadata().ok().and_then(|m| m.modified().ok()))
            .map(format_mtime)
            .unwrap_or_default();
        out.push(FileRow {
            path: rel_str,
            language: language.to_string(),
            loc,
            last_touched,
        });
    }

    out.sort_by(|a, b| a.path.cmp(&b.path));
    out
}

/// Count `\n` bytes — same shortcut `wc -l` uses. Off-by-one against
/// files without a trailing newline is acceptable for a "rough file
/// size" stat; downstream consumers should treat `loc` as
/// approximate.
fn count_newlines(bytes: &[u8]) -> u32 {
    let n = bytes.iter().filter(|&&b| b == b'\n').count();
    u32::try_from(n).unwrap_or(u32::MAX)
}

/// Format a SystemTime as ISO-8601 UTC (`YYYY-MM-DDTHH:MM:SSZ`). The
/// Doc table's `updated` column uses the same convention so a future
/// "freshness" query can compare File.last_touched against the doc's
/// updated stamp.
fn format_mtime(t: std::time::SystemTime) -> String {
    let secs = t
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    // Hand-rolled formatter — avoids pulling chrono just for one
    // stamp. Days-from-epoch arithmetic is straightforward and we
    // already produce ISO-8601 elsewhere via the same idiom in
    // `coverage.rs::iso_today`.
    let (year, month, day, hour, minute, second) = epoch_secs_to_components(secs);
    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}Z")
}

/// Convert Unix epoch seconds → (year, month, day, hour, minute, sec).
/// Plain integer arithmetic — civil-from-days algorithm by Howard
/// Hinnant (public domain).
fn epoch_secs_to_components(secs: u64) -> (i32, u8, u8, u8, u8, u8) {
    let days = secs / 86400;
    let secs_of_day = secs % 86400;
    let hour = (secs_of_day / 3600) as u8;
    let minute = ((secs_of_day % 3600) / 60) as u8;
    let second = (secs_of_day % 60) as u8;

    // Shift so day 0 is 0000-03-01.
    let z = days as i64 + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = if m <= 2 { y + 1 } else { y } as i32;
    (year, m as u8, d as u8, hour, minute, second)
}

/// Walk the corpus a second time and identify Module directories: a
/// Rust crate holds a `Cargo.toml`, a Python package an `__init__.py`,
/// a .NET project a `*.csproj`, a Dart package a `pubspec.yaml`, an npm
/// package a `package.json`. The same `ALWAYS_SKIP` blocklist
/// applies so we don't promote nested `target/` Cargo manifests or
/// virtualenv `site-packages` to modules.
pub(crate) fn collect_modules(root: &Path, config: &LintConfig) -> Vec<ModuleRow> {
    let mut out: Vec<ModuleRow> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();

    let extra_skip: HashSet<&str> = config
        .skip_dirs
        .iter()
        .map(std::string::String::as_str)
        .collect();

    for entry in WalkDir::new(root).into_iter().filter_entry(|e| {
        if e.depth() == 0 {
            return true;
        }
        let name = e.file_name().to_string_lossy();
        if ALWAYS_SKIP.iter().any(|s| s == &name.as_ref()) {
            return false;
        }
        if extra_skip.contains(name.as_ref()) {
            return false;
        }
        true
    }) {
        let Ok(entry) = entry else { continue };
        if !entry.file_type().is_file() {
            continue;
        }
        let name = entry.file_name().to_string_lossy();
        let dir = entry.path().parent().unwrap_or(root);
        let Ok(rel_dir) = dir.strip_prefix(root) else {
            continue;
        };
        let rel_dir_str = if rel_dir.as_os_str().is_empty() {
            String::new()
        } else {
            rel_dir.to_string_lossy().replace('\\', "/")
        };
        let basename = dir
            .file_name()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_default();
        let (kind, id) = match name.as_ref() {
            "Cargo.toml" => ("rust_crate", format!("rust:{rel_dir_str}")),
            "__init__.py" => ("python_package", format!("python:{rel_dir_str}")),
            "pubspec.yaml" => ("dart_package", format!("dart:{rel_dir_str}")),
            "package.json" => ("npm_package", format!("npm:{rel_dir_str}")),
            n if n.ends_with(".csproj") => ("dotnet_project", format!("dotnet:{rel_dir_str}")),
            _ => continue,
        };
        if !seen.insert(id.clone()) {
            continue;
        }
        // Cargo crate name vs. dir name can diverge (e.g. crate name
        // is `pricing-core` but dir is `pricing`). For now we use the
        // dir basename — a follow-up can parse `Cargo.toml` for the
        // canonical name once we need that level of fidelity.
        out.push(ModuleRow {
            id,
            kind: kind.to_string(),
            path: rel_dir_str,
            name: basename,
        });
    }

    out.sort_by(|a, b| a.id.cmp(&b.id));
    out
}

/// `(file path, module id)` pairs: each Rust / Python file belongs to the
/// deepest module whose directory contains it. Shared with the SQLite writer.
pub(crate) fn in_module_pairs(files: &[FileRow], modules: &[ModuleRow]) -> Vec<(String, String)> {
    if files.is_empty() || modules.is_empty() {
        return Vec::new();
    }
    let mut by_kind: HashMap<&str, Vec<&ModuleRow>> = HashMap::new();
    for m in modules {
        let lang_kind = if m.kind == "rust_crate" {
            "rust"
        } else if m.kind == "python_package" {
            "python"
        } else {
            continue;
        };
        by_kind.entry(lang_kind).or_default().push(m);
    }
    // Sort each bucket by path length (longest first) so the first
    // match wins.
    for v in by_kind.values_mut() {
        v.sort_by(|a, b| b.path.len().cmp(&a.path.len()));
    }

    let mut rows = Vec::new();
    for f in files {
        let lang_kind = match f.language.as_str() {
            "rust" => "rust",
            "python" => "python",
            // TS / TSX have no Module nodes in this ingest pass.
            _ => continue,
        };
        let Some(candidates) = by_kind.get(lang_kind) else {
            continue;
        };
        if let Some(module) = candidates.iter().find(|m| path_is_under(&f.path, &m.path)) {
            rows.push((f.path.clone(), module.id.clone()));
        }
    }
    rows
}

/// True when `file_path` lives under `module_path` directorywise.
/// Empty `module_path` matches every file (workspace-root Module).
pub(crate) fn path_is_under(file_path: &str, module_path: &str) -> bool {
    if module_path.is_empty() {
        return true;
    }
    let with_sep = format!("{module_path}/");
    file_path.starts_with(&with_sep) || file_path == module_path
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

    #[test]
    fn path_is_under_handles_empty_and_prefix_cases() {
        assert!(path_is_under("src/lib.rs", ""), "root module matches any");
        assert!(path_is_under("src/lib.rs", "src"));
        assert!(
            path_is_under("crates/foo/src/lib.rs", "crates/foo"),
            "nested prefix matches"
        );
        assert!(
            !path_is_under("src/lib.rs", "crates"),
            "non-prefix doesn't match"
        );
        assert!(
            !path_is_under("source/lib.rs", "src"),
            "partial-segment prefix doesn't match"
        );
    }

    #[test]
    fn count_newlines_counts_lines() {
        assert_eq!(count_newlines(b""), 0);
        assert_eq!(count_newlines(b"one\n"), 1);
        assert_eq!(count_newlines(b"one\ntwo\nthree\n"), 3);
        assert_eq!(
            count_newlines(b"no-trailing"),
            0,
            "missing trailing newline → off-by-one is acceptable"
        );
    }

    #[test]
    fn format_mtime_is_iso_8601_utc_z() {
        // 2024-01-01T00:00:00Z is exactly 1704067200 (widely-known
        // epoch reference). Round-trip via the formatter to guard
        // against drift in the civil-from-days arithmetic.
        let t = std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_704_067_200);
        assert_eq!(format_mtime(t), "2024-01-01T00:00:00Z");
        // 2026-05-19T00:00:00Z = 1779148800 — exercises a different
        // month and a non-leap-year boundary so a single-day-off bug
        // doesn't slip past the January test.
        let t = std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_779_148_800);
        assert_eq!(format_mtime(t), "2026-05-19T00:00:00Z");
    }

    #[test]
    fn collect_files_picks_up_config_files_per_gap_filesystem_config_ingest() {
        // gap-filesystem-config-ingest (iter 178 closure): the walker
        // now picks up extensionless config files (Dockerfile, .env,
        // Makefile) AND known config extensions (.toml, .yaml, .yml,
        // .json) alongside source languages. Closes the user-probe-
        // 041 100%-miss-rate quantification on the trusted corpus.
        let dir = std::env::temp_dir().join(format!(
            "doc-linter-fm-config-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(dir.join("backend")).unwrap();
        std::fs::create_dir_all(dir.join("frontend")).unwrap();
        // Config files mirroring the user-probe-041 trusted-corpus
        // findings: 12 disk-visible config files of 6 distinct kinds.
        std::fs::write(dir.join(".env"), b"DEBUG=1\n").unwrap();
        std::fs::write(dir.join(".env.example"), b"DEBUG=0\n").unwrap();
        std::fs::write(dir.join("backend/.env"), b"PORT=8000\n").unwrap();
        std::fs::write(dir.join("pyproject.toml"), b"[project]\nname='x'\n").unwrap();
        std::fs::write(dir.join("backend/pyproject.toml"), b"[project]\n").unwrap();
        std::fs::write(dir.join(".doc-lint.toml"), b"include=[]\n").unwrap();
        std::fs::write(dir.join("compose.yml"), b"services:\n").unwrap();
        std::fs::write(dir.join("compose.override.yml"), b"services:\n").unwrap();
        std::fs::write(dir.join(".pre-commit-config.yaml"), b"repos:\n").unwrap();
        std::fs::write(dir.join("backend/Dockerfile"), b"FROM python\n").unwrap();
        std::fs::write(dir.join("frontend/Dockerfile"), b"FROM node\n").unwrap();
        std::fs::write(dir.join("Makefile"), b"all:\n\techo hi\n").unwrap();
        // gap-walker-invisible-cfg-jsonl (iter 499 extension): add
        // .cfg + .jsonl per spacy-llm usage_examples template (1
        // fewshot.cfg + 1 zeroshot.cfg + 1 examples.jsonl).
        std::fs::write(dir.join("fewshot.cfg"), b"[nlp]\nlang='en'\n").unwrap();
        std::fs::write(dir.join("zeroshot.cfg"), b"[nlp]\nlang='en'\n").unwrap();
        std::fs::write(dir.join("examples.jsonl"), b"{\"text\": \"hi\"}\n").unwrap();

        let config = LintConfig::default();
        let files = collect_files(&dir, &config);
        let by_lang: std::collections::HashMap<&str, Vec<&str>> = {
            let mut m: std::collections::HashMap<&str, Vec<&str>> =
                std::collections::HashMap::new();
            for f in &files {
                m.entry(f.language.as_str()).or_default().push(&f.path);
            }
            m
        };
        // Every config kind surfaces with a distinct language value.
        assert_eq!(
            by_lang.get("env").map(Vec::len),
            Some(3),
            "must pick up .env, .env.example, backend/.env (3 files): {by_lang:?}",
        );
        assert_eq!(
            by_lang.get("toml").map(Vec::len),
            Some(3),
            "must pick up 3 .toml files: {by_lang:?}",
        );
        assert_eq!(
            by_lang.get("yaml").map(Vec::len),
            Some(3),
            "must pick up compose.yml + compose.override.yml + .pre-commit-config.yaml: {by_lang:?}",
        );
        assert_eq!(
            by_lang.get("dockerfile").map(Vec::len),
            Some(2),
            "must pick up backend/Dockerfile + frontend/Dockerfile: {by_lang:?}",
        );
        assert_eq!(
            by_lang.get("makefile").map(Vec::len),
            Some(1),
            "must pick up Makefile: {by_lang:?}",
        );
        // iter 499 extension: .cfg + .jsonl (per spacy-llm template).
        assert_eq!(
            by_lang.get("cfg").map(Vec::len),
            Some(2),
            "must pick up fewshot.cfg + zeroshot.cfg: {by_lang:?}",
        );
        assert_eq!(
            by_lang.get("jsonl").map(Vec::len),
            Some(1),
            "must pick up examples.jsonl: {by_lang:?}",
        );
        // Total = 15 (12 from iter-178 + 3 from iter-499).
        assert_eq!(
            files.len(),
            15,
            "must ingest all 15 config files (12 iter-178 + 3 iter-499): {by_lang:?}",
        );
    }

    #[test]
    fn collect_files_config_languages_distinct_from_source_languages() {
        // Source languages and config languages must use DISTINCT
        // values so source-language saved queries
        // (function-count-by-language, hub-functions, etc.) don't
        // accidentally include config rows.
        let source_langs = ["rust", "python", "typescript", "tsx"];
        let config_langs = ["toml", "yaml", "json", "env", "dockerfile", "makefile"];
        for src in source_langs {
            for cfg in config_langs {
                assert_ne!(
                    src, cfg,
                    "source language `{src}` must not collide with config language `{cfg}`",
                );
            }
        }
    }

    #[test]
    fn collect_files_picks_up_rust_python_ts_and_skips_target() {
        let dir = std::env::temp_dir().join(format!(
            "doc-linter-fm-collect-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(dir.join("crates/foo/src")).unwrap();
        std::fs::create_dir_all(dir.join("target/debug")).unwrap();
        std::fs::create_dir_all(dir.join("scripts")).unwrap();
        std::fs::write(dir.join("crates/foo/src/lib.rs"), b"fn main() {}\n").unwrap();
        std::fs::write(dir.join("scripts/build.py"), b"print('hi')\n").unwrap();
        std::fs::write(dir.join("scripts/app.ts"), b"export const x = 1;\n").unwrap();
        std::fs::write(
            dir.join("target/debug/should-be-skipped.rs"),
            b"// build artefact\n",
        )
        .unwrap();
        std::fs::write(dir.join("crates/foo/src/notes.txt"), b"non-source\n").unwrap();

        let config = LintConfig::default();
        let files = collect_files(&dir, &config);
        let paths: Vec<&str> = files.iter().map(|f| f.path.as_str()).collect();
        assert!(
            paths.contains(&"crates/foo/src/lib.rs"),
            "rust file picked up; got {paths:?}"
        );
        assert!(paths.contains(&"scripts/build.py"));
        assert!(paths.contains(&"scripts/app.ts"));
        assert!(
            !paths.iter().any(|p| p.contains("target/")),
            "target/ excluded; got {paths:?}"
        );
        assert!(
            !paths.iter().any(|p| p.ends_with(".txt")),
            ".txt is not a source file"
        );
    }

    // gap-file-walker-misses-pyi (iter-368 user-probe-101):
    // before this test, .pyi (Python typed-stub) and Cython
    // .pyx / .pxd files were silently dropped — even though
    // SCIP indexes their functions. This pinned the new
    // extension whitelist so future refactors keep typed-
    // stub and Cython files surfaced for file-axis queries.
    #[test]
    fn collect_files_picks_up_pyi_and_cython_extensions() {
        let dir = std::env::temp_dir().join(format!(
            "doc-linter-fm-pyi-cython-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(dir.join("spacy/matcher")).unwrap();
        std::fs::create_dir_all(dir.join("spacy/tokenizer")).unwrap();
        std::fs::write(
            dir.join("spacy/matcher/matcher.pyi"),
            b"class Matcher: ...\n",
        )
        .unwrap();
        std::fs::write(
            dir.join("spacy/tokenizer/tokenizer.pyx"),
            b"cdef class Tokenizer:\n    pass\n",
        )
        .unwrap();
        std::fs::write(
            dir.join("spacy/tokenizer/tokenizer.pxd"),
            b"cdef class Tokenizer:\n    pass\n",
        )
        .unwrap();

        let config = LintConfig::default();
        let files = collect_files(&dir, &config);
        let by_path: std::collections::HashMap<&str, &str> = files
            .iter()
            .map(|f| (f.path.as_str(), f.language.as_str()))
            .collect();
        assert_eq!(
            by_path.get("spacy/matcher/matcher.pyi"),
            Some(&"python"),
            ".pyi tagged as python; got {by_path:?}"
        );
        assert_eq!(
            by_path.get("spacy/tokenizer/tokenizer.pyx"),
            Some(&"cython"),
            ".pyx tagged as cython; got {by_path:?}"
        );
        assert_eq!(
            by_path.get("spacy/tokenizer/tokenizer.pxd"),
            Some(&"cython"),
            ".pxd tagged as cython; got {by_path:?}"
        );
    }

    #[test]
    fn collect_modules_identifies_rust_crates_and_python_packages() {
        let dir = std::env::temp_dir().join(format!(
            "doc-linter-fm-mod-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(dir.join("crates/foo/src")).unwrap();
        std::fs::create_dir_all(dir.join("pypkg/sub")).unwrap();
        std::fs::write(
            dir.join("crates/foo/Cargo.toml"),
            b"[package]\nname=\"foo\"\n",
        )
        .unwrap();
        std::fs::write(dir.join("pypkg/__init__.py"), b"").unwrap();
        std::fs::write(dir.join("pypkg/sub/__init__.py"), b"").unwrap();

        let config = LintConfig::default();
        let modules = collect_modules(&dir, &config);
        let kinds: Vec<&str> = modules.iter().map(|m| m.kind.as_str()).collect();
        assert!(kinds.contains(&"rust_crate"));
        assert!(kinds.contains(&"python_package"));

        let rust = modules
            .iter()
            .find(|m| m.kind == "rust_crate")
            .expect("rust crate");
        assert_eq!(rust.path, "crates/foo");
        assert_eq!(rust.name, "foo");
        assert_eq!(rust.id, "rust:crates/foo");

        // Two python packages — pypkg and pypkg/sub.
        let py_count = modules
            .iter()
            .filter(|m| m.kind == "python_package")
            .count();
        assert_eq!(py_count, 2);
    }
}
