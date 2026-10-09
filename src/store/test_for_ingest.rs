//! Roadmap issue #24 (v0.3.0): TEST_FOR edges — link test
//! functions to the function they test.
//!
//! v1 surface: Python convention-based detection only.
//!
//!   - **Test function**: a Function whose file matches
//!     `**/test_*.py`, `**/*_test.py`, or lives directly under
//!     a `tests/` directory, AND whose simple symbol name (the
//!     part after the last descriptor delimiter) starts with
//!     `test_`.
//!   - **Tested function**: the Function whose simple name
//!     equals the test's `test_<X>` minus the `test_` prefix,
//!     and which lives in a non-test file.
//!
//! Each match emits a `TEST_FOR` edge with `confidence`:
//!
//!   - `"high"` — exactly one matching non-test function in the
//!     corpus. Unambiguous.
//!   - `"low"` — multiple candidates (same simple name across
//!     several non-test files). Each candidate gets an edge so
//!     the matcher can still walk both; the `low` flag signals
//!     the ambiguity.
//!
//! ## What's not here
//!
//!   - Rust `#[test]` + `#[cfg(test)] mod tests` detection.
//!   - JS/TS `describe(...) / it(...)` blocks.
//!   - The `@tests <symbol>` author-marker comment the issue
//!     calls out.
//!
//! Each is its own focused follow-up. The Python v1 captures
//! the example-app shape that triggered the issue and gives the
//! matcher a working `TEST_FOR` surface to consume.

use std::collections::{HashMap, HashSet};

/// Stats returned to `cmd_check` for the stderr line.
#[derive(Debug, Default, Clone, Copy)]
pub struct TestForIngestStats {
    /// Python test Functions identified.
    pub test_functions: usize,
    /// TEST_FOR edges emitted.
    pub edges: usize,
    /// Test functions that found no tested counterpart. Surfaces
    /// the "test for nothing" smell agents can clean up.
    pub orphan_tests: usize,
}

/// Pure TEST_FOR derivation over `(symbol, file)` rows; returns the stats
/// and `(test, tested, confidence)` edges. Shared with the SQLite writer.
pub(crate) fn plan_test_for(
    rows: Vec<(String, String)>,
) -> (TestForIngestStats, Vec<(String, String, &'static str)>) {
    let mut stats = TestForIngestStats::default();
    let funcs: Vec<FunctionRow> = rows
        .into_iter()
        .filter(|(sym, _)| !sym.is_empty())
        .map(|(symbol, file)| {
            let simple = simple_name_of(&symbol);
            FunctionRow {
                symbol,
                file,
                simple,
            }
        })
        .collect();
    if funcs.is_empty() {
        return (stats, Vec::new());
    }

    // 2. Build a `simple_name → Vec<&FunctionRow>` index over
    //    NON-test functions. Tested-side ambiguity falls out of
    //    this index — `Vec::len() > 1` means "low" confidence.
    let mut non_test_by_name: HashMap<String, Vec<&FunctionRow>> = HashMap::new();
    for f in &funcs {
        if is_python_test_file(&f.file) {
            continue;
        }
        if f.simple.is_empty() {
            continue;
        }
        non_test_by_name
            .entry(f.simple.clone())
            .or_default()
            .push(f);
    }

    // 3. Walk test functions; emit TEST_FOR edges.
    // Dedup edge keys so two test files with the same `test_X`
    // function name don't emit duplicate edges to the same
    // tested function.
    let mut edges: Vec<(String, String, &'static str)> = Vec::new();
    let mut emitted: HashSet<(String, String)> = HashSet::new();
    for f in &funcs {
        if !is_python_test_function(&f.file, &f.simple) {
            continue;
        }
        stats.test_functions += 1;
        let Some(tested_name) = f.simple.strip_prefix("test_") else {
            continue;
        };
        if tested_name.is_empty() {
            continue;
        }
        let Some(candidates) = non_test_by_name.get(tested_name) else {
            stats.orphan_tests += 1;
            continue;
        };
        let confidence = if candidates.len() == 1 { "high" } else { "low" };
        for tested in candidates {
            let key = (f.symbol.clone(), tested.symbol.clone());
            if !emitted.insert(key) {
                continue;
            }
            edges.push((f.symbol.clone(), tested.symbol.clone(), confidence));
            stats.edges += 1;
        }
    }
    (stats, edges)
}

/// One row from the Function table, projected for the test_for
/// pass.
struct FunctionRow {
    symbol: String,
    file: String,
    simple: String,
}

/// Strip the SCIP symbol down to its simple name — the final
/// descriptor token before the suffix character. For
/// `rust-analyzer cargo demo 0.1.0 src/foo.rs/Bar#baz().` the
/// simple name is `baz`. For `scip-python python pkg .
/// tests/test_foo.py/test_detect_pattern().` the simple name
/// is `test_detect_pattern`. Empty string when the symbol
/// shape isn't recognised.
fn simple_name_of(symbol: &str) -> String {
    // SCIP symbols separate descriptors with `/`; the last
    // descriptor carries the suffix. Strip the `()`/`#`/`.`
    // suffix and any leading `Type#` prefix.
    let last_seg = symbol.rsplit('/').next().unwrap_or("");
    // Drop trailing `().` or `()` (function/method) and `#`
    // (type member).
    let trimmed = last_seg
        .trim_end_matches('.')
        .trim_end_matches(')')
        .trim_end_matches('(');
    // If the descriptor is `Type#method` keep only `method`.
    let after_hash = trimmed.rsplit('#').next().unwrap_or(trimmed);
    after_hash.to_string()
}

/// Recognise a Python test file by path convention.
fn is_python_test_file(file: &str) -> bool {
    if !file.ends_with(".py") {
        return false;
    }
    // `**/test_*.py`
    if file.split('/').any(|seg| seg.starts_with("test_")) {
        return true;
    }
    // `**/*_test.py`
    if let Some(basename) = file.rsplit('/').next() {
        if basename
            .strip_suffix(".py")
            .is_some_and(|stem| stem.ends_with("_test"))
        {
            return true;
        }
    }
    // `tests/...` directory anywhere in the path.
    file.split('/').any(|seg| seg == "tests")
}

/// Recognise a Python test function: lives in a Python test
/// file AND has a `test_` simple-name prefix.
fn is_python_test_function(file: &str, simple: &str) -> bool {
    is_python_test_file(file) && simple.starts_with("test_")
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    reason = "tests panic on broken invariants — that is the point of a test"
)]
mod tests {
    use super::*;

    #[test]
    fn simple_name_of_python_function() {
        let sym = "scip-python python pkg . src/foo.py/detect_pattern().";
        assert_eq!(simple_name_of(sym), "detect_pattern");
    }

    #[test]
    fn simple_name_of_python_method() {
        let sym = "scip-python python pkg . src/foo.py/Bar#detect().";
        assert_eq!(simple_name_of(sym), "detect");
    }

    #[test]
    fn simple_name_of_rust_function() {
        let sym = "rust-analyzer cargo demo 0.1.0 src/lib.rs/parse_doc().";
        assert_eq!(simple_name_of(sym), "parse_doc");
    }

    #[test]
    fn is_python_test_file_matches_conventions() {
        assert!(is_python_test_file("tests/test_foo.py"));
        assert!(is_python_test_file("backend/tests/test_bar.py"));
        assert!(is_python_test_file("backend/foo_test.py"));
        assert!(is_python_test_file("tests/helpers/test_util.py"));
        // `tests` segment in the middle still counts.
        assert!(is_python_test_file("backend/tests/conftest.py"));
        // Bare `.py` outside tests/ is NOT a test file.
        assert!(!is_python_test_file("backend/server.py"));
        // Non-.py files are never test files.
        assert!(!is_python_test_file("tests/README.md"));
    }

    #[test]
    fn is_python_test_function_requires_both_conditions() {
        // Test-file path + test_ name prefix.
        assert!(is_python_test_function(
            "tests/test_pattern.py",
            "test_detect"
        ));
        // Test-file path but no test_ prefix (helper function in
        // a test file).
        assert!(!is_python_test_function(
            "tests/test_pattern.py",
            "make_fixture"
        ));
        // Non-test file with test_ prefix (e.g. a `test_runner`
        // function in production code) — not a test.
        assert!(!is_python_test_function(
            "backend/server.py",
            "test_connection"
        ));
    }
}
