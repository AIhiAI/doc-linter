//! Semantic tests of the built-in saved queries' SQL: each seeds a small SQLite
//! graph (through [`super::testkit`], which accepts the Cypher `CREATE`
//! notation these tests were written in) and checks what the query returns.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::ignored_unit_patterns,
    reason = "tests panic on broken invariants — that is the point of a test"
)]

use std::collections::HashMap;

use super::testkit::{self, kv, run_saved_query, sql_of};
use crate::store::query::saved::SAVED_QUERIES;

/// #226: `entity-relations` lists authored RELATES_TO (with its type)
/// and ENTITY_CALLS at or above `$min_calls` (default 5) as `calls`.
#[test]
fn entity_relations_unions_authored_and_call_density() {
    let dir = std::env::temp_dir().join(format!(
        "doc-linter-saved-entity-relations-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let db = testkit::open_db(&dir);
    {
        let conn = testkit::connect(&db);
        testkit::reset(&conn);
        for q in [
                "CREATE (:Entity {id: 'a'}), (:Entity {id: 'b'}), (:Entity {id: 'c'})",
                "MATCH (a:Entity {id: 'a'}), (b:Entity {id: 'b'}) CREATE (a)-[:RELATES_TO {type: 'depends_on', weight: 1.0, frequency: 0, source: 'frontmatter'}]->(b)",
                "MATCH (b:Entity {id: 'b'}), (c:Entity {id: 'c'}) CREATE (b)-[:ENTITY_CALLS {frequency: 7, weight: 1.0, source: 'derived'}]->(c)",
                "MATCH (a:Entity {id: 'a'}), (c:Entity {id: 'c'}) CREATE (a)-[:ENTITY_CALLS {frequency: 2, weight: 0.3, source: 'derived'}]->(c)",
            ] {
                conn.query(q).unwrap();
            }
    }
    let rows = |params: &[(&str, &str)]| -> Vec<String> {
        let params: HashMap<String, String> = params
            .iter()
            .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
            .collect();
        let out = run_saved_query(&db, "entity-relations", &params).unwrap();
        let mut rows: Vec<String> = out["rows"]
            .as_array()
            .unwrap()
            .iter()
            .map(|r| {
                format!("{}>{} {} {}", r["from"], r["to"], r["type"], r["origin"]).replace('"', "")
            })
            .collect();
        rows.sort();
        rows
    };
    assert_eq!(rows(&[]), ["a>b depends_on authored", "b>c calls derived"]);
    assert_eq!(
        rows(&[("min_calls", "2")]),
        [
            "a>b depends_on authored",
            "a>c calls derived",
            "b>c calls derived"
        ]
    );
    drop(db);
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn catalog_declared_params_appear_in_sql() {
    // A declared param that doesn't appear in the SQL
    // template would be a bug — the substitution would
    // silently skip and the user's `--param` value would
    // never reach the query.
    for q in SAVED_QUERIES {
        for p in q.params {
            // `name=default` declares an optional param named `name`.
            let p = p.split_once('=').map_or(*p, |(name, _)| name);
            let placeholder = format!("${p}");
            assert!(
                    sql_of(q.name).contains(&placeholder),
                    "saved query `{}` declares param `{p}` but its SQL doesn't reference `{placeholder}`",
                    q.name
                );
        }
    }
}

#[test]
fn generated_files_matches_four_generation_conventions() {
    let q = SAVED_QUERIES
        .iter()
        .find(|q| q.name == "generated-files")
        .expect("generated-files must exist");
    for pattern in [".gen.", "/generated/", "/__generated__/", ".pb."] {
        assert!(
            sql_of(q.name).contains(pattern),
            "generated-files must include `{pattern}` pattern: {}",
            sql_of(q.name)
        );
    }
    // OR-composition rather than AND-composition — each
    // pattern is sufficient.
    assert!(
        sql_of(q.name).contains(" OR "),
        "generated-files must use OR to combine patterns: {}",
        sql_of(q.name)
    );
}

#[test]
fn corpus_shape_classifier_emits_4_regime_buckets() {
    let q = SAVED_QUERIES
        .iter()
        .find(|q| q.name == "corpus-shape-classifier")
        .expect("corpus-shape-classifier must exist");
    // Per iter-187: 4 categorical regime buckets must be
    // present so the agent's switch-on-result-string code
    // matches the documented taxonomy.
    for regime in ["doc-empty-ontology-only", "doc-sparse", "doc-rich", "mixed"] {
        assert!(
            sql_of(q.name).contains(&format!("'{regime}'")),
            "must emit `{regime}` regime: {}",
            sql_of(q.name)
        );
    }
    // Must project all 4 fields so the agent can audit the
    // classification reasoning, not just trust the verdict.
    assert!(
        sql_of(q.name).contains("total_docs")
            && sql_of(q.name).contains("ontology_docs")
            && sql_of(q.name).contains("narrative_docs")
            && sql_of(q.name).contains("AS regime"),
        "must project total_docs + ontology_docs + narrative_docs + regime: {}",
        sql_of(q.name)
    );
    // Boundary at 10 (doc-sparse) and 50 (doc-rich) per the
    // user-probe-037 / user-probe-043 evidence.
    assert!(
        sql_of(q.name).contains("narrative_docs < 10"),
        "must use the doc-sparse boundary at 10: {}",
        sql_of(q.name)
    );
    assert!(
        sql_of(q.name).contains("narrative_docs >= 50"),
        "must use the doc-rich boundary at 50: {}",
        sql_of(q.name)
    );
}

#[test]
fn stale_narrative_docs_excludes_ontology_roles_and_orders_asc() {
    let q = SAVED_QUERIES
        .iter()
        .find(|q| q.name == "stale-narrative-docs")
        .expect("stale-narrative-docs must exist");
    // Per user-probe-037 Finding D: ontology-value / -axis /
    // -entity are starter-ontology entries whose staleness is
    // structurally OK. Must explicitly exclude all three.
    for role in [
        "ontology-value",
        "ontology-axis",
        "ontology-entity",
        "ontology-migration",
        "index",
    ] {
        assert!(
            sql_of(q.name).contains(&format!("'{role}'")),
            "stale-narrative-docs must mention `{role}` in the exclude list: {}",
            sql_of(q.name)
        );
    }
    assert!(
        sql_of(q.name).contains("NOT d.role IN"),
        "must use NOT d.role IN [...] exclusion: {}",
        sql_of(q.name)
    );
    // Filter for non-null updated; the cypher would still parse
    // without this, but the query would emit `updated: null`
    // rows that the agent can't act on.
    assert!(
        sql_of(q.name).contains("d.updated IS NOT NULL"),
        "must filter for non-null updated: {}",
        sql_of(q.name)
    );
    // Stalest first — the workflow's defining ordering.
    assert!(
        sql_of(q.name).contains("ORDER BY d.updated ASC"),
        "must order by updated ASC (stalest first): {}",
        sql_of(q.name)
    );
}

#[test]
fn function_signature_population_by_language_diagnoses_column_gap() {
    let q = SAVED_QUERIES
        .iter()
        .find(|q| q.name == "function-signature-population-by-language")
        .expect("function-signature-population-by-language must exist");
    // Per interrogation-037: must distinguish populated
    // signatures from null/empty so the agent can detect
    // sub-cause 3 (column gap) vs sub-cause 1+2 (row /
    // mention gaps).
    assert!(
        sql_of(q.name).contains("f.signature IS NULL OR f.signature = ''"),
        "must check both NULL and empty-string: {}",
        sql_of(q.name)
    );
    assert!(
        sql_of(q.name).contains("AS populated_signatures"),
        "must project populated_signatures scalar: {}",
        sql_of(q.name)
    );
    assert!(
        sql_of(q.name).contains("AS percent_populated"),
        "must project percent_populated for at-a-glance read: {}",
        sql_of(q.name)
    );
    // Guard against division by zero — fn count can be 0
    // on a Function-row-empty corpus (sub-cause 1).
    assert!(
        sql_of(q.name).contains("WHEN function_count = 0 THEN 0"),
        "must guard percent calculation against zero-row corpus: {}",
        sql_of(q.name)
    );
    // Per-language grouping per the iter-159 diagnostic
    // shape.
    assert!(
        sql_of(q.name).contains("f.language AS language"),
        "must group by Function.language: {}",
        sql_of(q.name)
    );
}

#[test]
fn function_mentions_by_language_walks_function_mentions_edges() {
    let q = SAVED_QUERIES
        .iter()
        .find(|q| q.name == "function-mentions-by-language")
        .expect("function-mentions-by-language must exist");
    // Must walk the FUNCTION_MENTIONS edge so a count of 0
    // means the edge table is empty for that language even
    // when Function rows exist — the specific user-probe-035
    // failure mode (functions present, mentions absent).
    assert!(
        sql_of(q.name).contains("FUNCTION_MENTIONS"),
        "must walk FUNCTION_MENTIONS edge: {}",
        sql_of(q.name)
    );
    assert!(
        sql_of(q.name).contains("f.language AS language"),
        "must group by source Function's language: {}",
        sql_of(q.name)
    );
    assert!(
        sql_of(q.name).contains("AS mention_count"),
        "must project mention_count: {}",
        sql_of(q.name)
    );
}

#[test]
fn files_by_recency_desc_orders_descending_with_limit() {
    let q = SAVED_QUERIES
        .iter()
        .find(|q| q.name == "files-by-recency-desc")
        .expect("files-by-recency-desc must exist");
    assert!(
        sql_of(q.name).contains("ORDER BY f.last_touched DESC"),
        "files-by-recency-desc must order DESC: {}",
        sql_of(q.name)
    );
    assert!(
        sql_of(q.name).contains("LIMIT 10"),
        "files-by-recency-desc must cap at 10 rows: {}",
        sql_of(q.name)
    );
    assert!(
        sql_of(q.name).contains("WHERE f.last_touched IS NOT NULL"),
        "files-by-recency-desc must filter out NULL last_touched: {}",
        sql_of(q.name)
    );
}

#[test]
fn files_by_recency_asc_orders_ascending_with_limit() {
    let q = SAVED_QUERIES
        .iter()
        .find(|q| q.name == "files-by-recency-asc")
        .expect("files-by-recency-asc must exist");
    assert!(
        sql_of(q.name).contains("ORDER BY f.last_touched ASC"),
        "files-by-recency-asc must order ASC: {}",
        sql_of(q.name)
    );
    assert!(
        sql_of(q.name).contains("LIMIT 10"),
        "files-by-recency-asc must cap at 10 rows: {}",
        sql_of(q.name)
    );
}

#[test]
fn files_by_path_pattern_with_coupling_returns_paths_plus_degree() {
    // Per iter 269 (operationalises iter-268 user-probe-064
    // recipe): files matching pattern + their coupling
    // degree in 1 call. Fixture mirrors user-probe-064's
    // auth-flow probe shape:
    //   login.tsx (frontend) coupled with 3 other files
    //   login.py (backend) with 0 couplings (degree=0)
    //   signup.tsx coupled with 1 file
    //   unrelated.tsx (no 'login' in path) — excluded
    // Expected with pattern='login':
    //   login.tsx degree=3
    //   login.py degree=0
    //   (signup excluded — no 'login' in path)
    let dir = std::env::temp_dir().join(format!(
        "doc-linter-saved-files-by-pattern-coupling-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let db = testkit::open_db(&dir);
    {
        let conn = testkit::connect(&db);
        testkit::reset(&conn);
    }
    let conn = testkit::connect(&db);
    for (path, lang, loc) in [
        ("frontend/routes/login.tsx", "tsx", 142u32),
        ("backend/routes/login.py", "python", 123),
        ("frontend/routes/signup.tsx", "tsx", 189),
        ("frontend/routes/unrelated.tsx", "tsx", 50),
        ("frontend/routes/neighbor-a.tsx", "tsx", 50),
        ("frontend/routes/neighbor-b.tsx", "tsx", 50),
        ("frontend/routes/neighbor-c.tsx", "tsx", 50),
    ] {
        conn.query(&format!(
            "CREATE (:File {{path: '{path}', language: '{lang}', \
                 loc: {loc}, last_touched: '2026-06-01'}})"
        ))
        .expect("seed File");
    }
    for (a, b) in [
        (
            "frontend/routes/login.tsx",
            "frontend/routes/neighbor-a.tsx",
        ),
        (
            "frontend/routes/login.tsx",
            "frontend/routes/neighbor-b.tsx",
        ),
        (
            "frontend/routes/login.tsx",
            "frontend/routes/neighbor-c.tsx",
        ),
        (
            "frontend/routes/signup.tsx",
            "frontend/routes/neighbor-a.tsx",
        ),
    ] {
        conn.query(&format!(
            "MATCH (a:File {{path: '{a}'}}), (b:File {{path: '{b}'}}) \
                 CREATE (a)-[:COUPLED_WITH {{commits: 5, jaccard: 0.5, \
                 last_co_change_at: '2026-05-01T00:00:00Z'}}]->(b)"
        ))
        .expect("seed COUPLED_WITH");
    }

    let q = SAVED_QUERIES
        .iter()
        .find(|q| q.name == "files-by-path-pattern-with-coupling")
        .expect("files-by-path-pattern-with-coupling must exist");
    let cypher = sql_of(q.name).replace("$pattern", "'login'");
    let mut result = conn.query(&cypher).expect("query runs");
    let mut returned: Vec<(String, i64)> = Vec::new();
    for row in &mut result {
        let path = if let kv::Value::String(s) = &row[0] {
            s.clone()
        } else {
            panic!("path must be String: {row:?}");
        };
        let degree = match &row[3] {
            kv::Value::Int64(n) => *n,
            kv::Value::Int32(n) => *n as i64,
            kv::Value::UInt64(n) => *n as i64,
            other => panic!("degree must be int: {other:?}"),
        };
        returned.push((path, degree));
    }
    assert!(
        returned
            .iter()
            .any(|(p, d)| p == "frontend/routes/login.tsx" && *d == 3),
        "login.tsx must surface at degree=3: {returned:?}",
    );
    assert!(
        returned
            .iter()
            .any(|(p, d)| p == "backend/routes/login.py" && *d == 0),
        "login.py must surface at degree=0 (no coupling): {returned:?}",
    );
    assert!(
        !returned
            .iter()
            .any(|(p, _)| p.contains("signup") || p.contains("unrelated")),
        "non-login paths must NOT surface: {returned:?}",
    );
    // Order: degree DESC. login.tsx (3) before login.py (0).
    let login_tsx_pos = returned
        .iter()
        .position(|(p, _)| p == "frontend/routes/login.tsx")
        .unwrap();
    let login_py_pos = returned
        .iter()
        .position(|(p, _)| p == "backend/routes/login.py")
        .unwrap();
    assert!(
        login_tsx_pos < login_py_pos,
        "login.tsx (degree=3) must rank before login.py (degree=0): {returned:?}",
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn files_by_pattern_language_split_aggregates_per_language() {
    // Per iter 269 (companion at per-language split):
    // Fixture: 3 tsx files at LOC 100/200/300 + 2 py
    // files at LOC 100/200 + 1 non-matching file.
    // pattern='auth' should aggregate matching files
    // per language with total_loc DESC.
    // Expected: tsx (3 files, 600 LOC) > python (2, 300).
    let dir = std::env::temp_dir().join(format!(
        "doc-linter-saved-files-by-pattern-language-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let db = testkit::open_db(&dir);
    {
        let conn = testkit::connect(&db);
        testkit::reset(&conn);
    }
    let conn = testkit::connect(&db);
    for (path, lang, loc) in [
        ("frontend/auth-login.tsx", "tsx", 100u32),
        ("frontend/auth-signup.tsx", "tsx", 200),
        ("frontend/auth-reset.tsx", "tsx", 300),
        ("backend/auth-handler.py", "python", 100),
        ("backend/auth-utils.py", "python", 200),
        ("frontend/unrelated.tsx", "tsx", 999),
    ] {
        conn.query(&format!(
            "CREATE (:File {{path: '{path}', language: '{lang}', \
                 loc: {loc}, last_touched: '2026-06-01'}})"
        ))
        .expect("seed File");
    }

    let q = SAVED_QUERIES
        .iter()
        .find(|q| q.name == "files-by-pattern-language-split")
        .expect("files-by-pattern-language-split must exist");
    let cypher = sql_of(q.name).replace("$pattern", "'auth'");
    let mut result = conn.query(&cypher).expect("query runs");
    let mut returned: Vec<(String, i64, i64)> = Vec::new();
    for row in &mut result {
        let lang = if let kv::Value::String(s) = &row[0] {
            s.clone()
        } else {
            panic!("language must be String: {row:?}");
        };
        let file_count = match &row[1] {
            kv::Value::Int64(n) => *n,
            kv::Value::Int32(n) => *n as i64,
            kv::Value::UInt64(n) => *n as i64,
            other => panic!("file_count must be int: {other:?}"),
        };
        let total_loc = match &row[2] {
            kv::Value::Int64(n) => *n,
            kv::Value::Int32(n) => *n as i64,
            kv::Value::UInt64(n) => *n as i64,
            kv::Value::Int128(n) => *n as i64,
            other => panic!("total_loc must be int: {other:?}"),
        };
        returned.push((lang, file_count, total_loc));
    }
    assert!(
        returned
            .iter()
            .any(|(l, c, t)| l == "tsx" && *c == 3 && *t == 600),
        "tsx must surface at 3 files / 600 LOC: {returned:?}",
    );
    assert!(
        returned
            .iter()
            .any(|(l, c, t)| l == "python" && *c == 2 && *t == 300),
        "python must surface at 2 files / 300 LOC: {returned:?}",
    );
    // Order: total_loc DESC. tsx (600) before python (300).
    assert_eq!(
        returned[0].0, "tsx",
        "tsx must rank first by total_loc: {returned:?}",
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn functions_by_symbol_pattern_excludes_test_paths() {
    // Per iter 271 (closes user-probe-065 Finding D —
    // Function-axis test-bias filter): pattern + tests/
    // exclusion. Fixture:
    //   Tokenizer#tokenize (file = spacy/tokenizer.py) —
    //     IMPLEMENTATION → surfaces
    //   tests/tokenizer/test_tokenizer.py:test_tokenize_basic
    //     — TEST file → excluded
    //   spacy/lang/tests/test_tokenize_lang.py:test_lang —
    //     TEST path nested → excluded
    //   spacy/util/helper() — no 'tokeniz' in symbol → excluded
    let dir = std::env::temp_dir().join(format!(
        "doc-linter-saved-fns-by-symbol-pattern-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let db = testkit::open_db(&dir);
    {
        let conn = testkit::connect(&db);
        testkit::reset(&conn);
    }
    let conn = testkit::connect(&db);
    for (sym, file) in [
        ("Tokenizer#tokenize()", "spacy/tokenizer.py"),
        ("Tokenizer#retokenize()", "spacy/tokenizer.py"),
        ("test_tokenize_basic()", "tests/tokenizer/test_tokenizer.py"),
        (
            "test_lang_tokenize()",
            "spacy/lang/tests/test_tokenize_lang.py",
        ),
        ("util/helper()", "spacy/util.py"),
    ] {
        conn.query(&format!(
            "CREATE (:Function {{symbol: '{sym}', crate: 'c', \
                 file: '{file}', line: 1, doc_comment: '', \
                 language: 'python', signature: '', body_excerpt: '', \
                 last_touched: '2026-06-01', repo_id: 'r'}})"
        ))
        .expect("seed Function");
    }
    let q = SAVED_QUERIES
        .iter()
        .find(|q| q.name == "functions-by-symbol-pattern")
        .expect("functions-by-symbol-pattern must exist");
    let cypher = sql_of(q.name).replace("$pattern", "'tokeniz'");
    let mut result = conn.query(&cypher).expect("query runs");
    let mut returned: Vec<String> = Vec::new();
    for row in &mut result {
        if let kv::Value::String(s) = &row[0] {
            returned.push(s.clone());
        }
    }
    for surfaced in ["Tokenizer#tokenize()", "Tokenizer#retokenize()"] {
        assert!(
            returned.iter().any(|s| s == surfaced),
            "{surfaced} must surface (implementation): {returned:?}",
        );
    }
    for excluded in [
        "test_tokenize_basic()",
        "test_lang_tokenize()",
        "util/helper()",
    ] {
        assert!(
            !returned.iter().any(|s| s == excluded),
            "{excluded} must NOT surface: {returned:?}",
        );
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn cython_files_by_pattern_surfaces_pyx_and_pxd() {
    // Per iter 271 (closes user-probe-065 Finding E):
    // Cython source/header detection. Fixture:
    //   spacy/tokenizer.pyx → cython-source ✓
    //   spacy/tokenizer.pxd → cython-header ✓
    //   spacy/tokenizer.py  → excluded (.py not .pyx)
    //   spacy/lang/en/tokenizer_data.py → excluded (.py)
    //   spacy/util.pyx → excluded (no 'tokenizer' in path)
    let dir = std::env::temp_dir().join(format!(
        "doc-linter-saved-cython-by-pattern-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let db = testkit::open_db(&dir);
    {
        let conn = testkit::connect(&db);
        testkit::reset(&conn);
    }
    let conn = testkit::connect(&db);
    for path in [
        "spacy/tokenizer.pyx",
        "spacy/tokenizer.pxd",
        "spacy/tokenizer.py",
        "spacy/lang/en/tokenizer_data.py",
        "spacy/util.pyx",
    ] {
        conn.query(&format!(
            "CREATE (:File {{path: '{path}', language: 'python', \
                 loc: 100, last_touched: '2026-06-01'}})"
        ))
        .expect("seed File");
    }
    let q = SAVED_QUERIES
        .iter()
        .find(|q| q.name == "cython-files-by-pattern")
        .expect("cython-files-by-pattern must exist");
    let cypher = sql_of(q.name).replace("$pattern", "'tokenizer'");
    let mut result = conn.query(&cypher).expect("query runs");
    let mut returned: Vec<(String, String)> = Vec::new();
    for row in &mut result {
        let path = if let kv::Value::String(s) = &row[0] {
            s.clone()
        } else {
            panic!("path must be String: {row:?}");
        };
        let kind = if let kv::Value::String(s) = &row[2] {
            s.clone()
        } else {
            panic!("cython_kind must be String: {row:?}");
        };
        returned.push((path, kind));
    }
    assert!(
        returned
            .iter()
            .any(|(p, k)| p == "spacy/tokenizer.pyx" && k == "cython-source"),
        "tokenizer.pyx must surface as cython-source: {returned:?}",
    );
    assert!(
        returned
            .iter()
            .any(|(p, k)| p == "spacy/tokenizer.pxd" && k == "cython-header"),
        "tokenizer.pxd must surface as cython-header: {returned:?}",
    );
    for excluded in [
        "spacy/tokenizer.py",
        "spacy/lang/en/tokenizer_data.py",
        "spacy/util.pyx",
    ] {
        assert!(
            !returned.iter().any(|(p, _)| p == excluded),
            "{excluded} must NOT surface: {returned:?}",
        );
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn types_by_symbol_pattern_filters_pattern_and_excludes_tests() {
    // Per iter 275 (Type-axis sibling of iter-271 functions
    // -by-symbol-pattern): types matching pattern with test
    // exclusion. Fixture: 5 Types covering pattern matches +
    // exclusions:
    //   LintConfig (src/config.rs)        — IMPL → surfaces
    //   DocLintConfig (src/lib.rs)        — IMPL → surfaces
    //   TestConfig (tests/helpers.rs)     — test file → excluded
    //   tests/MockConfig (src/mock.rs)    — test symbol → excluded
    //   StateMachine (src/state.rs)       — no 'Config' → excluded
    let dir = std::env::temp_dir().join(format!(
        "doc-linter-saved-types-by-symbol-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let db = testkit::open_db(&dir);
    {
        let conn = testkit::connect(&db);
        testkit::reset(&conn);
    }
    let conn = testkit::connect(&db);
    for (sym, kind, file) in [
        ("LintConfig", "struct", "src/config.rs"),
        ("DocLintConfig", "struct", "src/lib.rs"),
        ("TestConfig", "struct", "tests/helpers.rs"),
        ("tests/MockConfig", "struct", "src/mock.rs"),
        ("StateMachine", "enum", "src/state.rs"),
    ] {
        conn.query(&format!(
            "CREATE (:Type {{symbol: '{sym}', kind: '{kind}', crate: 'd', \
                 file: '{file}', line: 1, doc_comment: '', language: 'rust', \
                 signature: '', body_excerpt: '', last_touched: '2026-06-01', \
                 repo_id: 'd'}})"
        ))
        .expect("seed Type");
    }
    let q = SAVED_QUERIES
        .iter()
        .find(|q| q.name == "types-by-symbol-pattern")
        .expect("types-by-symbol-pattern must exist");
    let cypher = sql_of(q.name).replace("$pattern", "'Config'");
    let mut result = conn.query(&cypher).expect("query runs");
    let mut returned: Vec<String> = Vec::new();
    for row in &mut result {
        if let kv::Value::String(s) = &row[0] {
            returned.push(s.clone());
        }
    }
    for surfaced in ["LintConfig", "DocLintConfig"] {
        assert!(
            returned.iter().any(|s| s == surfaced),
            "{surfaced} must surface: {returned:?}",
        );
    }
    for excluded in ["TestConfig", "tests/MockConfig", "StateMachine"] {
        assert!(
            !returned.iter().any(|s| s == excluded),
            "{excluded} must NOT surface: {returned:?}",
        );
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn types_by_file_pattern_filters_file_and_excludes_tests() {
    // Per iter 275 + iter-277 [[user-probe-067]] Finding B
    // refinement: file-axis Type lookup with BOTH file +
    // symbol-side test-exclusion filters. Fixture: 5 Types:
    //   Foo (src/store/query/saved.rs) — IMPL surfaces
    //   Bar (src/store/query/util.rs)  — IMPL surfaces
    //   Baz (src/other/module.rs)           — no 'query' → excluded
    //   Qux (tests/store/test_query.rs) — test file → excluded
    //   InFileTestMod (src/store/query/dead_code.rs, symbol
    //     contains 'tests/') — IN-FILE #[cfg(test)] mod tests
    //     block: file is non-test BUT symbol-CONTAINS='tests/'.
    //     iter-275 lacked symbol-side filter; iter-276 found
    //     this case leaking through on doc-linter source;
    //     iter-277 fix excludes via NOT t.symbol CONTAINS 'tests/'.
    let dir = std::env::temp_dir().join(format!(
        "doc-linter-saved-types-by-file-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let db = testkit::open_db(&dir);
    {
        let conn = testkit::connect(&db);
        testkit::reset(&conn);
    }
    let conn = testkit::connect(&db);
    for (sym, file) in [
        ("Foo", "src/store/query/saved.rs"),
        ("Bar", "src/store/query/util.rs"),
        ("Baz", "src/other/module.rs"),
        ("Qux", "tests/store/test_query.rs"),
        (
            "rust-analyzer cargo dl 0.1 store/query/dead_code/tests/",
            "src/store/query/dead_code.rs",
        ),
    ] {
        conn.query(&format!(
            "CREATE (:Type {{symbol: '{sym}', kind: 'struct', crate: 'd', \
                 file: '{file}', line: 1, doc_comment: '', language: 'rust', \
                 signature: '', body_excerpt: '', last_touched: '2026-06-01', \
                 repo_id: 'd'}})"
        ))
        .expect("seed Type");
    }
    let q = SAVED_QUERIES
        .iter()
        .find(|q| q.name == "types-by-file-pattern")
        .expect("types-by-file-pattern must exist");
    let cypher = sql_of(q.name).replace("$pattern", "'store/query'");
    let mut result = conn.query(&cypher).expect("query runs");
    let mut returned: Vec<(String, String)> = Vec::new();
    for row in &mut result {
        let file = if let kv::Value::String(s) = &row[0] {
            s.clone()
        } else {
            panic!("file must be String: {row:?}");
        };
        let sym = if let kv::Value::String(s) = &row[1] {
            s.clone()
        } else {
            panic!("symbol must be String: {row:?}");
        };
        returned.push((file, sym));
    }
    assert!(
        returned.iter().any(|(_, s)| s == "Foo"),
        "Foo must surface (in src/store/query/): {returned:?}",
    );
    assert!(
        returned.iter().any(|(_, s)| s == "Bar"),
        "Bar must surface: {returned:?}",
    );
    assert!(
        !returned.iter().any(|(_, s)| s == "Baz"),
        "Baz must NOT surface (file lacks 'query'): {returned:?}",
    );
    assert!(
        !returned.iter().any(|(_, s)| s == "Qux"),
        "Qux must NOT surface (tests/-file): {returned:?}",
    );
    // iter-277 fix: in-file `#[cfg(test)] mod tests` block — file
    // path is non-test but symbol contains 'tests/'. Must be
    // excluded via the symbol-side filter. Surfaces the iter-275
    // bug [[user-probe-067]] surfaced on doc-linter source.
    assert!(
        !returned.iter().any(|(_, s)| s.contains("dead_code/tests/")),
        "in-file mod tests block (symbol CONTAINS 'tests/') must \
             NOT surface even when file path is non-test: {returned:?}",
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn architectural_patterns_lists_known_positive_pattern_tags() {
    // interrogation-028 Finding C: symmetric to user-probe-024's
    // failure-mode-catalog gap on the POSITIVE-pattern axis.
    // Pin the cypher to the 5 known positive pattern-tags so a
    // refactor that drops one breaks loudly.
    let q = SAVED_QUERIES
        .iter()
        .find(|q| q.name == "architectural-patterns")
        .expect("architectural-patterns must exist");
    for tag in [
        "absence-as-signal",
        "test-co-location",
        "query-reformulation",
        "cross-corpus-validation",
        "cross-workflow-comparison",
    ] {
        assert!(
            sql_of(q.name).contains(tag),
            "architectural-patterns must include pattern tag `{tag}`: {}",
            sql_of(q.name)
        );
    }
    assert!(
        sql_of(q.name).contains("pattern_tags"),
        "architectural-patterns must project pattern_tags: {}",
        sql_of(q.name)
    );
}

#[test]
fn docs_by_kind_filters_doc_kind_with_param() {
    let q = SAVED_QUERIES
        .iter()
        .find(|q| q.name == "docs-by-kind")
        .expect("docs-by-kind must exist");
    assert!(
        sql_of(q.name).contains("d.kind = $kind"),
        "docs-by-kind must filter by Doc.kind = $kind: {}",
        sql_of(q.name)
    );
    assert_eq!(q.params, &["kind"], "docs-by-kind must take kind param");
    // Project summary so the agent gets the kind sample with context:
    assert!(
        sql_of(q.name).contains("d.summary"),
        "docs-by-kind must project summary: {}",
        sql_of(q.name)
    );
}

#[test]
fn retrieval_failure_modes_lists_all_known_pattern_tags() {
    // user-probe-024 Finding C: no umbrella tag for the
    // failure-mode catalog. This compile-time query unifies
    // them by enumerating tag membership. Pin the cypher to
    // the FIVE distinct pattern-tag categories the loop has
    // named so a forgetting refactor breaks the test loudly.
    let q = SAVED_QUERIES
        .iter()
        .find(|q| q.name == "retrieval-failure-modes")
        .expect("retrieval-failure-modes must exist");
    for tag in [
        "vocabulary-mismatch",
        "tag-axis-loose-end",
        "negation-failure",
        "adjacent-family-displacement",
        "embedding-backend-not-compiled",
        "vocabulary-asymmetry",
    ] {
        assert!(
            sql_of(q.name).contains(tag),
            "retrieval-failure-modes must include pattern tag `{tag}`: {}",
            sql_of(q.name)
        );
    }
    assert!(
        sql_of(q.name).contains("failure_tags"),
        "retrieval-failure-modes must project failure_tags: {}",
        sql_of(q.name)
    );
}

#[test]
fn retrieval_mitigation_modes_lists_three_mitigation_tags() {
    // user-probe-024 Finding C companion: the mitigation
    // side of the catalogue, mirroring user-probe-023's
    // three-mitigation framing.
    let q = SAVED_QUERIES
        .iter()
        .find(|q| q.name == "retrieval-mitigation-modes")
        .expect("retrieval-mitigation-modes must exist");
    for tag in [
        "vocabulary-mismatch-mitigation",
        "audit-as-surface",
        "query-reformulation",
    ] {
        assert!(
            sql_of(q.name).contains(tag),
            "retrieval-mitigation-modes must include mitigation tag `{tag}`: {}",
            sql_of(q.name)
        );
    }
    assert!(
        sql_of(q.name).contains("mitigation_tags"),
        "retrieval-mitigation-modes must project mitigation_tags: {}",
        sql_of(q.name)
    );
}

/// Per iter 196 (interrogation-039 Finding D): seeds a
/// 5-Doc fixture representing the iter-195 code-embedding-
/// lineage absorption pattern. Two wikilink-paired papers
/// SHARE a meaningful family tag (must NOT surface). Two
/// wikilink-paired papers share NO meaningful tag (MUST
/// surface as a pair). One orphan paper has no wikilinks
/// but its sole meaningful tag is corpus-singleton among
/// research docs (MUST surface in the node-axis variant).
fn seed_research_family_tagging_fixture(conn: &testkit::Conn<'_>) {
    // 7 research-tagged Docs with varying tag/wikilink shapes.
    for (id, tags) in [
        // a + b + f: 3 members of code-embedding-lineage family.
        (
            "rp-a-good",
            "['research','paper','code-embedding-lineage','ast-paths']",
        ),
        (
            "rp-b-good",
            "['research','paper','code-embedding-lineage','capsules']",
        ),
        (
            "rp-f-shared",
            "['research','paper','code-embedding-lineage','embedding-model']",
        ),
        // c + d: wikilink-paired, share 0 meaningful tags.
        ("rp-c-bad", "['research','paper','unique-c-tag']"),
        ("rp-d-bad", "['research','paper','unique-d-tag']"),
        // e: no wikilinks, has only stopword + corpus-singleton tag.
        ("rp-e-orphan", "['research','paper','unique-e-tag']"),
        // hub-index: research-tagged BUT carries `index` tag.
        // Has wikilink to every paper for navigation. iter-199 fix
        // excludes this from all 3 family-coherence queries.
        ("rp-hub-index", "['research','index']"),
        // control — non-research narrative doc (must NOT surface).
        ("non-research-doc", "['narrative']"),
    ] {
        let cypher = format!(
            "CREATE (:Doc {{id: '{id}', path: '{id}.md', role: 'doc', \
                 kind: 'reference', lifecycle: '', bounded_context: '', \
                 title: '{id} title', summary: '', status: 'stable', \
                 updated: '2026-06-01', tags: {tags}, covers: []}})"
        );
        conn.query(&cypher).expect("seed research Doc insert");
    }
    // Wikilink edges:
    // (rp-a-good)->(rp-b-good) — sibling sharing family tag
    // (rp-c-bad)->(rp-d-bad) — sibling tag-disjoint
    // (rp-hub-index)->(rp-a-good) + (rp-hub-index)->(rp-c-bad) —
    //   hub-index navigational; iter-199 filter must drop these
    //   from pair-axis even though they're tag-disjoint with each
    //   member.
    for (src, dst) in [
        ("rp-a-good", "rp-b-good"),
        ("rp-c-bad", "rp-d-bad"),
        ("rp-hub-index", "rp-a-good"),
        ("rp-hub-index", "rp-c-bad"),
    ] {
        let cypher = format!(
            "MATCH (a:Doc), (b:Doc) WHERE a.id = '{src}' AND b.id = '{dst}' \
                 CREATE (a)-[:WIKILINK {{line: 0}}]->(b)"
        );
        conn.query(&cypher).expect("seed WIKILINK insert");
    }
}

#[test]
fn research_pairs_without_shared_family_tag_semantic_excludes_paired_and_orphan() {
    // Per interrogation-039 Finding D: the pair-axis
    // diagnostic must surface (rp-c-bad, rp-d-bad) — wikilinked
    // siblings with disjoint meaningful tags — and EXCLUDE
    // (rp-a-good, rp-b-good) which share `code-embedding-lineage`.
    // It must also exclude rp-e-orphan (no wikilink partner).
    let dir = std::env::temp_dir().join(format!(
        "doc-linter-saved-research-pairs-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let db = testkit::open_db(&dir);
    {
        let conn = testkit::connect(&db);
        testkit::reset(&conn);
    }
    let conn = testkit::connect(&db);
    seed_research_family_tagging_fixture(&conn);

    let q = SAVED_QUERIES
        .iter()
        .find(|q| q.name == "research-pairs-without-shared-family-tag")
        .expect("research-pairs-without-shared-family-tag must exist");
    let mut result = conn.query(sql_of(q.name)).expect("query runs");
    let mut returned_pairs: Vec<(String, String)> = Vec::new();
    for row in &mut result {
        if let (kv::Value::String(a), kv::Value::String(b)) = (&row[0], &row[1]) {
            returned_pairs.push((a.clone(), b.clone()));
        }
    }
    assert!(
        returned_pairs.contains(&("rp-c-bad".into(), "rp-d-bad".into())),
        "wikilinked-but-tag-disjoint pair must surface: {returned_pairs:?}",
    );
    assert!(
        !returned_pairs
            .iter()
            .any(|(a, b)| a == "rp-a-good" || b == "rp-a-good"),
        "shared-family-tag pair must NOT surface: {returned_pairs:?}",
    );
    assert!(
        !returned_pairs
            .iter()
            .any(|(a, b)| a == "rp-e-orphan" || b == "rp-e-orphan"),
        "no-wikilink-partner doc must NOT surface in pair-axis: {returned_pairs:?}",
    );
    // user-probe-047 Finding B: index-tagged hub-doc has WIKILINK
    // to every paper for navigation but no paper-specific tags.
    // The iter-199 fix MUST exclude rp-hub-index from pair-axis
    // even though it satisfies the wikilink-and-tag-disjoint
    // criteria.
    assert!(
            !returned_pairs
                .iter()
                .any(|(a, b)| a == "rp-hub-index" || b == "rp-hub-index"),
            "iter-199 user-probe-047 regression: index-hub-doc must NOT surface in pair-axis: {returned_pairs:?}",
        );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn research_docs_with_all_singleton_meaningful_tags_semantic_includes_orphan() {
    // Per interrogation-039 Finding A (node-axis companion):
    // rp-c-bad, rp-d-bad, rp-e-orphan all have their sole
    // meaningful tag appear on no OTHER research-tagged doc —
    // they must surface. rp-a-good and rp-b-good share
    // `code-embedding-lineage` so the orphan-count !=
    // total-meaningful — they must NOT surface.
    let dir = std::env::temp_dir().join(format!(
        "doc-linter-saved-research-docs-orphan-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let db = testkit::open_db(&dir);
    {
        let conn = testkit::connect(&db);
        testkit::reset(&conn);
    }
    let conn = testkit::connect(&db);
    seed_research_family_tagging_fixture(&conn);

    let q = SAVED_QUERIES
        .iter()
        .find(|q| q.name == "research-docs-with-all-singleton-meaningful-tags")
        .expect("research-docs-with-all-singleton-meaningful-tags must exist");
    let mut result = conn.query(sql_of(q.name)).expect("query runs");
    let mut returned_ids: Vec<String> = Vec::new();
    for row in &mut result {
        if let kv::Value::String(id) = &row[0] {
            returned_ids.push(id.clone());
        }
    }
    for expected in ["rp-c-bad", "rp-d-bad", "rp-e-orphan"] {
        assert!(
            returned_ids.iter().any(|id| id == expected),
            "all-singleton-meaningful-tags doc `{expected}` must surface: {returned_ids:?}",
        );
    }
    for excluded in [
        "rp-a-good",
        "rp-b-good",
        "rp-f-shared",
        "non-research-doc",
        // user-probe-047 Finding B: index-hub-doc excluded
        // by iter-199 fix even though `index` is its only
        // meaningful tag and corpus-singleton.
        "rp-hub-index",
    ] {
        assert!(
                !returned_ids.iter().any(|id| id == excluded),
                "non-orphan, non-research, or index-hub doc `{excluded}` must NOT surface: {returned_ids:?}",
            );
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn established_research_families_semantic_returns_3plus_member_families() {
    // Per iter 199 (user-probe-047 Finding C — positive-axis
    // companion test): the fixture seeds 3 docs (a + b + f)
    // carrying `code-embedding-lineage`. Established-research-
    // families MUST return that tag with 3 members. All other
    // tags appear on ≤1 doc and MUST NOT surface. The
    // index-hub-doc filter MUST also keep `index` out.
    let dir = std::env::temp_dir().join(format!(
        "doc-linter-saved-established-families-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let db = testkit::open_db(&dir);
    {
        let conn = testkit::connect(&db);
        testkit::reset(&conn);
    }
    let conn = testkit::connect(&db);
    seed_research_family_tagging_fixture(&conn);

    let q = SAVED_QUERIES
        .iter()
        .find(|q| q.name == "established-research-families")
        .expect("established-research-families must exist");
    let mut result = conn.query(sql_of(q.name)).expect("query runs");
    let mut returned_tags: Vec<String> = Vec::new();
    for row in &mut result {
        if let kv::Value::String(tag) = &row[0] {
            returned_tags.push(tag.clone());
        }
    }
    assert!(
        returned_tags.contains(&"code-embedding-lineage".to_string()),
        "3-member family tag must surface: {returned_tags:?}",
    );
    for excluded in [
        "unique-c-tag",
        "unique-d-tag",
        "unique-e-tag",
        "ast-paths",
        "capsules",
        "embedding-model",
        "index",
        "research",
        "paper",
        "narrative",
    ] {
        assert!(
            !returned_tags.iter().any(|t| t == excluded),
            "non-family or stopword tag `{excluded}` must NOT surface: {returned_tags:?}",
        );
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn cross_family_concept_tags_returns_family_with_sibling_spread() {
    // Per iter 286 ([[interrogation-061]] Finding B):
    // On the shared fixture, code-embedding-lineage is the
    // only family-tag (3 carriers: rp-a-good + rp-b-good +
    // rp-f-shared). Its sibling tags across members:
    //   ast-paths (from a) + capsules (from b) + embedding-model (from f)
    // = 3 distinct sibling tags. Must surface.
    // No other tag has ≥3 carriers; no other family-tag rows.
    let dir = std::env::temp_dir().join(format!(
        "doc-linter-saved-cross-family-concept-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let db = testkit::open_db(&dir);
    {
        let conn = testkit::connect(&db);
        testkit::reset(&conn);
    }
    let conn = testkit::connect(&db);
    seed_research_family_tagging_fixture(&conn);

    let q = SAVED_QUERIES
        .iter()
        .find(|q| q.name == "cross-family-concept-tags")
        .expect("cross-family-concept-tags must exist");
    let mut result = conn.query(sql_of(q.name)).expect("query runs");
    let mut rows: Vec<(String, i64, i64)> = Vec::new();
    for row in &mut result {
        let r = row.clone();
        let tag = if let kv::Value::String(s) = &r[0] {
            s.clone()
        } else {
            panic!("family_tag String: {r:?}");
        };
        // member_count from size(collect) is INT64.
        let mc = if let kv::Value::Int64(n) = &r[1] {
            *n
        } else {
            panic!("member_count Int64: {r:?}");
        };
        // sibling_tag_count via size(collect DISTINCT) is INT64.
        let sc = if let kv::Value::Int64(n) = &r[2] {
            *n
        } else {
            panic!("sibling_tag_count Int64: {r:?}");
        };
        rows.push((tag, mc, sc));
    }
    assert_eq!(rows.len(), 1, "exactly 1 family-tag in fixture: {rows:?}");
    let (tag, mc, sc) = &rows[0];
    assert_eq!(tag, "code-embedding-lineage");
    assert_eq!(*mc, 3, "3 carriers (a + b + f): {rows:?}");
    assert_eq!(
        *sc, 3,
        "3 distinct sibling tags (ast-paths + capsules + \
             embedding-model): {rows:?}",
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn cross_family_concept_tags_excludes_ingest_pass_storyline_meta_tag() {
    // Per iter 288 ([[user-probe-070]] Finding D): the
    // META-DOC tag `ingest-pass-storyline` leaked through
    // the iter-286 stopword filter and ranked 3rd at
    // sibling_tag_count=17 on the design corpus. Tag is
    // carried by research docs that authored absorbed-
    // lessons-need-to-filter-into-ingest notes. The
    // iter-288 fix adds it to BOTH focus + sibling
    // stopword filters. Fixture: 3 research docs each
    // carrying both `code-embedding-lineage` (family) AND
    // `ingest-pass-storyline` (meta). Post-fix:
    //   - family_tag = code-embedding-lineage MUST surface.
    //   - family_tag = ingest-pass-storyline MUST NOT.
    //   - ingest-pass-storyline MUST NOT appear in
    //     sibling_tags of code-embedding-lineage.
    let dir = std::env::temp_dir().join(format!(
        "doc-linter-saved-cross-family-meta-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let db = testkit::open_db(&dir);
    {
        let conn = testkit::connect(&db);
        testkit::reset(&conn);
    }
    let conn = testkit::connect(&db);
    for (id, sibling_tag) in [
        ("rp-x", "ast-paths"),
        ("rp-y", "capsules"),
        ("rp-z", "embedding-model"),
    ] {
        let tags = format!(
            "['research','paper','code-embedding-lineage',\
                 '{sibling_tag}','ingest-pass-storyline']"
        );
        conn.query(&format!(
            "CREATE (:Doc {{id: '{id}', path: '{id}.md', \
                 role: 'doc', kind: 'reference', lifecycle: '', \
                 bounded_context: '', title: 'T', summary: '', \
                 status: 'stable', updated: '2026-06-01', \
                 tags: {tags}, covers: []}})"
        ))
        .unwrap();
    }
    let q = SAVED_QUERIES
        .iter()
        .find(|q| q.name == "cross-family-concept-tags")
        .expect("cross-family-concept-tags must exist");
    let mut result = conn.query(sql_of(q.name)).expect("query runs");
    let mut rows: Vec<(String, Vec<String>)> = Vec::new();
    for row in &mut result {
        let r = row.clone();
        let tag = if let kv::Value::String(s) = &r[0] {
            s.clone()
        } else {
            panic!("family_tag String: {r:?}");
        };
        // sibling_tags is column 3 (List of String).
        let siblings = if let kv::Value::List(_, items) = &r[3] {
            items
                .iter()
                .map(|v| {
                    if let kv::Value::String(s) = v {
                        s.clone()
                    } else {
                        panic!("sibling element String: {v:?}")
                    }
                })
                .collect()
        } else {
            panic!("sibling_tags List: {r:?}");
        };
        rows.push((tag, siblings));
    }
    let cel = rows
        .iter()
        .find(|(t, _)| t == "code-embedding-lineage")
        .expect("code-embedding-lineage must surface as family_tag");
    assert!(
        !cel.1.iter().any(|s| s == "ingest-pass-storyline"),
        "ingest-pass-storyline must be filtered from sibling_tags: \
             {rows:?}",
    );
    assert!(
        !rows.iter().any(|(t, _)| t == "ingest-pass-storyline"),
        "ingest-pass-storyline must NOT appear as a family_tag: \
             {rows:?}",
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn family_tag_absorption_depth_returns_composite_score() {
    // Per iter 286 ([[interrogation-061]] Finding E
    // operationalisation): on the shared fixture,
    // code-embedding-lineage has member_count=3 and
    // sibling_tag_count=3, so depth_score = 9.
    let dir = std::env::temp_dir().join(format!(
        "doc-linter-saved-family-tag-depth-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let db = testkit::open_db(&dir);
    {
        let conn = testkit::connect(&db);
        testkit::reset(&conn);
    }
    let conn = testkit::connect(&db);
    seed_research_family_tagging_fixture(&conn);

    let q = SAVED_QUERIES
        .iter()
        .find(|q| q.name == "family-tag-absorption-depth")
        .expect("family-tag-absorption-depth must exist");
    let mut result = conn.query(sql_of(q.name)).expect("query runs");
    let mut rows: Vec<(String, i64, i64, i64)> = Vec::new();
    let as_i64 = |v: &kv::Value| -> i64 {
        match v {
            kv::Value::Int64(n) => *n,
            kv::Value::Int128(n) => *n as i64,
            other => panic!("expected int: {other:?}"),
        }
    };
    for row in &mut result {
        let r = row.clone();
        let tag = if let kv::Value::String(s) = &r[0] {
            s.clone()
        } else {
            panic!("family_tag String: {r:?}");
        };
        let mc = as_i64(&r[1]);
        let sc = as_i64(&r[2]);
        let ds = as_i64(&r[3]);
        rows.push((tag, mc, sc, ds));
    }
    assert_eq!(rows.len(), 1, "exactly 1 family-tag in fixture: {rows:?}");
    let (tag, mc, sc, ds) = &rows[0];
    assert_eq!(tag, "code-embedding-lineage");
    assert_eq!(*mc, 3, "member_count=3: {rows:?}");
    assert_eq!(*sc, 3, "sibling_tag_count=3: {rows:?}");
    assert_eq!(*ds, 9, "depth_score = 3 × 3 = 9: {rows:?}",);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn lessons_for_research_family_surfaces_audits_citing_family() {
    // Per iter 292 ([[user-probe-071]] Finding D):
    // Fixture: 3 research docs with `embedding-model`
    // tag + 2 audits (interrogation-x + user-probe-y)
    // that WIKILINK to 2 of them + 1 unrelated audit
    // citing a non-family doc. Query with tag=
    // 'embedding-model' must surface the 2 family-
    // citing audits ranked by cites_count, NOT the
    // unrelated audit.
    let dir = std::env::temp_dir().join(format!("doc-linter-saved-lessons-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let db = testkit::open_db(&dir);
    {
        let conn = testkit::connect(&db);
        testkit::reset(&conn);
    }
    let conn = testkit::connect(&db);
    let seed = |conn: &testkit::Conn<'_>, id: &str, tags: &str| {
        conn.query(&format!(
            "CREATE (:Doc {{id: '{id}', path: '{id}.md', \
                 role: 'doc', kind: 'reference', lifecycle: '', \
                 bounded_context: '', title: '{id} title', \
                 summary: '', status: 'stable', \
                 updated: '2026-06-01', tags: {tags}, covers: []}})"
        ))
        .unwrap();
    };
    seed(&conn, "research-a", "['research','embedding-model']");
    seed(&conn, "research-b", "['research','embedding-model']");
    seed(&conn, "research-c", "['research','embedding-model']");
    seed(&conn, "research-unrelated", "['research','other-fam']");
    seed(&conn, "interrogation-100", "['interrogation']");
    seed(&conn, "user-probe-200", "['user-probe']");
    seed(&conn, "interrogation-300", "['interrogation']");
    for (src, dst) in [
        ("interrogation-100", "research-a"),
        ("interrogation-100", "research-b"),
        ("user-probe-200", "research-c"),
        ("interrogation-300", "research-unrelated"),
    ] {
        conn.query(&format!(
            "MATCH (a:Doc), (b:Doc) WHERE a.id = '{src}' \
                 AND b.id = '{dst}' \
                 CREATE (a)-[:WIKILINK {{line: 0}}]->(b)"
        ))
        .unwrap();
    }
    let q = SAVED_QUERIES
        .iter()
        .find(|q| q.name == "lessons-for-research-family")
        .expect("lessons-for-research-family must exist");
    let cypher = sql_of(q.name).replace("$tag", "'embedding-model'");
    let mut result = conn.query(&cypher).expect("query runs");
    let mut rows: Vec<(String, i64)> = Vec::new();
    for row in &mut result {
        let r = row.clone();
        let id = if let kv::Value::String(s) = &r[0] {
            s.clone()
        } else {
            panic!("audit_id String: {r:?}");
        };
        let cites = if let kv::Value::Int64(n) = &r[2] {
            *n
        } else {
            panic!("cites_count Int64: {r:?}");
        };
        rows.push((id, cites));
    }
    let i100 = rows
        .iter()
        .find(|(id, _)| id == "interrogation-100")
        .expect("interrogation-100 must surface");
    assert_eq!(i100.1, 2, "interrogation-100 cites 2 family papers");
    let up200 = rows
        .iter()
        .find(|(id, _)| id == "user-probe-200")
        .expect("user-probe-200 must surface");
    assert_eq!(up200.1, 1, "user-probe-200 cites 1 family paper");
    assert!(
        !rows.iter().any(|(id, _)| id == "interrogation-300"),
        "interrogation-300 cites non-family doc; must NOT surface: \
             {rows:?}",
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn family_tags_by_code_domain_classifies_three_buckets() {
    // Per iter 292 ([[user-probe-071]] Finding B):
    // Fixture seeds 3 research docs per family:
    //   code-family (3 carry code-rag marker) → 'code-domain'
    //   mixed-family (1 carries code-rag, 2 don't) → 'mixed' (33%)
    //   generic-family (0 code markers) → 'generic'
    let dir = std::env::temp_dir().join(format!(
        "doc-linter-saved-code-domain-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let db = testkit::open_db(&dir);
    {
        let conn = testkit::connect(&db);
        testkit::reset(&conn);
    }
    let conn = testkit::connect(&db);
    let seed = |conn: &testkit::Conn<'_>, id: &str, tags: &str| {
        conn.query(&format!(
            "CREATE (:Doc {{id: '{id}', path: '{id}.md', \
                 role: 'doc', kind: 'reference', lifecycle: '', \
                 bounded_context: '', title: 'T', \
                 summary: '', status: 'stable', \
                 updated: '2026-06-01', tags: {tags}, covers: []}})"
        ))
        .unwrap();
    };
    // code-family: all 3 carry code-rag marker.
    seed(&conn, "rp-c1", "['research','code-family','code-rag']");
    seed(&conn, "rp-c2", "['research','code-family','code-rag']");
    seed(&conn, "rp-c3", "['research','code-family','code-rag']");
    // mixed-family: 1 of 3 carries marker (33% = mixed band).
    seed(&conn, "rp-m1", "['research','mixed-family','code-rag']");
    seed(&conn, "rp-m2", "['research','mixed-family','other-attr1']");
    seed(&conn, "rp-m3", "['research','mixed-family','other-attr2']");
    // generic-family: 0 markers.
    seed(&conn, "rp-g1", "['research','generic-family','attrA']");
    seed(&conn, "rp-g2", "['research','generic-family','attrB']");
    seed(&conn, "rp-g3", "['research','generic-family','attrC']");
    let q = SAVED_QUERIES
        .iter()
        .find(|q| q.name == "family-tags-by-code-domain")
        .expect("family-tags-by-code-domain must exist");
    let mut result = conn.query(sql_of(q.name)).expect("query runs");
    let mut rows: Vec<(String, String)> = Vec::new();
    for row in &mut result {
        let r = row.clone();
        let tag = if let kv::Value::String(s) = &r[0] {
            s.clone()
        } else {
            panic!("family_tag String: {r:?}");
        };
        let lbl = if let kv::Value::String(s) = &r[3] {
            s.clone()
        } else {
            panic!("domain_label String: {r:?}");
        };
        rows.push((tag, lbl));
    }
    let by_tag = |t: &str| -> String {
        rows.iter()
            .find(|(tag, _)| tag == t)
            .unwrap_or_else(|| panic!("{t} must surface: {rows:?}"))
            .1
            .clone()
    };
    assert_eq!(by_tag("code-family"), "code-domain");
    assert_eq!(by_tag("mixed-family"), "mixed");
    assert_eq!(by_tag("generic-family"), "generic");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn audits_mentioning_family_tag_surfaces_title_and_summary_matches() {
    // Per iter 294 ([[user-probe-072]] Finding D
    // fallback): title/summary CONTAINS lookup for
    // audit wisdom-layer when WIKILINK is sparse.
    // Fixture: 4 audits — 1 with family-tag in title,
    // 1 with family-tag in summary, 1 with both, 1
    // unrelated.
    let dir = std::env::temp_dir().join(format!(
        "doc-linter-saved-audits-mentioning-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let db = testkit::open_db(&dir);
    {
        let conn = testkit::connect(&db);
        testkit::reset(&conn);
    }
    let conn = testkit::connect(&db);
    let seed = |conn: &testkit::Conn<'_>, id: &str, title: &str, summary: &str| {
        conn.query(&format!(
            "CREATE (:Doc {{id: '{id}', path: '{id}.md', \
                 role: 'doc', kind: 'reference', lifecycle: '', \
                 bounded_context: '', title: '{title}', \
                 summary: '{summary}', status: 'stable', \
                 updated: '2026-06-01', tags: [], covers: []}})"
        ))
        .unwrap();
    };
    seed(
        &conn,
        "interrogation-001",
        "Title with dense-retrieval mention",
        "Generic summary",
    );
    seed(
        &conn,
        "interrogation-002",
        "Generic title",
        "Summary mentions dense-retrieval here",
    );
    seed(
        &conn,
        "user-probe-003",
        "Title dense-retrieval",
        "Summary dense-retrieval",
    );
    seed(
        &conn,
        "interrogation-004",
        "Unrelated title",
        "Unrelated summary",
    );
    let q = SAVED_QUERIES
        .iter()
        .find(|q| q.name == "audits-mentioning-family-tag")
        .expect("audits-mentioning-family-tag must exist");
    let cypher = sql_of(q.name).replace("$tag", "'dense-retrieval'");
    let mut result = conn.query(&cypher).expect("query runs");
    let mut ids: Vec<String> = Vec::new();
    for row in &mut result {
        if let kv::Value::String(s) = &row[0] {
            ids.push(s.clone());
        }
    }
    assert!(ids.contains(&"interrogation-001".to_string()));
    assert!(ids.contains(&"interrogation-002".to_string()));
    assert!(ids.contains(&"user-probe-003".to_string()));
    assert!(
        !ids.contains(&"interrogation-004".to_string()),
        "unrelated audit must NOT surface: {ids:?}",
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn audits_by_iter_tag_returns_audits_carrying_tag() {
    // Per iter 294 (sibling to audits-mentioning-
    // family-tag): iter-cohort-tag-based lookup.
    // Fixture: 4 audits — 2 carrying iter-283-
    // trio-retrievability tag, 1 with different
    // iter tag, 1 with no relevant tag.
    let dir = std::env::temp_dir().join(format!(
        "doc-linter-saved-audits-by-iter-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let db = testkit::open_db(&dir);
    {
        let conn = testkit::connect(&db);
        testkit::reset(&conn);
    }
    let conn = testkit::connect(&db);
    let seed = |conn: &testkit::Conn<'_>, id: &str, tags: &str| {
        conn.query(&format!(
            "CREATE (:Doc {{id: '{id}', path: '{id}.md', \
                 role: 'doc', kind: 'reference', lifecycle: '', \
                 bounded_context: '', title: 'T', summary: '', \
                 status: 'stable', updated: '2026-06-01', \
                 tags: {tags}, covers: []}})"
        ))
        .unwrap();
    };
    seed(
        &conn,
        "interrogation-100",
        "['interrogation','iter-283-trio']",
    );
    seed(&conn, "user-probe-200", "['user-probe','iter-283-trio']");
    seed(
        &conn,
        "interrogation-300",
        "['interrogation','iter-289-deployment']",
    );
    seed(&conn, "interrogation-400", "['interrogation']");
    let q = SAVED_QUERIES
        .iter()
        .find(|q| q.name == "audits-by-iter-tag")
        .expect("audits-by-iter-tag must exist");
    let cypher = sql_of(q.name).replace("$iter_tag", "'iter-283-trio'");
    let mut result = conn.query(&cypher).expect("query runs");
    let mut ids: Vec<String> = Vec::new();
    for row in &mut result {
        if let kv::Value::String(s) = &row[0] {
            ids.push(s.clone());
        }
    }
    assert_eq!(ids.len(), 2, "exactly 2 audits carry tag: {ids:?}");
    assert!(ids.contains(&"interrogation-100".to_string()));
    assert!(ids.contains(&"user-probe-200".to_string()));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn family_tag_audit_counts_summary_counts_audits_per_family() {
    // Per iter 297 (operationalises [[interrogation-063]]
    // Finding B): for each family-tag with ≥3 carriers,
    // count audits mentioning it. Fixture: 3 research
    // docs tagged 'code-embedding-lineage' + 2 audits
    // mentioning the tag in title + 1 audit NOT mentioning.
    let dir = std::env::temp_dir().join(format!(
        "doc-linter-saved-family-audit-counts-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let db = testkit::open_db(&dir);
    {
        let conn = testkit::connect(&db);
        testkit::reset(&conn);
    }
    let conn = testkit::connect(&db);
    seed_research_family_tagging_fixture(&conn);
    // Add 2 audits mentioning the family-tag + 1 not.
    let seed_audit = |conn: &testkit::Conn<'_>, id: &str, title: &str| {
        conn.query(&format!(
            "CREATE (:Doc {{id: '{id}', path: '{id}.md', \
                 role: 'doc', kind: 'reference', lifecycle: '', \
                 bounded_context: '', title: '{title}', \
                 summary: '', status: 'stable', \
                 updated: '2026-06-01', tags: [], covers: []}})"
        ))
        .unwrap();
    };
    seed_audit(
        &conn,
        "interrogation-901",
        "Probe of code-embedding-lineage",
    );
    seed_audit(
        &conn,
        "user-probe-902",
        "Validation of code-embedding-lineage",
    );
    seed_audit(&conn, "interrogation-903", "Unrelated topic");
    let q = SAVED_QUERIES
        .iter()
        .find(|q| q.name == "family-tag-audit-counts-summary")
        .expect("family-tag-audit-counts-summary must exist");
    let mut result = conn.query(sql_of(q.name)).expect("query runs");
    let mut rows: Vec<(String, i64)> = Vec::new();
    let as_i64 = |v: &kv::Value| -> i64 {
        match v {
            kv::Value::Int64(n) => *n,
            kv::Value::Int128(n) => *n as i64,
            other => panic!("expected int: {other:?}"),
        }
    };
    for row in &mut result {
        let r = row.clone();
        let tag = if let kv::Value::String(s) = &r[0] {
            s.clone()
        } else {
            panic!("family_tag String: {r:?}");
        };
        let count = as_i64(&r[2]);
        rows.push((tag, count));
    }
    let cel = rows
        .iter()
        .find(|(t, _)| t == "code-embedding-lineage")
        .expect("code-embedding-lineage must surface");
    assert_eq!(cel.1, 2, "2 audits mention the tag: {rows:?}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn family_tag_audit_counts_summary_excludes_short_generic_tags() {
    // Per iter 299 ([[user-probe-074]] Finding B fix):
    // short generic tags (3-5 chars) like 'doc' match
    // almost every audit title via CONTAINS, blowing
    // up audit_count spuriously. iter-299 filter:
    // size(family_tag) >= 6. Fixture: 3 research docs
    // carrying both 'doc' (3 chars — must be excluded)
    // and 'code-embedding-lineage' (22 chars — must
    // surface). Plus 5 audits whose titles contain
    // 'doc' substring (e.g., 'User probe X: doc-linter
    // setup' — pre-fix would attribute these to the
    // 'doc' family-tag).
    let dir = std::env::temp_dir().join(format!(
        "doc-linter-saved-short-tag-filter-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let db = testkit::open_db(&dir);
    {
        let conn = testkit::connect(&db);
        testkit::reset(&conn);
    }
    let conn = testkit::connect(&db);
    for id in ["rp-x", "rp-y", "rp-z"] {
        conn.query(&format!(
            "CREATE (:Doc {{id: '{id}', path: '{id}.md', \
                 role: 'doc', kind: 'reference', lifecycle: '', \
                 bounded_context: '', title: 'T', summary: '', \
                 status: 'stable', updated: '2026-06-01', \
                 tags: ['research','paper','doc',\
                 'code-embedding-lineage','ast-paths'], \
                 covers: []}})"
        ))
        .unwrap();
    }
    let seed_audit = |conn: &testkit::Conn<'_>, id: &str, title: &str| {
        conn.query(&format!(
            "CREATE (:Doc {{id: '{id}', path: '{id}.md', \
                 role: 'doc', kind: 'reference', lifecycle: '', \
                 bounded_context: '', title: '{title}', \
                 summary: '', status: 'stable', \
                 updated: '2026-06-01', tags: [], covers: []}})"
        ))
        .unwrap();
    };
    // 3 audits CONTAIN 'doc' substring (would be false-
    // positive matches to 'doc' family-tag pre-fix) +
    // 2 audits CONTAIN 'code-embedding-lineage' (true
    // matches).
    seed_audit(&conn, "interrogation-901", "User probe of doc-linter setup");
    seed_audit(&conn, "user-probe-902", "Interrogation of doc tooling");
    seed_audit(
        &conn,
        "interrogation-903",
        "Probe of code-embedding-lineage",
    );
    seed_audit(
        &conn,
        "user-probe-904",
        "Validation of code-embedding-lineage",
    );
    seed_audit(&conn, "interrogation-905", "Unrelated topic");
    let q = SAVED_QUERIES
        .iter()
        .find(|q| q.name == "family-tag-audit-counts-summary")
        .expect("family-tag-audit-counts-summary must exist");
    let mut result = conn.query(sql_of(q.name)).expect("query runs");
    let mut tags: Vec<String> = Vec::new();
    for row in &mut result {
        if let kv::Value::String(s) = &row[0] {
            tags.push(s.clone());
        }
    }
    assert!(
        !tags.contains(&"doc".to_string()),
        "'doc' (3 chars) must be filtered out: {tags:?}",
    );
    assert!(
        tags.contains(&"code-embedding-lineage".to_string()),
        "long family tag must surface: {tags:?}",
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn family_maturity_2d_returns_composite_axis_with_maturity_label() {
    // Per iter 297 (operationalises [[interrogation-063]]
    // Finding C): 2-axis composite ranking with
    // categorical maturity label. Use the shared
    // family-tagging fixture (3 carriers of code-
    // embedding-lineage + 3 sibling tags → depth_score
    // = 3 × 3 = 9). With 0 audits mentioning the tag,
    // the family won't surface (audit_count=0 filters
    // it out via the inner MATCH). Seed 2 audits
    // explicitly mentioning the family-tag so it
    // surfaces. depth_score=9 < 100 + audit_count=2
    // < 7 → 'narrow-recent' label.
    let dir = std::env::temp_dir().join(format!(
        "doc-linter-saved-family-maturity-2d-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let db = testkit::open_db(&dir);
    {
        let conn = testkit::connect(&db);
        testkit::reset(&conn);
    }
    let conn = testkit::connect(&db);
    seed_research_family_tagging_fixture(&conn);
    let seed_audit = |conn: &testkit::Conn<'_>, id: &str, title: &str| {
        conn.query(&format!(
            "CREATE (:Doc {{id: '{id}', path: '{id}.md', \
                 role: 'doc', kind: 'reference', lifecycle: '', \
                 bounded_context: '', title: '{title}', \
                 summary: '', status: 'stable', \
                 updated: '2026-06-01', tags: [], covers: []}})"
        ))
        .unwrap();
    };
    seed_audit(
        &conn,
        "interrogation-901",
        "Probe of code-embedding-lineage",
    );
    seed_audit(
        &conn,
        "user-probe-902",
        "Validation of code-embedding-lineage family",
    );
    let q = SAVED_QUERIES
        .iter()
        .find(|q| q.name == "family-maturity-2d")
        .expect("family-maturity-2d must exist");
    let mut result = conn.query(sql_of(q.name)).expect("query runs");
    let mut rows: Vec<(String, i64, i64, i64, String)> = Vec::new();
    let as_i64 = |v: &kv::Value| -> i64 {
        match v {
            kv::Value::Int64(n) => *n,
            kv::Value::Int128(n) => *n as i64,
            other => panic!("expected int: {other:?}"),
        }
    };
    for row in &mut result {
        let r = row.clone();
        let tag = if let kv::Value::String(s) = &r[0] {
            s.clone()
        } else {
            panic!("family_tag String: {r:?}");
        };
        let mc = as_i64(&r[1]);
        let ds = as_i64(&r[3]);
        let ac = as_i64(&r[4]);
        let lbl = if let kv::Value::String(s) = &r[5] {
            s.clone()
        } else {
            panic!("maturity_label String: {r:?}");
        };
        rows.push((tag, mc, ds, ac, lbl));
    }
    let cel = rows
        .iter()
        .find(|(t, _, _, _, _)| t == "code-embedding-lineage")
        .expect("code-embedding-lineage must surface");
    assert_eq!(cel.1, 3, "member_count=3");
    assert_eq!(cel.2, 9, "depth_score = 3 × 3 = 9");
    assert_eq!(cel.3, 2, "audit_count=2");
    assert_eq!(
        cel.4, "narrow-recent",
        "depth<100 + audits<7 → narrow-recent: {rows:?}",
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn doc_connectivity_ranking_returns_hubs_above_threshold() {
    // Per iter 303 ([[user-probe-075]] Finding B):
    // Fixture seeds 5 Docs in a hub-and-spoke topology:
    //   hub-doc: connected to 4 other docs (4 edges,
    //     mix of inbound + outbound)
    //   spoke-a: connected to hub + spoke-b (2 edges)
    //   spoke-b: connected to spoke-a + hub (2 edges)
    //   leaf-c: connected only to hub (1 edge) — below
    //     threshold 3
    //   isolated: no edges
    // Post-query: hub-doc surfaces with total=4;
    // spoke-a + spoke-b have 2 each (below threshold).
    // Adjust to threshold 3 — only hub surfaces.
    let dir = std::env::temp_dir().join(format!(
        "doc-linter-saved-doc-connectivity-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let db = testkit::open_db(&dir);
    {
        let conn = testkit::connect(&db);
        testkit::reset(&conn);
    }
    let conn = testkit::connect(&db);
    for id in ["hub-doc", "spoke-a", "spoke-b", "leaf-c", "isolated"] {
        conn.query(&format!(
            "CREATE (:Doc {{id: '{id}', path: '{id}.md', \
                 role: 'doc', kind: 'reference', lifecycle: '', \
                 bounded_context: '', title: 'T', summary: '', \
                 status: 'stable', updated: '2026-06-01', tags: [], \
                 covers: []}})"
        ))
        .unwrap();
    }
    // hub-doc has 4 edges (mix in + out).
    for (src, dst) in [
        ("hub-doc", "spoke-a"),
        ("hub-doc", "spoke-b"),
        ("leaf-c", "hub-doc"),
        ("spoke-a", "hub-doc"),
    ] {
        conn.query(&format!(
            "MATCH (a:Doc), (b:Doc) WHERE a.id = '{src}' \
                 AND b.id = '{dst}' \
                 CREATE (a)-[:WIKILINK {{line: 0}}]->(b)"
        ))
        .unwrap();
    }
    // spoke-a ↔ spoke-b: 1 edge (puts spoke-a at 3, spoke-b at 2).
    conn.query(
        "MATCH (a:Doc), (b:Doc) WHERE a.id = 'spoke-a' \
             AND b.id = 'spoke-b' \
             CREATE (a)-[:WIKILINK {line: 0}]->(b)",
    )
    .unwrap();
    let q = SAVED_QUERIES
        .iter()
        .find(|q| q.name == "doc-connectivity-ranking")
        .expect("doc-connectivity-ranking must exist");
    let mut result = conn.query(sql_of(q.name)).expect("query runs");
    let mut rows: Vec<(String, i64)> = Vec::new();
    let as_i64 = |v: &kv::Value| -> i64 {
        match v {
            kv::Value::Int64(n) => *n,
            kv::Value::Int128(n) => *n as i64,
            other => panic!("expected int: {other:?}"),
        }
    };
    for row in &mut result {
        let r = row.clone();
        let id = if let kv::Value::String(s) = &r[0] {
            s.clone()
        } else {
            panic!("id String: {r:?}");
        };
        let total = as_i64(&r[3]);
        rows.push((id, total));
    }
    let hub = rows
        .iter()
        .find(|(id, _)| id == "hub-doc")
        .expect("hub-doc must surface");
    assert_eq!(hub.1, 4, "hub-doc has 4 wikilink connections");
    let sa = rows
        .iter()
        .find(|(id, _)| id == "spoke-a")
        .expect("spoke-a has 3 connections, must surface");
    assert_eq!(sa.1, 3);
    assert!(
        !rows.iter().any(|(id, _)| id == "leaf-c"),
        "leaf-c has 1 connection, below threshold: {rows:?}",
    );
    assert!(
        !rows.iter().any(|(id, _)| id == "isolated"),
        "isolated has 0 connections, must NOT surface: {rows:?}",
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn feature_doc_connectivity_filters_to_feature_role() {
    // Per iter 303 (sibling): only feature-role docs
    // surface. Fixture: 1 feature + 2 docs + 1 leaf
    // feature with no edges (must still surface; this
    // variant has no threshold).
    let dir = std::env::temp_dir().join(format!(
        "doc-linter-saved-feature-conn-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let db = testkit::open_db(&dir);
    {
        let conn = testkit::connect(&db);
        testkit::reset(&conn);
    }
    let conn = testkit::connect(&db);
    for (id, role) in [
        ("feature-x", "feature"),
        ("feature-y", "feature"),
        ("doc-a", "doc"),
        ("doc-b", "doc"),
    ] {
        conn.query(&format!(
            "CREATE (:Doc {{id: '{id}', path: '{id}.md', \
                 role: '{role}', kind: 'reference', lifecycle: '', \
                 bounded_context: '', title: 'T', summary: '', \
                 status: 'stable', updated: '2026-06-01', tags: [], \
                 covers: []}})"
        ))
        .unwrap();
    }
    // feature-x has 2 edges; feature-y has 0; doc-a has 1.
    for (src, dst) in [("feature-x", "doc-a"), ("doc-b", "feature-x")] {
        conn.query(&format!(
            "MATCH (a:Doc), (b:Doc) WHERE a.id = '{src}' \
                 AND b.id = '{dst}' \
                 CREATE (a)-[:WIKILINK {{line: 0}}]->(b)"
        ))
        .unwrap();
    }
    let q = SAVED_QUERIES
        .iter()
        .find(|q| q.name == "feature-doc-connectivity")
        .expect("feature-doc-connectivity must exist");
    let mut result = conn.query(sql_of(q.name)).expect("query runs");
    let mut rows: Vec<(String, i64)> = Vec::new();
    let as_i64 = |v: &kv::Value| -> i64 {
        match v {
            kv::Value::Int64(n) => *n,
            kv::Value::Int128(n) => *n as i64,
            other => panic!("expected int: {other:?}"),
        }
    };
    for row in &mut result {
        let r = row.clone();
        let id = if let kv::Value::String(s) = &r[0] {
            s.clone()
        } else {
            panic!("id String: {r:?}");
        };
        let total = as_i64(&r[2]);
        rows.push((id, total));
    }
    let fx = rows
        .iter()
        .find(|(id, _)| id == "feature-x")
        .expect("feature-x must surface");
    assert_eq!(fx.1, 2);
    let fy = rows
        .iter()
        .find(|(id, _)| id == "feature-y")
        .expect("feature-y must surface (no threshold)");
    assert_eq!(fy.1, 0);
    assert!(
        !rows.iter().any(|(id, _)| id == "doc-a"),
        "doc-a is role=doc, must NOT surface: {rows:?}",
    );
    assert!(
        !rows.iter().any(|(id, _)| id == "doc-b"),
        "doc-b is role=doc, must NOT surface: {rows:?}",
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn doc_hub_tier_classifier_assigns_4_tier_labels() {
    // Per iter 305 ([[user-probe-076]] Finding D):
    // fixture seeds 4 docs at threshold boundaries:
    //   t1-hub: 40 connections → 'tier-1-architectural-narrative'
    //   t2-hub: 25 connections → 'tier-2-substantial'
    //   t3-hub: 15 connections → 'tier-3-active-participation'
    //   t4-hub: 3 connections  → 'tier-4-leaf-or-feature'
    let dir = std::env::temp_dir().join(format!(
        "doc-linter-saved-tier-classifier-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let db = testkit::open_db(&dir);
    {
        let conn = testkit::connect(&db);
        testkit::reset(&conn);
    }
    let conn = testkit::connect(&db);
    // Seed the 4 hub docs.
    for id in ["t1-hub", "t2-hub", "t3-hub", "t4-hub"] {
        conn.query(&format!(
            "CREATE (:Doc {{id: '{id}', path: '{id}.md', \
                 role: 'doc', kind: 'reference', lifecycle: '', \
                 bounded_context: '', title: 'T', summary: '', \
                 status: 'stable', updated: '2026-06-01', tags: [], \
                 covers: []}})"
        ))
        .unwrap();
    }
    // Seed leaf docs to connect to each hub at the
    // boundary count. Each leaf gets 1 wikilink to its hub.
    let mut leaf_idx = 0;
    for (hub, edge_count) in [
        ("t1-hub", 40),
        ("t2-hub", 25),
        ("t3-hub", 15),
        ("t4-hub", 3),
    ] {
        for _ in 0..edge_count {
            let leaf_id = format!("leaf-{leaf_idx:04}");
            conn.query(&format!(
                "CREATE (:Doc {{id: '{leaf_id}', \
                     path: '{leaf_id}.md', role: 'doc', \
                     kind: 'reference', lifecycle: '', \
                     bounded_context: '', title: 'L', \
                     summary: '', status: 'stable', \
                     updated: '2026-06-01', tags: [], covers: []}})"
            ))
            .unwrap();
            conn.query(&format!(
                "MATCH (a:Doc), (b:Doc) WHERE a.id = '{leaf_id}' \
                     AND b.id = '{hub}' \
                     CREATE (a)-[:WIKILINK {{line: 0}}]->(b)"
            ))
            .unwrap();
            leaf_idx += 1;
        }
    }
    let q = SAVED_QUERIES
        .iter()
        .find(|q| q.name == "doc-hub-tier-classifier")
        .expect("doc-hub-tier-classifier must exist");
    let mut result = conn.query(sql_of(q.name)).expect("query runs");
    let mut rows: Vec<(String, String)> = Vec::new();
    for row in &mut result {
        let r = row.clone();
        let id = if let kv::Value::String(s) = &r[0] {
            s.clone()
        } else {
            panic!("id String: {r:?}");
        };
        let tier = if let kv::Value::String(s) = &r[4] {
            s.clone()
        } else {
            panic!("tier_label String: {r:?}");
        };
        rows.push((id, tier));
    }
    let by_id = |t: &str| -> String {
        rows.iter()
            .find(|(id, _)| id == t)
            .unwrap_or_else(|| panic!("{t} must surface: {rows:?}"))
            .1
            .clone()
    };
    assert_eq!(by_id("t1-hub"), "tier-1-architectural-narrative");
    assert_eq!(by_id("t2-hub"), "tier-2-substantial");
    assert_eq!(by_id("t3-hub"), "tier-3-active-participation");
    assert_eq!(by_id("t4-hub"), "tier-4-leaf-or-feature");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn tier1_architectural_hubs_filters_to_40_plus() {
    // Per iter 305 (sibling): tier-1-only filter.
    // Fixture: 2 hubs — t1 with 40 connections (must
    // surface), t2 with 39 connections (must NOT
    // surface — below threshold).
    let dir = std::env::temp_dir().join(format!(
        "doc-linter-saved-tier1-hubs-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let db = testkit::open_db(&dir);
    {
        let conn = testkit::connect(&db);
        testkit::reset(&conn);
    }
    let conn = testkit::connect(&db);
    for id in ["t1-hub", "t2-near-miss"] {
        conn.query(&format!(
            "CREATE (:Doc {{id: '{id}', path: '{id}.md', \
                 role: 'doc', kind: 'reference', lifecycle: '', \
                 bounded_context: '', title: 'T', summary: '', \
                 status: 'stable', updated: '2026-06-01', tags: [], \
                 covers: []}})"
        ))
        .unwrap();
    }
    let mut leaf_idx = 0;
    for (hub, edge_count) in [("t1-hub", 40), ("t2-near-miss", 39)] {
        for _ in 0..edge_count {
            let leaf_id = format!("leaf-{leaf_idx:04}");
            conn.query(&format!(
                "CREATE (:Doc {{id: '{leaf_id}', \
                     path: '{leaf_id}.md', role: 'doc', \
                     kind: 'reference', lifecycle: '', \
                     bounded_context: '', title: 'L', \
                     summary: '', status: 'stable', \
                     updated: '2026-06-01', tags: [], covers: []}})"
            ))
            .unwrap();
            conn.query(&format!(
                "MATCH (a:Doc), (b:Doc) WHERE a.id = '{leaf_id}' \
                     AND b.id = '{hub}' \
                     CREATE (a)-[:WIKILINK {{line: 0}}]->(b)"
            ))
            .unwrap();
            leaf_idx += 1;
        }
    }
    let q = SAVED_QUERIES
        .iter()
        .find(|q| q.name == "tier1-architectural-hubs")
        .expect("tier1-architectural-hubs must exist");
    let mut result = conn.query(sql_of(q.name)).expect("query runs");
    let mut ids: Vec<String> = Vec::new();
    for row in &mut result {
        if let kv::Value::String(s) = &row[0] {
            ids.push(s.clone());
        }
    }
    assert_eq!(ids.len(), 1, "exactly 1 tier-1 hub: {ids:?}");
    assert_eq!(ids[0], "t1-hub");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn family_to_feature_bridges_surfaces_research_to_feature_wikilinks() {
    // Per iter 312 ([[feedback_cross_axis_bridge_pattern]]):
    // Fixture: 2 research docs in 'reranking' family
    // + 1 feature doc + WIKILINK from research-x → feature-y.
    // Also seed a research doc in different family that
    // wikilinks the same feature (must NOT surface for
    // 'reranking' query).
    let dir = std::env::temp_dir().join(format!(
        "doc-linter-saved-family-bridges-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let db = testkit::open_db(&dir);
    {
        let conn = testkit::connect(&db);
        testkit::reset(&conn);
    }
    let conn = testkit::connect(&db);
    let seed_doc = |conn: &testkit::Conn<'_>, id: &str, role: &str, tags: &str| {
        conn.query(&format!(
            "CREATE (:Doc {{id: '{id}', path: '{id}.md', \
                 role: '{role}', kind: 'reference', lifecycle: '', \
                 bounded_context: '', title: '{id} title', \
                 summary: '', status: 'stable', \
                 updated: '2026-06-01', tags: {tags}, covers: []}})"
        ))
        .unwrap();
    };
    seed_doc(&conn, "research-x", "doc", "['research','reranking']");
    seed_doc(&conn, "research-y", "doc", "['research','reranking']");
    seed_doc(&conn, "research-z", "doc", "['research','other-family']");
    seed_doc(&conn, "feature-judge", "feature", "['design']");
    for (src, dst) in [
        ("research-x", "feature-judge"),
        ("research-z", "feature-judge"),
    ] {
        conn.query(&format!(
            "MATCH (a:Doc), (b:Doc) WHERE a.id = '{src}' \
                 AND b.id = '{dst}' \
                 CREATE (a)-[:WIKILINK {{line: 0}}]->(b)"
        ))
        .unwrap();
    }
    let q = SAVED_QUERIES
        .iter()
        .find(|q| q.name == "family-to-feature-bridges")
        .expect("family-to-feature-bridges must exist");
    let cypher = sql_of(q.name).replace("$tag", "'reranking'");
    let mut result = conn.query(&cypher).expect("query runs");
    let mut rows: Vec<(String, String)> = Vec::new();
    for row in &mut result {
        let r = row.clone();
        let r_id = if let kv::Value::String(s) = &r[0] {
            s.clone()
        } else {
            panic!("research_id String: {r:?}");
        };
        let f_id = if let kv::Value::String(s) = &r[2] {
            s.clone()
        } else {
            panic!("feature_id String: {r:?}");
        };
        rows.push((r_id, f_id));
    }
    assert_eq!(rows.len(), 1, "1 bridge in family: {rows:?}");
    assert_eq!(rows[0].0, "research-x");
    assert_eq!(rows[0].1, "feature-judge");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn feature_incoming_from_research_counts_per_feature() {
    // Per iter 312 (sibling): for each feature, count
    // incoming wikilinks from research-tagged Docs.
    // Fixture: 2 features + 3 research docs.
    // feature-popular receives 2 research wikilinks;
    // feature-leaf receives 0. Plus a non-research
    // doc wikilinks both — should NOT count.
    let dir = std::env::temp_dir().join(format!(
        "doc-linter-saved-feature-incoming-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let db = testkit::open_db(&dir);
    {
        let conn = testkit::connect(&db);
        testkit::reset(&conn);
    }
    let conn = testkit::connect(&db);
    let seed = |conn: &testkit::Conn<'_>, id: &str, role: &str, tags: &str| {
        conn.query(&format!(
            "CREATE (:Doc {{id: '{id}', path: '{id}.md', \
                 role: '{role}', kind: 'reference', lifecycle: '', \
                 bounded_context: '', title: '{id}', \
                 summary: '', status: 'stable', \
                 updated: '2026-06-01', tags: {tags}, covers: []}})"
        ))
        .unwrap();
    };
    seed(&conn, "feature-popular", "feature", "['design']");
    seed(&conn, "feature-leaf", "feature", "['design']");
    seed(&conn, "research-a", "doc", "['research']");
    seed(&conn, "research-b", "doc", "['research']");
    seed(&conn, "non-research", "doc", "['interrogation']");
    for (src, dst) in [
        ("research-a", "feature-popular"),
        ("research-b", "feature-popular"),
        ("non-research", "feature-popular"),
        ("non-research", "feature-leaf"),
    ] {
        conn.query(&format!(
            "MATCH (a:Doc), (b:Doc) WHERE a.id = '{src}' \
                 AND b.id = '{dst}' \
                 CREATE (a)-[:WIKILINK {{line: 0}}]->(b)"
        ))
        .unwrap();
    }
    let q = SAVED_QUERIES
        .iter()
        .find(|q| q.name == "feature-incoming-from-research")
        .expect("feature-incoming-from-research must exist");
    let mut result = conn.query(sql_of(q.name)).expect("query runs");
    let mut rows: Vec<(String, i64)> = Vec::new();
    let as_i64 = |v: &kv::Value| -> i64 {
        match v {
            kv::Value::Int64(n) => *n,
            kv::Value::Int128(n) => *n as i64,
            other => panic!("expected int: {other:?}"),
        }
    };
    for row in &mut result {
        let r = row.clone();
        let id = if let kv::Value::String(s) = &r[0] {
            s.clone()
        } else {
            panic!("feature_id String: {r:?}");
        };
        let count = as_i64(&r[2]);
        rows.push((id, count));
    }
    let popular = rows
        .iter()
        .find(|(id, _)| id == "feature-popular")
        .expect("feature-popular must surface");
    assert_eq!(popular.1, 2, "2 research incoming: {rows:?}");
    let leaf = rows
        .iter()
        .find(|(id, _)| id == "feature-leaf")
        .expect("feature-leaf must surface (OPTIONAL MATCH)");
    assert_eq!(leaf.1, 0);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn retrieval_pipeline_cookbook_stages_research_docs_by_tag() {
    // Per iter 317 ([[user-probe-080]] PIPELINE
    // COMPLETENESS operationalisation):
    // Fixture seeds 5 research docs, one per
    // pipeline-stage family-tag. Plus a non-pipeline
    // research doc and a non-research doc.
    let dir = std::env::temp_dir().join(format!(
        "doc-linter-saved-pipeline-cookbook-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let db = testkit::open_db(&dir);
    {
        let conn = testkit::connect(&db);
        testkit::reset(&conn);
    }
    let conn = testkit::connect(&db);
    let seed = |conn: &testkit::Conn<'_>, id: &str, tags: &str| {
        conn.query(&format!(
            "CREATE (:Doc {{id: '{id}', path: '{id}.md', \
                 role: 'doc', kind: 'reference', lifecycle: '', \
                 bounded_context: '', title: '{id}', summary: '', \
                 status: 'stable', updated: '2026-06-01', \
                 tags: {tags}, covers: []}})"
        ))
        .unwrap();
    };
    seed(&conn, "r-q", "['research','llm-era-query-rewriting']");
    seed(&conn, "r-c", "['research','chunking']");
    seed(&conn, "r-e", "['research','deployment-model']");
    seed(&conn, "r-r", "['research','reranking']");
    seed(&conn, "r-eval", "['research','rag-evaluation']");
    seed(&conn, "r-other", "['research','graph-similarity']");
    seed(&conn, "non-research", "['interrogation']");
    let q = SAVED_QUERIES
        .iter()
        .find(|q| q.name == "retrieval-pipeline-cookbook")
        .expect("retrieval-pipeline-cookbook must exist");
    let mut result = conn.query(sql_of(q.name)).expect("query runs");
    let mut rows: Vec<(String, String)> = Vec::new();
    for row in &mut result {
        let r = row.clone();
        let id = if let kv::Value::String(s) = &r[0] {
            s.clone()
        } else {
            panic!("id String: {r:?}");
        };
        let stage = if let kv::Value::String(s) = &r[2] {
            s.clone()
        } else {
            panic!("stage_label String: {r:?}");
        };
        rows.push((id, stage));
    }
    assert_eq!(rows.len(), 5, "5 pipeline docs: {rows:?}");
    let by_id = |t: &str| -> String {
        rows.iter()
            .find(|(id, _)| id == t)
            .unwrap_or_else(|| panic!("{t} must surface: {rows:?}"))
            .1
            .clone()
    };
    assert_eq!(by_id("r-q"), "1-query-rewriting");
    assert_eq!(by_id("r-c"), "2-chunking");
    assert_eq!(by_id("r-e"), "3-embedding");
    assert_eq!(by_id("r-r"), "4-reranking");
    assert_eq!(by_id("r-eval"), "5-evaluation");
    assert!(
        !rows.iter().any(|(id, _)| id == "r-other"),
        "non-pipeline research excluded: {rows:?}",
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn pipeline_stage_coverage_emits_per_stage_label() {
    // Per iter 317 (sibling): per-stage member-count +
    // coverage_label. Fixture seeds 3 docs at one
    // stage (trio) + 1 doc at another (partial) + 0
    // at others.
    let dir = std::env::temp_dir().join(format!(
        "doc-linter-saved-pipeline-coverage-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let db = testkit::open_db(&dir);
    {
        let conn = testkit::connect(&db);
        testkit::reset(&conn);
    }
    let conn = testkit::connect(&db);
    let seed = |conn: &testkit::Conn<'_>, id: &str, tags: &str| {
        conn.query(&format!(
            "CREATE (:Doc {{id: '{id}', path: '{id}.md', \
                 role: 'doc', kind: 'reference', lifecycle: '', \
                 bounded_context: '', title: '{id}', summary: '', \
                 status: 'stable', updated: '2026-06-01', \
                 tags: {tags}, covers: []}})"
        ))
        .unwrap();
    };
    // 3 chunking docs (trio).
    seed(&conn, "r-c-1", "['research','chunking']");
    seed(&conn, "r-c-2", "['research','chunking']");
    seed(&conn, "r-c-3", "['research','chunking']");
    // 1 reranking doc (partial).
    seed(&conn, "r-r-1", "['research','reranking']");
    let q = SAVED_QUERIES
        .iter()
        .find(|q| q.name == "pipeline-stage-coverage")
        .expect("pipeline-stage-coverage must exist");
    let mut result = conn.query(sql_of(q.name)).expect("query runs");
    let mut rows: Vec<(String, i64, String)> = Vec::new();
    let as_i64 = |v: &kv::Value| -> i64 {
        match v {
            kv::Value::Int64(n) => *n,
            kv::Value::Int128(n) => *n as i64,
            other => panic!("expected int: {other:?}"),
        }
    };
    for row in &mut result {
        let r = row.clone();
        let stage = if let kv::Value::String(s) = &r[0] {
            s.clone()
        } else {
            panic!("stage_label String: {r:?}");
        };
        let count = as_i64(&r[1]);
        let lbl = if let kv::Value::String(s) = &r[2] {
            s.clone()
        } else {
            panic!("coverage_label String: {r:?}");
        };
        rows.push((stage, count, lbl));
    }
    let by_stage = |t: &str| -> (i64, String) {
        let r = rows
            .iter()
            .find(|(stage, _, _)| stage == t)
            .unwrap_or_else(|| panic!("{t} must surface: {rows:?}"));
        (r.1, r.2.clone())
    };
    let (cc, cl) = by_stage("2-chunking");
    assert_eq!(cc, 3);
    assert_eq!(cl, "trio-coverage");
    let (rc, rl) = by_stage("4-reranking");
    assert_eq!(rc, 1);
    assert_eq!(rl, "partial-coverage");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn graph_similarity_cookbook_labels_subfamilies() {
    // Per iter 319 (extends iter-317 pattern):
    // Fixture seeds graph-similarity members across
    // 4 subfamilies + 1 'other' + 1 unrelated.
    let dir = std::env::temp_dir().join(format!(
        "doc-linter-saved-graph-sim-cookbook-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let db = testkit::open_db(&dir);
    {
        let conn = testkit::connect(&db);
        testkit::reset(&conn);
    }
    let conn = testkit::connect(&db);
    let seed = |conn: &testkit::Conn<'_>, id: &str, tags: &str| {
        conn.query(&format!(
            "CREATE (:Doc {{id: '{id}', path: '{id}.md', \
                 role: 'doc', kind: 'reference', lifecycle: '', \
                 bounded_context: '', title: '{id}', summary: '', \
                 status: 'stable', updated: '2026-06-01', \
                 tags: {tags}, covers: []}})"
        ))
        .unwrap();
    };
    seed(
        &conn,
        "r-wl",
        "['research','graph-similarity','graph-kernel']",
    );
    seed(
        &conn,
        "r-g2v",
        "['research','graph-similarity','graph-embedding']",
    );
    seed(
        &conn,
        "r-simgnn",
        "['research','graph-similarity','neural-graph-matching']",
    );
    seed(
        &conn,
        "r-gmn",
        "['research','graph-similarity','cross-attention']",
    );
    seed(&conn, "r-nm", "['research','graph-similarity']");
    seed(&conn, "r-unrelated", "['research','code-rag']");
    let q = SAVED_QUERIES
        .iter()
        .find(|q| q.name == "graph-similarity-cookbook")
        .expect("graph-similarity-cookbook must exist");
    let mut result = conn.query(sql_of(q.name)).expect("query runs");
    let mut rows: Vec<(String, String)> = Vec::new();
    for row in &mut result {
        let r = row.clone();
        let id = if let kv::Value::String(s) = &r[0] {
            s.clone()
        } else {
            panic!("id String: {r:?}");
        };
        let lbl = if let kv::Value::String(s) = &r[2] {
            s.clone()
        } else {
            panic!("subfamily_label String: {r:?}");
        };
        rows.push((id, lbl));
    }
    assert_eq!(rows.len(), 5, "5 graph-similarity members: {rows:?}");
    let by_id = |t: &str| -> String {
        rows.iter()
            .find(|(id, _)| id == t)
            .unwrap_or_else(|| panic!("{t} must surface: {rows:?}"))
            .1
            .clone()
    };
    assert_eq!(by_id("r-wl"), "graph-kernel");
    assert_eq!(by_id("r-g2v"), "graph-embedding");
    assert_eq!(by_id("r-simgnn"), "neural-graph-matching");
    assert_eq!(by_id("r-gmn"), "cross-attention");
    assert_eq!(by_id("r-nm"), "other");
    assert!(
        !rows.iter().any(|(id, _)| id == "r-unrelated"),
        "non-family research excluded: {rows:?}",
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn graph_similarity_subfamily_coverage_emits_categorical_label() {
    // Per iter 319 sibling: per-subfamily count +
    // coverage_label. Fixture: 2 graph-kernel (rich)
    // + 1 neural-graph-matching (singleton).
    let dir = std::env::temp_dir().join(format!(
        "doc-linter-saved-graph-sim-cov-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let db = testkit::open_db(&dir);
    {
        let conn = testkit::connect(&db);
        testkit::reset(&conn);
    }
    let conn = testkit::connect(&db);
    let seed = |conn: &testkit::Conn<'_>, id: &str, tags: &str| {
        conn.query(&format!(
            "CREATE (:Doc {{id: '{id}', path: '{id}.md', \
                 role: 'doc', kind: 'reference', lifecycle: '', \
                 bounded_context: '', title: '{id}', summary: '', \
                 status: 'stable', updated: '2026-06-01', \
                 tags: {tags}, covers: []}})"
        ))
        .unwrap();
    };
    seed(
        &conn,
        "r-wl1",
        "['research','graph-similarity','graph-kernel']",
    );
    seed(
        &conn,
        "r-wl2",
        "['research','graph-similarity','graph-kernel']",
    );
    seed(
        &conn,
        "r-simgnn",
        "['research','graph-similarity','neural-graph-matching']",
    );
    let q = SAVED_QUERIES
        .iter()
        .find(|q| q.name == "graph-similarity-subfamily-coverage")
        .expect("graph-similarity-subfamily-coverage must exist");
    let mut result = conn.query(sql_of(q.name)).expect("query runs");
    let mut rows: Vec<(String, i64, String)> = Vec::new();
    let as_i64 = |v: &kv::Value| -> i64 {
        match v {
            kv::Value::Int64(n) => *n,
            kv::Value::Int128(n) => *n as i64,
            other => panic!("expected int: {other:?}"),
        }
    };
    for row in &mut result {
        let r = row.clone();
        let lbl = if let kv::Value::String(s) = &r[0] {
            s.clone()
        } else {
            panic!("subfamily_label String: {r:?}");
        };
        let count = as_i64(&r[1]);
        let cov = if let kv::Value::String(s) = &r[2] {
            s.clone()
        } else {
            panic!("coverage_label String: {r:?}");
        };
        rows.push((lbl, count, cov));
    }
    let by_sub = |t: &str| -> (i64, String) {
        let r = rows
            .iter()
            .find(|(lbl, _, _)| lbl == t)
            .unwrap_or_else(|| panic!("{t} must surface: {rows:?}"));
        (r.1, r.2.clone())
    };
    let (gk_count, gk_cov) = by_sub("graph-kernel");
    assert_eq!(gk_count, 2);
    assert_eq!(gk_cov, "rich");
    let (ngm_count, ngm_cov) = by_sub("neural-graph-matching");
    assert_eq!(ngm_count, 1);
    assert_eq!(ngm_cov, "singleton");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn query_expansion_cookbook_labels_carpineto_taxonomy() {
    // Per iter 321: query-expansion family with
    // mixed-mature shape. Fixture: 1 each of aqe-
    // local + aqe-global + aqe-external + survey +
    // 3 llm-era. Expected: 4 singletons + 1 rich.
    let dir = std::env::temp_dir().join(format!(
        "doc-linter-saved-qe-cookbook-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let db = testkit::open_db(&dir);
    {
        let conn = testkit::connect(&db);
        testkit::reset(&conn);
    }
    let conn = testkit::connect(&db);
    let seed = |conn: &testkit::Conn<'_>, id: &str, tags: &str| {
        conn.query(&format!(
            "CREATE (:Doc {{id: '{id}', path: '{id}.md', \
                 role: 'doc', kind: 'reference', lifecycle: '', \
                 bounded_context: '', title: '{id}', summary: '', \
                 status: 'stable', updated: '2026-06-01', \
                 tags: {tags}, covers: []}})"
        ))
        .unwrap();
    };
    seed(
        &conn,
        "r-rocchio",
        "['research','query-expansion','aqe-local']",
    );
    seed(
        &conn,
        "r-lsi",
        "['research','query-expansion','aqe-global']",
    );
    seed(
        &conn,
        "r-voorhees",
        "['research','query-expansion','aqe-external']",
    );
    seed(
        &conn,
        "r-carpineto",
        "['research','query-expansion','vocabulary-mismatch']",
    );
    seed(
        &conn,
        "r-hyde",
        "['research','query-expansion','llm-era-query-rewriting']",
    );
    seed(
        &conn,
        "r-query2doc",
        "['research','query-expansion','llm-era-query-rewriting']",
    );
    seed(
        &conn,
        "r-stepback",
        "['research','query-expansion','llm-era-query-rewriting']",
    );
    let q = SAVED_QUERIES
        .iter()
        .find(|q| q.name == "query-expansion-cookbook")
        .expect("query-expansion-cookbook must exist");
    let mut result = conn.query(sql_of(q.name)).expect("query runs");
    let mut rows: Vec<(String, String)> = Vec::new();
    for row in &mut result {
        let r = row.clone();
        let id = if let kv::Value::String(s) = &r[0] {
            s.clone()
        } else {
            panic!("id String: {r:?}");
        };
        let lbl = if let kv::Value::String(s) = &r[2] {
            s.clone()
        } else {
            panic!("subfamily_label String: {r:?}");
        };
        rows.push((id, lbl));
    }
    assert_eq!(rows.len(), 7, "7 members: {rows:?}");
    let by_id = |t: &str| -> String {
        rows.iter()
            .find(|(id, _)| id == t)
            .unwrap_or_else(|| panic!("{t} must surface: {rows:?}"))
            .1
            .clone()
    };
    assert_eq!(by_id("r-rocchio"), "aqe-local");
    assert_eq!(by_id("r-lsi"), "aqe-global");
    assert_eq!(by_id("r-voorhees"), "aqe-external");
    assert_eq!(by_id("r-carpineto"), "vocabulary-mismatch-survey");
    assert_eq!(by_id("r-hyde"), "llm-era");
    assert_eq!(by_id("r-query2doc"), "llm-era");
    assert_eq!(by_id("r-stepback"), "llm-era");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn query_expansion_subfamily_coverage_mixed_shape() {
    // Per iter 321: 4 singletons + 1 rich (llm-era at 3).
    let dir = std::env::temp_dir().join(format!("doc-linter-saved-qe-cov-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let db = testkit::open_db(&dir);
    {
        let conn = testkit::connect(&db);
        testkit::reset(&conn);
    }
    let conn = testkit::connect(&db);
    let seed = |conn: &testkit::Conn<'_>, id: &str, tags: &str| {
        conn.query(&format!(
            "CREATE (:Doc {{id: '{id}', path: '{id}.md', \
                 role: 'doc', kind: 'reference', lifecycle: '', \
                 bounded_context: '', title: '{id}', summary: '', \
                 status: 'stable', updated: '2026-06-01', \
                 tags: {tags}, covers: []}})"
        ))
        .unwrap();
    };
    seed(
        &conn,
        "r-rocchio",
        "['research','query-expansion','aqe-local']",
    );
    seed(
        &conn,
        "r-h1",
        "['research','query-expansion','llm-era-query-rewriting']",
    );
    seed(
        &conn,
        "r-h2",
        "['research','query-expansion','llm-era-query-rewriting']",
    );
    seed(
        &conn,
        "r-h3",
        "['research','query-expansion','llm-era-query-rewriting']",
    );
    let q = SAVED_QUERIES
        .iter()
        .find(|q| q.name == "query-expansion-subfamily-coverage")
        .expect("query-expansion-subfamily-coverage must exist");
    let mut result = conn.query(sql_of(q.name)).expect("query runs");
    let mut rows: Vec<(String, i64, String)> = Vec::new();
    let as_i64 = |v: &kv::Value| -> i64 {
        match v {
            kv::Value::Int64(n) => *n,
            kv::Value::Int128(n) => *n as i64,
            other => panic!("expected int: {other:?}"),
        }
    };
    for row in &mut result {
        let r = row.clone();
        let lbl = if let kv::Value::String(s) = &r[0] {
            s.clone()
        } else {
            panic!("subfamily_label String: {r:?}");
        };
        let count = as_i64(&r[1]);
        let cov = if let kv::Value::String(s) = &r[2] {
            s.clone()
        } else {
            panic!("coverage_label String: {r:?}");
        };
        rows.push((lbl, count, cov));
    }
    let by_sub = |t: &str| -> (i64, String) {
        let r = rows
            .iter()
            .find(|(lbl, _, _)| lbl == t)
            .unwrap_or_else(|| panic!("{t} must surface: {rows:?}"));
        (r.1, r.2.clone())
    };
    let (local_count, local_cov) = by_sub("aqe-local");
    assert_eq!(local_count, 1);
    assert_eq!(local_cov, "singleton");
    let (llm_count, llm_cov) = by_sub("llm-era");
    assert_eq!(llm_count, 3);
    assert_eq!(llm_cov, "rich");
    let _ = std::fs::remove_dir_all(&dir);
}

/// Per iter 323 (mature-topic-shape-distribution +
/// mature-topic-shape-census fixtures): seeds 21 Doc
/// rows across 3 synthetic family-tags with the 3
/// distinct mature-topic shapes from user-probe-083:
/// - deep-topic-fixture: 9 docs / 3 subfamilies × 3
///   (DEEP-mature shape: rich=3, singleton=0).
/// - broad-topic-fixture: 6 docs / 6 subfamilies × 1
///   (BROAD-mature shape: rich=0, singleton=6).
/// - mixed-topic-fixture: 6 docs / 1 rich (×3) + 3
///   singletons (MIXED-mature shape: rich=1,
///   singleton=3).
fn seed_mature_topic_shape_fixture(conn: &testkit::Conn<'_>) {
    let seed = |id: &str, tags: &str| {
        conn.query(&format!(
            "CREATE (:Doc {{id: '{id}', path: '{id}.md', \
                 role: 'doc', kind: 'reference', lifecycle: '', \
                 bounded_context: '', title: '{id}', summary: '', \
                 status: 'stable', updated: '2026-06-01', \
                 tags: {tags}, covers: []}})"
        ))
        .unwrap();
    };
    for (id, sub) in [
        ("d1", "subfam-alpha"),
        ("d2", "subfam-alpha"),
        ("d3", "subfam-alpha"),
        ("d4", "subfam-beta-x"),
        ("d5", "subfam-beta-x"),
        ("d6", "subfam-beta-x"),
        ("d7", "subfam-gamma"),
        ("d8", "subfam-gamma"),
        ("d9", "subfam-gamma"),
    ] {
        seed(id, &format!("['research','deep-topic-fixture','{sub}']"));
    }
    for (id, sub) in [
        ("b1", "subfam-alpha"),
        ("b2", "subfam-beta-x"),
        ("b3", "subfam-gamma"),
        ("b4", "subfam-delta"),
        ("b5", "subfam-epsilon"),
        ("b6", "subfam-zeta-y"),
    ] {
        seed(id, &format!("['research','broad-topic-fixture','{sub}']"));
    }
    for (id, sub) in [
        ("m1", "subfam-alpha"),
        ("m2", "subfam-alpha"),
        ("m3", "subfam-alpha"),
        ("m4", "subfam-beta-x"),
        ("m5", "subfam-gamma"),
        ("m6", "subfam-delta"),
    ] {
        seed(id, &format!("['research','mixed-topic-fixture','{sub}']"));
    }
}

#[test]
fn mature_topic_shape_distribution_classifies_3_shapes() {
    // Per iter 323: parameterized $family_tag query
    // classifies ANY family-tag's co-tag distribution
    // generically — replaces per-topic CASE-WHEN
    // siblings (pipeline-stage-coverage / graph-
    // similarity-subfamily-coverage / query-
    // expansion-subfamily-coverage).
    let dir = std::env::temp_dir().join(format!(
        "doc-linter-saved-shape-dist-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let db = testkit::open_db(&dir);
    {
        let conn = testkit::connect(&db);
        testkit::reset(&conn);
    }
    let conn = testkit::connect(&db);
    seed_mature_topic_shape_fixture(&conn);

    let q = SAVED_QUERIES
        .iter()
        .find(|q| q.name == "mature-topic-shape-distribution")
        .expect("mature-topic-shape-distribution must exist");
    let as_i64 = |v: &kv::Value| -> i64 {
        match v {
            kv::Value::Int64(n) => *n,
            kv::Value::Int128(n) => *n as i64,
            other => panic!("expected int: {other:?}"),
        }
    };
    let run_for = |family: &str| -> Vec<(String, i64, String)> {
        let cypher = sql_of(q.name).replace("$family_tag", &format!("'{family}'"));
        let mut result = conn.query(&cypher).expect("query runs");
        let mut rows: Vec<(String, i64, String)> = Vec::new();
        for row in &mut result {
            let r = row.clone();
            let tag = if let kv::Value::String(s) = &r[0] {
                s.clone()
            } else {
                panic!("co_tag String: {r:?}");
            };
            let count = as_i64(&r[1]);
            let cov = if let kv::Value::String(s) = &r[2] {
                s.clone()
            } else {
                panic!("coverage_label String: {r:?}");
            };
            rows.push((tag, count, cov));
        }
        rows
    };

    let deep = run_for("deep-topic-fixture");
    assert_eq!(
        deep.len(),
        3,
        "DEEP fixture must surface 3 co-tags: {deep:?}"
    );
    for (_, c, cov) in &deep {
        assert_eq!(*c, 3);
        assert_eq!(cov, "rich");
    }

    let broad = run_for("broad-topic-fixture");
    assert_eq!(
        broad.len(),
        6,
        "BROAD fixture must surface 6 co-tags: {broad:?}"
    );
    for (_, c, cov) in &broad {
        assert_eq!(*c, 1);
        assert_eq!(cov, "singleton");
    }

    let mixed = run_for("mixed-topic-fixture");
    assert_eq!(
        mixed.len(),
        4,
        "MIXED fixture must surface 4 co-tags: {mixed:?}"
    );
    let by_tag = |t: &str| -> (i64, String) {
        let r = mixed
            .iter()
            .find(|(tag, _, _)| tag == t)
            .unwrap_or_else(|| panic!("{t} must surface: {mixed:?}"));
        (r.1, r.2.clone())
    };
    assert_eq!(by_tag("subfam-alpha"), (3, "rich".to_string()));
    assert_eq!(by_tag("subfam-beta-x"), (1, "singleton".to_string()));
    assert_eq!(by_tag("subfam-gamma"), (1, "singleton".to_string()));
    assert_eq!(by_tag("subfam-delta"), (1, "singleton".to_string()));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn mature_topic_shape_census_emits_3_shape_labels() {
    // Per iter 323: corpus-wide no-param census
    // returns one row per family-tag with ≥5 research
    // members + shape_label (DEEP/BROAD/MIXED/
    // emergent). The 3 fixture families must surface
    // with the 3 distinct shape_labels.
    let dir = std::env::temp_dir().join(format!(
        "doc-linter-saved-shape-census-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let db = testkit::open_db(&dir);
    {
        let conn = testkit::connect(&db);
        testkit::reset(&conn);
    }
    let conn = testkit::connect(&db);
    seed_mature_topic_shape_fixture(&conn);

    let q = SAVED_QUERIES
        .iter()
        .find(|q| q.name == "mature-topic-shape-census")
        .expect("mature-topic-shape-census must exist");
    let mut result = conn.query(sql_of(q.name)).expect("query runs");
    let as_i64 = |v: &kv::Value| -> i64 {
        match v {
            kv::Value::Int64(n) => *n,
            kv::Value::Int128(n) => *n as i64,
            other => panic!("expected int: {other:?}"),
        }
    };
    let mut rows: Vec<(String, i64, i64, i64, i64, String)> = Vec::new();
    for row in &mut result {
        let r = row.clone();
        let family = if let kv::Value::String(s) = &r[0] {
            s.clone()
        } else {
            panic!("family_tag String: {r:?}");
        };
        let total = as_i64(&r[1]);
        let rich = as_i64(&r[2]);
        let couple = as_i64(&r[3]);
        let singleton = as_i64(&r[4]);
        let shape = if let kv::Value::String(s) = &r[5] {
            s.clone()
        } else {
            panic!("shape_label String: {r:?}");
        };
        rows.push((family, total, rich, couple, singleton, shape));
    }
    let by_family = |f: &str| -> (i64, i64, i64, i64, String) {
        let r = rows
            .iter()
            .find(|(family, _, _, _, _, _)| family == f)
            .unwrap_or_else(|| panic!("{f} must surface: {rows:?}"));
        (r.1, r.2, r.3, r.4, r.5.clone())
    };
    let (deep_total, deep_rich, _, deep_singleton, deep_shape) = by_family("deep-topic-fixture");
    assert_eq!(deep_total, 9);
    assert_eq!(deep_rich, 3);
    assert_eq!(deep_singleton, 0);
    assert_eq!(deep_shape, "DEEP-mature");

    let (broad_total, broad_rich, _, broad_singleton, broad_shape) =
        by_family("broad-topic-fixture");
    assert_eq!(broad_total, 6);
    assert_eq!(broad_rich, 0);
    assert_eq!(broad_singleton, 6);
    assert_eq!(broad_shape, "BROAD-mature");

    let (mixed_total, mixed_rich, _, mixed_singleton, mixed_shape) =
        by_family("mixed-topic-fixture");
    assert_eq!(mixed_total, 6);
    assert_eq!(mixed_rich, 1);
    assert_eq!(mixed_singleton, 3);
    assert_eq!(mixed_shape, "MIXED-mature");
    let _ = std::fs::remove_dir_all(&dir);
}

/// Per iter 326 (cross-family-bridge-papers + cross-
/// family-bridge-density fixtures): seeds 7 Doc rows
/// across 3 family-tags with intersecting
/// memberships: 'family-alpha-7c' (4 members),
/// 'family-beta-7c' (4 members), 'family-gamma-7c'
/// (3 members). Bridges: alpha∩beta=3 (cross-bridge),
/// alpha∩gamma=2 (sub-threshold), beta∩gamma=2
/// (sub-threshold). Only the alpha-beta pair passes
/// bridge_count>=3 cutoff.
fn seed_cross_family_bridge_fixture(conn: &testkit::Conn<'_>) {
    let seed = |id: &str, tags: &str| {
        conn.query(&format!(
            "CREATE (:Doc {{id: '{id}', path: '{id}.md', \
                 role: 'doc', kind: 'reference', lifecycle: '', \
                 bounded_context: '', title: '{id}', summary: '', \
                 status: 'stable', updated: '2026-06-01', \
                 tags: {tags}, covers: []}})"
        ))
        .unwrap();
    };
    // alpha-only members
    seed("r-alpha-only-1", "['research','family-alpha-7c']");
    // beta-only members
    seed("r-beta-only-1", "['research','family-beta-7c']");
    // gamma-only members
    seed("r-gamma-only-1", "['research','family-gamma-7c']");
    // alpha∩beta bridges (3 → meets threshold)
    for id in ["r-ab1", "r-ab2", "r-ab3"] {
        seed(id, "['research','family-alpha-7c','family-beta-7c']");
    }
    // alpha∩gamma bridge (1 → sub-threshold)
    seed("r-ag1", "['research','family-alpha-7c','family-gamma-7c']");
    // beta∩gamma bridge (1 → sub-threshold)
    seed("r-bg1", "['research','family-beta-7c','family-gamma-7c']");
    // Result: alpha=5 (1+3+1=5), beta=5 (1+3+1=5),
    // gamma=3 (1+1+1=3); only alpha-beta pair has
    // bridge_count=3.
}

#[test]
fn cross_family_bridge_papers_returns_intersection() {
    // Per iter 326: parameterized intersection of 2
    // family tags; returns the 3 alpha∩beta bridge
    // docs.
    let dir = std::env::temp_dir().join(format!(
        "doc-linter-saved-bridge-papers-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let db = testkit::open_db(&dir);
    {
        let conn = testkit::connect(&db);
        testkit::reset(&conn);
    }
    let conn = testkit::connect(&db);
    seed_cross_family_bridge_fixture(&conn);

    let q = SAVED_QUERIES
        .iter()
        .find(|q| q.name == "cross-family-bridge-papers")
        .expect("cross-family-bridge-papers must exist");
    let cypher = sql_of(q.name)
        .replace("$family_a", "'family-alpha-7c'")
        .replace("$family_b", "'family-beta-7c'");
    let mut result = conn.query(&cypher).expect("query runs");
    let mut ids: Vec<String> = Vec::new();
    for row in &mut result {
        let r = row.clone();
        if let kv::Value::String(s) = &r[0] {
            ids.push(s.clone());
        }
    }
    assert_eq!(
        ids,
        vec![
            "r-ab1".to_string(),
            "r-ab2".to_string(),
            "r-ab3".to_string()
        ],
        "alpha∩beta intersection must be 3 bridge docs: {ids:?}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn cross_family_bridge_density_emits_above_threshold_pairs_only() {
    // Per iter 326: no-param corpus-wide bridge
    // density; only alpha-beta pair (bridge=3) must
    // surface. alpha-gamma + beta-gamma (bridge=1
    // each) below threshold.
    let dir = std::env::temp_dir().join(format!(
        "doc-linter-saved-bridge-density-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let db = testkit::open_db(&dir);
    {
        let conn = testkit::connect(&db);
        testkit::reset(&conn);
    }
    let conn = testkit::connect(&db);
    seed_cross_family_bridge_fixture(&conn);

    let q = SAVED_QUERIES
        .iter()
        .find(|q| q.name == "cross-family-bridge-density")
        .expect("cross-family-bridge-density must exist");
    let mut result = conn.query(sql_of(q.name)).expect("query runs");
    let as_i64 = |v: &kv::Value| -> i64 {
        match v {
            kv::Value::Int64(n) => *n,
            kv::Value::Int128(n) => *n as i64,
            other => panic!("expected int: {other:?}"),
        }
    };
    let mut rows: Vec<(String, String, i64)> = Vec::new();
    for row in &mut result {
        let r = row.clone();
        let a = if let kv::Value::String(s) = &r[0] {
            s.clone()
        } else {
            panic!("family_a String: {r:?}");
        };
        let b = if let kv::Value::String(s) = &r[1] {
            s.clone()
        } else {
            panic!("family_b String: {r:?}");
        };
        let cnt = as_i64(&r[2]);
        rows.push((a, b, cnt));
    }
    assert_eq!(
        rows.len(),
        1,
        "only alpha-beta pair (bridge=3) must surface: {rows:?}"
    );
    assert_eq!(rows[0].0, "family-alpha-7c");
    assert_eq!(rows[0].1, "family-beta-7c");
    assert_eq!(rows[0].2, 3);
    let _ = std::fs::remove_dir_all(&dir);
}

/// Per iter 328 (shared-layer-source-files + resource-
/// file-cluster fixtures): seeds 9 File rows
/// simulating a FastAPI-template-like layout.
/// Includes per-resource files (items.py route +
/// test_items.py + items.tsx + items.spec.ts +
/// users.py route) AND shared-layer files (models.py
/// + crud.py + main.py + __init__.py + layout.tsx).
fn seed_resource_recipe_fixture(conn: &testkit::Conn<'_>) {
    let seed = |path: &str, lang: &str, loc: u32| {
        conn.query(&format!(
            "CREATE (:File {{path: '{path}', language: '{lang}', \
                 loc: {loc}, last_touched: '2026-06-01'}})"
        ))
        .unwrap();
    };
    // Per-resource clone-targets
    seed("backend/app/api/routes/items.py", "python", 113);
    seed("backend/app/api/routes/users.py", "python", 100);
    seed("backend/tests/api/routes/test_items.py", "python", 164);
    seed("frontend/src/routes/_layout/items.tsx", "tsx", 69);
    seed("frontend/tests/items.spec.ts", "typescript", 132);
    // Shared-layer edit-targets
    seed("backend/app/models.py", "python", 129);
    seed("backend/app/crud.py", "python", 68);
    seed("backend/app/main.py", "python", 33);
    seed("backend/app/__init__.py", "python", 0);
}

#[test]
fn shared_layer_source_files_classifies_edit_targets() {
    // Per iter 328: shared-layer files (models.py +
    // crud.py + main.py + __init__.py) must surface
    // with correct layer_role; per-resource files
    // (items.py + users.py) must NOT (they're CLONE
    // targets per resource-file-cluster).
    let dir = std::env::temp_dir().join(format!(
        "doc-linter-saved-shared-layer-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let db = testkit::open_db(&dir);
    {
        let conn = testkit::connect(&db);
        testkit::reset(&conn);
    }
    let conn = testkit::connect(&db);
    seed_resource_recipe_fixture(&conn);

    let q = SAVED_QUERIES
        .iter()
        .find(|q| q.name == "shared-layer-source-files")
        .expect("shared-layer-source-files must exist");
    let mut result = conn.query(sql_of(q.name)).expect("query runs");
    let mut rows: Vec<(String, String)> = Vec::new();
    for row in &mut result {
        let r = row.clone();
        let path = if let kv::Value::String(s) = &r[0] {
            s.clone()
        } else {
            panic!("path: {r:?}");
        };
        let role = if let kv::Value::String(s) = &r[3] {
            s.clone()
        } else {
            panic!("layer_role: {r:?}");
        };
        rows.push((path, role));
    }
    let by_path = |p: &str| -> String {
        rows.iter()
            .find(|(path, _)| path == p)
            .unwrap_or_else(|| panic!("{p} must surface: {rows:?}"))
            .1
            .clone()
    };
    assert_eq!(by_path("backend/app/models.py"), "models-shared");
    assert_eq!(by_path("backend/app/crud.py"), "crud-shared");
    assert_eq!(by_path("backend/app/main.py"), "app-entry");
    assert_eq!(by_path("backend/app/__init__.py"), "python-package-init");
    for noisy in [
        "backend/app/api/routes/items.py",
        "backend/app/api/routes/users.py",
        "backend/tests/api/routes/test_items.py",
    ] {
        assert!(
            !rows.iter().any(|(path, _)| path == noisy),
            "per-resource/test file `{noisy}` must NOT surface: \
                 {rows:?}",
        );
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn resource_file_cluster_returns_per_resource_files() {
    // Per iter 328: with $resource_stem='items',
    // returns the 4 items-related files (route +
    // test + frontend-route + frontend-spec)
    // classified by layer; must NOT include users.py
    // or shared-layer files.
    let dir = std::env::temp_dir().join(format!(
        "doc-linter-saved-resource-cluster-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let db = testkit::open_db(&dir);
    {
        let conn = testkit::connect(&db);
        testkit::reset(&conn);
    }
    let conn = testkit::connect(&db);
    seed_resource_recipe_fixture(&conn);

    let q = SAVED_QUERIES
        .iter()
        .find(|q| q.name == "resource-file-cluster")
        .expect("resource-file-cluster must exist");
    let cypher = sql_of(q.name).replace("$resource_stem", "'items'");
    let mut result = conn.query(&cypher).expect("query runs");
    let mut rows: Vec<(String, String)> = Vec::new();
    for row in &mut result {
        let r = row.clone();
        let path = if let kv::Value::String(s) = &r[0] {
            s.clone()
        } else {
            panic!("path: {r:?}");
        };
        let layer = if let kv::Value::String(s) = &r[3] {
            s.clone()
        } else {
            panic!("layer: {r:?}");
        };
        rows.push((path, layer));
    }
    assert_eq!(rows.len(), 4, "must surface 4 items files: {rows:?}");
    let by_path = |p: &str| -> String {
        rows.iter()
            .find(|(path, _)| path == p)
            .unwrap_or_else(|| panic!("{p} must surface: {rows:?}"))
            .1
            .clone()
    };
    assert_eq!(by_path("backend/app/api/routes/items.py"), "route");
    assert_eq!(by_path("backend/tests/api/routes/test_items.py"), "test");
    assert_eq!(by_path("frontend/src/routes/_layout/items.tsx"), "route");
    assert_eq!(by_path("frontend/tests/items.spec.ts"), "test");
    for noisy in [
        "backend/app/api/routes/users.py",
        "backend/app/models.py",
        "backend/app/crud.py",
    ] {
        assert!(
            !rows.iter().any(|(path, _)| path == noisy),
            "non-items file `{noisy}` must NOT surface: {rows:?}",
        );
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// Per iter 330 (resource-shape-classifier + app-
/// route-files fixtures): seeds 11 File rows
/// covering the 3 asymmetric-resource sub-shapes
/// (symmetric items + backend-only users + frontend-
/// only admin) plus scaffold-noise to filter out.
fn seed_resource_shape_fixture(conn: &testkit::Conn<'_>) {
    let seed = |path: &str, lang: &str, loc: u32| {
        conn.query(&format!(
            "CREATE (:File {{path: '{path}', language: '{lang}', \
                 loc: {loc}, last_touched: '2026-06-01'}})"
        ))
        .unwrap();
    };
    // items: symmetric (backend route + test +
    // frontend route + spec)
    seed("backend/app/api/routes/items.py", "python", 113);
    seed("backend/tests/api/routes/test_items.py", "python", 164);
    seed("frontend/src/routes/_layout/items.tsx", "tsx", 69);
    seed("frontend/tests/items.spec.ts", "typescript", 132);
    // users: backend-only (route + test, no frontend)
    seed("backend/app/api/routes/users.py", "python", 232);
    seed("backend/tests/api/routes/test_users.py", "python", 521);
    // admin: frontend-only (route + spec, no backend)
    seed("frontend/src/routes/_layout/admin.tsx", "tsx", 73);
    seed("frontend/tests/admin.spec.ts", "typescript", 205);
    // Scaffold-noise (should be filtered)
    seed("backend/app/api/routes/__init__.py", "python", 0);
    seed("frontend/src/routes/_layout.tsx", "tsx", 42);
    seed("frontend/src/routes/__root.tsx", "tsx", 18);
}

#[test]
fn resource_shape_classifier_emits_3_asymmetric_shapes() {
    let dir = std::env::temp_dir().join(format!(
        "doc-linter-saved-resource-shape-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let db = testkit::open_db(&dir);
    {
        let conn = testkit::connect(&db);
        testkit::reset(&conn);
    }
    let conn = testkit::connect(&db);
    seed_resource_shape_fixture(&conn);

    let q = SAVED_QUERIES
        .iter()
        .find(|q| q.name == "resource-shape-classifier")
        .expect("resource-shape-classifier must exist");
    let as_i64 = |v: &kv::Value| -> i64 {
        match v {
            kv::Value::Int64(n) => *n,
            kv::Value::Int128(n) => *n as i64,
            other => panic!("expected int: {other:?}"),
        }
    };
    let run_for = |stem: &str| -> (i64, i64, i64, i64, String) {
        let cypher = sql_of(q.name).replace("$resource_stem", &format!("'{stem}'"));
        let mut result = conn.query(&cypher).expect("query runs");
        let row = result.next().expect("one row");
        let label = if let kv::Value::String(s) = &row[4] {
            s.clone()
        } else {
            panic!("shape_label String: {row:?}");
        };
        (
            as_i64(&row[0]),
            as_i64(&row[1]),
            as_i64(&row[2]),
            as_i64(&row[3]),
            label,
        )
    };

    let (b_src, b_test, f_src, f_test, shape) = run_for("items");
    assert_eq!(
        (b_src, b_test, f_src, f_test, shape.as_str()),
        (1, 1, 1, 1, "symmetric"),
        "items must be symmetric"
    );
    let (b_src, b_test, f_src, f_test, shape) = run_for("users");
    assert_eq!(
        (b_src, b_test, f_src, f_test, shape.as_str()),
        (1, 1, 0, 0, "backend-only"),
        "users must be backend-only"
    );
    let (b_src, b_test, f_src, f_test, shape) = run_for("admin");
    assert_eq!(
        (b_src, b_test, f_src, f_test, shape.as_str()),
        (0, 0, 1, 1, "frontend-only"),
        "admin must be frontend-only"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn app_route_files_excludes_scaffold() {
    let dir = std::env::temp_dir().join(format!(
        "doc-linter-saved-app-routes-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let db = testkit::open_db(&dir);
    {
        let conn = testkit::connect(&db);
        testkit::reset(&conn);
    }
    let conn = testkit::connect(&db);
    seed_resource_shape_fixture(&conn);

    let q = SAVED_QUERIES
        .iter()
        .find(|q| q.name == "app-route-files")
        .expect("app-route-files must exist");
    let mut result = conn.query(sql_of(q.name)).expect("query runs");
    let mut rows: Vec<(String, String)> = Vec::new();
    for row in &mut result {
        let r = row.clone();
        let path = if let kv::Value::String(s) = &r[0] {
            s.clone()
        } else {
            panic!("path: {r:?}");
        };
        let layer = if let kv::Value::String(s) = &r[2] {
            s.clone()
        } else {
            panic!("layer: {r:?}");
        };
        rows.push((path, layer));
    }
    // Expected: 2 backend routes (items + users) +
    // 2 frontend routes (admin + items).
    // Test files / __init__ / _layout.tsx / __root.tsx
    // must be excluded.
    for must_have in [
        ("backend/app/api/routes/items.py", "backend-route"),
        ("backend/app/api/routes/users.py", "backend-route"),
        ("frontend/src/routes/_layout/admin.tsx", "frontend-route"),
        ("frontend/src/routes/_layout/items.tsx", "frontend-route"),
    ] {
        assert!(
            rows.contains(&(must_have.0.to_string(), must_have.1.to_string())),
            "{} ({}) must surface: {rows:?}",
            must_have.0,
            must_have.1
        );
    }
    for noisy in [
        "backend/tests/api/routes/test_items.py",
        "backend/tests/api/routes/test_users.py",
        "frontend/tests/items.spec.ts",
        "frontend/tests/admin.spec.ts",
        "backend/app/api/routes/__init__.py",
        "frontend/src/routes/_layout.tsx",
        "frontend/src/routes/__root.tsx",
    ] {
        assert!(
            !rows.iter().any(|(p, _)| p == noisy),
            "scaffold/test file `{noisy}` must NOT surface: \
                 {rows:?}",
        );
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// Per iter 332 (family-top-bridges + family-rich-
/// cotags fixtures): seeds 6 Doc rows on
/// 'introspective-fixture' family + 3 bridges
/// to (production, agent-shell, code-context-engine
/// per iter-325 archetype). Plus 4 Doc rows on
/// 'companion-fixture' family with 3 bridges to
/// 'introspective-fixture' to test bridge-axis.
fn seed_family_per_axis_fixture(conn: &testkit::Conn<'_>) {
    let seed = |id: &str, tags: &str| {
        conn.query(&format!(
            "CREATE (:Doc {{id: '{id}', path: '{id}.md', \
                 role: 'doc', kind: 'reference', lifecycle: '', \
                 bounded_context: '', title: '{id}', summary: '', \
                 status: 'stable', updated: '2026-06-01', \
                 tags: {tags}, covers: []}})"
        ))
        .unwrap();
    };
    // 6 docs in introspective-fixture with rich
    // co-tags simulating workflow-companion archetype.
    // rich-topic-tag-7c=4, agent-shell=3, code-context=3, singleton=1
    // (uses non-flavour 'rich-topic-tag-7c' since
    // 'production' is filtered as flavour per iter-334
    // [[feedback_topical_vs_flavour_tag]])
    seed(
            "if-d1",
            "['research','introspective-fixture','rich-topic-tag-7c','agent-shell','code-context-engine','companion-fixture']",
        );
    seed(
            "if-d2",
            "['research','introspective-fixture','rich-topic-tag-7c','agent-shell','code-context-engine','companion-fixture']",
        );
    seed(
            "if-d3",
            "['research','introspective-fixture','rich-topic-tag-7c','agent-shell','code-context-engine','companion-fixture']",
        );
    seed(
        "if-d4",
        "['research','introspective-fixture','rich-topic-tag-7c','singleton-x']",
    );
    seed(
        "if-d5",
        "['research','introspective-fixture','llm-era-rewriting']",
    );
    seed(
        "if-d6",
        "['research','introspective-fixture','isolated-tag']",
    );
    // 4 docs in companion-fixture (3 bridge to
    // introspective-fixture from above + 1 own).
    seed("cf-d1", "['research','companion-fixture','other-feature']");
}

#[test]
fn family_top_bridges_returns_cross_family_neighbors() {
    // Per iter 332: introspective-fixture bridges to
    // companion-fixture via 3 shared docs (if-d1/d2/d3).
    // production also bridges via 4 docs (rich cluster).
    let dir = std::env::temp_dir().join(format!(
        "doc-linter-saved-fam-bridges-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let db = testkit::open_db(&dir);
    {
        let conn = testkit::connect(&db);
        testkit::reset(&conn);
    }
    let conn = testkit::connect(&db);
    seed_family_per_axis_fixture(&conn);

    let q = SAVED_QUERIES
        .iter()
        .find(|q| q.name == "family-top-bridges")
        .expect("family-top-bridges must exist");
    let cypher = sql_of(q.name).replace("$family_tag", "'introspective-fixture'");
    let mut result = conn.query(&cypher).expect("query runs");
    let as_i64 = |v: &kv::Value| -> i64 {
        match v {
            kv::Value::Int64(n) => *n,
            kv::Value::Int128(n) => *n as i64,
            other => panic!("expected int: {other:?}"),
        }
    };
    let mut rows: Vec<(String, i64)> = Vec::new();
    for row in &mut result {
        let r = row.clone();
        let tag = if let kv::Value::String(s) = &r[0] {
            s.clone()
        } else {
            panic!("other_family: {r:?}");
        };
        rows.push((tag, as_i64(&r[1])));
    }
    let by_tag = |t: &str| -> i64 {
        rows.iter()
            .find(|(tag, _)| tag == t)
            .unwrap_or_else(|| panic!("{t} must surface: {rows:?}"))
            .1
    };
    assert_eq!(by_tag("rich-topic-tag-7c"), 4);
    assert_eq!(by_tag("agent-shell"), 3);
    assert_eq!(by_tag("code-context-engine"), 3);
    assert_eq!(by_tag("companion-fixture"), 3);
    // isolated-tag (bridge=1) below threshold; must NOT surface
    assert!(
        !rows.iter().any(|(t, _)| t == "isolated-tag"),
        "isolated-tag (bridge=1) must NOT surface: {rows:?}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn family_rich_cotags_returns_archetype_cluster_only() {
    // Per iter 332: introspective-fixture's rich cluster:
    // production (4) + agent-shell (3) + code-context (3) +
    // companion-fixture (3). NOT singleton-x (1), llm-era (1),
    // isolated-tag (1).
    let dir =
        std::env::temp_dir().join(format!("doc-linter-saved-fam-rich-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let db = testkit::open_db(&dir);
    {
        let conn = testkit::connect(&db);
        testkit::reset(&conn);
    }
    let conn = testkit::connect(&db);
    seed_family_per_axis_fixture(&conn);

    let q = SAVED_QUERIES
        .iter()
        .find(|q| q.name == "family-rich-cotags")
        .expect("family-rich-cotags must exist");
    let cypher = sql_of(q.name).replace("$family_tag", "'introspective-fixture'");
    let mut result = conn.query(&cypher).expect("query runs");
    let as_i64 = |v: &kv::Value| -> i64 {
        match v {
            kv::Value::Int64(n) => *n,
            kv::Value::Int128(n) => *n as i64,
            other => panic!("expected int: {other:?}"),
        }
    };
    let mut rows: Vec<(String, i64, String)> = Vec::new();
    for row in &mut result {
        let r = row.clone();
        let tag = if let kv::Value::String(s) = &r[0] {
            s.clone()
        } else {
            panic!("co_tag: {r:?}");
        };
        let label = if let kv::Value::String(s) = &r[2] {
            s.clone()
        } else {
            panic!("coverage_label: {r:?}");
        };
        rows.push((tag, as_i64(&r[1]), label));
    }
    // 4 rich co-tags expected: rich-topic-tag-7c / agent-shell /
    // code-context-engine / companion-fixture
    assert_eq!(rows.len(), 4, "must surface 4 rich co-tags: {rows:?}");
    for (_, count, label) in &rows {
        assert!(*count >= 3);
        assert_eq!(label, "rich");
    }
    for noisy in ["singleton-x", "llm-era-rewriting", "isolated-tag"] {
        assert!(
            !rows.iter().any(|(t, _, _)| t == noisy),
            "singleton co_tag `{noisy}` must NOT surface: {rows:?}"
        );
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn family_top_bridges_excludes_flavour_tags() {
    // Per iter 334: flavour tags ('production', 'tool',
    // 'doc') must be filtered out of family-axis
    // aggregation per iter-333 [[interrogation-070]]
    // Finding C. Otherwise they appear as inflated
    // cross-family bridges (15 'production' tagged docs
    // were misclassified as MIXED-mature family in
    // iter-324).
    let dir = std::env::temp_dir().join(format!(
        "doc-linter-saved-flavour-filter-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let db = testkit::open_db(&dir);
    {
        let conn = testkit::connect(&db);
        testkit::reset(&conn);
    }
    let conn = testkit::connect(&db);
    let seed = |id: &str, tags: &str| {
        conn.query(&format!(
            "CREATE (:Doc {{id: '{id}', path: '{id}.md', \
                 role: 'doc', kind: 'reference', lifecycle: '', \
                 bounded_context: '', title: '{id}', summary: '', \
                 status: 'stable', updated: '2026-06-01', \
                 tags: {tags}, covers: []}})"
        ))
        .unwrap();
    };
    // 4 docs in 'family-fixture-7c' with 'production'
    // (flavour) + 'real-topic-7c' (topical) co-tags.
    seed(
        "ft-d1",
        "['research','family-fixture-7c','production','tool','doc','real-topic-7c']",
    );
    seed(
        "ft-d2",
        "['research','family-fixture-7c','production','tool','doc','real-topic-7c']",
    );
    seed(
        "ft-d3",
        "['research','family-fixture-7c','production','tool','doc','real-topic-7c']",
    );
    seed(
        "ft-d4",
        "['research','family-fixture-7c','production','tool','real-topic-7c']",
    );

    let q = SAVED_QUERIES
        .iter()
        .find(|q| q.name == "family-top-bridges")
        .expect("family-top-bridges must exist");
    let cypher = sql_of(q.name).replace("$family_tag", "'family-fixture-7c'");
    let mut result = conn.query(&cypher).expect("query runs");
    let as_i64 = |v: &kv::Value| -> i64 {
        match v {
            kv::Value::Int64(n) => *n,
            kv::Value::Int128(n) => *n as i64,
            other => panic!("expected int: {other:?}"),
        }
    };
    let mut rows: Vec<(String, i64)> = Vec::new();
    for row in &mut result {
        let r = row.clone();
        let tag = if let kv::Value::String(s) = &r[0] {
            s.clone()
        } else {
            panic!("other_family: {r:?}");
        };
        rows.push((tag, as_i64(&r[1])));
    }
    // Only the topical co-tag must surface; flavour tags
    // (production/tool/doc) must be filtered out.
    assert_eq!(rows.len(), 1, "only topical bridge must surface: {rows:?}");
    assert_eq!(rows[0].0, "real-topic-7c");
    for flavour in ["production", "tool", "doc"] {
        assert!(
            !rows.iter().any(|(t, _)| t == flavour),
            "flavour tag `{flavour}` must NOT surface: {rows:?}",
        );
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// Per iter 201 (user-probe-048 Finding A — scaffold-
/// coupling-noise pattern): seeds 5 File rows with 2
/// distinct coupling clusters — a scaffold-pattern clique
/// (3 files, every pair at commits=3 jaccard=1.0,
/// representing template setup) and a feature-evolution
/// pair (2 files at commits=12 jaccard=0.6, representing
/// real co-evolution). The feature-evolution-coupling
/// queries MUST surface only the feature pair; the
/// scaffold clique MUST be filtered out.
fn seed_feature_vs_scaffold_coupling_fixture(conn: &testkit::Conn<'_>) {
    for (path, language, loc) in [
        ("scaffold-a.tsx", "tsx", 100u32),
        ("scaffold-b.tsx", "tsx", 100u32),
        ("scaffold-c.tsx", "tsx", 100u32),
        ("feature-x.py", "python", 200u32),
        ("feature-y.py", "python", 200u32),
    ] {
        let cypher = format!(
            "CREATE (:File {{path: '{path}', language: '{language}', \
                 loc: {loc}, last_touched: '2026-06-01'}})"
        );
        conn.query(&cypher).expect("seed File insert");
    }
    // Scaffold clique: 3 files, every pair at commits=3
    // jaccard=1.0 (the user-probe-048 trusted pattern).
    for (a, b) in [
        ("scaffold-a.tsx", "scaffold-b.tsx"),
        ("scaffold-a.tsx", "scaffold-c.tsx"),
        ("scaffold-b.tsx", "scaffold-c.tsx"),
    ] {
        let cypher = format!(
            "MATCH (a:File {{path: '{a}'}}), (b:File {{path: '{b}'}}) \
                 CREATE (a)-[:COUPLED_WITH {{commits: 3, jaccard: 1.0, \
                 last_co_change_at: '2026-02-09T00:00:00Z'}}]->(b)"
        );
        conn.query(&cypher).expect("seed COUPLED_WITH scaffold");
    }
    // Feature-evolution pair: commits=12 jaccard=0.6 (real
    // co-evolution).
    let cypher = "MATCH (a:File {path: 'feature-x.py'}), \
                            (b:File {path: 'feature-y.py'}) \
                      CREATE (a)-[:COUPLED_WITH {commits: 12, \
                      jaccard: 0.6, last_co_change_at: \
                      '2026-05-15T00:00:00Z'}]->(b)";
    conn.query(cypher).expect("seed COUPLED_WITH feature");
}

#[test]
fn entity_state_grid_classifies_5_clusters() {
    // Per iter 361: 5 fixture entities pin all 5
    // named cluster classes. Per iter-360
    // user-probe-096 taxonomy.
    let dir = std::env::temp_dir().join(format!("doc-linter-saved-esg-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let db = testkit::open_db(&dir);
    {
        let conn = testkit::connect(&db);
        testkit::reset(&conn);
    }
    let conn = testkit::connect(&db);
    let seed_entity = |id: &str| {
        conn.query(&format!(
            "CREATE (:Entity {{id: '{id}', display: '{id}', \
                 description: '', synonyms: [], bounded_contexts: [], \
                 scanner_coverage: [], source_modules: [], \
                 mention_count: 0, is_god_node: false, \
                 entity_class: '', repo_id: 'doc-linter', \
                 attributes: []}})"
        ))
        .unwrap();
    };
    let seed_fn = |sym: &str| {
        conn.query(&format!(
            "CREATE (:Function {{symbol: '{sym}', crate: 'c', \
                 file: 'src/x.rs', line: 1, doc_comment: '', \
                 language: 'rust', signature: '', body_excerpt: '', \
                 last_touched: '2026-06-01', repo_id: 'doc-linter'}})"
        ))
        .unwrap();
    };
    let seed_type = |sym: &str| {
        conn.query(&format!(
            "CREATE (:Type {{symbol: '{sym}', kind: 'struct', \
                 crate: 'c', file: 'src/x.rs', line: 1, \
                 doc_comment: '', language: 'rust', signature: '', \
                 body_excerpt: '', last_touched: '2026-06-01'}})"
        ))
        .unwrap();
    };
    // Helper to create a (fn, mention, entity) edge
    let mention = |conn: &testkit::Conn<'_>, fn_sym: &str, eid: &str| {
        conn.query(&format!(
            "MATCH (fn:Function {{symbol: '{fn_sym}'}}), \
                       (e:Entity {{id: '{eid}'}}) \
                 CREATE (fn)-[:FUNCTION_MENTIONS {{confidence: 'high'}}]->(e)"
        ))
        .unwrap();
    };
    let belongs = |conn: &testkit::Conn<'_>, fn_sym: &str, eid: &str| {
        conn.query(&format!(
            "MATCH (fn:Function {{symbol: '{fn_sym}'}}), \
                       (e:Entity {{id: '{eid}'}}) \
                 CREATE (fn)-[:FUNCTION_BELONGS_TO]->(e)"
        ))
        .unwrap();
    };
    let type_mention = |conn: &testkit::Conn<'_>, t_sym: &str, eid: &str| {
        conn.query(&format!(
            "MATCH (t:Type {{symbol: '{t_sym}'}}), \
                       (e:Entity {{id: '{eid}'}}) \
                 CREATE (t)-[:TYPE_MENTIONS {{confidence: 'high'}}]->(e)"
        ))
        .unwrap();
    };

    // 1. implemented-infrastructure: type-heavy +
    //    implementation-heavy. fn_m=2, t_m=2 (ratio 1
    //    = type-heavy), belongs=6 (ratio 6/2=3 = impl)
    seed_entity("impl_infra");
    for i in 0..2 {
        let s = format!("ii_fm_{i}");
        seed_fn(&s);
        mention(&conn, &s, "impl_infra");
    }
    for i in 0..2 {
        let s = format!("ii_tm_{i}");
        seed_type(&s);
        type_mention(&conn, &s, "impl_infra");
    }
    for i in 0..6 {
        let s = format!("ii_b_{i}");
        seed_fn(&s);
        belongs(&conn, &s, "impl_infra");
    }
    // 2. discussed-handlers: function-heavy +
    //    discussion-heavy. fn_m=6, t_m=1 (ratio 6
    //    = function), belongs=2 (ratio 2/6=0.33 = disc)
    seed_entity("disc_hand");
    for i in 0..6 {
        let s = format!("dh_fm_{i}");
        seed_fn(&s);
        mention(&conn, &s, "disc_hand");
    }
    seed_type("dh_tm_0");
    type_mention(&conn, "dh_tm_0", "disc_hand");
    for i in 0..2 {
        let s = format!("dh_b_{i}");
        seed_fn(&s);
        belongs(&conn, &s, "disc_hand");
    }
    // 3. architectural-center: function-heavy +
    //    balanced. fn_m=4, t_m=1 (ratio 4 = function),
    //    belongs=4 (ratio 1.0 = balanced)
    seed_entity("arch_ctr");
    for i in 0..4 {
        let s = format!("ac_fm_{i}");
        seed_fn(&s);
        mention(&conn, &s, "arch_ctr");
    }
    seed_type("ac_tm_0");
    type_mention(&conn, "ac_tm_0", "arch_ctr");
    for i in 0..4 {
        let s = format!("ac_b_{i}");
        seed_fn(&s);
        belongs(&conn, &s, "arch_ctr");
    }
    // 4. future-work: pure-discussion (any flavor + 0
    //    belongs). fn_m=3, t_m=2, belongs=0
    seed_entity("future");
    for i in 0..3 {
        let s = format!("fw_fm_{i}");
        seed_fn(&s);
        mention(&conn, &s, "future");
    }
    for i in 0..2 {
        let s = format!("fw_tm_{i}");
        seed_type(&s);
        type_mention(&conn, &s, "future");
    }
    // 5. implementation-anomaly: balanced flavor +
    //    impl-heavy lifecycle. fn_m=5, t_m=3 (ratio
    //    1.67 = balanced strictly within 1.5-2.0).
    //    belongs=10 (ratio 10/5 = 2.0 = impl-heavy)
    seed_entity("impl_anom");
    for i in 0..5 {
        let s = format!("ia_fm_{i}");
        seed_fn(&s);
        mention(&conn, &s, "impl_anom");
    }
    for i in 0..3 {
        let s = format!("ia_tm_{i}");
        seed_type(&s);
        type_mention(&conn, &s, "impl_anom");
    }
    for i in 0..10 {
        let s = format!("ia_b_{i}");
        seed_fn(&s);
        belongs(&conn, &s, "impl_anom");
    }

    let q = SAVED_QUERIES
        .iter()
        .find(|q| q.name == "entity-state-grid")
        .expect("entity-state-grid must exist");
    let mut result = conn.query(sql_of(q.name)).expect("query runs");
    let mut rows: Vec<(String, String)> = Vec::new();
    for row in &mut result {
        let r = row.clone();
        let id = if let kv::Value::String(s) = &r[0] {
            s.clone()
        } else {
            panic!("entity_id: {r:?}");
        };
        let cluster = if let kv::Value::String(s) = &r[3] {
            s.clone()
        } else {
            panic!("cluster_label: {r:?}");
        };
        rows.push((id, cluster));
    }
    let by_id = |id: &str| -> String {
        rows.iter()
            .find(|(eid, _)| eid == id)
            .unwrap_or_else(|| panic!("{id} must surface: {rows:?}"))
            .1
            .clone()
    };
    assert_eq!(by_id("impl_infra"), "implemented-infrastructure");
    assert_eq!(by_id("disc_hand"), "discussed-handlers");
    assert_eq!(by_id("arch_ctr"), "architectural-center");
    assert_eq!(by_id("future"), "future-work");
    assert_eq!(by_id("impl_anom"), "implementation-anomaly");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn entity_belongs_vs_mentions_ratio_classifies_lifecycle() {
    // Per iter 359: 4 fixture entities pin all 4
    // lifecycle classes:
    // - impl_heavy: 6 belongs / 2 mentions = ratio 3
    //   = implementation-heavy
    // - balanced: 2 belongs / 2 mentions = ratio 1
    //   = balanced
    // - disc_heavy: 1 belongs / 5 mentions = 0.2
    //   = discussion-heavy
    // - pure_disc: 0 belongs / 3 mentions = pure-
    //   discussion
    let dir = std::env::temp_dir().join(format!("doc-linter-saved-ebvmr-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let db = testkit::open_db(&dir);
    {
        let conn = testkit::connect(&db);
        testkit::reset(&conn);
    }
    let conn = testkit::connect(&db);
    let seed_entity = |id: &str| {
        conn.query(&format!(
            "CREATE (:Entity {{id: '{id}', display: '{id}', \
                 description: '', synonyms: [], bounded_contexts: [], \
                 scanner_coverage: [], source_modules: [], \
                 mention_count: 0, is_god_node: false, \
                 entity_class: '', repo_id: 'doc-linter', \
                 attributes: []}})"
        ))
        .unwrap();
    };
    let seed_fn = |sym: &str| {
        conn.query(&format!(
            "CREATE (:Function {{symbol: '{sym}', crate: 'c', \
                 file: 'src/x.rs', line: 1, doc_comment: '', \
                 language: 'rust', signature: '', body_excerpt: '', \
                 last_touched: '2026-06-01', repo_id: 'doc-linter'}})"
        ))
        .unwrap();
    };
    seed_entity("impl_heavy");
    seed_entity("balanced");
    seed_entity("disc_heavy");
    seed_entity("pure_disc");
    // impl_heavy: 6 belongs + 2 mentions
    for i in 0..6 {
        let sym = format!("ih_b_{i}");
        seed_fn(&sym);
        conn.query(&format!(
            "MATCH (fn:Function {{symbol: '{sym}'}}), \
                       (e:Entity {{id: 'impl_heavy'}}) \
                 CREATE (fn)-[:FUNCTION_BELONGS_TO]->(e)"
        ))
        .unwrap();
    }
    for i in 0..2 {
        let sym = format!("ih_m_{i}");
        seed_fn(&sym);
        conn.query(&format!(
            "MATCH (fn:Function {{symbol: '{sym}'}}), \
                       (e:Entity {{id: 'impl_heavy'}}) \
                 CREATE (fn)-[:FUNCTION_MENTIONS {{confidence: 'high'}}]->(e)"
        ))
        .unwrap();
    }
    // balanced: 2 belongs + 2 mentions
    for i in 0..2 {
        let sym = format!("bl_b_{i}");
        seed_fn(&sym);
        conn.query(&format!(
            "MATCH (fn:Function {{symbol: '{sym}'}}), \
                       (e:Entity {{id: 'balanced'}}) \
                 CREATE (fn)-[:FUNCTION_BELONGS_TO]->(e)"
        ))
        .unwrap();
        let sym2 = format!("bl_m_{i}");
        seed_fn(&sym2);
        conn.query(&format!(
            "MATCH (fn:Function {{symbol: '{sym2}'}}), \
                       (e:Entity {{id: 'balanced'}}) \
                 CREATE (fn)-[:FUNCTION_MENTIONS {{confidence: 'high'}}]->(e)"
        ))
        .unwrap();
    }
    // disc_heavy: 1 belongs + 5 mentions
    seed_fn("dh_b_0");
    conn.query(
        "MATCH (fn:Function {symbol: 'dh_b_0'}), \
                   (e:Entity {id: 'disc_heavy'}) \
             CREATE (fn)-[:FUNCTION_BELONGS_TO]->(e)",
    )
    .unwrap();
    for i in 0..5 {
        let sym = format!("dh_m_{i}");
        seed_fn(&sym);
        conn.query(&format!(
            "MATCH (fn:Function {{symbol: '{sym}'}}), \
                       (e:Entity {{id: 'disc_heavy'}}) \
                 CREATE (fn)-[:FUNCTION_MENTIONS {{confidence: 'high'}}]->(e)"
        ))
        .unwrap();
    }
    // pure_disc: 0 belongs + 3 mentions
    for i in 0..3 {
        let sym = format!("pd_m_{i}");
        seed_fn(&sym);
        conn.query(&format!(
            "MATCH (fn:Function {{symbol: '{sym}'}}), \
                       (e:Entity {{id: 'pure_disc'}}) \
                 CREATE (fn)-[:FUNCTION_MENTIONS {{confidence: 'high'}}]->(e)"
        ))
        .unwrap();
    }

    let q = SAVED_QUERIES
        .iter()
        .find(|q| q.name == "entity-belongs-vs-mentions-ratio")
        .expect("entity-belongs-vs-mentions-ratio must exist");
    let mut result = conn.query(sql_of(q.name)).expect("query runs");
    let as_i64 = |v: &kv::Value| -> i64 {
        match v {
            kv::Value::Int64(n) => *n,
            kv::Value::Int128(n) => *n as i64,
            other => panic!("expected int: {other:?}"),
        }
    };
    let mut rows: Vec<(String, i64, i64, String)> = Vec::new();
    for row in &mut result {
        let r = row.clone();
        let id = if let kv::Value::String(s) = &r[0] {
            s.clone()
        } else {
            panic!("entity_id: {r:?}");
        };
        let bc = as_i64(&r[1]);
        let mc = as_i64(&r[2]);
        let label = if let kv::Value::String(s) = &r[4] {
            s.clone()
        } else {
            panic!("lifecycle_label: {r:?}");
        };
        rows.push((id, bc, mc, label));
    }
    let by_id = |id: &str| -> (i64, i64, String) {
        let r = rows
            .iter()
            .find(|(eid, _, _, _)| eid == id)
            .unwrap_or_else(|| panic!("{id} must surface: {rows:?}"));
        (r.1, r.2, r.3.clone())
    };
    let (bc, mc, label) = by_id("impl_heavy");
    assert_eq!((bc, mc), (6, 2));
    assert_eq!(label, "implementation-heavy");
    let (bc, mc, label) = by_id("balanced");
    assert_eq!((bc, mc), (2, 2));
    assert_eq!(label, "balanced");
    let (bc, mc, label) = by_id("disc_heavy");
    assert_eq!((bc, mc), (1, 5));
    assert_eq!(label, "discussion-heavy");
    let (bc, mc, label) = by_id("pure_disc");
    assert_eq!((bc, mc), (0, 3));
    assert_eq!(label, "pure-discussion");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn entity_mention_ratio_classifies_flavor() {
    // Per iter 357: 3 fixture entities with distinct
    // flavors:
    // - func_heavy: 4 function mentions / 1 type
    //   mention = ratio 4.0 = function-heavy
    // - balanced: 7 function / 4 type = ratio 1.75
    //   = balanced
    // - type_heavy: 3 function / 3 type = ratio 1.0
    //   = type-heavy
    let dir = std::env::temp_dir().join(format!("doc-linter-saved-emr-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let db = testkit::open_db(&dir);
    {
        let conn = testkit::connect(&db);
        testkit::reset(&conn);
    }
    let conn = testkit::connect(&db);
    let seed_entity = |id: &str| {
        conn.query(&format!(
            "CREATE (:Entity {{id: '{id}', display: '{id}', \
                 description: '', synonyms: [], bounded_contexts: [], \
                 scanner_coverage: [], source_modules: [], \
                 mention_count: 0, is_god_node: false, \
                 entity_class: '', repo_id: 'doc-linter', \
                 attributes: []}})"
        ))
        .unwrap();
    };
    let seed_fn = |sym: &str| {
        conn.query(&format!(
            "CREATE (:Function {{symbol: '{sym}', crate: 'c', \
                 file: 'src/x.rs', line: 1, doc_comment: '', \
                 language: 'rust', signature: '', body_excerpt: '', \
                 last_touched: '2026-06-01', repo_id: 'doc-linter'}})"
        ))
        .unwrap();
    };
    let seed_type = |sym: &str| {
        conn.query(&format!(
            "CREATE (:Type {{symbol: '{sym}', kind: 'struct', \
                 crate: 'c', file: 'src/x.rs', line: 1, \
                 doc_comment: '', language: 'rust', signature: '', \
                 body_excerpt: '', last_touched: '2026-06-01'}})"
        ))
        .unwrap();
    };
    seed_entity("func_heavy");
    seed_entity("balanced");
    seed_entity("type_heavy");
    // 4 functions + 1 type mention func_heavy (4.0 ratio)
    for i in 0..4 {
        let sym = format!("ff_{i}");
        seed_fn(&sym);
        conn.query(&format!(
            "MATCH (fn:Function {{symbol: '{sym}'}}), \
                       (e:Entity {{id: 'func_heavy'}}) \
                 CREATE (fn)-[:FUNCTION_MENTIONS {{confidence: 'high'}}]->(e)"
        ))
        .unwrap();
    }
    seed_type("ftype_1");
    conn.query(
        "MATCH (t:Type {symbol: 'ftype_1'}), \
             (e:Entity {id: 'func_heavy'}) \
             CREATE (t)-[:TYPE_MENTIONS {confidence: 'high'}]->(e)",
    )
    .unwrap();
    // 7 functions + 4 types mention balanced (1.75 ratio)
    for i in 0..7 {
        let sym = format!("bf_{i}");
        seed_fn(&sym);
        conn.query(&format!(
            "MATCH (fn:Function {{symbol: '{sym}'}}), \
                       (e:Entity {{id: 'balanced'}}) \
                 CREATE (fn)-[:FUNCTION_MENTIONS {{confidence: 'high'}}]->(e)"
        ))
        .unwrap();
    }
    for i in 0..4 {
        let sym = format!("bt_{i}");
        seed_type(&sym);
        conn.query(&format!(
            "MATCH (t:Type {{symbol: '{sym}'}}), \
                       (e:Entity {{id: 'balanced'}}) \
                 CREATE (t)-[:TYPE_MENTIONS {{confidence: 'high'}}]->(e)"
        ))
        .unwrap();
    }
    // 3 functions + 3 types mention type_heavy (1.0 ratio)
    for i in 0..3 {
        let sym = format!("tf_{i}");
        seed_fn(&sym);
        conn.query(&format!(
            "MATCH (fn:Function {{symbol: '{sym}'}}), \
                       (e:Entity {{id: 'type_heavy'}}) \
                 CREATE (fn)-[:FUNCTION_MENTIONS {{confidence: 'high'}}]->(e)"
        ))
        .unwrap();
        let tsym = format!("tt_{i}");
        seed_type(&tsym);
        conn.query(&format!(
            "MATCH (t:Type {{symbol: '{tsym}'}}), \
                       (e:Entity {{id: 'type_heavy'}}) \
                 CREATE (t)-[:TYPE_MENTIONS {{confidence: 'high'}}]->(e)"
        ))
        .unwrap();
    }

    let q = SAVED_QUERIES
        .iter()
        .find(|q| q.name == "entity-mention-ratio")
        .expect("entity-mention-ratio must exist");
    let mut result = conn.query(sql_of(q.name)).expect("query runs");
    let as_i64 = |v: &kv::Value| -> i64 {
        match v {
            kv::Value::Int64(n) => *n,
            kv::Value::Int128(n) => *n as i64,
            other => panic!("expected int: {other:?}"),
        }
    };
    let mut rows: Vec<(String, i64, i64, String)> = Vec::new();
    for row in &mut result {
        let r = row.clone();
        let id = if let kv::Value::String(s) = &r[0] {
            s.clone()
        } else {
            panic!("entity_id: {r:?}");
        };
        let fm = as_i64(&r[1]);
        let tm = as_i64(&r[2]);
        let label = if let kv::Value::String(s) = &r[4] {
            s.clone()
        } else {
            panic!("flavor_label: {r:?}");
        };
        rows.push((id, fm, tm, label));
    }
    let by_id = |id: &str| -> (i64, i64, String) {
        let r = rows
            .iter()
            .find(|(eid, _, _, _)| eid == id)
            .unwrap_or_else(|| panic!("{id} must surface: {rows:?}"));
        (r.1, r.2, r.3.clone())
    };
    let (fm, tm, label) = by_id("func_heavy");
    assert_eq!((fm, tm), (4, 1));
    assert_eq!(label, "function-heavy");
    let (fm, tm, label) = by_id("balanced");
    assert_eq!((fm, tm), (7, 4));
    assert_eq!(label, "balanced");
    let (fm, tm, label) = by_id("type_heavy");
    assert_eq!((fm, tm), (3, 3));
    assert_eq!(label, "type-heavy");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn findings_by_attachment_mode_classifies_bimodal() {
    // Per iter 355: 3 unstubbed-concept + 4 source-
    // marker findings classify into 2 rows.
    let dir = std::env::temp_dir().join(format!("doc-linter-saved-fbam-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let db = testkit::open_db(&dir);
    {
        let conn = testkit::connect(&db);
        testkit::reset(&conn);
    }
    let conn = testkit::connect(&db);
    let seed_finding = |id: &str, kind: &str, file: &str, sev: &str| {
        conn.query(&format!(
            "CREATE (:Finding {{id: '{id}', kind: '{kind}', \
                 file: '{file}', line: 1, message: '', \
                 severity: '{sev}'}})"
        ))
        .unwrap();
    };
    seed_finding("usc-1", "unstubbed-concept", "doc-a", "info");
    seed_finding("usc-2", "unstubbed-concept", "doc-a", "info");
    seed_finding("usc-3", "unstubbed-concept", "doc-b", "info");
    seed_finding("td-1", "todo", "src/x.rs", "info");
    seed_finding("td-2", "todo", "src/x.rs", "info");
    seed_finding("fm-1", "fixme", "src/y.rs", "warning");
    seed_finding("xx-1", "xxx", "src/z.rs", "info");

    let q = SAVED_QUERIES
        .iter()
        .find(|q| q.name == "findings-by-attachment-mode")
        .expect("findings-by-attachment-mode must exist");
    let mut result = conn.query(sql_of(q.name)).expect("query runs");
    let as_i64 = |v: &kv::Value| -> i64 {
        match v {
            kv::Value::Int64(n) => *n,
            kv::Value::Int128(n) => *n as i64,
            other => panic!("expected int: {other:?}"),
        }
    };
    let mut rows: Vec<(String, i64)> = Vec::new();
    for row in &mut result {
        let r = row.clone();
        let mode = if let kv::Value::String(s) = &r[0] {
            s.clone()
        } else {
            panic!("attachment_mode: {r:?}");
        };
        rows.push((mode, as_i64(&r[1])));
    }
    let by_mode = |m: &str| -> i64 {
        rows.iter()
            .find(|(mode, _)| mode == m)
            .unwrap_or_else(|| panic!("{m} must surface: {rows:?}"))
            .1
    };
    assert_eq!(by_mode("source-code-marker"), 4);
    assert_eq!(by_mode("doc-unstubbed-concept"), 3);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn unstubbed_concepts_by_doc_aggregates_per_doc_id() {
    // Per iter 355: fixture 2 unstubbed in doc-a + 1
    // in doc-b + 0 in doc-c (only TODO). doc-a returns
    // first (count=2), doc-b second.
    let dir = std::env::temp_dir().join(format!("doc-linter-saved-ucbd-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let db = testkit::open_db(&dir);
    {
        let conn = testkit::connect(&db);
        testkit::reset(&conn);
    }
    let conn = testkit::connect(&db);
    let seed_finding = |id: &str, kind: &str, file: &str, sev: &str| {
        conn.query(&format!(
            "CREATE (:Finding {{id: '{id}', kind: '{kind}', \
                 file: '{file}', line: 1, message: '', \
                 severity: '{sev}'}})"
        ))
        .unwrap();
    };
    seed_finding("usc-a1", "unstubbed-concept", "doc-a", "info");
    seed_finding("usc-a2", "unstubbed-concept", "doc-a", "info");
    seed_finding("usc-b1", "unstubbed-concept", "doc-b", "info");
    // TODO not on a doc — must NOT surface.
    seed_finding("td-c", "todo", "doc-c", "info");

    let q = SAVED_QUERIES
        .iter()
        .find(|q| q.name == "unstubbed-concepts-by-doc")
        .expect("unstubbed-concepts-by-doc must exist");
    let mut result = conn.query(sql_of(q.name)).expect("query runs");
    let as_i64 = |v: &kv::Value| -> i64 {
        match v {
            kv::Value::Int64(n) => *n,
            kv::Value::Int128(n) => *n as i64,
            other => panic!("expected int: {other:?}"),
        }
    };
    let mut rows: Vec<(String, i64)> = Vec::new();
    for row in &mut result {
        let r = row.clone();
        let did = if let kv::Value::String(s) = &r[0] {
            s.clone()
        } else {
            panic!("doc_id: {r:?}");
        };
        rows.push((did, as_i64(&r[1])));
    }
    let by_doc = |d: &str| -> i64 {
        rows.iter()
            .find(|(did, _)| did == d)
            .unwrap_or_else(|| panic!("{d} must surface: {rows:?}"))
            .1
    };
    assert_eq!(by_doc("doc-a"), 2);
    assert_eq!(by_doc("doc-b"), 1);
    // doc-c (TODO not unstubbed-concept) must NOT surface.
    assert!(
        !rows.iter().any(|(did, _)| did == "doc-c"),
        "doc-c (TODO kind) must NOT surface: {rows:?}"
    );
    // Ordering: doc-a (2) before doc-b (1).
    assert_eq!(rows[0].0, "doc-a");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn function_hub_files_aggregates_per_file() {
    // Per iter 353: 2 fixture hubs in 2 distinct
    // files. Each meets distinct_callers >= 5 AND
    // caller_files >= 5. Returns 2 distinct files.
    let dir = std::env::temp_dir().join(format!("doc-linter-saved-fhf-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let db = testkit::open_db(&dir);
    {
        let conn = testkit::connect(&db);
        testkit::reset(&conn);
    }
    let conn = testkit::connect(&db);
    let seed_fn = |sym: &str, file: &str| {
        conn.query(&format!(
            "CREATE (:Function {{symbol: '{sym}', crate: 'c', \
                 file: '{file}', line: 1, doc_comment: '', \
                 language: 'rust', signature: '', body_excerpt: '', \
                 last_touched: '2026-06-01', repo_id: 'doc-linter'}})"
        ))
        .unwrap();
    };
    seed_fn("hub_a", "src/file_a.rs");
    seed_fn("hub_b", "src/file_b.rs");
    // 5 callers each in 5 distinct files for both hubs
    for i in 0..5 {
        let caller_file = format!("src/caller_{i}.rs");
        seed_fn(&format!("caller_a_{i}"), &caller_file);
        conn.query(&format!(
            "MATCH (a:Function {{symbol: 'caller_a_{i}'}}), \
                       (b:Function {{symbol: 'hub_a'}}) \
                 CREATE (a)-[:CALLS]->(b)"
        ))
        .unwrap();
        seed_fn(&format!("caller_b_{i}"), &format!("src/caller_b_{i}.rs"));
        conn.query(&format!(
            "MATCH (a:Function {{symbol: 'caller_b_{i}'}}), \
                       (b:Function {{symbol: 'hub_b'}}) \
                 CREATE (a)-[:CALLS]->(b)"
        ))
        .unwrap();
    }
    let q = SAVED_QUERIES
        .iter()
        .find(|q| q.name == "function-hub-files")
        .expect("function-hub-files must exist");
    let mut result = conn.query(sql_of(q.name)).expect("query runs");
    let mut files: Vec<String> = Vec::new();
    for row in &mut result {
        let r = row.clone();
        if let kv::Value::String(s) = &r[0] {
            files.push(s.clone());
        }
    }
    assert_eq!(files.len(), 2, "2 distinct files: {files:?}");
    assert!(files.contains(&"src/file_a.rs".to_string()));
    assert!(files.contains(&"src/file_b.rs".to_string()));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn type_hub_files_aggregates_per_file() {
    // Per iter 353 sibling: 2 fixture type hubs
    // each meets thresholds → 2 distinct files.
    let dir = std::env::temp_dir().join(format!("doc-linter-saved-thf-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let db = testkit::open_db(&dir);
    {
        let conn = testkit::connect(&db);
        testkit::reset(&conn);
    }
    let conn = testkit::connect(&db);
    let seed_fn = |sym: &str, file: &str| {
        conn.query(&format!(
            "CREATE (:Function {{symbol: '{sym}', crate: 'c', \
                 file: '{file}', line: 1, doc_comment: '', \
                 language: 'rust', signature: '', body_excerpt: '', \
                 last_touched: '2026-06-01', repo_id: 'doc-linter'}})"
        ))
        .unwrap();
    };
    let seed_type = |sym: &str, file: &str| {
        conn.query(&format!(
            "CREATE (:Type {{symbol: '{sym}', kind: 'struct', \
                 crate: 'c', file: '{file}', line: 1, doc_comment: '', \
                 language: 'rust', signature: '', body_excerpt: '', \
                 last_touched: '2026-06-01'}})"
        ))
        .unwrap();
    };
    seed_type("TypeA", "src/types_a.rs");
    seed_type("TypeB", "src/types_b.rs");
    for i in 0..5 {
        let f1 = format!("src/user_a_{i}.rs");
        seed_fn(&format!("user_a_{i}"), &f1);
        conn.query(&format!(
            "MATCH (fn:Function {{symbol: 'user_a_{i}'}}), \
                       (t:Type {{symbol: 'TypeA'}}) \
                 CREATE (fn)-[:USES_TYPE]->(t)"
        ))
        .unwrap();
        seed_fn(&format!("user_b_{i}"), &format!("src/user_b_{i}.rs"));
        conn.query(&format!(
            "MATCH (fn:Function {{symbol: 'user_b_{i}'}}), \
                       (t:Type {{symbol: 'TypeB'}}) \
                 CREATE (fn)-[:USES_TYPE]->(t)"
        ))
        .unwrap();
    }
    let q = SAVED_QUERIES
        .iter()
        .find(|q| q.name == "type-hub-files")
        .expect("type-hub-files must exist");
    let mut result = conn.query(sql_of(q.name)).expect("query runs");
    let mut files: Vec<String> = Vec::new();
    for row in &mut result {
        let r = row.clone();
        if let kv::Value::String(s) = &r[0] {
            files.push(s.clone());
        }
    }
    assert_eq!(files.len(), 2, "2 distinct files: {files:?}");
    assert!(files.contains(&"src/types_a.rs".to_string()));
    assert!(files.contains(&"src/types_b.rs".to_string()));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn hub_types_by_caller_breadth_classifies_spread() {
    // Per iter 351 ([[user-probe-093]] Finding F+G):
    // 3 fixture types with distinct spread shapes.
    let dir = std::env::temp_dir().join(format!("doc-linter-saved-htbcb-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let db = testkit::open_db(&dir);
    {
        let conn = testkit::connect(&db);
        testkit::reset(&conn);
    }
    let conn = testkit::connect(&db);
    let seed_type = |sym: &str, kind: &str, file: &str| {
        conn.query(&format!(
            "CREATE (:Type {{symbol: '{sym}', kind: '{kind}', \
                 crate: 'c', file: '{file}', line: 1, \
                 doc_comment: '', language: 'rust', signature: '', \
                 body_excerpt: '', last_touched: '2026-06-01'}})"
        ))
        .unwrap();
    };
    let seed_fn = |sym: &str, file: &str| {
        conn.query(&format!(
            "CREATE (:Function {{symbol: '{sym}', crate: 'c', \
                 file: '{file}', line: 1, doc_comment: '', \
                 language: 'rust', signature: '', body_excerpt: '', \
                 last_touched: '2026-06-01', repo_id: 'doc-linter'}})"
        ))
        .unwrap();
    };
    seed_type("Hub_api", "struct", "src/api.rs");
    seed_type("Hub_mid", "struct", "src/mid.rs");
    seed_type("Hub_internal", "struct", "src/cluster.rs");

    // api-tier: 5 distinct users in 5 distinct files
    for i in 0..5 {
        seed_fn(&format!("user_api_{i}"), &format!("src/api_caller_{i}.rs"));
        conn.query(&format!(
            "MATCH (fn:Function {{symbol: 'user_api_{i}'}}), \
                       (t:Type {{symbol: 'Hub_api'}}) \
                 CREATE (fn)-[:USES_TYPE]->(t)"
        ))
        .unwrap();
    }
    // mid-tier: 5 users in 3 files
    for i in 0..5 {
        let file_i = i % 3;
        seed_fn(
            &format!("user_mid_{i}"),
            &format!("src/mid_file_{file_i}.rs"),
        );
        conn.query(&format!(
            "MATCH (fn:Function {{symbol: 'user_mid_{i}'}}), \
                       (t:Type {{symbol: 'Hub_mid'}}) \
                 CREATE (fn)-[:USES_TYPE]->(t)"
        ))
        .unwrap();
    }
    // internal-cluster: 5 users all in same file
    for i in 0..5 {
        seed_fn(&format!("user_int_{i}"), "src/cluster.rs");
        conn.query(&format!(
            "MATCH (fn:Function {{symbol: 'user_int_{i}'}}), \
                       (t:Type {{symbol: 'Hub_internal'}}) \
                 CREATE (fn)-[:USES_TYPE]->(t)"
        ))
        .unwrap();
    }

    let q = SAVED_QUERIES
        .iter()
        .find(|q| q.name == "hub-types-by-caller-breadth")
        .expect("hub-types-by-caller-breadth must exist");
    let mut result = conn.query(sql_of(q.name)).expect("query runs");
    let as_i64 = |v: &kv::Value| -> i64 {
        match v {
            kv::Value::Int64(n) => *n,
            kv::Value::Int128(n) => *n as i64,
            other => panic!("expected int: {other:?}"),
        }
    };
    let mut rows: Vec<(String, i64, i64, String)> = Vec::new();
    for row in &mut result {
        let r = row.clone();
        let sym = if let kv::Value::String(s) = &r[0] {
            s.clone()
        } else {
            panic!("symbol: {r:?}");
        };
        let du = as_i64(&r[3]);
        let uf = as_i64(&r[4]);
        let label = if let kv::Value::String(s) = &r[5] {
            s.clone()
        } else {
            panic!("spread_label: {r:?}");
        };
        rows.push((sym, du, uf, label));
    }
    let by_sym = |s: &str| -> (i64, i64, String) {
        let r = rows
            .iter()
            .find(|(sym, _, _, _)| sym == s)
            .unwrap_or_else(|| panic!("{s} must surface: {rows:?}"));
        (r.1, r.2, r.3.clone())
    };
    let (du, uf, label) = by_sym("Hub_api");
    assert_eq!(du, 5);
    assert_eq!(uf, 5);
    assert_eq!(label, "api-tier-type");
    let (du, uf, label) = by_sym("Hub_mid");
    assert_eq!(du, 5);
    assert_eq!(uf, 3);
    assert_eq!(label, "mid-tier-type");
    let (du, uf, label) = by_sym("Hub_internal");
    assert_eq!(du, 5);
    assert_eq!(uf, 1);
    assert_eq!(label, "internal-cluster-type");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn rust_test_files_by_pattern_classifies_sibling_integration_helper() {
    // Per iter 349: 4 fixture File rows + classify.
    let dir = std::env::temp_dir().join(format!("doc-linter-saved-rtfbp-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let db = testkit::open_db(&dir);
    {
        let conn = testkit::connect(&db);
        testkit::reset(&conn);
    }
    let conn = testkit::connect(&db);
    let seed_file = |path: &str, loc: u32| {
        conn.query(&format!(
            "CREATE (:File {{path: '{path}', language: 'rust', \
                 loc: {loc}, last_touched: '2026-06-01'}})"
        ))
        .unwrap();
    };
    seed_file("src/store/symbols/path.rs", 200);
    seed_file("src/store/symbols/path_tests.rs", 118);
    seed_file("tests/dropin_init.rs", 291);
    seed_file("tests/common/mod.rs", 233);
    seed_file("src/main.rs", 50);

    let q = SAVED_QUERIES
        .iter()
        .find(|q| q.name == "rust-test-files-by-pattern")
        .expect("rust-test-files-by-pattern must exist");
    let mut result = conn.query(sql_of(q.name)).expect("query runs");
    let mut rows: Vec<(String, String)> = Vec::new();
    for row in &mut result {
        let r = row.clone();
        let path = if let kv::Value::String(s) = &r[0] {
            s.clone()
        } else {
            panic!("path: {r:?}");
        };
        let kind = if let kv::Value::String(s) = &r[2] {
            s.clone()
        } else {
            panic!("test_kind: {r:?}");
        };
        rows.push((path, kind));
    }
    let by_path = |p: &str| -> String {
        rows.iter()
            .find(|(path, _)| path == p)
            .unwrap_or_else(|| panic!("{p} must surface: {rows:?}"))
            .1
            .clone()
    };
    assert_eq!(by_path("src/store/symbols/path_tests.rs"), "sibling");
    assert_eq!(by_path("tests/dropin_init.rs"), "integration");
    assert_eq!(by_path("tests/common/mod.rs"), "helper");
    // path.rs (production) + main.rs (production) must NOT
    // surface.
    for noisy in ["src/store/symbols/path.rs", "src/main.rs"] {
        assert!(
            !rows.iter().any(|(p, _)| p == noisy),
            "production `{noisy}` must NOT surface: {rows:?}"
        );
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn rust_production_source_files_excludes_tests_and_mod() {
    // Per iter 349 sibling: 6 fixture files +
    // exclusion check.
    let dir = std::env::temp_dir().join(format!("doc-linter-saved-rpsf-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let db = testkit::open_db(&dir);
    {
        let conn = testkit::connect(&db);
        testkit::reset(&conn);
    }
    let conn = testkit::connect(&db);
    let seed_file = |path: &str, loc: u32| {
        conn.query(&format!(
            "CREATE (:File {{path: '{path}', language: 'rust', \
                 loc: {loc}, last_touched: '2026-06-01'}})"
        ))
        .unwrap();
    };
    seed_file("src/main.rs", 50);
    seed_file("src/store/symbols/path.rs", 200);
    seed_file("src/store/symbols/path_tests.rs", 118);
    seed_file("src/store/mod.rs", 30);
    seed_file("tests/dropin_init.rs", 291);
    seed_file("src/parser.rs", 100);

    let q = SAVED_QUERIES
        .iter()
        .find(|q| q.name == "rust-production-source-files")
        .expect("rust-production-source-files must exist");
    let mut result = conn.query(sql_of(q.name)).expect("query runs");
    let mut paths: Vec<String> = Vec::new();
    for row in &mut result {
        let r = row.clone();
        if let kv::Value::String(s) = &r[0] {
            paths.push(s.clone());
        }
    }
    // main.rs + store/symbols/path.rs + parser.rs must surface
    for must in ["src/main.rs", "src/store/symbols/path.rs", "src/parser.rs"] {
        assert!(
            paths.iter().any(|p| p == must),
            "production `{must}` must surface: {paths:?}"
        );
    }
    // path_tests.rs + mod.rs + tests/dropin_init.rs must NOT surface
    for noisy in [
        "src/store/symbols/path_tests.rs",
        "src/store/mod.rs",
        "tests/dropin_init.rs",
    ] {
        assert!(
            !paths.iter().any(|p| p == noisy),
            "non-production `{noisy}` must NOT surface: {paths:?}"
        );
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn feature_evolution_coupling_hybrid_catches_real_excludes_scaffold() {
    // Per iter 347 ([[user-probe-091]] correction):
    // hybrid filter commits>=5 OR jaccard<=0.55 must
    // surface the feature pair (commits=12 jaccard=0.6)
    // — qualifies via commits>=5. Must exclude
    // scaffold-clique (commits=3 jaccard=1.0) — fails
    // BOTH commits>=5 AND jaccard<=0.55.
    let dir = std::env::temp_dir().join(format!("doc-linter-saved-fech-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let db = testkit::open_db(&dir);
    {
        let conn = testkit::connect(&db);
        testkit::reset(&conn);
    }
    let conn = testkit::connect(&db);
    seed_feature_vs_scaffold_coupling_fixture(&conn);
    // Add a trusted-style real coupling
    // (commits=3 jaccard=0.5) to verify the
    // jaccard<=0.55 path catches it.
    conn.query(
        "CREATE (:File {path: 'items.py', language: 'python', \
             loc: 113, last_touched: '2026-06-01'})",
    )
    .unwrap();
    conn.query(
        "CREATE (:File {path: 'users.py', language: 'python', \
             loc: 232, last_touched: '2026-06-01'})",
    )
    .unwrap();
    conn.query(
        "MATCH (a:File {path: 'items.py'}), \
                   (b:File {path: 'users.py'}) \
             CREATE (a)-[:COUPLED_WITH {commits: 3, jaccard: 0.5, \
             last_co_change_at: '2026-04-03T11:45:40Z'}]->(b)",
    )
    .unwrap();

    let q = SAVED_QUERIES
        .iter()
        .find(|q| q.name == "feature-evolution-coupling-hybrid")
        .expect("feature-evolution-coupling-hybrid must exist");
    let mut result = conn.query(sql_of(q.name)).expect("query runs");
    let mut pairs: Vec<(String, String)> = Vec::new();
    for row in &mut result {
        let r = row.clone();
        if let (kv::Value::String(a), kv::Value::String(b)) = (&r[0], &r[1]) {
            pairs.push((a.clone(), b.clone()));
        }
    }
    // commits>=5 path: feature-x.py ↔ feature-y.py
    assert!(
        pairs
            .iter()
            .any(|(a, b)| (a == "feature-x.py" && b == "feature-y.py")
                || (a == "feature-y.py" && b == "feature-x.py")),
        "feature pair (commits=12) must surface: {pairs:?}"
    );
    // jaccard<=0.55 path: items.py ↔ users.py
    assert!(
        pairs
            .iter()
            .any(|(a, b)| (a == "items.py" && b == "users.py")
                || (a == "users.py" && b == "items.py")),
        "items↔users (jaccard=0.5) must surface: {pairs:?}"
    );
    // Scaffold-clique (commits=3 jaccard=1.0) must NOT surface.
    for noisy in ["scaffold-a.tsx", "scaffold-b.tsx", "scaffold-c.tsx"] {
        assert!(
            !pairs.iter().any(|(a, b)| a == noisy || b == noisy),
            "scaffold `{noisy}` must NOT surface: {pairs:?}"
        );
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn coupling_density_stats_classifies_scaffold_vs_real() {
    // Per iter 347: on the iter-201 fixture (3
    // scaffold pairs commits=3 jaccard=1.0 + 1
    // feature pair commits=12 jaccard=0.6) the
    // stats query must return total=4, scaffold=3,
    // real=1.
    let dir = std::env::temp_dir().join(format!("doc-linter-saved-cds-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let db = testkit::open_db(&dir);
    {
        let conn = testkit::connect(&db);
        testkit::reset(&conn);
    }
    let conn = testkit::connect(&db);
    seed_feature_vs_scaffold_coupling_fixture(&conn);

    let q = SAVED_QUERIES
        .iter()
        .find(|q| q.name == "coupling-density-stats")
        .expect("coupling-density-stats must exist");
    let mut result = conn.query(sql_of(q.name)).expect("query runs");
    let row = result.next().expect("1 row");
    let as_i64 = |v: &kv::Value| -> i64 {
        match v {
            kv::Value::Int64(n) => *n,
            kv::Value::Int128(n) => *n as i64,
            other => panic!("expected int: {other:?}"),
        }
    };
    let total = as_i64(&row[0]);
    let scaffold = as_i64(&row[1]);
    let real = as_i64(&row[2]);
    // 3 scaffold-clique pairs (commits=3 jaccard=1.0)
    // + 1 feature pair (commits=12 jaccard=0.6).
    assert_eq!(total, 4);
    assert_eq!(scaffold, 3, "3 scaffold-clique pairs: {scaffold}");
    assert_eq!(real, 1, "1 feature pair: {real}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn coupled_files_without_scaffold_excludes_jaccard_1() {
    // Per iter 345 ([[user-probe-090]] Finding G):
    // jaccard=1.0 scaffold-clique pairs MUST be
    // filtered out. The 1 real feature pair at
    // jaccard=0.5 MUST surface.
    let dir = std::env::temp_dir().join(format!("doc-linter-saved-cwos-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let db = testkit::open_db(&dir);
    {
        let conn = testkit::connect(&db);
        testkit::reset(&conn);
    }
    let conn = testkit::connect(&db);
    seed_feature_vs_scaffold_coupling_fixture(&conn);

    let q = SAVED_QUERIES
        .iter()
        .find(|q| q.name == "coupled-files-without-scaffold")
        .expect("coupled-files-without-scaffold must exist");
    let mut result = conn.query(sql_of(q.name)).expect("query runs");
    let mut pairs: Vec<(String, String)> = Vec::new();
    for row in &mut result {
        let r = row.clone();
        if let (kv::Value::String(a), kv::Value::String(b)) = (&r[0], &r[1]) {
            pairs.push((a.clone(), b.clone()));
        }
    }
    // Feature pair (jaccard=0.6 in this fixture)
    // must surface.
    assert!(
        pairs
            .iter()
            .any(|(a, b)| (a == "feature-x.py" && b == "feature-y.py")
                || (a == "feature-y.py" && b == "feature-x.py")),
        "feature pair (jaccard=0.6) must surface: {pairs:?}"
    );
    // Scaffold-clique (jaccard=1.0) must NOT surface.
    for noisy in ["scaffold-a.tsx", "scaffold-b.tsx", "scaffold-c.tsx"] {
        assert!(
            !pairs.iter().any(|(a, b)| a == noisy || b == noisy),
            "scaffold (jaccard=1.0) `{noisy}` must NOT surface: \
                 {pairs:?}"
        );
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn coupled_files_by_jaccard_range_filters_by_bounds() {
    // Per iter 345 (sibling): parameterized range
    // filter. min=0.4 max=0.9 surfaces feature pair
    // only (0.6 in fixture). min=0.95 max=1.0
    // surfaces only scaffold-clique. min=0.0 max=0.55
    // surfaces nothing (feature at 0.6 above max,
    // scaffold at 1.0 above max).
    let dir = std::env::temp_dir().join(format!("doc-linter-saved-cfbjr-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let db = testkit::open_db(&dir);
    {
        let conn = testkit::connect(&db);
        testkit::reset(&conn);
    }
    let conn = testkit::connect(&db);
    seed_feature_vs_scaffold_coupling_fixture(&conn);

    let q = SAVED_QUERIES
        .iter()
        .find(|q| q.name == "coupled-files-by-jaccard-range")
        .expect("coupled-files-by-jaccard-range must exist");
    let run = |min_j: &str, max_j: &str| -> Vec<(String, String)> {
        let cypher = sql_of(q.name)
            .replace("$min_jaccard", min_j)
            .replace("$max_jaccard", max_j);
        let mut result = conn.query(&cypher).expect("query runs");
        let mut pairs: Vec<(String, String)> = Vec::new();
        for row in &mut result {
            let r = row.clone();
            if let (kv::Value::String(a), kv::Value::String(b)) = (&r[0], &r[1]) {
                pairs.push((a.clone(), b.clone()));
            }
        }
        pairs
    };
    // 0.4 - 0.9: only feature pair (0.6) qualifies.
    let mid = run("0.4", "0.9");
    assert!(
        mid.iter()
            .any(|(a, b)| (a == "feature-x.py" && b == "feature-y.py")
                || (a == "feature-y.py" && b == "feature-x.py")),
        "feature pair must surface in 0.4-0.9: {mid:?}"
    );
    for noisy in ["scaffold-a.tsx", "scaffold-b.tsx", "scaffold-c.tsx"] {
        assert!(
            !mid.iter().any(|(a, b)| a == noisy || b == noisy),
            "scaffold must NOT surface in 0.4-0.9: {mid:?}"
        );
    }
    // 0.95 - 1.0: only scaffold-clique qualifies.
    let high = run("0.95", "1.0");
    for noisy in ["scaffold-a.tsx", "scaffold-b.tsx", "scaffold-c.tsx"] {
        assert!(
            high.iter().any(|(a, b)| a == noisy || b == noisy),
            "scaffold must surface in 0.95-1.0: {high:?}"
        );
    }
    assert!(
        !high
            .iter()
            .any(|(a, b)| (a == "feature-x.py" && b == "feature-y.py")
                || (a == "feature-y.py" && b == "feature-x.py")),
        "feature pair (0.6) must NOT surface in 0.95-1.0: {high:?}"
    );
    // 0.0 - 0.55: nothing (feature 0.6, scaffold 1.0
    // both above max).
    let low = run("0.0", "0.55");
    assert_eq!(low.len(), 0, "nothing in 0.0-0.55 band: {low:?}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn feature_evolution_coupling_semantic_excludes_scaffold_clique() {
    // Per user-probe-048 Finding A: the scaffold-clique
    // (commits=3 jaccard=1.0) must be filtered out; only
    // the feature pair (commits=12) must surface.
    let dir = std::env::temp_dir().join(format!(
        "doc-linter-saved-feature-coupling-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let db = testkit::open_db(&dir);
    {
        let conn = testkit::connect(&db);
        testkit::reset(&conn);
    }
    let conn = testkit::connect(&db);
    seed_feature_vs_scaffold_coupling_fixture(&conn);

    let q = SAVED_QUERIES
        .iter()
        .find(|q| q.name == "feature-evolution-coupling")
        .expect("feature-evolution-coupling must exist");
    let mut result = conn.query(sql_of(q.name)).expect("query runs");
    let mut returned_pairs: Vec<(String, String)> = Vec::new();
    for row in &mut result {
        if let (kv::Value::String(a), kv::Value::String(b)) = (&row[0], &row[1]) {
            returned_pairs.push((a.clone(), b.clone()));
        }
    }
    assert!(
        returned_pairs
            .iter()
            .any(|(a, b)| (a == "feature-x.py" && b == "feature-y.py")
                || (a == "feature-y.py" && b == "feature-x.py")),
        "feature-evolution pair (commits=12) must surface: {returned_pairs:?}",
    );
    for noisy in ["scaffold-a.tsx", "scaffold-b.tsx", "scaffold-c.tsx"] {
        assert!(
            !returned_pairs.iter().any(|(a, b)| a == noisy || b == noisy),
            "scaffold-clique file `{noisy}` (commits=3) must NOT surface: {returned_pairs:?}",
        );
    }
    let _ = std::fs::remove_dir_all(&dir);
}

fn run_ingest_classifier_with_fixture<F>(seed: F) -> String
where
    F: FnOnce(&testkit::Conn<'_>),
{
    let dir = std::env::temp_dir().join(format!(
        "doc-linter-saved-ingest-classifier-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let db = testkit::open_db(&dir);
    {
        let conn = testkit::connect(&db);
        testkit::reset(&conn);
    }
    let conn = testkit::connect(&db);
    seed(&conn);

    let q = SAVED_QUERIES
        .iter()
        .find(|q| q.name == "corpus-ingest-state-classifier")
        .expect("corpus-ingest-state-classifier must exist");
    let mut result = conn.query(sql_of(q.name)).expect("query runs");
    let row = result.next().expect("at least one row");
    // Columns (iter 205 extended): fn_count, file_count, doc_count,
    // coupled_with_count, endpoint_count, ingest_state.
    let state = if let kv::Value::String(s) = &row[5] {
        s.clone()
    } else {
        panic!("ingest_state column must be String: row={row:?}");
    };
    let _ = std::fs::remove_dir_all(&dir);
    state
}

#[test]
fn corpus_ingest_state_classifier_emits_full_scip_when_functions_present() {
    // Per interrogation-040 Finding A: full-scip = Function
    // rows populated.
    let state = run_ingest_classifier_with_fixture(|conn| {
        conn.query(
            "CREATE (:File {path: 'a.rs', language: 'rust', loc: 100, last_touched: '2026-06-01'})",
        )
        .unwrap();
        conn.query("CREATE (:Function {symbol: 'foo()', crate: 'doc-linter', file: 'a.rs', line: 1, doc_comment: '', language: 'rust', signature: 'fn foo()', body_excerpt: '', last_touched: '2026-06-01', repo_id: 'doc-linter'})").unwrap();
    });
    assert_eq!(state, "full-scip");
}

#[test]
fn corpus_ingest_state_classifier_emits_file_only_no_scip_when_files_but_no_functions() {
    // Per interrogation-040 Finding A: file-only-no-scip =
    // iter-200 trusted-corpus pattern (Files present, 0
    // Functions).
    let state = run_ingest_classifier_with_fixture(|conn| {
        conn.query("CREATE (:File {path: 'a.py', language: 'python', loc: 100, last_touched: '2026-06-01'})").unwrap();
    });
    assert_eq!(state, "file-only-no-scip");
}

#[test]
fn corpus_ingest_state_classifier_emits_doc_only_by_design_when_docs_but_no_files() {
    // Per interrogation-040 Finding A: doc-only-by-design =
    // design corpus pattern (Docs but no Files / Functions).
    let state = run_ingest_classifier_with_fixture(|conn| {
        conn.query("CREATE (:Doc {id: 'a-doc', path: 'a.md', role: 'doc', kind: 'reference', lifecycle: '', bounded_context: '', title: 'A', summary: '', status: 'stable', updated: '2026-06-01', tags: [], covers: []})").unwrap();
    });
    assert_eq!(state, "doc-only-by-design");
}

#[test]
fn corpus_ingest_state_classifier_emits_empty_on_empty_corpus() {
    let state = run_ingest_classifier_with_fixture(|_conn| {});
    assert_eq!(state, "empty");
}

#[test]
fn corpus_ingest_state_classifier_exposes_all_5_counts_alongside_label() {
    // Per iter 205 (user-probe-049 Finding D): the extended
    // RETURN clause must include coupled_with_count +
    // endpoint_count alongside the original 3 counts +
    // label. This pins the column ordering so a future edit
    // doesn't silently drop a sub-pass-presence column.
    let dir = std::env::temp_dir().join(format!(
        "doc-linter-saved-ingest-5col-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let db = testkit::open_db(&dir);
    {
        let conn = testkit::connect(&db);
        testkit::reset(&conn);
    }
    let conn = testkit::connect(&db);
    // Seed exactly 1 of each so the counts are
    // distinguishable from defaults.
    conn.query(
        "CREATE (:File {path: 'a.rs', language: 'rust', loc: 100, last_touched: '2026-06-01'})",
    )
    .unwrap();
    conn.query(
        "CREATE (:File {path: 'b.rs', language: 'rust', loc: 100, last_touched: '2026-06-01'})",
    )
    .unwrap();
    conn.query("CREATE (:Function {symbol: 'foo()', crate: 'doc-linter', file: 'a.rs', line: 1, doc_comment: '', language: 'rust', signature: 'fn foo()', body_excerpt: '', last_touched: '2026-06-01', repo_id: 'doc-linter'})").unwrap();
    conn.query("CREATE (:Doc {id: 'a-doc', path: 'a.md', role: 'doc', kind: 'reference', lifecycle: '', bounded_context: '', title: 'A', summary: '', status: 'stable', updated: '2026-06-01', tags: [], covers: []})").unwrap();
    conn.query("MATCH (a:File {path: 'a.rs'}), (b:File {path: 'b.rs'}) CREATE (a)-[:COUPLED_WITH {commits: 5, jaccard: 0.7, last_co_change_at: '2026-05-01T00:00:00Z'}]->(b)").unwrap();
    conn.query("CREATE (:Endpoint {id: 'GET /health', kind: 'axum', method: 'GET', path: '/health', handler_symbol: 'health()', source_file: 'a.rs', source_line: 5})").unwrap();

    let q = SAVED_QUERIES
        .iter()
        .find(|q| q.name == "corpus-ingest-state-classifier")
        .expect("classifier must exist");
    let mut result = conn.query(sql_of(q.name)).expect("query runs");
    let row = result.next().expect("one row");
    // fn_count + file_count + doc_count + coupled_with_count
    // + endpoint_count + ingest_state — 6 columns total.
    assert_eq!(row.len(), 6, "must return 6 columns: {row:?}");
    // Spot-check that the new counts are present (positions
    // 3 + 4) and the label still at position 5.
    let cw = match &row[3] {
        kv::Value::Int64(n) => *n,
        other => panic!("coupled_with_count must be Int64: {other:?}"),
    };
    let ep = match &row[4] {
        kv::Value::Int64(n) => *n,
        other => panic!("endpoint_count must be Int64: {other:?}"),
    };
    assert_eq!(cw, 1, "1 COUPLED_WITH seeded; row={row:?}");
    assert_eq!(ep, 1, "1 Endpoint seeded; row={row:?}");
    let state = if let kv::Value::String(s) = &row[5] {
        s.as_str()
    } else {
        panic!("state at position 5 must be String: {row:?}");
    };
    assert_eq!(state, "full-scip");
    let _ = std::fs::remove_dir_all(&dir);
}

fn seed_tag_axis_coverage_fixture(conn: &testkit::Conn<'_>) {
    // 11 Doc rows producing distinct tag-count categories:
    // load-bearing-tag → 7 docs
    // mid-family-tag   → 4 docs
    // small-family-tag → 2 docs
    // unique-singleton → 1 doc
    // stopword `research` carried by 5 docs (must be excluded
    // by the stopword filter even though it's ≥4).
    let docs: &[(&str, &str)] = &[
        ("d-a", "['load-bearing-tag','mid-family-tag','research']"),
        ("d-b", "['load-bearing-tag','mid-family-tag','research']"),
        ("d-c", "['load-bearing-tag','mid-family-tag','research']"),
        ("d-d", "['load-bearing-tag','mid-family-tag','research']"),
        ("d-e", "['load-bearing-tag','small-family-tag','research']"),
        ("d-f", "['load-bearing-tag','small-family-tag']"),
        ("d-g", "['load-bearing-tag','unique-singleton']"),
        ("d-h", "['unrelated-tag']"),
        ("d-i", "['unrelated-tag']"),
        ("d-j", "['unrelated-tag']"),
        ("d-k", "['unrelated-tag']"),
    ];
    for (id, tags) in docs {
        let cypher = format!(
            "CREATE (:Doc {{id: '{id}', path: '{id}.md', role: 'doc', \
                 kind: 'reference', lifecycle: '', bounded_context: '', \
                 title: 'T', summary: '', status: 'stable', updated: \
                 '2026-06-01', tags: {tags}, covers: []}})"
        );
        conn.query(&cypher).expect("seed Doc");
    }
}

#[test]
fn tag_axis_coverage_summary_emits_categorical_labels_per_tag() {
    // Per iter 209 (interrogation-042 follow-up): the
    // categorical-emission classifier on tag use count.
    // Fixture has 7-use + 4-use + 2-use + 1-use tags
    // exercising all 4 categories.
    let dir = std::env::temp_dir().join(format!(
        "doc-linter-saved-tag-coverage-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let db = testkit::open_db(&dir);
    {
        let conn = testkit::connect(&db);
        testkit::reset(&conn);
    }
    let conn = testkit::connect(&db);
    seed_tag_axis_coverage_fixture(&conn);

    let q = SAVED_QUERIES
        .iter()
        .find(|q| q.name == "tag-axis-coverage-summary")
        .expect("tag-axis-coverage-summary must exist");
    let mut result = conn.query(sql_of(q.name)).expect("query runs");
    let mut by_tag: std::collections::HashMap<String, String> = std::collections::HashMap::new();
    for row in &mut result {
        // Columns: tag, uses, health.
        if let (kv::Value::String(tag), kv::Value::String(health)) = (&row[0], &row[2]) {
            by_tag.insert(tag.clone(), health.clone());
        }
    }
    assert_eq!(
        by_tag.get("load-bearing-tag").map(String::as_str),
        Some("load-bearing"),
        "7-use tag must be load-bearing: {by_tag:?}",
    );
    assert_eq!(
        by_tag.get("mid-family-tag").map(String::as_str),
        Some("mid-family"),
        "4-use tag must be mid-family: {by_tag:?}",
    );
    assert_eq!(
        by_tag.get("small-family-tag").map(String::as_str),
        Some("small-family"),
        "2-use tag must be small-family: {by_tag:?}",
    );
    assert_eq!(
        by_tag.get("unique-singleton").map(String::as_str),
        Some("singleton"),
        "1-use tag must be singleton: {by_tag:?}",
    );
    assert!(
        !by_tag.contains_key("research"),
        "stopword `research` must NOT surface even at 5 uses: {by_tag:?}",
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn storyline_siblings_orders_by_shared_tag_count() {
    // Per iter 209 (interrogation-042 Finding B): given
    // a seed doc, return others sharing non-stopword tags
    // ordered by overlap count. Reuse the tag-axis fixture:
    // seed d-a shares 2 meaningful tags (load-bearing-tag
    // + mid-family-tag) with d-b/c/d, 1 (load-bearing) with
    // d-e/f/g, 0 with d-h-k.
    let dir = std::env::temp_dir().join(format!(
        "doc-linter-saved-storyline-siblings-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let db = testkit::open_db(&dir);
    {
        let conn = testkit::connect(&db);
        testkit::reset(&conn);
    }
    let conn = testkit::connect(&db);
    seed_tag_axis_coverage_fixture(&conn);

    let q = SAVED_QUERIES
        .iter()
        .find(|q| q.name == "storyline-siblings")
        .expect("storyline-siblings must exist");
    let cypher = sql_of(q.name).replace("$doc_id", "'d-a'");
    let mut result = conn.query(&cypher).expect("query runs");
    let mut returned: Vec<(String, i64)> = Vec::new();
    for row in &mut result {
        let id = if let kv::Value::String(s) = &row[0] {
            s.clone()
        } else {
            panic!("doc_id must be String: {row:?}");
        };
        let count = match &row[2] {
            kv::Value::Int64(n) => *n,
            kv::Value::Int32(n) => *n as i64,
            kv::Value::UInt64(n) => *n as i64,
            other => panic!("shared_tag_count must be int: {other:?}"),
        };
        returned.push((id, count));
    }
    // Top results should be d-b/c/d with shared_tag_count=2.
    for top in ["d-b", "d-c", "d-d"] {
        assert!(
            returned.iter().any(|(id, c)| id == top && *c == 2),
            "doc `{top}` must surface with shared=2: {returned:?}",
        );
    }
    // Mid results: d-e/f/g with shared=1.
    for mid in ["d-e", "d-f", "d-g"] {
        assert!(
            returned.iter().any(|(id, c)| id == mid && *c == 1),
            "doc `{mid}` must surface with shared=1: {returned:?}",
        );
    }
    // Excluded: d-h/i/j/k (no meaningful overlap).
    for excluded in ["d-h", "d-i", "d-j", "d-k"] {
        assert!(
            !returned.iter().any(|(id, _)| id == excluded),
            "doc `{excluded}` must NOT surface (no meaningful overlap): {returned:?}",
        );
    }
    // Self-exclusion.
    assert!(
        !returned.iter().any(|(id, _)| id == "d-a"),
        "seed doc must self-exclude: {returned:?}",
    );
    // Order check: shared_tag_count DESC.
    for window in returned.windows(2) {
        assert!(
            window[0].1 >= window[1].1,
            "must order by shared_tag_count DESC: {returned:?}",
        );
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn tag_cooccurrence_ranks_pairs_by_shared_doc_count() {
    // Per iter 245 (Carpineto global-AQE operationalisation):
    // tag pair co-occurrence. Reuse the tag-axis-coverage
    // fixture which has:
    //   load-bearing-tag + mid-family-tag on d-a/b/c/d (4 co)
    //   load-bearing-tag + small-family-tag on d-e/f (2 co)
    //   load-bearing-tag + unique-singleton on d-g (1 co —
    //     filtered by the >=2 floor)
    //   mid-family-tag + small-family-tag never coexist (0)
    // Expected rows:
    //   (load-bearing-tag, mid-family-tag, 4) → top
    //   (load-bearing-tag, small-family-tag, 2)
    //   `research` stopword filtered.
    //   iter-* tags absent so the STARTS WITH filter is a
    //   no-op on this fixture.
    let dir = std::env::temp_dir().join(format!(
        "doc-linter-saved-tag-cooccur-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let db = testkit::open_db(&dir);
    {
        let conn = testkit::connect(&db);
        testkit::reset(&conn);
    }
    let conn = testkit::connect(&db);
    seed_tag_axis_coverage_fixture(&conn);

    let q = SAVED_QUERIES
        .iter()
        .find(|q| q.name == "tag-cooccurrence")
        .expect("tag-cooccurrence must exist");
    let mut result = conn.query(sql_of(q.name)).expect("query runs");
    let mut returned: Vec<(String, String, i64)> = Vec::new();
    for row in &mut result {
        let a = if let kv::Value::String(s) = &row[0] {
            s.clone()
        } else {
            panic!("tag_a must be String: {row:?}");
        };
        let b = if let kv::Value::String(s) = &row[1] {
            s.clone()
        } else {
            panic!("tag_b must be String: {row:?}");
        };
        let count = match &row[2] {
            kv::Value::Int64(n) => *n,
            kv::Value::Int32(n) => *n as i64,
            kv::Value::UInt64(n) => *n as i64,
            other => panic!("cooccur_count must be int: {other:?}"),
        };
        returned.push((a, b, count));
    }
    assert!(
        returned
            .iter()
            .any(|(a, b, c)| a == "load-bearing-tag" && b == "mid-family-tag" && *c == 4),
        "load-bearing-tag + mid-family-tag must co-occur 4×: {returned:?}",
    );
    assert!(
        returned
            .iter()
            .any(|(a, b, c)| a == "load-bearing-tag" && b == "small-family-tag" && *c == 2),
        "load-bearing-tag + small-family-tag must co-occur 2×: {returned:?}",
    );
    // Singleton co-occurrence (1×) below the >= 2 floor.
    assert!(
        !returned.iter().any(|(_, b, _)| b == "unique-singleton"),
        "single-occurrence pair must NOT surface: {returned:?}",
    );
    // Stopword filter.
    assert!(
        !returned
            .iter()
            .any(|(a, b, _)| a == "research" || b == "research"),
        "research stopword must NOT appear in any pair: {returned:?}",
    );
    // Order check: cooccur_count DESC.
    for window in returned.windows(2) {
        assert!(
            window[0].2 >= window[1].2,
            "must order by cooccur_count DESC: {returned:?}",
        );
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn entity_cooccurrence_ranks_entity_pairs_by_co_cover_count() {
    // Per iter 245 (Carpineto external-AQE sibling at the
    // entity axis): pairs of Entities co-covered by the
    // same Doc. Fixture: 3 entities (e-x, e-y, e-z) with
    // co-cover pattern:
    //   d-1 COVERS e-x + e-y
    //   d-2 COVERS e-x + e-y
    //   d-3 COVERS e-x + e-y + e-z
    //   d-4 COVERS e-x + e-z   (singleton e-x/e-z pair so
    //                            far + a 2nd via d-3 → 2 total)
    // Expected:
    //   (e-x, e-y, 3 co-covers from d-1/2/3) → top
    //   (e-x, e-z, 2 co-covers from d-3/4)
    //   (e-y, e-z, 1 co-cover from d-3 — below ≥ 2 floor)
    let dir = std::env::temp_dir().join(format!(
        "doc-linter-saved-entity-cooccur-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let db = testkit::open_db(&dir);
    {
        let conn = testkit::connect(&db);
        testkit::reset(&conn);
    }
    let conn = testkit::connect(&db);
    // Seed Entities.
    for id in ["e-x", "e-y", "e-z"] {
        conn.query(&format!(
            "CREATE (:Entity {{id: '{id}', display: '{id}', \
                 description: '', synonyms: [], \
                 bounded_contexts: [], scanner_coverage: [], \
                 source_modules: [], mention_count: 0, \
                 is_god_node: false, entity_class: 'concept', \
                 attributes: []}})"
        ))
        .expect("seed Entity");
    }
    // Seed Docs.
    for (id, covers) in [
        ("d-1", &["e-x", "e-y"][..]),
        ("d-2", &["e-x", "e-y"][..]),
        ("d-3", &["e-x", "e-y", "e-z"][..]),
        ("d-4", &["e-x", "e-z"][..]),
    ] {
        conn.query(&format!(
            "CREATE (:Doc {{id: '{id}', path: '{id}.md', role: 'doc', \
                 kind: 'reference', lifecycle: '', bounded_context: '', \
                 title: 'T', summary: '', status: 'stable', updated: \
                 '2026-06-01', tags: [], covers: []}})"
        ))
        .expect("seed Doc");
        for cover in covers {
            conn.query(&format!(
                "MATCH (d:Doc {{id: '{id}'}}), (e:Entity {{id: '{cover}'}}) \
                     CREATE (d)-[:COVERS {{line: 1}}]->(e)"
            ))
            .expect("seed COVERS");
        }
    }

    let q = SAVED_QUERIES
        .iter()
        .find(|q| q.name == "entity-cooccurrence")
        .expect("entity-cooccurrence must exist");
    let mut result = conn.query(sql_of(q.name)).expect("query runs");
    let mut returned: Vec<(String, String, i64)> = Vec::new();
    for row in &mut result {
        let a = if let kv::Value::String(s) = &row[0] {
            s.clone()
        } else {
            panic!("entity_a must be String: {row:?}");
        };
        let b = if let kv::Value::String(s) = &row[1] {
            s.clone()
        } else {
            panic!("entity_b must be String: {row:?}");
        };
        let count = match &row[2] {
            kv::Value::Int64(n) => *n,
            kv::Value::Int32(n) => *n as i64,
            kv::Value::UInt64(n) => *n as i64,
            other => panic!("cooccur_count must be int: {other:?}"),
        };
        returned.push((a, b, count));
    }
    assert!(
        returned
            .iter()
            .any(|(a, b, c)| a == "e-x" && b == "e-y" && *c == 3),
        "e-x + e-y must co-cover 3×: {returned:?}",
    );
    assert!(
        returned
            .iter()
            .any(|(a, b, c)| a == "e-x" && b == "e-z" && *c == 2),
        "e-x + e-z must co-cover 2×: {returned:?}",
    );
    // (e-y, e-z) only on d-3 → below >= 2 floor.
    assert!(
        !returned.iter().any(|(a, b, _)| a == "e-y" && b == "e-z"),
        "single-doc co-cover must NOT surface: {returned:?}",
    );
    for window in returned.windows(2) {
        assert!(
            window[0].2 >= window[1].2,
            "must order by cooccur_count DESC: {returned:?}",
        );
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn docs_without_family_tag_surfaces_stopword_only_and_empty_tag_docs() {
    // Per iter 247 (user-probe-060 Finding B): docs whose
    // tags contain ZERO non-stopword entries. Fixture:
    //   d-stopword-only — ['tool', 'production'] → loose-end (all stopwords)
    //   d-empty-tags   — []                       → loose-end (empty)
    //   d-mixed        — ['tool', 'production', 'graph-similarity']
    //                                              → NOT loose-end (has discriminative)
    //   d-iter-only    — ['iter-247-validation']  → loose-end (iter-* filtered)
    //   d-audit-only   — ['audit']                → loose-end (audit-tag filtered)
    //   d-discriminative — ['vocabulary-mismatch'] → NOT loose-end
    // Expected: surfaces d-stopword-only + d-empty-tags +
    // d-iter-only + d-audit-only (4 rows); excludes d-mixed
    // + d-discriminative.
    let dir = std::env::temp_dir().join(format!(
        "doc-linter-saved-docs-without-family-tag-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let db = testkit::open_db(&dir);
    {
        let conn = testkit::connect(&db);
        testkit::reset(&conn);
    }
    let conn = testkit::connect(&db);
    // Per iter 251 (closes [[interrogation-052]] Finding C):
    // historical-record IDs (audit-run-* + feature-*) must
    // be excluded even when they have only the bucket-tag —
    // the loop's prior self-referential set is DO-NOT-TOUCH
    // per loop spec; they aren't authored against the
    // current tagging convention.
    for (id, tags) in [
        ("d-stopword-only", "['tool', 'production']"),
        ("d-empty-tags", "[]"),
        ("d-mixed", "['tool', 'production', 'graph-similarity']"),
        ("d-iter-only", "['iter-247-validation']"),
        ("d-audit-only", "['audit']"),
        ("d-discriminative", "['vocabulary-mismatch']"),
        ("audit-run-001", "['audit']"),
        ("feature-foo", "['feature']"),
    ] {
        conn.query(&format!(
            "CREATE (:Doc {{id: '{id}', path: '{id}.md', role: 'doc', \
                 kind: 'reference', lifecycle: '', bounded_context: '', \
                 title: 'T', summary: '', status: 'stable', updated: \
                 '2026-06-01', tags: {tags}, covers: []}})"
        ))
        .expect("seed Doc");
    }

    let q = SAVED_QUERIES
        .iter()
        .find(|q| q.name == "docs-without-family-tag")
        .expect("docs-without-family-tag must exist");
    let mut result = conn.query(sql_of(q.name)).expect("query runs");
    let mut returned: Vec<String> = Vec::new();
    for row in &mut result {
        if let kv::Value::String(s) = &row[0] {
            returned.push(s.clone());
        }
    }
    for surfaced in [
        "d-stopword-only",
        "d-empty-tags",
        "d-iter-only",
        "d-audit-only",
    ] {
        assert!(
            returned.contains(&surfaced.to_string()),
            "{surfaced} must surface (no discriminative tag): {returned:?}",
        );
    }
    for excluded in ["d-mixed", "d-discriminative"] {
        assert!(
            !returned.contains(&excluded.to_string()),
            "{excluded} must NOT surface (carries discriminative tag): {returned:?}",
        );
    }
    // Per iter 251: historical-record exclusion. Per
    // interrogation-052 Finding C, audit-run-* + feature-*
    // are DO-NOT-TOUCH historical sets.
    for historical in ["audit-run-001", "feature-foo"] {
        assert!(
            !returned.contains(&historical.to_string()),
            "{historical} must NOT surface (historical-record exclusion): {returned:?}",
        );
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn docs_via_shared_entity_coverage_ranks_by_shared_entity_count() {
    // Per iter 251 (operationalises [[research-rocchio-
    // relevance-feedback]]'s local-AQE signal at entity
    // axis): given a seed Doc, find OTHER Docs covering the
    // most overlapping Entities. Fixture:
    //   3 entities: e-x, e-y, e-z
    //   seed-doc COVERS e-x + e-y + e-z (3 covered)
    //   d-near  COVERS e-x + e-y (2 shared)
    //   d-mid   COVERS e-x + e-w (1 shared with seed via e-x;
    //                             e-w not covered by seed)
    //   d-far   COVERS e-w only (0 shared with seed)
    // Expected: d-near (2 shared) > d-mid (1 shared) > [d-far
    // excluded — no shared entity]; seed self-excluded.
    let dir = std::env::temp_dir().join(format!(
        "doc-linter-saved-docs-via-shared-entity-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let db = testkit::open_db(&dir);
    {
        let conn = testkit::connect(&db);
        testkit::reset(&conn);
    }
    let conn = testkit::connect(&db);
    for id in ["e-x", "e-y", "e-z", "e-w"] {
        conn.query(&format!(
            "CREATE (:Entity {{id: '{id}', display: '{id}', \
                 description: '', synonyms: [], \
                 bounded_contexts: [], scanner_coverage: [], \
                 source_modules: [], mention_count: 0, \
                 is_god_node: false, entity_class: 'concept', \
                 attributes: []}})"
        ))
        .expect("seed Entity");
    }
    for (id, covers) in [
        ("seed-doc", &["e-x", "e-y", "e-z"][..]),
        ("d-near", &["e-x", "e-y"][..]),
        ("d-mid", &["e-x", "e-w"][..]),
        ("d-far", &["e-w"][..]),
    ] {
        conn.query(&format!(
            "CREATE (:Doc {{id: '{id}', path: '{id}.md', role: 'doc', \
                 kind: 'reference', lifecycle: '', bounded_context: '', \
                 title: '{id}', summary: '', status: 'stable', updated: \
                 '2026-06-01', tags: [], covers: []}})"
        ))
        .expect("seed Doc");
        for cover in covers {
            conn.query(&format!(
                "MATCH (d:Doc {{id: '{id}'}}), (e:Entity {{id: '{cover}'}}) \
                     CREATE (d)-[:COVERS {{line: 1}}]->(e)"
            ))
            .expect("seed COVERS");
        }
    }

    let q = SAVED_QUERIES
        .iter()
        .find(|q| q.name == "docs-via-shared-entity-coverage")
        .expect("docs-via-shared-entity-coverage must exist");
    let cypher = sql_of(q.name).replace("$doc_id", "'seed-doc'");
    let mut result = conn.query(&cypher).expect("query runs");
    let mut returned: Vec<(String, i64)> = Vec::new();
    for row in &mut result {
        let id = if let kv::Value::String(s) = &row[0] {
            s.clone()
        } else {
            panic!("doc_id must be String: {row:?}");
        };
        let count = match &row[3] {
            kv::Value::Int64(n) => *n,
            kv::Value::Int32(n) => *n as i64,
            kv::Value::UInt64(n) => *n as i64,
            other => panic!("shared_count must be int: {other:?}"),
        };
        returned.push((id, count));
    }
    assert!(
        returned.iter().any(|(id, c)| id == "d-near" && *c == 2),
        "d-near must surface with shared_count=2: {returned:?}",
    );
    assert!(
        returned.iter().any(|(id, c)| id == "d-mid" && *c == 1),
        "d-mid must surface with shared_count=1: {returned:?}",
    );
    assert!(
        !returned.iter().any(|(id, _)| id == "d-far"),
        "d-far must NOT surface (no shared entity): {returned:?}",
    );
    assert!(
        !returned.iter().any(|(id, _)| id == "seed-doc"),
        "seed must self-exclude: {returned:?}",
    );
    // Order check: shared_count DESC.
    for window in returned.windows(2) {
        assert!(
            window[0].1 >= window[1].1,
            "must order by shared_count DESC: {returned:?}",
        );
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn docs_via_shared_entity_coverage_filters_hub_entities_via_idf() {
    // Per iter 253 (closes [[user-probe-061]] Finding A
    // operationalising [[research-voorhees-wordnet-ir]]'s
    // external-AQE entity-discriminativeness): the ≤7-
    // coverer filter must drop HUB entities from the shared-
    // entity count, leaving only DISCRIMINATIVE entities.
    // Fixture:
    //   e-hub covered by 9 docs (above threshold — must be filtered)
    //   e-a + e-b each covered by 2 docs (discriminative)
    //   seed-doc covers all 3
    //   d-near-disc covers e-hub + e-a (1 discriminative shared)
    //   d-disc covers e-hub + e-b (1 discriminative shared)
    //   d-hub-only covers e-hub only (0 discriminative shared — excluded)
    //   6 d-noise-N each cover e-hub only (boost e-hub's coverer count)
    // Expected: d-near-disc (1) + d-disc (1) surface; d-hub-only
    // excluded because the only shared entity is filtered as
    // non-discriminative.
    let dir = std::env::temp_dir().join(format!(
        "doc-linter-saved-docs-via-shared-entity-idf-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let db = testkit::open_db(&dir);
    {
        let conn = testkit::connect(&db);
        testkit::reset(&conn);
    }
    let conn = testkit::connect(&db);
    for id in ["e-hub", "e-a", "e-b"] {
        conn.query(&format!(
            "CREATE (:Entity {{id: '{id}', display: '{id}', \
                 description: '', synonyms: [], \
                 bounded_contexts: [], scanner_coverage: [], \
                 source_modules: [], mention_count: 0, \
                 is_god_node: false, entity_class: 'concept', \
                 attributes: []}})"
        ))
        .expect("seed Entity");
    }
    // Real docs sharing discriminative entities with the seed.
    for (id, covers) in [
        ("seed-doc", &["e-hub", "e-a", "e-b"][..]),
        ("d-near-disc", &["e-hub", "e-a"][..]),
        ("d-disc", &["e-hub", "e-b"][..]),
        ("d-hub-only", &["e-hub"][..]),
    ] {
        conn.query(&format!(
            "CREATE (:Doc {{id: '{id}', path: '{id}.md', role: 'doc', \
                 kind: 'reference', lifecycle: '', bounded_context: '', \
                 title: '{id}', summary: '', status: 'stable', updated: \
                 '2026-06-01', tags: [], covers: []}})"
        ))
        .expect("seed Doc");
        for cover in covers {
            conn.query(&format!(
                "MATCH (d:Doc {{id: '{id}'}}), (e:Entity {{id: '{cover}'}}) \
                     CREATE (d)-[:COVERS {{line: 1}}]->(e)"
            ))
            .expect("seed COVERS");
        }
    }
    // 5 noise docs covering e-hub only — pushes e-hub's
    // total coverer count to 4 (seed) + 1 (near-disc) + 1
    // (disc) + 1 (hub-only) + 5 (noise) = 9 > 7 threshold.
    for i in 0..5 {
        let id = format!("d-noise-{i}");
        conn.query(&format!(
            "CREATE (:Doc {{id: '{id}', path: '{id}.md', role: 'doc', \
                 kind: 'reference', lifecycle: '', bounded_context: '', \
                 title: '{id}', summary: '', status: 'stable', updated: \
                 '2026-06-01', tags: [], covers: []}})"
        ))
        .expect("seed Doc");
        conn.query(&format!(
            "MATCH (d:Doc {{id: '{id}'}}), (e:Entity {{id: 'e-hub'}}) \
                 CREATE (d)-[:COVERS {{line: 1}}]->(e)"
        ))
        .expect("seed COVERS");
    }

    let q = SAVED_QUERIES
        .iter()
        .find(|q| q.name == "docs-via-shared-entity-coverage")
        .expect("docs-via-shared-entity-coverage must exist");
    let cypher = sql_of(q.name).replace("$doc_id", "'seed-doc'");
    let mut result = conn.query(&cypher).expect("query runs");
    let mut returned: Vec<(String, i64)> = Vec::new();
    for row in &mut result {
        let id = if let kv::Value::String(s) = &row[0] {
            s.clone()
        } else {
            panic!("doc_id must be String: {row:?}");
        };
        let count = match &row[3] {
            kv::Value::Int64(n) => *n,
            kv::Value::Int32(n) => *n as i64,
            kv::Value::UInt64(n) => *n as i64,
            other => panic!("shared_count must be int: {other:?}"),
        };
        returned.push((id, count));
    }
    assert!(
        returned
            .iter()
            .any(|(id, c)| id == "d-near-disc" && *c == 1),
        "d-near-disc must surface with shared_count=1 (e-a only): {returned:?}",
    );
    assert!(
        returned.iter().any(|(id, c)| id == "d-disc" && *c == 1),
        "d-disc must surface with shared_count=1 (e-b only): {returned:?}",
    );
    assert!(
            !returned.iter().any(|(id, _)| id == "d-hub-only"),
            "d-hub-only must NOT surface — its only shared entity (e-hub) is filtered as non-discriminative: {returned:?}",
        );
    // Noise docs cover only e-hub which is filtered; they must
    // not surface.
    for i in 0..5 {
        let noise_id = format!("d-noise-{i}");
        assert!(
            !returned.iter().any(|(id, _)| id == &noise_id),
            "{noise_id} must NOT surface — covers only filtered hub: {returned:?}",
        );
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn discriminative_entities_ranks_by_inverse_doc_coverage() {
    // Per iter 253 (sibling to docs-via-shared-entity-
    // coverage): entities ranked by ASC coverer-count,
    // filtered to ≤ 7 covers. Reuses the same noise
    // pattern: e-hub at 9 covers (filtered), e-a at 2,
    // e-b at 2, e-c at 1. Expected return: e-c (1), e-a
    // (2), e-b (2) in that order; e-hub NOT returned.
    let dir = std::env::temp_dir().join(format!(
        "doc-linter-saved-discriminative-entities-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let db = testkit::open_db(&dir);
    {
        let conn = testkit::connect(&db);
        testkit::reset(&conn);
    }
    let conn = testkit::connect(&db);
    for id in ["e-hub", "e-a", "e-b", "e-c"] {
        conn.query(&format!(
            "CREATE (:Entity {{id: '{id}', display: 'D-{id}', \
                 description: '', synonyms: [], \
                 bounded_contexts: [], scanner_coverage: [], \
                 source_modules: [], mention_count: 0, \
                 is_god_node: false, entity_class: 'concept', \
                 attributes: []}})"
        ))
        .expect("seed Entity");
    }
    // 9 docs cover e-hub (above threshold).
    for i in 0..9 {
        let id = format!("d-hub-{i}");
        conn.query(&format!(
            "CREATE (:Doc {{id: '{id}', path: '{id}.md', role: 'doc', \
                 kind: 'reference', lifecycle: '', bounded_context: '', \
                 title: '{id}', summary: '', status: 'stable', updated: \
                 '2026-06-01', tags: [], covers: []}})"
        ))
        .expect("seed Doc");
        conn.query(&format!(
            "MATCH (d:Doc {{id: '{id}'}}), (e:Entity {{id: 'e-hub'}}) \
                 CREATE (d)-[:COVERS {{line: 1}}]->(e)"
        ))
        .expect("seed COVERS");
    }
    // 2 docs cover e-a + 2 docs cover e-b + 1 doc covers e-c.
    for (id, target) in [
        ("d-a-1", "e-a"),
        ("d-a-2", "e-a"),
        ("d-b-1", "e-b"),
        ("d-b-2", "e-b"),
        ("d-c-1", "e-c"),
    ] {
        conn.query(&format!(
            "CREATE (:Doc {{id: '{id}', path: '{id}.md', role: 'doc', \
                 kind: 'reference', lifecycle: '', bounded_context: '', \
                 title: '{id}', summary: '', status: 'stable', updated: \
                 '2026-06-01', tags: [], covers: []}})"
        ))
        .expect("seed Doc");
        conn.query(&format!(
            "MATCH (d:Doc {{id: '{id}'}}), (e:Entity {{id: '{target}'}}) \
                 CREATE (d)-[:COVERS {{line: 1}}]->(e)"
        ))
        .expect("seed COVERS");
    }

    let q = SAVED_QUERIES
        .iter()
        .find(|q| q.name == "discriminative-entities")
        .expect("discriminative-entities must exist");
    let mut result = conn.query(sql_of(q.name)).expect("query runs");
    let mut returned: Vec<(String, i64)> = Vec::new();
    for row in &mut result {
        let id = if let kv::Value::String(s) = &row[0] {
            s.clone()
        } else {
            panic!("entity_id must be String: {row:?}");
        };
        let count = match &row[2] {
            kv::Value::Int64(n) => *n,
            kv::Value::Int32(n) => *n as i64,
            kv::Value::UInt64(n) => *n as i64,
            other => panic!("coverer_count must be int: {other:?}"),
        };
        returned.push((id, count));
    }
    assert!(
        returned.iter().any(|(id, c)| id == "e-c" && *c == 1),
        "e-c must surface at coverer_count=1: {returned:?}",
    );
    assert!(
        returned.iter().any(|(id, c)| id == "e-a" && *c == 2),
        "e-a must surface at coverer_count=2: {returned:?}",
    );
    assert!(
        returned.iter().any(|(id, c)| id == "e-b" && *c == 2),
        "e-b must surface at coverer_count=2: {returned:?}",
    );
    assert!(
        !returned.iter().any(|(id, _)| id == "e-hub"),
        "e-hub must NOT surface — 9 covers above threshold: {returned:?}",
    );
    // First row must have the smallest coverer_count.
    assert_eq!(
        returned[0].0, "e-c",
        "lowest-cover entity must rank first: {returned:?}"
    );
    // Order check: coverer_count ASC.
    for window in returned.windows(2) {
        assert!(
            window[0].1 <= window[1].1,
            "must order by coverer_count ASC: {returned:?}",
        );
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn entities_via_shared_doc_coverage_ranks_by_shared_doc_count() {
    // Per iter 257 (entity-axis sibling of iter-251 docs-
    // via-shared-entity-coverage): given a seed Entity,
    // return OTHER Entities co-covered by the same docs.
    // Fixture:
    //   seed-entity covered by d-1 + d-2 + d-3
    //   e-near covered by d-1 + d-2 (2 shared docs with seed)
    //   e-mid covered by d-2 + d-other (1 shared doc)
    //   e-far covered by d-other only (0 shared docs)
    // Expected: e-near (2) > e-mid (1); e-far excluded; self-
    // exclude.
    let dir = std::env::temp_dir().join(format!(
        "doc-linter-saved-entities-via-shared-doc-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let db = testkit::open_db(&dir);
    {
        let conn = testkit::connect(&db);
        testkit::reset(&conn);
    }
    let conn = testkit::connect(&db);
    for id in ["seed-entity", "e-near", "e-mid", "e-far"] {
        conn.query(&format!(
            "CREATE (:Entity {{id: '{id}', display: '{id}', \
                 description: '', synonyms: [], \
                 bounded_contexts: [], scanner_coverage: [], \
                 source_modules: [], mention_count: 0, \
                 is_god_node: false, entity_class: 'concept', \
                 attributes: []}})"
        ))
        .expect("seed Entity");
    }
    for (id, covered_entities) in [
        ("d-1", &["seed-entity", "e-near"][..]),
        ("d-2", &["seed-entity", "e-near", "e-mid"][..]),
        ("d-3", &["seed-entity"][..]),
        ("d-other", &["e-mid", "e-far"][..]),
    ] {
        conn.query(&format!(
            "CREATE (:Doc {{id: '{id}', path: '{id}.md', role: 'doc', \
                 kind: 'reference', lifecycle: '', bounded_context: '', \
                 title: '{id}', summary: '', status: 'stable', updated: \
                 '2026-06-01', tags: [], covers: []}})"
        ))
        .expect("seed Doc");
        for e in covered_entities {
            conn.query(&format!(
                "MATCH (d:Doc {{id: '{id}'}}), (ent:Entity {{id: '{e}'}}) \
                     CREATE (d)-[:COVERS {{line: 1}}]->(ent)"
            ))
            .expect("seed COVERS");
        }
    }

    let q = SAVED_QUERIES
        .iter()
        .find(|q| q.name == "entities-via-shared-doc-coverage")
        .expect("entities-via-shared-doc-coverage must exist");
    let cypher = sql_of(q.name).replace("$entity_id", "'seed-entity'");
    let mut result = conn.query(&cypher).expect("query runs");
    let mut returned: Vec<(String, i64)> = Vec::new();
    for row in &mut result {
        let id = if let kv::Value::String(s) = &row[0] {
            s.clone()
        } else {
            panic!("entity_id must be String: {row:?}");
        };
        let count = match &row[2] {
            kv::Value::Int64(n) => *n,
            kv::Value::Int32(n) => *n as i64,
            kv::Value::UInt64(n) => *n as i64,
            other => panic!("shared_doc_count must be int: {other:?}"),
        };
        returned.push((id, count));
    }
    assert!(
        returned.iter().any(|(id, c)| id == "e-near" && *c == 2),
        "e-near must surface at shared_doc_count=2: {returned:?}",
    );
    assert!(
        returned.iter().any(|(id, c)| id == "e-mid" && *c == 1),
        "e-mid must surface at shared_doc_count=1: {returned:?}",
    );
    assert!(
        !returned.iter().any(|(id, _)| id == "e-far"),
        "e-far must NOT surface (no shared doc): {returned:?}",
    );
    assert!(
        !returned.iter().any(|(id, _)| id == "seed-entity"),
        "seed-entity must self-exclude: {returned:?}",
    );
    // Order check: shared_doc_count DESC.
    for window in returned.windows(2) {
        assert!(
            window[0].1 >= window[1].1,
            "must order by shared_doc_count DESC: {returned:?}",
        );
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn entities_coupled_to_tag_ranks_by_tag_doc_coverage() {
    // Per iter 257 (cross-axis tag → entity bridge for
    // Voorhees external-AQE): given a tag, find entities
    // most frequently covered by docs carrying that tag.
    // Fixture:
    //   3 docs with tag 'family-x' covering different entities:
    //     d-a: e-hot + e-warm
    //     d-b: e-hot + e-warm
    //     d-c: e-hot + e-cold
    //   1 doc WITHOUT 'family-x' covering e-other (not counted)
    // Expected: e-hot (3 family-x docs) > e-warm (2) > e-cold (1);
    // e-other excluded.
    let dir = std::env::temp_dir().join(format!(
        "doc-linter-saved-entities-coupled-to-tag-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let db = testkit::open_db(&dir);
    {
        let conn = testkit::connect(&db);
        testkit::reset(&conn);
    }
    let conn = testkit::connect(&db);
    for id in ["e-hot", "e-warm", "e-cold", "e-other"] {
        conn.query(&format!(
            "CREATE (:Entity {{id: '{id}', display: '{id}', \
                 description: '', synonyms: [], \
                 bounded_contexts: [], scanner_coverage: [], \
                 source_modules: [], mention_count: 0, \
                 is_god_node: false, entity_class: 'concept', \
                 attributes: []}})"
        ))
        .expect("seed Entity");
    }
    for (id, tags, covered_entities) in [
        ("d-a", "['family-x']", &["e-hot", "e-warm"][..]),
        ("d-b", "['family-x']", &["e-hot", "e-warm"][..]),
        ("d-c", "['family-x']", &["e-hot", "e-cold"][..]),
        ("d-noise", "['other-tag']", &["e-other"][..]),
    ] {
        conn.query(&format!(
            "CREATE (:Doc {{id: '{id}', path: '{id}.md', role: 'doc', \
                 kind: 'reference', lifecycle: '', bounded_context: '', \
                 title: '{id}', summary: '', status: 'stable', updated: \
                 '2026-06-01', tags: {tags}, covers: []}})"
        ))
        .expect("seed Doc");
        for e in covered_entities {
            conn.query(&format!(
                "MATCH (d:Doc {{id: '{id}'}}), (ent:Entity {{id: '{e}'}}) \
                     CREATE (d)-[:COVERS {{line: 1}}]->(ent)"
            ))
            .expect("seed COVERS");
        }
    }

    let q = SAVED_QUERIES
        .iter()
        .find(|q| q.name == "entities-coupled-to-tag")
        .expect("entities-coupled-to-tag must exist");
    let cypher = sql_of(q.name).replace("$tag", "'family-x'");
    let mut result = conn.query(&cypher).expect("query runs");
    let mut returned: Vec<(String, i64)> = Vec::new();
    for row in &mut result {
        let id = if let kv::Value::String(s) = &row[0] {
            s.clone()
        } else {
            panic!("entity_id must be String: {row:?}");
        };
        let count = match &row[2] {
            kv::Value::Int64(n) => *n,
            kv::Value::Int32(n) => *n as i64,
            kv::Value::UInt64(n) => *n as i64,
            other => panic!("tag_doc_count must be int: {other:?}"),
        };
        returned.push((id, count));
    }
    assert!(
        returned.iter().any(|(id, c)| id == "e-hot" && *c == 3),
        "e-hot must surface at tag_doc_count=3: {returned:?}",
    );
    assert!(
        returned.iter().any(|(id, c)| id == "e-warm" && *c == 2),
        "e-warm must surface at tag_doc_count=2: {returned:?}",
    );
    assert!(
        returned.iter().any(|(id, c)| id == "e-cold" && *c == 1),
        "e-cold must surface at tag_doc_count=1: {returned:?}",
    );
    assert!(
        !returned.iter().any(|(id, _)| id == "e-other"),
        "e-other must NOT surface (doc carries wrong tag): {returned:?}",
    );
    // Order: tag_doc_count DESC.
    for window in returned.windows(2) {
        assert!(
            window[0].1 >= window[1].1,
            "must order by tag_doc_count DESC: {returned:?}",
        );
    }
    let _ = std::fs::remove_dir_all(&dir);
}

fn seed_slice_file_axis_fixture(conn: &testkit::Conn<'_>) {
    // Per iter 231 (user-probe-055 Finding D): fixture
    // exercising file-axis blast-radius aggregation.
    // - target: the function being analyzed
    // - callers a/b/c: in fileA (3 callers → expect 3 in file_a)
    // - callers d/e: in fileB (2 callers → expect 2 in file_b)
    // - caller f: in fileC (1 caller → expect 1 in file_c)
    // - callees x/y/z: in fileD (3 callees → expect 3)
    // Reuses the basic Function seeding.
    for (sym, file) in [
        ("target()", "src/target.rs"),
        ("caller-a()", "src/fileA.rs"),
        ("caller-b()", "src/fileA.rs"),
        ("caller-c()", "src/fileA.rs"),
        ("caller-d()", "src/fileB.rs"),
        ("caller-e()", "src/fileB.rs"),
        ("caller-f()", "src/fileC.rs"),
        ("callee-x()", "src/fileD.rs"),
        ("callee-y()", "src/fileD.rs"),
        ("callee-z()", "src/fileD.rs"),
    ] {
        let cypher = format!(
            "CREATE (:Function {{symbol: '{sym}', crate: 'd', file: '{file}', \
                 line: 1, doc_comment: '', language: 'rust', signature: '', \
                 body_excerpt: '', last_touched: '2026-06-01', repo_id: 'd'}})"
        );
        conn.query(&cypher).expect("seed Function");
    }
    // CALLS: each of caller-a/b/c/d/e/f → target.
    // Plus target → callee-x/y/z.
    for (src, dst) in [
        ("caller-a()", "target()"),
        ("caller-b()", "target()"),
        ("caller-c()", "target()"),
        ("caller-d()", "target()"),
        ("caller-e()", "target()"),
        ("caller-f()", "target()"),
        ("target()", "callee-x()"),
        ("target()", "callee-y()"),
        ("target()", "callee-z()"),
    ] {
        let cypher = format!(
            "MATCH (s:Function {{symbol: '{src}'}}), \
                       (t:Function {{symbol: '{dst}'}}) \
                 CREATE (s)-[:CALLS {{source_file: '?', source_line: 5}}]->(t)"
        );
        conn.query(&cypher).expect("seed CALLS");
    }
}

#[test]
fn function_blast_radius_by_file_aggregates_by_caller_file() {
    // Per iter 231 (user-probe-055 Finding D): the
    // file-axis aggregation must return 3 rows on the
    // fixture — fileA (3 callers), fileB (2), fileC (1).
    let dir = std::env::temp_dir().join(format!(
        "doc-linter-saved-blast-radius-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let db = testkit::open_db(&dir);
    {
        let conn = testkit::connect(&db);
        testkit::reset(&conn);
    }
    let conn = testkit::connect(&db);
    seed_slice_file_axis_fixture(&conn);

    let q = SAVED_QUERIES
        .iter()
        .find(|q| q.name == "function-blast-radius-by-file")
        .expect("query must exist");
    let cypher = sql_of(q.name).replace("$symbol", "'target()'");
    let mut result = conn.query(&cypher).expect("query runs");
    let mut by_file: std::collections::HashMap<String, i64> = std::collections::HashMap::new();
    for row in &mut result {
        let file = if let kv::Value::String(s) = &row[0] {
            s.clone()
        } else {
            panic!("file String: {row:?}");
        };
        let count = match &row[1] {
            kv::Value::Int64(n) => *n,
            other => panic!("count int: {other:?}"),
        };
        by_file.insert(file, count);
    }
    assert_eq!(by_file.get("src/fileA.rs"), Some(&3));
    assert_eq!(by_file.get("src/fileB.rs"), Some(&2));
    assert_eq!(by_file.get("src/fileC.rs"), Some(&1));
    // fileD has CALLEES not callers — must NOT surface
    // in the backward direction.
    assert!(
        !by_file.contains_key("src/fileD.rs"),
        "fileD has callees not callers — must NOT surface: {by_file:?}",
    );
}

fn run_function_shape_summary_query(
    conn: &testkit::Conn<'_>,
    symbol: &str,
) -> (i64, i64, i64, i64, String) {
    let q = SAVED_QUERIES
        .iter()
        .find(|q| q.name == "function-shape-summary")
        .expect("query must exist");
    let cypher = sql_of(q.name).replace("$symbol", &format!("'{symbol}'"));
    let mut result = conn.query(&cypher).expect("query runs");
    let row = result.next().expect("1 row");
    let cc = match &row[0] {
        kv::Value::Int64(n) => *n,
        other => panic!("caller_count int: {other:?}"),
    };
    let cfc = match &row[1] {
        kv::Value::Int64(n) => *n,
        other => panic!("caller_file_count int: {other:?}"),
    };
    let ec = match &row[2] {
        kv::Value::Int64(n) => *n,
        other => panic!("callee_count int: {other:?}"),
    };
    let efc = match &row[3] {
        kv::Value::Int64(n) => *n,
        other => panic!("callee_file_count int: {other:?}"),
    };
    let shape = if let kv::Value::String(s) = &row[4] {
        s.clone()
    } else {
        panic!("shape String: {row:?}");
    };
    (cc, cfc, ec, efc, shape)
}

fn seed_function_shape_fixture(conn: &testkit::Conn<'_>) {
    // Per iter 233 (user-probe-056 Finding E) + iter 235
    // (user-probe-057 Finding B refinement adding 6th
    // wide-leaf-utility category):
    // - factory-target: 6 callers all in src/mcp.rs, 0 callees
    //   → pure-leaf-factory
    // - widely-used: 6 callers across src/W/X/Y.rs (≥2 files),
    //   0 callees → wide-leaf-utility (iter-235 NEW)
    // - entry-target: 6 callers across src/A/B/C/D/E/F.rs,
    //   1 callee → thin-entry-point
    // - hub-target: 6 callers in 1 file, 3 callees → subsystem-hub
    // - peripheral: 2 callers in 1 file, 0 callees → peripheral
    for (sym, file) in [
        ("factory-target()", "src/mcp.rs"),
        ("widely-used()", "src/projections.rs"),
        ("entry-target()", "src/parser.rs"),
        ("hub-target()", "src/util.rs"),
        ("peripheral()", "src/leaf.rs"),
        ("entry-callee()", "src/parser_helper.rs"),
        ("hub-callee-a()", "src/util_helper.rs"),
        ("hub-callee-b()", "src/util_helper.rs"),
        ("hub-callee-c()", "src/util_helper.rs"),
    ] {
        let cypher = format!(
            "CREATE (:Function {{symbol: '{sym}', crate: 'd', file: '{file}', \
                 line: 1, doc_comment: '', language: 'rust', signature: '', \
                 body_excerpt: '', last_touched: '2026-06-01', repo_id: 'd'}})"
        );
        conn.query(&cypher).expect("seed Function");
    }
    // Factory: 6 callers all in src/mcp.rs.
    for i in 0..6 {
        let sym = format!("factory-caller-{i}()");
        conn.query(&format!(
            "CREATE (:Function {{symbol: '{sym}', crate: 'd', file: 'src/mcp.rs', \
                 line: 1, doc_comment: '', language: 'rust', signature: '', \
                 body_excerpt: '', last_touched: '2026-06-01', repo_id: 'd'}})"
        ))
        .expect("seed factory caller");
        conn.query(&format!(
            "MATCH (s:Function {{symbol: '{sym}'}}), \
                       (t:Function {{symbol: 'factory-target()'}}) \
                 CREATE (s)-[:CALLS {{source_file: '?', source_line: 5}}]->(t)"
        ))
        .expect("seed CALLS");
    }
    // Entry: 6 callers across 6 distinct files.
    for (i, file) in [
        "src/A.rs", "src/B.rs", "src/C.rs", "src/D.rs", "src/E.rs", "src/F.rs",
    ]
    .iter()
    .enumerate()
    {
        let sym = format!("entry-caller-{i}()");
        conn.query(&format!(
            "CREATE (:Function {{symbol: '{sym}', crate: 'd', file: '{file}', \
                 line: 1, doc_comment: '', language: 'rust', signature: '', \
                 body_excerpt: '', last_touched: '2026-06-01', repo_id: 'd'}})"
        ))
        .expect("seed entry caller");
        conn.query(&format!(
            "MATCH (s:Function {{symbol: '{sym}'}}), \
                       (t:Function {{symbol: 'entry-target()'}}) \
                 CREATE (s)-[:CALLS {{source_file: '?', source_line: 5}}]->(t)"
        ))
        .expect("seed CALLS");
    }
    // Entry target → 1 callee.
    conn.query(
        "MATCH (s:Function {symbol: 'entry-target()'}), \
                   (t:Function {symbol: 'entry-callee()'}) \
             CREATE (s)-[:CALLS {source_file: '?', source_line: 5}]->(t)",
    )
    .expect("seed entry callee CALLS");
    // Hub: 6 callers all in src/util.rs (same file as hub-target)
    // — wait, callers must be DIFFERENT from hub-target which lives
    // in src/util.rs. Per the iter-228 query semantics, callers
    // exclude self-recursion. Place hub callers in src/util.rs
    // but their symbols differ from 'hub-target()'.
    for i in 0..6 {
        let sym = format!("hub-caller-{i}()");
        conn.query(&format!(
            "CREATE (:Function {{symbol: '{sym}', crate: 'd', file: 'src/util.rs', \
                 line: 1, doc_comment: '', language: 'rust', signature: '', \
                 body_excerpt: '', last_touched: '2026-06-01', repo_id: 'd'}})"
        ))
        .expect("seed hub caller");
        conn.query(&format!(
            "MATCH (s:Function {{symbol: '{sym}'}}), \
                       (t:Function {{symbol: 'hub-target()'}}) \
                 CREATE (s)-[:CALLS {{source_file: '?', source_line: 5}}]->(t)"
        ))
        .expect("seed CALLS");
    }
    // Hub target → 3 callees in src/util_helper.rs.
    for callee in ["hub-callee-a()", "hub-callee-b()", "hub-callee-c()"] {
        conn.query(&format!(
            "MATCH (s:Function {{symbol: 'hub-target()'}}), \
                       (t:Function {{symbol: '{callee}'}}) \
                 CREATE (s)-[:CALLS {{source_file: '?', source_line: 5}}]->(t)"
        ))
        .expect("seed hub callee CALLS");
    }
    // Wide-leaf-utility: 6 callers across 3 files, 0 callees.
    // Per iter 235 (user-probe-057 Finding B): val_string-shape
    // — widely used (multi-file) but calls nothing downstream.
    for (i, file) in [
        "src/W.rs", "src/W.rs", "src/X.rs", "src/X.rs", "src/Y.rs", "src/Y.rs",
    ]
    .iter()
    .enumerate()
    {
        let sym = format!("wide-caller-{i}()");
        conn.query(&format!(
            "CREATE (:Function {{symbol: '{sym}', crate: 'd', file: '{file}', \
                 line: 1, doc_comment: '', language: 'rust', signature: '', \
                 body_excerpt: '', last_touched: '2026-06-01', repo_id: 'd'}})"
        ))
        .expect("seed wide caller");
        conn.query(&format!(
            "MATCH (s:Function {{symbol: '{sym}'}}), \
                       (t:Function {{symbol: 'widely-used()'}}) \
                 CREATE (s)-[:CALLS {{source_file: '?', source_line: 5}}]->(t)"
        ))
        .expect("seed CALLS");
    }
    // Peripheral: 2 callers, 0 callees → peripheral.
    for i in 0..2 {
        let sym = format!("peri-caller-{i}()");
        conn.query(&format!(
            "CREATE (:Function {{symbol: '{sym}', crate: 'd', file: 'src/leaf.rs', \
                 line: 1, doc_comment: '', language: 'rust', signature: '', \
                 body_excerpt: '', last_touched: '2026-06-01', repo_id: 'd'}})"
        ))
        .expect("seed peripheral caller");
        conn.query(&format!(
            "MATCH (s:Function {{symbol: '{sym}'}}), \
                       (t:Function {{symbol: 'peripheral()'}}) \
                 CREATE (s)-[:CALLS {{source_file: '?', source_line: 5}}]->(t)"
        ))
        .expect("seed CALLS");
    }
    // Per iter 237 (user-probe-058 Finding D): test-fixture
    // target that should be EXCLUDED by functions-of-shape
    // + function-shape-distribution per the iter-218/225
    // tests/ + symbol-CONTAINS filter. Shape would be
    // wide-leaf-utility (6 callers across 3 files + 0
    // callees) but the iter-237 filter must drop it.
    conn.query(
        "CREATE (:Function {symbol: 'common/tests/test_helper()', crate: 'd', \
             file: 'tests/common/mod.rs', line: 1, doc_comment: '', language: 'rust', \
             signature: '', body_excerpt: '', last_touched: '2026-06-01', repo_id: 'd'})",
    )
    .expect("seed test-fixture target");
    for (i, file) in [
        "tests/A.rs",
        "tests/A.rs",
        "tests/B.rs",
        "tests/B.rs",
        "tests/C.rs",
        "tests/C.rs",
    ]
    .iter()
    .enumerate()
    {
        let sym = format!("test-caller-{i}()");
        conn.query(&format!(
            "CREATE (:Function {{symbol: '{sym}', crate: 'd', file: '{file}', \
                 line: 1, doc_comment: '', language: 'rust', signature: '', \
                 body_excerpt: '', last_touched: '2026-06-01', repo_id: 'd'}})"
        ))
        .expect("seed test caller");
        conn.query(&format!(
            "MATCH (s:Function {{symbol: '{sym}'}}), \
                       (t:Function {{symbol: 'common/tests/test_helper()'}}) \
                 CREATE (s)-[:CALLS {{source_file: '?', source_line: 5}}]->(t)"
        ))
        .expect("seed CALLS");
    }
}

#[test]
fn function_shape_summary_classifies_pure_leaf_factory() {
    let dir = std::env::temp_dir().join(format!(
        "doc-linter-saved-shape-factory-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let db = testkit::open_db(&dir);
    {
        let conn = testkit::connect(&db);
        testkit::reset(&conn);
    }
    let conn = testkit::connect(&db);
    seed_function_shape_fixture(&conn);
    let (cc, cfc, ec, efc, shape) = run_function_shape_summary_query(&conn, "factory-target()");
    assert_eq!(cc, 6, "6 callers");
    assert_eq!(cfc, 1, "1 caller file (src/mcp.rs)");
    assert_eq!(ec, 0, "0 callees");
    assert_eq!(efc, 0, "0 callee files");
    assert_eq!(shape, "pure-leaf-factory");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn function_shape_summary_classifies_thin_entry_point() {
    let dir = std::env::temp_dir().join(format!(
        "doc-linter-saved-shape-entry-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let db = testkit::open_db(&dir);
    {
        let conn = testkit::connect(&db);
        testkit::reset(&conn);
    }
    let conn = testkit::connect(&db);
    seed_function_shape_fixture(&conn);
    let (cc, cfc, ec, _efc, shape) = run_function_shape_summary_query(&conn, "entry-target()");
    assert_eq!(cc, 6, "6 callers");
    assert_eq!(cfc, 6, "6 caller files");
    assert_eq!(ec, 1, "1 callee");
    assert_eq!(shape, "thin-entry-point");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn function_shape_summary_classifies_subsystem_hub() {
    let dir = std::env::temp_dir().join(format!(
        "doc-linter-saved-shape-hub-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let db = testkit::open_db(&dir);
    {
        let conn = testkit::connect(&db);
        testkit::reset(&conn);
    }
    let conn = testkit::connect(&db);
    seed_function_shape_fixture(&conn);
    let (cc, cfc, ec, _efc, shape) = run_function_shape_summary_query(&conn, "hub-target()");
    assert_eq!(cc, 6, "6 callers");
    assert_eq!(cfc, 1, "1 caller file (src/util.rs)");
    assert_eq!(ec, 3, "3 callees");
    assert_eq!(shape, "subsystem-hub");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn function_shape_summary_classifies_wide_leaf_utility() {
    // Per iter 235 (user-probe-057 Finding B): 6 callers
    // across 3 files + 0 callees → wide-leaf-utility (the
    // NEW 6th category distinguishing val_string-shape from
    // thin-entry-point).
    let dir = std::env::temp_dir().join(format!(
        "doc-linter-saved-shape-wide-leaf-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let db = testkit::open_db(&dir);
    {
        let conn = testkit::connect(&db);
        testkit::reset(&conn);
    }
    let conn = testkit::connect(&db);
    seed_function_shape_fixture(&conn);
    let (cc, cfc, ec, _efc, shape) = run_function_shape_summary_query(&conn, "widely-used()");
    assert_eq!(cc, 6, "6 callers");
    assert_eq!(cfc, 3, "3 caller files (W + X + Y)");
    assert_eq!(ec, 0, "0 callees");
    assert_eq!(shape, "wide-leaf-utility");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn functions_of_shape_returns_inverse_matching_shape() {
    // Per iter 235 (companion to function-shape-summary):
    // given a shape label, return functions matching that
    // shape. Verify pure-leaf-factory returns
    // factory-target() (from fixture); wide-leaf-utility
    // returns widely-used().
    let dir = std::env::temp_dir().join(format!(
        "doc-linter-saved-functions-of-shape-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let db = testkit::open_db(&dir);
    {
        let conn = testkit::connect(&db);
        testkit::reset(&conn);
    }
    let conn = testkit::connect(&db);
    seed_function_shape_fixture(&conn);

    let q = SAVED_QUERIES
        .iter()
        .find(|q| q.name == "functions-of-shape")
        .expect("query must exist");
    // Query for pure-leaf-factory.
    let cypher = sql_of(q.name).replace("$shape_label", "'pure-leaf-factory'");
    let mut result = conn.query(&cypher).expect("query runs");
    let mut returned: Vec<String> = Vec::new();
    for row in &mut result {
        if let kv::Value::String(s) = &row[0] {
            returned.push(s.clone());
        }
    }
    assert!(
        returned.contains(&"factory-target()".to_string()),
        "pure-leaf-factory query must surface factory-target(): {returned:?}",
    );
    // Query for wide-leaf-utility.
    let cypher2 = sql_of(q.name).replace("$shape_label", "'wide-leaf-utility'");
    let mut result2 = conn.query(&cypher2).expect("query runs");
    let mut returned2: Vec<String> = Vec::new();
    for row in &mut result2 {
        if let kv::Value::String(s) = &row[0] {
            returned2.push(s.clone());
        }
    }
    assert!(
        returned2.contains(&"widely-used()".to_string()),
        "wide-leaf-utility query must surface widely-used(): {returned2:?}",
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn functions_of_shape_excludes_test_fixtures() {
    // Per iter 237 (user-probe-058 Finding D): the
    // tests/ filter must drop the common/tests/test_helper
    // function from the wide-leaf-utility result even
    // though it has the wide-leaf-utility shape (6
    // callers across 3 files + 0 callees).
    let dir = std::env::temp_dir().join(format!(
        "doc-linter-saved-functions-of-shape-tests-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let db = testkit::open_db(&dir);
    {
        let conn = testkit::connect(&db);
        testkit::reset(&conn);
    }
    let conn = testkit::connect(&db);
    seed_function_shape_fixture(&conn);

    let q = SAVED_QUERIES
        .iter()
        .find(|q| q.name == "functions-of-shape")
        .expect("query must exist");
    let cypher = sql_of(q.name).replace("$shape_label", "'wide-leaf-utility'");
    let mut result = conn.query(&cypher).expect("query runs");
    let mut returned: Vec<String> = Vec::new();
    for row in &mut result {
        if let kv::Value::String(s) = &row[0] {
            returned.push(s.clone());
        }
    }
    assert!(
        returned.contains(&"widely-used()".to_string()),
        "production widely-used() must surface: {returned:?}",
    );
    assert!(
        !returned.contains(&"common/tests/test_helper()".to_string()),
        "test-fixture function must be EXCLUDED per iter-237 filter: {returned:?}",
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn function_shape_distribution_aggregates_per_shape_excluding_tests() {
    // Per iter 237 (user-probe-058 Finding D companion):
    // corpus-wide distribution. Fixture has 5 distinct
    // production shapes (factory + wide-leaf + entry +
    // hub + peripheral) + 1 test-fixture (excluded).
    // Peripheral (2 callers) below the ≥5 floor so it
    // doesn't appear. Expect: factory + wide-leaf + entry
    // + hub each 1 row.
    let dir = std::env::temp_dir().join(format!(
        "doc-linter-saved-shape-distribution-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let db = testkit::open_db(&dir);
    {
        let conn = testkit::connect(&db);
        testkit::reset(&conn);
    }
    let conn = testkit::connect(&db);
    seed_function_shape_fixture(&conn);

    let q = SAVED_QUERIES
        .iter()
        .find(|q| q.name == "function-shape-distribution")
        .expect("query must exist");
    let mut result = conn.query(sql_of(q.name)).expect("query runs");
    let mut by_shape: std::collections::HashMap<String, i64> = std::collections::HashMap::new();
    for row in &mut result {
        let shape = if let kv::Value::String(s) = &row[0] {
            s.clone()
        } else {
            panic!("shape String: {row:?}");
        };
        let count = match &row[1] {
            kv::Value::Int64(n) => *n,
            other => panic!("function_count int: {other:?}"),
        };
        by_shape.insert(shape, count);
    }
    // Expected: 4 production shapes each at 1 (factory,
    // wide-leaf-utility, thin-entry-point, subsystem-hub).
    // Peripheral function has <5 callers so it's
    // filtered by the cc >= 5 floor in the cypher.
    // common/tests/test_helper would be wide-leaf-utility
    // but is filtered by the tests/ exclusion.
    assert_eq!(
        by_shape.get("pure-leaf-factory"),
        Some(&1),
        "1 factory: {by_shape:?}",
    );
    assert_eq!(
        by_shape.get("wide-leaf-utility"),
        Some(&1),
        "1 wide-leaf-utility (test-fixture filtered out): {by_shape:?}",
    );
    assert_eq!(
        by_shape.get("thin-entry-point"),
        Some(&1),
        "1 thin-entry-point: {by_shape:?}",
    );
    assert_eq!(
        by_shape.get("subsystem-hub"),
        Some(&1),
        "1 subsystem-hub: {by_shape:?}",
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn function_shape_by_file_aggregates_per_file_excluding_tests() {
    // Per iter 239 (sibling of iter-237 function-shape-
    // distribution at File granularity): fixture has 4
    // production targets in 4 distinct files. Expect 4
    // rows, one per (file, shape) pair: src/mcp.rs +
    // pure-leaf-factory, src/parser.rs + thin-entry-point,
    // src/projections.rs + wide-leaf-utility, src/util.rs +
    // subsystem-hub. The tests/common/mod.rs +
    // wide-leaf-utility row is filtered by iter-237.
    let dir = std::env::temp_dir().join(format!(
        "doc-linter-saved-shape-by-file-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let db = testkit::open_db(&dir);
    {
        let conn = testkit::connect(&db);
        testkit::reset(&conn);
    }
    let conn = testkit::connect(&db);
    seed_function_shape_fixture(&conn);

    let q = SAVED_QUERIES
        .iter()
        .find(|q| q.name == "function-shape-by-file")
        .expect("query must exist");
    let mut result = conn.query(sql_of(q.name)).expect("query runs");
    let mut by_file: std::collections::HashMap<(String, String), i64> =
        std::collections::HashMap::new();
    for row in &mut result {
        let file = if let kv::Value::String(s) = &row[0] {
            s.clone()
        } else {
            panic!("file String: {row:?}");
        };
        let shape = if let kv::Value::String(s) = &row[1] {
            s.clone()
        } else {
            panic!("shape String: {row:?}");
        };
        let count = match &row[2] {
            kv::Value::Int64(n) => *n,
            other => panic!("function_count int: {other:?}"),
        };
        by_file.insert((file, shape), count);
    }
    assert_eq!(
        by_file.get(&("src/mcp.rs".to_string(), "pure-leaf-factory".to_string())),
        Some(&1),
        "src/mcp.rs has 1 pure-leaf-factory: {by_file:?}",
    );
    assert_eq!(
        by_file.get(&(
            "src/projections.rs".to_string(),
            "wide-leaf-utility".to_string()
        )),
        Some(&1),
        "src/projections.rs has 1 wide-leaf-utility: {by_file:?}",
    );
    assert_eq!(
        by_file.get(&("src/parser.rs".to_string(), "thin-entry-point".to_string())),
        Some(&1),
        "src/parser.rs has 1 thin-entry-point: {by_file:?}",
    );
    assert_eq!(
        by_file.get(&("src/util.rs".to_string(), "subsystem-hub".to_string())),
        Some(&1),
        "src/util.rs has 1 subsystem-hub: {by_file:?}",
    );
    // Test-fixture file must NOT appear per iter-237 filter.
    assert!(
        !by_file
            .keys()
            .any(|(f, _)| f.contains("tests/common/mod.rs")),
        "tests/common/mod.rs must be EXCLUDED: {by_file:?}",
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn file_shape_profile_returns_production_functions_in_file() {
    // Per iter 239 (sibling of iter-235 functions-of-shape
    // at File granularity): given a file_path substring,
    // return the production Functions in matching files
    // with shape labels. Query src/parser.rs from the
    // fixture — expect 1 row: entry-target() at thin-entry-
    // point. tests/common/mod.rs even with $file_path
    // 'common/' substring must be excluded.
    let dir = std::env::temp_dir().join(format!(
        "doc-linter-saved-file-shape-profile-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let db = testkit::open_db(&dir);
    {
        let conn = testkit::connect(&db);
        testkit::reset(&conn);
    }
    let conn = testkit::connect(&db);
    seed_function_shape_fixture(&conn);

    let q = SAVED_QUERIES
        .iter()
        .find(|q| q.name == "file-shape-profile")
        .expect("query must exist");
    // Query for src/parser.rs.
    let cypher = sql_of(q.name).replace("$file_path", "'src/parser.rs'");
    let mut result = conn.query(&cypher).expect("query runs");
    let mut rows: Vec<(String, String, String)> = Vec::new();
    for row in &mut result {
        let file = if let kv::Value::String(s) = &row[0] {
            s.clone()
        } else {
            panic!("file String: {row:?}");
        };
        let symbol = if let kv::Value::String(s) = &row[1] {
            s.clone()
        } else {
            panic!("symbol String: {row:?}");
        };
        let shape = if let kv::Value::String(s) = &row[5] {
            s.clone()
        } else {
            panic!("shape String: {row:?}");
        };
        rows.push((file, symbol, shape));
    }
    assert!(
        rows.iter().any(|(f, s, sh)| {
            f == "src/parser.rs" && s == "entry-target()" && sh == "thin-entry-point"
        }),
        "src/parser.rs query must surface entry-target() thin-entry-point: {rows:?}",
    );
    // Verify the test-fixture file is filtered even with
    // a substring 'common/' that would otherwise match.
    let cypher_tests = sql_of(q.name).replace("$file_path", "'common/'");
    let mut result_tests = conn.query(&cypher_tests).expect("query runs");
    let mut test_rows: Vec<String> = Vec::new();
    for row in &mut result_tests {
        if let kv::Value::String(s) = &row[0] {
            test_rows.push(s.clone());
        }
    }
    assert!(
        !test_rows.iter().any(|f| f.contains("tests/")),
        "tests/-files must be excluded by file-shape-profile: {test_rows:?}",
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn function_shape_summary_classifies_peripheral() {
    let dir = std::env::temp_dir().join(format!(
        "doc-linter-saved-shape-peripheral-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let db = testkit::open_db(&dir);
    {
        let conn = testkit::connect(&db);
        testkit::reset(&conn);
    }
    let conn = testkit::connect(&db);
    seed_function_shape_fixture(&conn);
    let (cc, _cfc, ec, _efc, shape) = run_function_shape_summary_query(&conn, "peripheral()");
    assert_eq!(cc, 2, "2 callers");
    assert_eq!(ec, 0, "0 callees");
    assert_eq!(shape, "peripheral", "below all hub thresholds");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn function_callees_by_file_aggregates_by_callee_file() {
    // Per iter 231: forward-direction file aggregation.
    // Expected: 1 row — src/fileD.rs with 3 distinct
    // callees (x, y, z).
    let dir = std::env::temp_dir().join(format!(
        "doc-linter-saved-callees-by-file-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let db = testkit::open_db(&dir);
    {
        let conn = testkit::connect(&db);
        testkit::reset(&conn);
    }
    let conn = testkit::connect(&db);
    seed_slice_file_axis_fixture(&conn);

    let q = SAVED_QUERIES
        .iter()
        .find(|q| q.name == "function-callees-by-file")
        .expect("query must exist");
    let cypher = sql_of(q.name).replace("$symbol", "'target()'");
    let mut result = conn.query(&cypher).expect("query runs");
    let mut by_file: std::collections::HashMap<String, i64> = std::collections::HashMap::new();
    for row in &mut result {
        let file = if let kv::Value::String(s) = &row[0] {
            s.clone()
        } else {
            panic!("file String: {row:?}");
        };
        let count = match &row[1] {
            kv::Value::Int64(n) => *n,
            other => panic!("count int: {other:?}"),
        };
        by_file.insert(file, count);
    }
    assert_eq!(by_file.get("src/fileD.rs"), Some(&3));
    // fileA/B/C have CALLERS not callees — must NOT
    // surface in the forward direction.
    for excluded in ["src/fileA.rs", "src/fileB.rs", "src/fileC.rs"] {
        assert!(
            !by_file.contains_key(excluded),
            "{excluded} has callers not callees — must NOT surface: {by_file:?}",
        );
    }
}

fn seed_test_coverage_fixture(conn: &testkit::Conn<'_>) {
    // Fixture exercising the 3-state hub classification +
    // .pyi exclusion + /tests/ exclusion.
    // - critical-hub: 12 callers, 0 tests → critical-untested-hub
    // - small-hub:    7 callers, 0 tests → untested-hub
    // - tested-hub:   8 callers, 1 test → tested-hub
    // - stub-hub:     20 callers in .pyi → EXCLUDED (.pyi filter)
    // - test-fn:      lives in /tests/ → EXCLUDED (path filter)
    // - leaf-fn:      0 callers, 0 tests → EXCLUDED (callers < 5)
    for (sym, file) in [
        ("critical-hub()", "src/critical.rs"),
        ("small-hub()", "src/small.rs"),
        ("tested-hub()", "src/tested.rs"),
        ("stub-hub()", "stub/types.pyi"),
        ("test-fn()", "src/tests/runner.rs"),
        ("leaf-fn()", "src/leaf.rs"),
        // Per iter 220 (user-probe-053 Finding B): root-
        // level tests directory must also be excluded by
        // the iter-220-fixed path filter `tests/`.
        ("root-test-fn()", "tests/test_helper.py"),
        // Per iter 225 (user-probe-054 Finding B): Rust
        // co-located test per [[feedback_test_co_location]]
        // — SYMBOL path contains 'tests/' (e.g. `cmd/mcp/
        // tests/server()`) but FILE path is a regular src/
        // path. The iter-225 symbol-CONTAINS filter must
        // catch this.
        ("cmd/mcp/tests/server()", "src/cmd/mcp.rs"),
    ] {
        let cypher = format!(
            "CREATE (:Function {{symbol: '{sym}', crate: 'd', file: '{file}', \
                 line: 1, doc_comment: '', language: 'rust', signature: '', \
                 body_excerpt: '', last_touched: '2026-06-01', repo_id: 'd'}})"
        );
        conn.query(&cypher).expect("seed Function");
    }
    // Seed callers — one fn per caller edge for distinct count.
    // critical-hub: 12 callers
    // small-hub: 7 callers
    // tested-hub: 8 callers
    // stub-hub: 20 callers (but stub-hub itself excluded)
    for i in 0..15 {
        let cypher = format!(
            "CREATE (:Function {{symbol: 'caller-{i}()', crate: 'd', file: 'callers/c{i}.rs', \
                 line: 1, doc_comment: '', language: 'rust', signature: '', \
                 body_excerpt: '', last_touched: '2026-06-01', repo_id: 'd'}})"
        );
        conn.query(&cypher).expect("seed caller Function");
    }
    for (caller_i, target) in [
        (0, "critical-hub()"),
        (1, "critical-hub()"),
        (2, "critical-hub()"),
        (3, "critical-hub()"),
        (4, "critical-hub()"),
        (5, "critical-hub()"),
        (6, "critical-hub()"),
        (7, "critical-hub()"),
        (8, "critical-hub()"),
        (9, "critical-hub()"),
        (10, "critical-hub()"),
        (11, "critical-hub()"),
        (0, "small-hub()"),
        (1, "small-hub()"),
        (2, "small-hub()"),
        (3, "small-hub()"),
        (4, "small-hub()"),
        (5, "small-hub()"),
        (6, "small-hub()"),
        (0, "tested-hub()"),
        (1, "tested-hub()"),
        (2, "tested-hub()"),
        (3, "tested-hub()"),
        (4, "tested-hub()"),
        (5, "tested-hub()"),
        (6, "tested-hub()"),
        (7, "tested-hub()"),
        (0, "stub-hub()"),
        (1, "stub-hub()"),
        (2, "stub-hub()"),
        (3, "stub-hub()"),
        (4, "stub-hub()"),
        (5, "stub-hub()"),
        (6, "stub-hub()"),
        (7, "stub-hub()"),
        (8, "stub-hub()"),
        (9, "stub-hub()"),
        (10, "stub-hub()"),
        (11, "stub-hub()"),
        (12, "stub-hub()"),
        (13, "stub-hub()"),
        (14, "stub-hub()"),
    ] {
        let cypher = format!(
            "MATCH (c:Function {{symbol: 'caller-{caller_i}()'}}), \
                       (t:Function {{symbol: '{target}'}}) \
                 CREATE (c)-[:CALLS {{source_file: 'callers/c{caller_i}.rs', \
                 source_line: 5}}]->(t)"
        );
        conn.query(&cypher).expect("seed CALLS");
    }
    // tested-hub has a TEST_FOR edge pointing to it.
    conn.query(
        "MATCH (test:Function {symbol: 'test-fn()'}), \
                   (tgt:Function {symbol: 'tested-hub()'}) \
             CREATE (test)-[:TEST_FOR {confidence: 'high'}]->(tgt)",
    )
    .expect("seed TEST_FOR");
}

#[test]
fn callers_density_hub_test_coverage_emits_3_categorical_labels_with_pyi_excluded() {
    // Per iter 218 (user-probe-052 Finding C + B): the
    // hub classification + .pyi exclusion both verified
    // on a fixture mirroring spacy's pattern.
    let dir = std::env::temp_dir().join(format!(
        "doc-linter-saved-hub-test-coverage-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let db = testkit::open_db(&dir);
    {
        let conn = testkit::connect(&db);
        testkit::reset(&conn);
    }
    let conn = testkit::connect(&db);
    seed_test_coverage_fixture(&conn);

    let q = SAVED_QUERIES
        .iter()
        .find(|q| q.name == "callers-density-hub-test-coverage")
        .expect("query must exist");
    let mut result = conn.query(sql_of(q.name)).expect("query runs");
    let mut by_symbol: std::collections::HashMap<String, String> = std::collections::HashMap::new();
    for row in &mut result {
        // Columns: symbol, file, callers, test_count, classification.
        let sym = if let kv::Value::String(s) = &row[0] {
            s.clone()
        } else {
            panic!("symbol String: {row:?}");
        };
        let cls = if let kv::Value::String(s) = &row[4] {
            s.clone()
        } else {
            panic!("classification String: {row:?}");
        };
        by_symbol.insert(sym, cls);
    }
    assert_eq!(
        by_symbol.get("critical-hub()").map(String::as_str),
        Some("critical-untested-hub"),
        "12 callers + 0 tests must classify critical-untested-hub: {by_symbol:?}",
    );
    assert_eq!(
        by_symbol.get("small-hub()").map(String::as_str),
        Some("untested-hub"),
        "7 callers + 0 tests must classify untested-hub: {by_symbol:?}",
    );
    assert_eq!(
        by_symbol.get("tested-hub()").map(String::as_str),
        Some("tested-hub"),
        "tested-hub fn must classify tested-hub: {by_symbol:?}",
    );
    assert!(
        !by_symbol.contains_key("stub-hub()"),
        "stub-hub() lives in .pyi — must be EXCLUDED: {by_symbol:?}",
    );
    // Per iter 220 (user-probe-053 Finding B): root-level
    // tests/ must also be excluded.
    assert!(
        !by_symbol.contains_key("root-test-fn()"),
        "root-level tests/test_helper.py must be EXCLUDED: {by_symbol:?}",
    );
    // Per iter 225 (user-probe-054 Finding B): Rust co-located
    // test SYMBOL containing 'tests/' must also be excluded
    // even though FILE path is regular src/.
    assert!(
        !by_symbol.contains_key("cmd/mcp/tests/server()"),
        "Rust co-located test symbol must be EXCLUDED via symbol-CONTAINS filter: {by_symbol:?}",
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn untested_by_file_aggregated_excludes_pyi_and_tests_dir() {
    // Per iter 218 (user-probe-052 Q2 + Finding B):
    // file-axis count must exclude .pyi + /tests/ + test_
    // path conventions.
    let dir = std::env::temp_dir().join(format!(
        "doc-linter-saved-untested-by-file-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let db = testkit::open_db(&dir);
    {
        let conn = testkit::connect(&db);
        testkit::reset(&conn);
    }
    let conn = testkit::connect(&db);
    seed_test_coverage_fixture(&conn);

    let q = SAVED_QUERIES
        .iter()
        .find(|q| q.name == "untested-by-file-aggregated")
        .expect("query must exist");
    let mut result = conn.query(sql_of(q.name)).expect("query runs");
    let mut by_file: std::collections::HashMap<String, i64> = std::collections::HashMap::new();
    for row in &mut result {
        // Columns: file, untested_count.
        let file = if let kv::Value::String(s) = &row[0] {
            s.clone()
        } else {
            panic!("file String: {row:?}");
        };
        let count = match &row[1] {
            kv::Value::Int64(n) => *n,
            other => panic!("untested_count int: {other:?}"),
        };
        by_file.insert(file, count);
    }
    // critical/small/leaf survive — none are tested.
    assert!(
        by_file.contains_key("src/critical.rs"),
        "src/critical.rs untested: {by_file:?}"
    );
    assert!(
        by_file.contains_key("src/small.rs"),
        "src/small.rs untested: {by_file:?}"
    );
    assert!(
        by_file.contains_key("src/leaf.rs"),
        "src/leaf.rs untested: {by_file:?}"
    );
    // 15 callers/c*.rs files also have functions with no test → they qualify.
    assert!(
        by_file.keys().any(|f| f.starts_with("callers/")),
        "caller files appear as untested too: {by_file:?}",
    );
    // .pyi excluded
    assert!(
        !by_file.contains_key("stub/types.pyi"),
        ".pyi file must be EXCLUDED: {by_file:?}",
    );
    // /tests/ excluded (nested tests dir)
    assert!(
        !by_file.contains_key("src/tests/runner.rs"),
        "/tests/ file must be EXCLUDED: {by_file:?}",
    );
    // Per iter 220 (user-probe-053 Finding B): root-level
    // tests/ must also be excluded.
    assert!(
        !by_file.contains_key("tests/test_helper.py"),
        "root-level tests/ file must be EXCLUDED: {by_file:?}",
    );
    // Per iter 225 (user-probe-054 Finding B): Rust co-located
    // test SYMBOL — file=src/cmd/mcp.rs gets counted ONLY
    // by virtue of OTHER functions in mcp.rs (critical-hub +
    // small-hub aren't there; only co-located-test is). But
    // the symbol-CONTAINS filter removes it, so src/cmd/mcp.rs
    // shouldn't appear in the result FROM the co-located test
    // contribution. Note: the same file MAY appear if other
    // functions there are also untested — fixture has critical-
    // hub etc. in DIFFERENT files (src/critical.rs etc.), so
    // src/cmd/mcp.rs should be ENTIRELY absent.
    assert!(
        !by_file.contains_key("src/cmd/mcp.rs"),
        "src/cmd/mcp.rs file (containing only the co-located test) must be EXCLUDED: {by_file:?}",
    );
    // tested-hub doesn't appear (it HAS a test).
    assert!(
        !by_file.contains_key("src/tested.rs"),
        "tested file (has TEST_FOR edge) must NOT appear in untested list: {by_file:?}",
    );
    let _ = std::fs::remove_dir_all(&dir);
}

fn run_test_coverage_baseline_with_fixture<F>(seed: F) -> (i64, i64, String)
where
    F: FnOnce(&testkit::Conn<'_>),
{
    let dir = std::env::temp_dir().join(format!(
        "doc-linter-saved-coverage-baseline-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let db = testkit::open_db(&dir);
    {
        let conn = testkit::connect(&db);
        testkit::reset(&conn);
    }
    let conn = testkit::connect(&db);
    seed(&conn);
    let q = SAVED_QUERIES
        .iter()
        .find(|q| q.name == "test-coverage-baseline")
        .expect("query must exist");
    let mut result = conn.query(sql_of(q.name)).expect("query runs");
    let row = result.next().expect("1 row");
    let total = match &row[0] {
        kv::Value::Int64(n) => *n,
        other => panic!("total int: {other:?}"),
    };
    let tested = match &row[1] {
        kv::Value::Int64(n) => *n,
        other => panic!("tested int: {other:?}"),
    };
    let label = if let kv::Value::String(s) = &row[3] {
        s.clone()
    } else {
        panic!("trust_label String: {row:?}");
    };
    let _ = std::fs::remove_dir_all(&dir);
    (total, tested, label)
}

fn seed_fns(conn: &testkit::Conn<'_>, count: usize, tested_count: usize) {
    for i in 0..count {
        let sym = format!("fn-{i}");
        let cypher = format!(
            "CREATE (:Function {{symbol: '{sym}', crate: 'd', file: 'a.rs', \
                 line: 1, doc_comment: '', language: 'rust', signature: '', \
                 body_excerpt: '', last_touched: '2026-06-01', repo_id: 'd'}})"
        );
        conn.query(&cypher).expect("seed Function");
    }
    for i in 0..tested_count {
        let target = format!("fn-{i}");
        let test_sym = format!("test-{i}");
        conn.query(&format!(
            "CREATE (:Function {{symbol: '{test_sym}', crate: 'd', \
                 file: 'tests/test_a.rs', line: 1, doc_comment: '', \
                 language: 'rust', signature: '', body_excerpt: '', \
                 last_touched: '2026-06-01', repo_id: 'd'}})"
        ))
        .expect("seed test Function");
        conn.query(&format!(
            "MATCH (test:Function {{symbol: '{test_sym}'}}), \
                       (tgt:Function {{symbol: '{target}'}}) \
                 CREATE (test)-[:TEST_FOR {{confidence: 'high'}}]->(tgt)"
        ))
        .expect("seed TEST_FOR");
    }
}

#[test]
fn test_coverage_baseline_emits_coverage_rich_at_high_coverage() {
    // Per iter 220 (user-probe-053 + feedback_test_for_sparse
    // _on_python memory): 20 of 80 source functions tested =
    // 20 / (80 source + 20 test) = 20% → coverage-rich.
    let (total, tested, label) = run_test_coverage_baseline_with_fixture(|conn| {
        seed_fns(conn, 80, 20);
    });
    assert_eq!(total, 100, "80 source + 20 test = 100 total");
    assert_eq!(tested, 20);
    assert_eq!(label, "coverage-rich");
}

#[test]
fn test_coverage_baseline_emits_coverage_sparse_in_python_range() {
    // Per memory: spacy 2% lands in coverage-sparse (1-10%).
    // Seed 98 source + 2 test → 100 total, 2 tested = 2.0%.
    let (total, tested, label) = run_test_coverage_baseline_with_fixture(|conn| {
        seed_fns(conn, 98, 2);
    });
    assert_eq!(total, 100);
    assert_eq!(tested, 2);
    assert_eq!(label, "coverage-sparse");
}

#[test]
fn test_coverage_baseline_emits_coverage_empty_below_1_percent() {
    // Per memory: example-app 0.1% (1/904) lands in coverage-empty.
    // Seed 100 source + 0 test → 0% coverage.
    let (total, tested, label) = run_test_coverage_baseline_with_fixture(|conn| {
        seed_fns(conn, 100, 0);
    });
    assert_eq!(total, 100);
    assert_eq!(tested, 0);
    assert_eq!(label, "coverage-empty");
}

#[test]
fn test_coverage_baseline_emits_coverage_empty_on_zero_functions() {
    // Edge case: 0/0 — division-by-zero guard. CASE WHEN
    // total = 0 returns 0.0.
    let (total, tested, label) = run_test_coverage_baseline_with_fixture(|_conn| {});
    assert_eq!(total, 0);
    assert_eq!(tested, 0);
    assert_eq!(label, "coverage-empty");
}

fn seed_storyline_leaf_fixture(conn: &testkit::Conn<'_>) {
    // Fixture exercising storyline-leaf-detector + recent-
    // leaf-docs:
    // - storyline-leaf:  has 'ingest-pass-storyline' tag,
    //                    ZERO inbound wikilinks → MUST surface
    //                    in storyline-leaf-detector + recent-
    //                    leaf-docs.
    // - storyline-linked: has the tag + 1 inbound wikilink →
    //                    must NOT surface in storyline-leaf.
    // - non-storyline-leaf: no storyline tag, 0 inbound →
    //                    must NOT surface in storyline-leaf
    //                    but MUST surface in recent-leaf-docs.
    // - referrer: provides the inbound to storyline-linked.
    // - ontology-doc: role != 'doc', should NOT surface in
    //                    recent-leaf-docs role filter.
    for (id, role, tags, updated) in [
        (
            "storyline-leaf",
            "doc",
            "['ingest-pass-storyline']",
            "2026-06-01",
        ),
        (
            "storyline-linked",
            "doc",
            "['ingest-pass-storyline']",
            "2026-05-30",
        ),
        (
            "non-storyline-leaf",
            "doc",
            "['unrelated-tag']",
            "2026-05-28",
        ),
        ("referrer", "doc", "[]", "2026-05-20"),
        ("ontology-doc", "ontology-entity", "[]", "2026-06-01"),
    ] {
        let cypher = format!(
            "CREATE (:Doc {{id: '{id}', path: '{id}.md', role: '{role}', \
                 kind: 'reference', lifecycle: '', bounded_context: '', \
                 title: '{id} title', summary: '', status: 'stable', \
                 updated: '{updated}', tags: {tags}, covers: []}})"
        );
        conn.query(&cypher).expect("seed Doc");
    }
    // Inbound wikilink: referrer → storyline-linked.
    conn.query(
        "MATCH (r:Doc {id: 'referrer'}), (t:Doc {id: 'storyline-linked'}) \
             CREATE (r)-[:WIKILINK {line: 0}]->(t)",
    )
    .expect("seed WIKILINK");
}

/// First-run false positives: starter docs of ALL FIVE roles must stay
/// out of the stale list; only the authored doc may surface.
#[test]
fn stale_queries_exclude_all_five_starter_roles() {
    let dir = std::env::temp_dir().join(format!(
        "doc-linter-saved-stale-five-roles-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let db = testkit::open_db(&dir);
    {
        let conn = testkit::connect(&db);
        testkit::reset(&conn);
    }
    let conn = testkit::connect(&db);
    for role in [
        "doc",
        "ontology-value",
        "ontology-axis",
        "ontology-entity",
        "ontology-migration",
        "index",
    ] {
        conn.query(&format!(
            "CREATE (:Doc {{id: 'd-{role}', path: '{role}.md', role: '{role}', \
                 kind: 'reference', lifecycle: '', bounded_context: '', \
                 title: 't', summary: '', status: 'stable', \
                 updated: '2020-01-01', tags: [], covers: []}})"
        ))
        .expect("seed Doc");
    }
    for name in ["stale-narrative-docs", "stale-docs-with-impact"] {
        let q = SAVED_QUERIES.iter().find(|q| q.name == name).unwrap();
        let mut result = conn.query(sql_of(q.name)).expect("query runs");
        let ids: Vec<String> = (&mut result)
            .map(|row| match &row[0] {
                kv::Value::String(id) => id.clone(),
                other => panic!("unexpected {other:?}"),
            })
            .collect();
        assert_eq!(ids, ["d-doc"], "{name}");
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn storyline_leaf_detector_surfaces_only_storyline_tagged_with_no_inbound() {
    let dir = std::env::temp_dir().join(format!(
        "doc-linter-saved-storyline-leaf-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let db = testkit::open_db(&dir);
    {
        let conn = testkit::connect(&db);
        testkit::reset(&conn);
    }
    let conn = testkit::connect(&db);
    seed_storyline_leaf_fixture(&conn);

    let q = SAVED_QUERIES
        .iter()
        .find(|q| q.name == "storyline-leaf-detector")
        .expect("query must exist");
    let mut result = conn.query(sql_of(q.name)).expect("query runs");
    let mut returned: Vec<String> = Vec::new();
    for row in &mut result {
        if let kv::Value::String(id) = &row[0] {
            returned.push(id.clone());
        }
    }
    assert!(
        returned.contains(&"storyline-leaf".to_string()),
        "storyline-tagged + 0 inbound must surface: {returned:?}",
    );
    assert!(
        !returned.contains(&"storyline-linked".to_string()),
        "storyline-tagged + has inbound must NOT surface: {returned:?}",
    );
    assert!(
        !returned.contains(&"non-storyline-leaf".to_string()),
        "non-storyline leaf must NOT surface: {returned:?}",
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn recent_leaf_docs_orders_by_updated_desc_and_filters_role() {
    let dir = std::env::temp_dir().join(format!(
        "doc-linter-saved-recent-leaf-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let db = testkit::open_db(&dir);
    {
        let conn = testkit::connect(&db);
        testkit::reset(&conn);
    }
    let conn = testkit::connect(&db);
    seed_storyline_leaf_fixture(&conn);

    let q = SAVED_QUERIES
        .iter()
        .find(|q| q.name == "recent-leaf-docs")
        .expect("query must exist");
    let mut result = conn.query(sql_of(q.name)).expect("query runs");
    let mut returned: Vec<String> = Vec::new();
    for row in &mut result {
        if let kv::Value::String(id) = &row[0] {
            returned.push(id.clone());
        }
    }
    assert!(
        returned.contains(&"storyline-leaf".to_string()),
        "doc with 0 inbound + role=doc must surface: {returned:?}",
    );
    assert!(
        returned.contains(&"non-storyline-leaf".to_string()),
        "non-storyline leaf with 0 inbound + role=doc must surface: {returned:?}",
    );
    assert!(
        returned.contains(&"referrer".to_string()),
        "referrer has 0 inbound + role=doc — must surface: {returned:?}",
    );
    assert!(
        !returned.contains(&"storyline-linked".to_string()),
        "storyline-linked has inbound from referrer — must NOT surface: {returned:?}",
    );
    assert!(
        !returned.contains(&"ontology-doc".to_string()),
        "ontology-entity role-filter excludes: {returned:?}",
    );
    // Order check: most-recently-updated first.
    let leaf_pos = returned.iter().position(|s| s == "storyline-leaf");
    let non_pos = returned.iter().position(|s| s == "non-storyline-leaf");
    let ref_pos = returned.iter().position(|s| s == "referrer");
    assert!(
        leaf_pos < non_pos && non_pos < ref_pos,
        "order DESC by updated: 06-01 → 05-28 → 05-20; got {returned:?}",
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn ingest_pass_status_emits_4_rows_with_populated_flag() {
    // Per iter 205 (user-probe-049 Finding D — row-per-pass
    // companion to the extended classifier). Seed 1 File +
    // 1 Function + 0 COUPLED_WITH + 0 Endpoint to exercise
    // the populated boolean correctly per-pass.
    let dir = std::env::temp_dir().join(format!(
        "doc-linter-saved-ingest-pass-status-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let db = testkit::open_db(&dir);
    {
        let conn = testkit::connect(&db);
        testkit::reset(&conn);
    }
    let conn = testkit::connect(&db);
    conn.query(
        "CREATE (:File {path: 'a.rs', language: 'rust', loc: 100, last_touched: '2026-06-01'})",
    )
    .unwrap();
    conn.query("CREATE (:Function {symbol: 'foo()', crate: 'doc-linter', file: 'a.rs', line: 1, doc_comment: '', language: 'rust', signature: 'fn foo()', body_excerpt: '', last_touched: '2026-06-01', repo_id: 'doc-linter'})").unwrap();

    let q = SAVED_QUERIES
        .iter()
        .find(|q| q.name == "ingest-pass-status")
        .expect("ingest-pass-status must exist");
    let mut result = conn.query(sql_of(q.name)).expect("query runs");
    let mut rows: std::collections::HashMap<String, bool> = std::collections::HashMap::new();
    for row in &mut result {
        // Columns: pass_name, row_count, populated.
        let name = if let kv::Value::String(s) = &row[0] {
            s.clone()
        } else {
            panic!("pass_name must be String: {row:?}");
        };
        let populated = match &row[2] {
            kv::Value::Int64(n) => *n != 0,
            other => panic!("populated must be 0/1: {other:?}"),
        };
        rows.insert(name, populated);
    }
    assert_eq!(rows.len(), 4, "must return 4 rows: {rows:?}");
    assert_eq!(
        rows.get("file_walker").copied(),
        Some(true),
        "file_walker populated: {rows:?}",
    );
    assert_eq!(
        rows.get("scip").copied(),
        Some(true),
        "scip populated: {rows:?}",
    );
    assert_eq!(
        rows.get("git_coupling").copied(),
        Some(false),
        "git_coupling NOT populated (the user-probe-049 spacy/example-app pattern): {rows:?}",
    );
    assert_eq!(
        rows.get("endpoint_extract").copied(),
        Some(false),
        "endpoint_extract NOT populated: {rows:?}",
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn tight_vs_loose_coupling_hubs_semantic_categorizes_avg_jaccard() {
    // Per interrogation-040 Finding D: hub-axis with avg-
    // jaccard threshold. Seed 3 hub-files:
    //   tight (3 neighbors, avg jaccard 0.9) → tight-cluster
    //   loose (3 neighbors, avg jaccard 0.3) → loose-orbit
    //   mid (3 neighbors, avg jaccard 0.6) → mixed
    // Plus enough leaf neighbors to populate the rels.
    let dir = std::env::temp_dir().join(format!(
        "doc-linter-saved-tight-loose-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let db = testkit::open_db(&dir);
    {
        let conn = testkit::connect(&db);
        testkit::reset(&conn);
    }
    let conn = testkit::connect(&db);
    for path in [
        "tight-hub.rs",
        "tight-a.rs",
        "tight-b.rs",
        "tight-c.rs",
        "loose-hub.rs",
        "loose-a.rs",
        "loose-b.rs",
        "loose-c.rs",
        "mid-hub.rs",
        "mid-a.rs",
        "mid-b.rs",
        "mid-c.rs",
    ] {
        let cypher = format!(
                "CREATE (:File {{path: '{path}', language: 'rust', loc: 100, last_touched: '2026-06-01'}})"
            );
        conn.query(&cypher).expect("seed File");
    }
    for (hub, leaf, jaccard) in [
        ("tight-hub.rs", "tight-a.rs", 0.9),
        ("tight-hub.rs", "tight-b.rs", 0.9),
        ("tight-hub.rs", "tight-c.rs", 0.9),
        ("loose-hub.rs", "loose-a.rs", 0.3),
        ("loose-hub.rs", "loose-b.rs", 0.3),
        ("loose-hub.rs", "loose-c.rs", 0.3),
        ("mid-hub.rs", "mid-a.rs", 0.6),
        ("mid-hub.rs", "mid-b.rs", 0.6),
        ("mid-hub.rs", "mid-c.rs", 0.6),
    ] {
        let cypher = format!(
            "MATCH (a:File {{path: '{hub}'}}), (b:File {{path: '{leaf}'}}) \
                 CREATE (a)-[:COUPLED_WITH {{commits: 8, jaccard: {jaccard}, \
                 last_co_change_at: '2026-05-01T00:00:00Z'}}]->(b)"
        );
        conn.query(&cypher).expect("seed COUPLED_WITH");
    }

    let q = SAVED_QUERIES
        .iter()
        .find(|q| q.name == "tight-vs-loose-coupling-hubs")
        .expect("tight-vs-loose-coupling-hubs must exist");
    let mut result = conn.query(sql_of(q.name)).expect("query runs");
    let mut by_file: std::collections::HashMap<String, String> = std::collections::HashMap::new();
    for row in &mut result {
        // Columns: file, neighbor_count, avg_jaccard, cluster_type.
        if let (kv::Value::String(file), kv::Value::String(cluster)) = (&row[0], &row[3]) {
            by_file.insert(file.clone(), cluster.clone());
        }
    }
    assert_eq!(
        by_file.get("tight-hub.rs").map(String::as_str),
        Some("tight-cluster"),
        "tight-hub.rs (jaccard=0.9) must be tight-cluster: {by_file:?}",
    );
    assert_eq!(
        by_file.get("loose-hub.rs").map(String::as_str),
        Some("loose-orbit"),
        "loose-hub.rs (jaccard=0.3) must be loose-orbit: {by_file:?}",
    );
    assert_eq!(
        by_file.get("mid-hub.rs").map(String::as_str),
        Some("mixed"),
        "mid-hub.rs (jaccard=0.6) must be mixed: {by_file:?}",
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn feature_evolution_coupling_by_commits_orders_by_commits_desc() {
    // Per iter 211 (user-probe-050 Finding B): the sibling
    // ordering surfaces high-commit pairs above high-jaccard
    // pairs. Fixture: 3 pairs all at commits >= 5 — one at
    // commits=15 jaccard=0.5 (architectural) + one at
    // commits=10 jaccard=0.6 (mid) + one at commits=5
    // jaccard=1.0 (test-scaffold). The commits-DESC ordering
    // must return them in commits-15 → 10 → 5 order.
    let dir = std::env::temp_dir().join(format!(
        "doc-linter-saved-fec-by-commits-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let db = testkit::open_db(&dir);
    {
        let conn = testkit::connect(&db);
        testkit::reset(&conn);
    }
    let conn = testkit::connect(&db);
    for path in [
        "arch-a.rs",
        "arch-b.rs",
        "mid-a.rs",
        "mid-b.rs",
        "test-a.rs",
        "test-b.rs",
    ] {
        let cypher = format!(
                "CREATE (:File {{path: '{path}', language: 'rust', loc: 100, last_touched: '2026-06-01'}})"
            );
        conn.query(&cypher).expect("seed File");
    }
    for (a, b, commits, jaccard) in [
        ("arch-a.rs", "arch-b.rs", 15, 0.5),
        ("mid-a.rs", "mid-b.rs", 10, 0.6),
        ("test-a.rs", "test-b.rs", 5, 1.0),
    ] {
        let cypher = format!(
            "MATCH (a:File {{path: '{a}'}}), (b:File {{path: '{b}'}}) \
                 CREATE (a)-[:COUPLED_WITH {{commits: {commits}, jaccard: {jaccard}, \
                 last_co_change_at: '2026-05-01T00:00:00Z'}}]->(b)"
        );
        conn.query(&cypher).expect("seed COUPLED_WITH");
    }

    let q = SAVED_QUERIES
        .iter()
        .find(|q| q.name == "feature-evolution-coupling-by-commits")
        .expect("feature-evolution-coupling-by-commits must exist");
    let mut result = conn.query(sql_of(q.name)).expect("query runs");
    let mut returned: Vec<(String, String, i64)> = Vec::new();
    for row in &mut result {
        // Columns: a.path, b.path, r.commits, r.jaccard, r.last_co_change_at.
        let a = if let kv::Value::String(s) = &row[0] {
            s.clone()
        } else {
            panic!("a.path String: {row:?}");
        };
        let b = if let kv::Value::String(s) = &row[1] {
            s.clone()
        } else {
            panic!("b.path String: {row:?}");
        };
        let commits = match &row[2] {
            kv::Value::UInt32(n) => *n as i64,
            kv::Value::Int64(n) => *n,
            other => panic!("commits int: {other:?}"),
        };
        returned.push((a, b, commits));
    }
    assert_eq!(returned.len(), 3, "3 pairs: {returned:?}");
    assert_eq!(
        returned[0].2, 15,
        "commits-DESC ordering — arch-pair (commits=15) must be first: {returned:?}"
    );
    assert_eq!(
        returned[1].2, 10,
        "mid pair (commits=10) must be second: {returned:?}"
    );
    assert_eq!(
        returned[2].2, 5,
        "test-scaffold pair (commits=5) must be last DESPITE jaccard=1.0: {returned:?}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

fn run_cold_start_overview_with_fixture<F>(seed: F) -> Vec<kv::Value>
where
    F: FnOnce(&testkit::Conn<'_>),
{
    let dir = std::env::temp_dir().join(format!(
        "doc-linter-saved-cold-start-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let db = testkit::open_db(&dir);
    {
        let conn = testkit::connect(&db);
        testkit::reset(&conn);
    }
    let conn = testkit::connect(&db);
    seed(&conn);
    let q = SAVED_QUERIES
        .iter()
        .find(|q| q.name == "cold-start-overview")
        .expect("cold-start-overview must exist");
    let mut result = conn.query(sql_of(q.name)).expect("query runs");
    let row = result.next().expect("at least 1 row");
    let _ = std::fs::remove_dir_all(&dir);
    row
}

fn extract_string(v: &kv::Value, label: &str) -> String {
    if let kv::Value::String(s) = v {
        s.clone()
    } else {
        panic!("{label} must be String: {v:?}");
    }
}

#[test]
fn cold_start_overview_emits_all_3_categorical_labels() {
    // Per iter 211 (user-probe-050 Finding E): composite
    // probe emitting 9 columns total (6 counts + 3 labels).
    // Seed 1 File + 1 Function + 1 narrative Doc + 1
    // COUPLED_WITH → expect full-scip + doc-sparse +
    // coupling-sparse.
    let row = run_cold_start_overview_with_fixture(|conn| {
        conn.query(
            "CREATE (:File {path: 'a.rs', language: 'rust', loc: 100, last_touched: '2026-06-01'})",
        )
        .unwrap();
        conn.query(
            "CREATE (:File {path: 'b.rs', language: 'rust', loc: 100, last_touched: '2026-06-01'})",
        )
        .unwrap();
        conn.query("CREATE (:Function {symbol: 'foo()', crate: 'd', file: 'a.rs', line: 1, doc_comment: '', language: 'rust', signature: 'fn foo()', body_excerpt: '', last_touched: '2026-06-01', repo_id: 'd'})").unwrap();
        conn.query("CREATE (:Doc {id: 'a-doc', path: 'a.md', role: 'doc', kind: 'reference', lifecycle: '', bounded_context: '', title: 'A', summary: '', status: 'stable', updated: '2026-06-01', tags: [], covers: []})").unwrap();
        conn.query("MATCH (a:File {path: 'a.rs'}), (b:File {path: 'b.rs'}) CREATE (a)-[:COUPLED_WITH {commits: 7, jaccard: 0.5, last_co_change_at: '2026-05-01T00:00:00Z'}]->(b)").unwrap();
    });
    // Columns: fn_count, file_count, doc_count, narrative_doc_count,
    // coupled_with_count, endpoint_count, ingest_state,
    // doc_axis_regime, coupling_axis.
    assert_eq!(row.len(), 9, "9 columns: {row:?}");
    assert_eq!(extract_string(&row[6], "ingest_state"), "full-scip");
    assert_eq!(extract_string(&row[7], "doc_axis_regime"), "doc-sparse");
    assert_eq!(extract_string(&row[8], "coupling_axis"), "coupling-sparse");
}

#[test]
fn cold_start_overview_emits_empty_doc_axis_and_coupling_empty_on_minimal() {
    // 0 Functions + 0 Files + 0 Docs + 0 COUPLED_WITH →
    // expect empty + doc-empty-ontology-only + coupling-empty.
    let row = run_cold_start_overview_with_fixture(|_conn| {});
    assert_eq!(row.len(), 9, "9 columns: {row:?}");
    assert_eq!(extract_string(&row[6], "ingest_state"), "empty");
    assert_eq!(
        extract_string(&row[7], "doc_axis_regime"),
        "doc-empty-ontology-only"
    );
    assert_eq!(extract_string(&row[8], "coupling_axis"), "coupling-empty");
}

#[test]
fn cold_start_overview_emits_doc_rich_and_coupling_rich_on_large_corpus() {
    // 50 narrative docs (doc-rich threshold) + 51 COUPLED_WITH
    // edges (coupling-rich threshold). No Functions or Files
    // beyond what's needed for the coupling edges →
    // doc-only-by-design ingest_state.
    let row = run_cold_start_overview_with_fixture(|conn| {
        for i in 0..50 {
            let id = format!("narr-{i}");
            let cypher = format!(
                "CREATE (:Doc {{id: '{id}', path: '{id}.md', role: 'doc', kind: 'reference', \
                     lifecycle: '', bounded_context: '', title: 'T', summary: '', \
                     status: 'stable', updated: '2026-06-01', tags: [], covers: []}})"
            );
            conn.query(&cypher).expect("seed Doc");
        }
        // Need 51 COUPLED_WITH edges to exceed the
        // coupling-rich threshold (>50). Each pair of files
        // makes 1 edge; seed 52 files + 51 chained pairs.
        for i in 0..52 {
            let cypher = format!(
                    "CREATE (:File {{path: 'f{i}.rs', language: 'rust', loc: 100, last_touched: '2026-06-01'}})"
                );
            conn.query(&cypher).expect("seed File");
        }
        for i in 0..51 {
            let j = i + 1;
            let cypher = format!(
                "MATCH (a:File {{path: 'f{i}.rs'}}), (b:File {{path: 'f{j}.rs'}}) \
                     CREATE (a)-[:COUPLED_WITH {{commits: 8, jaccard: 0.5, \
                     last_co_change_at: '2026-05-01T00:00:00Z'}}]->(b)"
            );
            conn.query(&cypher).expect("seed COUPLED_WITH");
        }
    });
    assert_eq!(row.len(), 9, "9 columns");
    // fn_count = 0 so ingest_state branches on file_count + fn_count.
    assert_eq!(
        extract_string(&row[6], "ingest_state"),
        "file-only-no-scip",
        "files but no functions: {row:?}"
    );
    assert_eq!(extract_string(&row[7], "doc_axis_regime"), "doc-rich");
    assert_eq!(extract_string(&row[8], "coupling_axis"), "coupling-rich");
}

fn run_named_classifier_with_fixture<F>(name: &str, seed: F) -> Vec<kv::Value>
where
    F: FnOnce(&testkit::Conn<'_>),
{
    let dir = std::env::temp_dir().join(format!(
        "doc-linter-saved-{name}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let db = testkit::open_db(&dir);
    {
        let conn = testkit::connect(&db);
        testkit::reset(&conn);
    }
    let conn = testkit::connect(&db);
    seed(&conn);
    let q = SAVED_QUERIES
        .iter()
        .find(|q| q.name == name)
        .unwrap_or_else(|| panic!("{name} must exist"));
    let mut result = conn.query(sql_of(q.name)).expect("query runs");
    let row = result.next().expect("at least 1 row");
    let _ = std::fs::remove_dir_all(&dir);
    row
}

#[test]
fn corpus_routing_recommendation_emits_doc_sparse_coupling_rich_on_trusted_shape() {
    // Per iter 261 (closes [[user-probe-063]] Finding C
    // 3-tier classification): trusted FastAPI shape =
    // 142 Files + 0 Functions + few Docs + many
    // COUPLED_WITH = doc-sparse-coupling-rich route.
    // Fixture compresses: 50 Files + 1 COUPLED_WITH +
    // 0 Functions + 0 narrative Docs.
    let row = run_named_classifier_with_fixture("corpus-routing-recommendation", |conn| {
        for i in 0..50 {
            conn.query(&format!(
                "CREATE (:File {{path: 'f{i}.py', language: 'python', \
                         loc: 100, last_touched: '2026-06-01'}})"
            ))
            .unwrap();
        }
        conn.query(
            "MATCH (a:File {path: 'f0.py'}), (b:File {path: 'f1.py'}) \
                     CREATE (a)-[:COUPLED_WITH {commits: 5, jaccard: 0.5, \
                     last_co_change_at: '2026-05-01T00:00:00Z'}]->(b)",
        )
        .unwrap();
    });
    // Columns 0-3 are counts; 4 = route; 5 = primitives.
    let route = extract_string(&row[4], "route");
    let primitives = extract_string(&row[5], "primitives");
    assert_eq!(
        route, "doc-sparse-coupling-rich",
        "trusted FastAPI shape must route to doc-sparse-coupling-rich: {row:?}",
    );
    assert!(
        primitives.contains("file-coupling-degree"),
        "recommendation must name file-coupling-degree: {primitives}",
    );
}

#[test]
fn corpus_routing_recommendation_emits_function_rich_on_spacy_shape() {
    // Per iter 261 + iter-260 [[user-probe-063]] spacy
    // shape: ~800 Files + ~4000 Functions + 0 COUPLED_WITH
    // + ~80 Docs (under doc-rich threshold 50, hmm — wait,
    // spacy has 91 Docs which IS >= 50). For the
    // function-rich-coupling-empty case we need 0
    // narrative Docs OR explicitly under the 50 threshold.
    // Use 49 docs + 100 functions + 50 files + 0 coupling.
    let row = run_named_classifier_with_fixture("corpus-routing-recommendation", |conn| {
        for i in 0..49 {
            conn.query(&format!(
                "CREATE (:Doc {{id: 'narr-{i}', path: 'narr-{i}.md', \
                         role: 'doc', kind: 'reference', lifecycle: '', \
                         bounded_context: '', title: 'T', summary: '', \
                         status: 'stable', updated: '2026-06-01', tags: [], covers: []}})"
            ))
            .unwrap();
        }
        for i in 0..50 {
            conn.query(&format!(
                "CREATE (:File {{path: 'f{i}.py', language: 'python', \
                         loc: 100, last_touched: '2026-06-01'}})"
            ))
            .unwrap();
        }
        for i in 0..100 {
            conn.query(&format!(
                "CREATE (:Function {{symbol: 'fn-{i}()', crate: 'c', \
                         file: 'f0.py', line: {i}, doc_comment: '', \
                         language: 'python', signature: 'def fn-{i}():', \
                         body_excerpt: '', last_touched: '2026-06-01', repo_id: 'r'}})"
            ))
            .unwrap();
        }
    });
    let route = extract_string(&row[4], "route");
    let primitives = extract_string(&row[5], "primitives");
    assert_eq!(
        route, "doc-sparse-coupling-empty-function-rich",
        "spacy shape must route to function-rich variant: {row:?}",
    );
    assert!(
        primitives.contains("query_similar type=function"),
        "recommendation must name Function-axis primitive: {primitives}",
    );
}

#[test]
fn corpus_routing_recommendation_emits_doc_rich_on_design_shape() {
    // Per iter 261: design corpus shape = many narrative
    // Docs + few Functions = doc-rich-function-sparse route.
    let row = run_named_classifier_with_fixture("corpus-routing-recommendation", |conn| {
        for i in 0..60 {
            conn.query(&format!(
                "CREATE (:Doc {{id: 'narr-{i}', path: 'narr-{i}.md', \
                         role: 'doc', kind: 'reference', lifecycle: '', \
                         bounded_context: '', title: 'T', summary: '', \
                         status: 'stable', updated: '2026-06-01', tags: [], covers: []}})"
            ))
            .unwrap();
        }
    });
    let route = extract_string(&row[4], "route");
    let primitives = extract_string(&row[5], "primitives");
    assert_eq!(
            route, "doc-rich-function-sparse",
            "design corpus shape (many docs + 0 functions) must route to doc-rich-function-sparse: {row:?}",
        );
    assert!(
        primitives.contains("research-by-tag") && primitives.contains("query_similar"),
        "recommendation must name Doc-axis primitives: {primitives}",
    );
}

#[test]
fn corpus_axis_saturation_reports_per_axis_metadata_fractions() {
    // Per iter 261: fixture seeds 3 docs (2 with summary, 1
    // without), 4 files (3 with language, 1 without), 5
    // functions (2 with signature, 3 without — matches the
    // Python-empty-signature pattern per
    // [[feedback_function_signature_column_empty_py]]).
    let row = run_named_classifier_with_fixture("corpus-axis-saturation", |conn| {
        for (id, summary) in [
            ("d1", "First doc summary"),
            ("d2", "Second doc summary"),
            ("d3", ""),
        ] {
            conn.query(&format!(
                "CREATE (:Doc {{id: '{id}', path: '{id}.md', role: 'doc', \
                     kind: 'reference', lifecycle: '', bounded_context: '', \
                     title: 'T', summary: '{summary}', status: 'stable', \
                     updated: '2026-06-01', tags: [], covers: []}})"
            ))
            .unwrap();
        }
        for (path, language) in [
            ("a.py", "python"),
            ("b.py", "python"),
            ("c.tsx", "tsx"),
            ("d.unknown", ""),
        ] {
            conn.query(&format!(
                "CREATE (:File {{path: '{path}', language: '{language}', \
                     loc: 100, last_touched: '2026-06-01'}})"
            ))
            .unwrap();
        }
        for (sym, sig) in [
            ("fn1()", "def fn1():"),
            ("fn2()", "def fn2():"),
            ("fn3()", ""),
            ("fn4()", ""),
            ("fn5()", ""),
        ] {
            conn.query(&format!(
                "CREATE (:Function {{symbol: '{sym}', crate: 'c', file: 'a.py', \
                     line: 1, doc_comment: '', language: 'python', \
                     signature: '{sig}', body_excerpt: '', last_touched: '2026-06-01', \
                     repo_id: 'r'}})"
            ))
            .unwrap();
        }
    });
    // Columns: doc_count, docs_with_summary, file_count,
    // files_with_language, function_count, functions_with_signature,
    // docs_summary_fraction, files_language_fraction, functions_signature_fraction.
    let extract_count = |v: &kv::Value, label: &str| -> i64 {
        match v {
            kv::Value::Int64(n) => *n,
            kv::Value::Int32(n) => *n as i64,
            kv::Value::UInt64(n) => *n as i64,
            kv::Value::Int128(n) => *n as i64,
            other => panic!("{label} must be int: {other:?}"),
        }
    };
    let extract_float = |v: &kv::Value, label: &str| -> f64 {
        match v {
            kv::Value::Double(n) => *n,
            kv::Value::Float(n) => *n as f64,
            kv::Value::Int64(n) => *n as f64,
            other => panic!("{label} must be number: {other:?}"),
        }
    };
    assert_eq!(extract_count(&row[0], "doc_count"), 3);
    assert_eq!(extract_count(&row[1], "docs_with_summary"), 2);
    assert_eq!(extract_count(&row[2], "file_count"), 4);
    assert_eq!(extract_count(&row[3], "files_with_language"), 3);
    assert_eq!(extract_count(&row[4], "function_count"), 5);
    assert_eq!(extract_count(&row[5], "functions_with_signature"), 2);
    let docs_frac = extract_float(&row[6], "docs_summary_fraction");
    let files_frac = extract_float(&row[7], "files_language_fraction");
    let fns_frac = extract_float(&row[8], "functions_signature_fraction");
    assert!(
        (docs_frac - 2.0 / 3.0).abs() < 0.01,
        "docs ≈ 0.67: {docs_frac}"
    );
    assert!(
        (files_frac - 0.75).abs() < 0.01,
        "files = 0.75: {files_frac}"
    );
    assert!((fns_frac - 0.4).abs() < 0.01, "fns = 0.4: {fns_frac}");
}

#[test]
fn doc_role_distribution_orders_by_count_desc() {
    // Per iter 264 (closes [[interrogation-056]] Finding B):
    // partition Docs by role, ordered by count DESC.
    // Fixture: 3 narrative docs + 2 ontology-entity + 1 feature.
    // Expected: doc(3) > ontology-entity(2) > feature(1).
    let dir = std::env::temp_dir().join(format!(
        "doc-linter-saved-doc-role-distribution-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let db = testkit::open_db(&dir);
    {
        let conn = testkit::connect(&db);
        testkit::reset(&conn);
    }
    let conn = testkit::connect(&db);
    for (id, role) in [
        ("d-1", "doc"),
        ("d-2", "doc"),
        ("d-3", "doc"),
        ("e-1", "ontology-entity"),
        ("e-2", "ontology-entity"),
        ("f-1", "feature"),
    ] {
        conn.query(&format!(
            "CREATE (:Doc {{id: '{id}', path: '{id}.md', role: '{role}', \
                 kind: 'reference', lifecycle: '', bounded_context: '', \
                 title: 'T', summary: '', status: 'stable', updated: \
                 '2026-06-01', tags: [], covers: []}})"
        ))
        .unwrap();
    }
    let q = SAVED_QUERIES
        .iter()
        .find(|q| q.name == "doc-role-distribution")
        .expect("doc-role-distribution must exist");
    let mut result = conn.query(sql_of(q.name)).expect("query runs");
    let mut returned: Vec<(String, i64)> = Vec::new();
    for row in &mut result {
        let role = if let kv::Value::String(s) = &row[0] {
            s.clone()
        } else {
            panic!("role must be String: {row:?}");
        };
        let count = match &row[1] {
            kv::Value::Int64(n) => *n,
            kv::Value::Int32(n) => *n as i64,
            kv::Value::UInt64(n) => *n as i64,
            other => panic!("doc_count must be int: {other:?}"),
        };
        returned.push((role, count));
    }
    assert_eq!(
        returned[0],
        ("doc".to_string(), 3),
        "doc role must rank first at 3: {returned:?}",
    );
    assert!(
        returned
            .iter()
            .any(|(r, c)| r == "ontology-entity" && *c == 2),
        "ontology-entity must surface at 2: {returned:?}",
    );
    assert!(
        returned.iter().any(|(r, c)| r == "feature" && *c == 1),
        "feature must surface at 1: {returned:?}",
    );
    for window in returned.windows(2) {
        assert!(
            window[0].1 >= window[1].1,
            "must order by doc_count DESC: {returned:?}",
        );
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn loop_process_doc_distribution_partitions_by_id_prefix() {
    // Per iter 264 ([[feedback_meta_doc_displaces_research]]
    // operationalisation): partition Docs into the loop's
    // process classes via id-prefix matching. Fixture:
    //   research-a, research-b → research=2
    //   interrogation-a, user-probe-a, audit-run-a → meta=3
    //   entity-x → ontology=1
    //   feature-y → feature=1
    //   gap-z → gap=1
    //   doc-design-misc with role=doc, id NOT in any prefix
    //     → other-narrative=1
    // meta_to_research_ratio = 3/2 = 1.5
    let row = run_named_classifier_with_fixture("loop-process-doc-distribution", |conn| {
        for (id, role) in [
            ("research-a", "doc"),
            ("research-b", "doc"),
            ("interrogation-a", "doc"),
            ("user-probe-a", "doc"),
            ("audit-run-a", "doc"),
            ("entity-x", "ontology-entity"),
            ("feature-y", "feature"),
            ("gap-z", "doc"),
            ("doc-design-misc", "doc"),
        ] {
            conn.query(&format!(
                "CREATE (:Doc {{id: '{id}', path: '{id}.md', \
                         role: '{role}', kind: 'reference', lifecycle: '', \
                         bounded_context: '', title: 'T', summary: '', \
                         status: 'stable', updated: '2026-06-01', \
                         tags: [], covers: []}})"
            ))
            .unwrap();
        }
    });
    let extract_count = |v: &kv::Value, label: &str| -> i64 {
        match v {
            kv::Value::Int64(n) => *n,
            kv::Value::Int32(n) => *n as i64,
            kv::Value::UInt64(n) => *n as i64,
            kv::Value::Int128(n) => *n as i64,
            other => panic!("{label} must be int: {other:?}"),
        }
    };
    let extract_float = |v: &kv::Value, label: &str| -> f64 {
        match v {
            kv::Value::Double(n) => *n,
            kv::Value::Float(n) => *n as f64,
            other => panic!("{label} must be number: {other:?}"),
        }
    };
    // iter 267 refined columns:
    //   [0] research_count, [1] interrogation_count,
    //   [2] user_probe_count, [3] audit_run_count,
    //   [4] ontology_count, [5] feature_count,
    //   [6] gap_count, [7] other_narrative_count,
    //   [8] meta_to_research_ratio,
    //   [9] interrogation_to_user_probe_ratio.
    assert_eq!(extract_count(&row[0], "research_count"), 2);
    assert_eq!(extract_count(&row[1], "interrogation_count"), 1);
    assert_eq!(extract_count(&row[2], "user_probe_count"), 1);
    assert_eq!(extract_count(&row[3], "audit_run_count"), 1);
    assert_eq!(extract_count(&row[4], "ontology_count"), 1);
    assert_eq!(extract_count(&row[5], "feature_count"), 1);
    assert_eq!(extract_count(&row[6], "gap_count"), 1);
    assert_eq!(extract_count(&row[7], "other_narrative_count"), 1);
    let meta_ratio = extract_float(&row[8], "meta_to_research_ratio");
    assert!(
        (meta_ratio - 1.5).abs() < 0.01,
        "meta_to_research_ratio = (1+1+1)/2 = 1.5: {meta_ratio}",
    );
    let interrog_ratio = extract_float(&row[9], "interrogation_to_user_probe_ratio");
    assert!(
        (interrog_ratio - 1.0).abs() < 0.01,
        "interrogation_to_user_probe_ratio = 1/1 = 1.0: {interrog_ratio}",
    );
}

#[test]
fn latest_by_loop_process_returns_latest_per_class() {
    // Per iter 267 (sibling to refined loop-process-doc-
    // distribution): given fixtures with 2 docs per class
    // (different updated dates), return the latest doc per
    // class. Fixture: 2 research + 2 interrogation + 2
    // user-probe + 2 audit-run with 'old' and 'new' updated
    // values. Expected: 4 rows (1 per class), each with
    // the NEW doc.
    let dir = std::env::temp_dir().join(format!(
        "doc-linter-saved-latest-by-loop-process-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let db = testkit::open_db(&dir);
    {
        let conn = testkit::connect(&db);
        testkit::reset(&conn);
    }
    let conn = testkit::connect(&db);
    for (id, updated) in [
        ("research-old", "2026-01-01"),
        ("research-new", "2026-06-01"),
        ("interrogation-old", "2026-02-01"),
        ("interrogation-new", "2026-06-15"),
        ("user-probe-old", "2026-03-01"),
        ("user-probe-new", "2026-07-01"),
        ("audit-run-old", "2026-04-01"),
        ("audit-run-new", "2026-08-01"),
    ] {
        conn.query(&format!(
            "CREATE (:Doc {{id: '{id}', path: '{id}.md', role: 'doc', \
                 kind: 'reference', lifecycle: '', bounded_context: '', \
                 title: '{id}', summary: '', status: 'stable', updated: \
                 '{updated}', tags: [], covers: []}})"
        ))
        .unwrap();
    }
    let q = SAVED_QUERIES
        .iter()
        .find(|q| q.name == "latest-by-loop-process")
        .expect("latest-by-loop-process must exist");
    let mut result = conn.query(sql_of(q.name)).expect("query runs");
    let mut returned: Vec<(String, String)> = Vec::new();
    for row in &mut result {
        let class = if let kv::Value::String(s) = &row[0] {
            s.clone()
        } else {
            panic!("process_class must be String: {row:?}");
        };
        let latest_id = if let kv::Value::String(s) = &row[1] {
            s.clone()
        } else {
            panic!("latest_doc_id must be String: {row:?}");
        };
        returned.push((class, latest_id));
    }
    // 4 classes; each gets the new (later) doc.
    for (class, expected_id) in [
        ("audit-run", "audit-run-new"),
        ("interrogation", "interrogation-new"),
        ("research", "research-new"),
        ("user-probe", "user-probe-new"),
    ] {
        assert!(
            returned
                .iter()
                .any(|(c, id)| c == class && id == expected_id),
            "{class} must surface latest {expected_id}: {returned:?}",
        );
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn latest_by_loop_process_handles_uniform_timestamp_ties() {
    // Per iter 274 (closes [[interrogation-058]] Finding B
    // BUG): the loop's authoring discipline produces
    // uniform timestamps within an iteration; latest-by-
    // loop-process must return EXACTLY 1 row per class,
    // not all docs tied at max(updated). Fixture: 5 docs
    // per class ALL at the SAME updated date. Pre-iter-
    // 274 the query returned 20 rows; post-fix returns 4
    // (one per class, using max(id) tie-break).
    let dir = std::env::temp_dir().join(format!(
        "doc-linter-saved-latest-by-loop-process-ties-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let db = testkit::open_db(&dir);
    {
        let conn = testkit::connect(&db);
        testkit::reset(&conn);
    }
    let conn = testkit::connect(&db);
    // 5 docs per class, all tied at 2026-06-01.
    // max(id) per class should pick the lexicographically
    // highest: research-005, interrogation-005, user-probe-005,
    // audit-run-005.
    for class in ["research", "interrogation", "user-probe", "audit-run"] {
        for i in 1..=5 {
            let id = format!("{class}-{i:03}");
            conn.query(&format!(
                "CREATE (:Doc {{id: '{id}', path: '{id}.md', role: 'doc', \
                     kind: 'reference', lifecycle: '', bounded_context: '', \
                     title: '{id}', summary: '', status: 'stable', updated: \
                     '2026-06-01', tags: [], covers: []}})"
            ))
            .unwrap();
        }
    }
    let q = SAVED_QUERIES
        .iter()
        .find(|q| q.name == "latest-by-loop-process")
        .expect("latest-by-loop-process must exist");
    let mut result = conn.query(sql_of(q.name)).expect("query runs");
    let mut returned: Vec<(String, String)> = Vec::new();
    for row in &mut result {
        let class = if let kv::Value::String(s) = &row[0] {
            s.clone()
        } else {
            panic!("process_class must be String: {row:?}");
        };
        let latest_id = if let kv::Value::String(s) = &row[1] {
            s.clone()
        } else {
            panic!("latest_doc_id must be String: {row:?}");
        };
        returned.push((class, latest_id));
    }
    assert_eq!(
        returned.len(),
        4,
        "EXACTLY 4 rows expected (1 per class) — pre-iter-274 returned 20: {returned:?}",
    );
    for (class, expected) in [
        ("audit-run", "audit-run-005"),
        ("interrogation", "interrogation-005"),
        ("research", "research-005"),
        ("user-probe", "user-probe-005"),
    ] {
        assert!(
            returned.iter().any(|(c, id)| c == class && id == expected),
            "{class} must surface max-id {expected}: {returned:?}",
        );
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn loop_process_balance_metric_classifies_v_heavy_balanced_i_heavy() {
    // Per iter 274 (operationalises [[interrogation-058]]
    // Finding C): 3 fixture scenarios for the 3 categorical
    // labels.
    // Helper to seed N docs of given prefix.
    let seed = |conn: &testkit::Conn<'_>, prefix: &str, n: usize| {
        for i in 0..n {
            let id = format!("{prefix}-{i:03}");
            conn.query(&format!(
                "CREATE (:Doc {{id: '{id}', path: '{id}.md', role: 'doc', \
                     kind: 'reference', lifecycle: '', bounded_context: '', \
                     title: 'T', summary: '', status: 'stable', updated: \
                     '2026-06-01', tags: [], covers: []}})"
            ))
            .unwrap();
        }
    };
    // V-heavy fixture: 13 user-probes / 10 interrogations
    // = 1.3 > 1.2 threshold.
    let label_v = {
        let dir =
            std::env::temp_dir().join(format!("doc-linter-saved-balance-v-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let db = testkit::open_db(&dir);
        {
            let conn = testkit::connect(&db);
            testkit::reset(&conn);
        }
        let conn = testkit::connect(&db);
        seed(&conn, "user-probe", 13);
        seed(&conn, "interrogation", 10);
        let q = SAVED_QUERIES
            .iter()
            .find(|q| q.name == "loop-process-balance-metric")
            .expect("loop-process-balance-metric must exist");
        let mut result = conn.query(sql_of(q.name)).expect("query runs");
        let row = result.next().expect("at least 1 row");
        let _ = std::fs::remove_dir_all(&dir);
        if let kv::Value::String(s) = &row[3] {
            s.clone()
        } else {
            panic!("balance_label String: {row:?}");
        }
    };
    assert_eq!(label_v, "V-heavy", "1.3 ratio should classify as V-heavy");
    // Balanced fixture: 10/10 = 1.0 (within 0.83-1.2).
    let label_balanced = {
        let dir = std::env::temp_dir().join(format!(
            "doc-linter-saved-balance-bal-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let db = testkit::open_db(&dir);
        {
            let conn = testkit::connect(&db);
            testkit::reset(&conn);
        }
        let conn = testkit::connect(&db);
        seed(&conn, "user-probe", 10);
        seed(&conn, "interrogation", 10);
        let q = SAVED_QUERIES
            .iter()
            .find(|q| q.name == "loop-process-balance-metric")
            .expect("must exist");
        let mut result = conn.query(sql_of(q.name)).expect("query runs");
        let row = result.next().expect("at least 1 row");
        let _ = std::fs::remove_dir_all(&dir);
        if let kv::Value::String(s) = &row[3] {
            s.clone()
        } else {
            panic!("balance_label String: {row:?}");
        }
    };
    assert_eq!(label_balanced, "balanced", "1.0 ratio should be balanced");
    // I-heavy fixture: 5/10 = 0.5 < 0.83.
    let label_i = {
        let dir =
            std::env::temp_dir().join(format!("doc-linter-saved-balance-i-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let db = testkit::open_db(&dir);
        {
            let conn = testkit::connect(&db);
            testkit::reset(&conn);
        }
        let conn = testkit::connect(&db);
        seed(&conn, "user-probe", 5);
        seed(&conn, "interrogation", 10);
        let q = SAVED_QUERIES
            .iter()
            .find(|q| q.name == "loop-process-balance-metric")
            .expect("must exist");
        let mut result = conn.query(sql_of(q.name)).expect("query runs");
        let row = result.next().expect("at least 1 row");
        let _ = std::fs::remove_dir_all(&dir);
        if let kv::Value::String(s) = &row[3] {
            s.clone()
        } else {
            panic!("balance_label String: {row:?}");
        }
    };
    assert_eq!(label_i, "I-heavy", "0.5 ratio should classify as I-heavy");
}

#[test]
fn corpus_doc_starter_ratio_classifies_three_regimes() {
    // Per iter 281 ([[user-probe-068]] Finding B + E
    // operationalisation): 1-call doc-null detector.
    // 3 fixture scenarios pin the 3 labels:
    //   doc-null:        5 starter / 0 authored → 'doc-null'
    //   mostly-starter:  4 starter / 1 authored → 'mostly-starter'
    //   mostly-authored: 1 starter / 4 authored → 'mostly-authored'
    let seed_starter = |conn: &testkit::Conn<'_>, n: usize| {
        let kinds = [
            ("ontology-value", "value-test"),
            ("ontology-axis", "axis-test"),
            ("ontology-entity", "entity-test"),
            ("ontology-migration", "ontology-mig-test"),
            ("index", "ontology-index-test"),
        ];
        for i in 0..n {
            let (role, prefix) = kinds[i % kinds.len()];
            let id = format!("{prefix}-{i:03}");
            conn.query(&format!(
                "CREATE (:Doc {{id: '{id}', path: '{id}.md', \
                     role: '{role}', kind: '', lifecycle: '', \
                     bounded_context: '', title: 'T', summary: '', \
                     status: 'stable', updated: '2026-06-01', tags: [], \
                     covers: []}})"
            ))
            .unwrap();
        }
    };
    let seed_authored = |conn: &testkit::Conn<'_>, n: usize| {
        for i in 0..n {
            let id = format!("research-test-{i:03}");
            conn.query(&format!(
                "CREATE (:Doc {{id: '{id}', path: '{id}.md', \
                     role: 'doc', kind: 'reference', lifecycle: '', \
                     bounded_context: '', title: 'T', summary: '', \
                     status: 'stable', updated: '2026-06-01', tags: [], \
                     covers: []}})"
            ))
            .unwrap();
        }
    };
    let run = |starter: usize, authored: usize| -> (i64, i64, i64, f64, String) {
        let dir = std::env::temp_dir().join(format!(
            "doc-linter-saved-starter-ratio-{}-{starter}-{authored}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let db = testkit::open_db(&dir);
        {
            let conn = testkit::connect(&db);
            testkit::reset(&conn);
        }
        let conn = testkit::connect(&db);
        seed_starter(&conn, starter);
        seed_authored(&conn, authored);
        let q = SAVED_QUERIES
            .iter()
            .find(|q| q.name == "corpus-doc-starter-ratio")
            .expect("corpus-doc-starter-ratio must exist");
        let mut result = conn.query(sql_of(q.name)).expect("query runs");
        let row = result.next().expect("at least 1 row");
        // sum() returns Int128; count() returns Int64. Per iter-
        // 253 Int128 footgun.
        let as_i64 = |v: &kv::Value| -> i64 {
            match v {
                kv::Value::Int64(n) => *n,
                kv::Value::Int128(n) => *n as i64,
                other => panic!("expected int: {other:?}"),
            }
        };
        let s_count = as_i64(&row[0]);
        let a_count = as_i64(&row[1]);
        let t_count = as_i64(&row[2]);
        let ratio = if let kv::Value::Double(f) = &row[3] {
            *f
        } else {
            panic!("starter_ratio Double: {row:?}");
        };
        let label = if let kv::Value::String(s) = &row[4] {
            s.clone()
        } else {
            panic!("shape_label String: {row:?}");
        };
        let _ = std::fs::remove_dir_all(&dir);
        (s_count, a_count, t_count, ratio, label)
    };
    let (sc, ac, tc, r, l) = run(5, 0);
    assert_eq!((sc, ac, tc), (5, 0, 5));
    assert!((r - 1.0).abs() < 1e-9, "ratio should be 1.0: {r}");
    assert_eq!(l, "doc-null", "5 starter / 0 authored = doc-null");
    let (sc, ac, tc, r, l) = run(4, 1);
    assert_eq!((sc, ac, tc), (4, 1, 5));
    assert!((r - 0.8).abs() < 1e-9, "ratio should be 0.8: {r}");
    assert_eq!(l, "mostly-starter", "4/5 = 0.8 = mostly-starter");
    let (sc, ac, tc, r, l) = run(1, 4);
    assert_eq!((sc, ac, tc), (1, 4, 5));
    assert!((r - 0.2).abs() < 1e-9, "ratio should be 0.2: {r}");
    assert_eq!(l, "mostly-authored", "1/5 = 0.2 = mostly-authored");
    // iter-336: 'authored-mixed' label for 0.3 <= ratio < 0.8
    // 2 starter + 3 authored = ratio 0.4 = authored-mixed
    let (sc, ac, tc, r, l) = run(2, 3);
    assert_eq!((sc, ac, tc), (2, 3, 5));
    assert!((r - 0.4).abs() < 1e-9, "ratio should be 0.4: {r}");
    assert_eq!(l, "authored-mixed", "2/5 = 0.4 = authored-mixed");
}

#[test]
fn corpus_narrative_doc_roles_distribution_per_role_counts() {
    // Per iter 336: returns per-role narrative_doc_count
    // for NON-starter Docs only. Fixture: 2 starter (filtered)
    // + 3 'research' role + 2 'how-to' role + 1 'feature'.
    let dir = std::env::temp_dir().join(format!(
        "doc-linter-saved-roles-dist-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let db = testkit::open_db(&dir);
    {
        let conn = testkit::connect(&db);
        testkit::reset(&conn);
    }
    let conn = testkit::connect(&db);
    let seed = |id: &str, role: &str| {
        conn.query(&format!(
            "CREATE (:Doc {{id: '{id}', path: '{id}.md', \
                 role: '{role}', kind: '', lifecycle: '', \
                 bounded_context: '', title: 'T', summary: '', \
                 status: 'stable', updated: '2026-06-01', \
                 tags: [], covers: []}})"
        ))
        .unwrap();
    };
    seed("value-test-1", "ontology-value");
    seed("axis-test-1", "ontology-axis");
    seed("research-test-1", "doc");
    seed("research-test-2", "doc");
    seed("research-test-3", "doc");
    seed("howto-test-1", "how-to");
    seed("howto-test-2", "how-to");
    seed("feature-test-1", "feature");

    let q = SAVED_QUERIES
        .iter()
        .find(|q| q.name == "corpus-narrative-doc-roles-distribution")
        .expect("corpus-narrative-doc-roles-distribution must exist");
    let mut result = conn.query(sql_of(q.name)).expect("query runs");
    let as_i64 = |v: &kv::Value| -> i64 {
        match v {
            kv::Value::Int64(n) => *n,
            kv::Value::Int128(n) => *n as i64,
            other => panic!("expected int: {other:?}"),
        }
    };
    let mut rows: Vec<(String, i64)> = Vec::new();
    for row in &mut result {
        let r = row.clone();
        let role = if let kv::Value::String(s) = &r[0] {
            s.clone()
        } else {
            panic!("role: {r:?}");
        };
        rows.push((role, as_i64(&r[1])));
    }
    let by_role = |r: &str| -> i64 {
        rows.iter()
            .find(|(role, _)| role == r)
            .unwrap_or_else(|| panic!("{r} must surface: {rows:?}"))
            .1
    };
    // 'doc' role with 3 narrative_docs first (ordered DESC)
    assert_eq!(by_role("doc"), 3);
    assert_eq!(by_role("how-to"), 2);
    assert_eq!(by_role("feature"), 1);
    // Starter docs (ontology-value + ontology-axis) excluded
    for noisy in ["ontology-value", "ontology-axis"] {
        assert!(
            !rows.iter().any(|(r, _)| r == noisy),
            "starter role `{noisy}` must NOT surface: {rows:?}"
        );
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn corpus_narrative_doc_kinds_distribution_per_kind_counts() {
    // Per iter 338: fixture: 1 starter (filtered) + 5 reference
    // + 2 how-to + 1 explanation. Expect 3 rows ordered DESC.
    let dir = std::env::temp_dir().join(format!(
        "doc-linter-saved-kinds-dist-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let db = testkit::open_db(&dir);
    {
        let conn = testkit::connect(&db);
        testkit::reset(&conn);
    }
    let conn = testkit::connect(&db);
    let seed = |id: &str, role: &str, kind: &str| {
        conn.query(&format!(
            "CREATE (:Doc {{id: '{id}', path: '{id}.md', \
                 role: '{role}', kind: '{kind}', lifecycle: '', \
                 bounded_context: '', title: 'T', summary: '', \
                 status: 'stable', updated: '2026-06-01', \
                 tags: [], covers: []}})"
        ))
        .unwrap();
    };
    seed("value-test-1", "ontology-value", "");
    for i in 0..5 {
        seed(&format!("ref-{i:03}"), "doc", "reference");
    }
    seed("how-1", "doc", "how-to");
    seed("how-2", "doc", "how-to");
    seed("exp-1", "doc", "explanation");

    let q = SAVED_QUERIES
        .iter()
        .find(|q| q.name == "corpus-narrative-doc-kinds-distribution")
        .expect("corpus-narrative-doc-kinds-distribution must exist");
    let mut result = conn.query(sql_of(q.name)).expect("query runs");
    let as_i64 = |v: &kv::Value| -> i64 {
        match v {
            kv::Value::Int64(n) => *n,
            kv::Value::Int128(n) => *n as i64,
            other => panic!("expected int: {other:?}"),
        }
    };
    let mut rows: Vec<(String, i64)> = Vec::new();
    for row in &mut result {
        let r = row.clone();
        let kind = if let kv::Value::String(s) = &r[0] {
            s.clone()
        } else {
            panic!("kind: {r:?}");
        };
        rows.push((kind, as_i64(&r[1])));
    }
    let by_kind = |k: &str| -> i64 {
        rows.iter()
            .find(|(kind, _)| kind == k)
            .unwrap_or_else(|| panic!("{k} must surface: {rows:?}"))
            .1
    };
    assert_eq!(by_kind("reference"), 5);
    assert_eq!(by_kind("how-to"), 2);
    assert_eq!(by_kind("explanation"), 1);
    // Ordering: reference (5) > how-to (2) > explanation (1).
    assert_eq!(rows[0].0, "reference");
    // Starter docs excluded (their kinds were '' but they're
    // filtered by role; iter-340 relabel applies to authored
    // docs only).
    assert!(
        !rows.iter().any(|(k, _)| k.is_empty()),
        "empty-kind starter must NOT surface: {rows:?}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn corpus_narrative_doc_kinds_distribution_relabels_empty_as_kind_unset() {
    // Per iter 340 ([[user-probe-089]] Finding H):
    // authored Docs with empty-string kind (e.g.
    // roadmap-entry role) must surface as 'kind-unset'
    // not literal '' — improves column readability.
    let dir = std::env::temp_dir().join(format!(
        "doc-linter-saved-kind-unset-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let db = testkit::open_db(&dir);
    {
        let conn = testkit::connect(&db);
        testkit::reset(&conn);
    }
    let conn = testkit::connect(&db);
    let seed = |id: &str, role: &str, kind: &str| {
        conn.query(&format!(
            "CREATE (:Doc {{id: '{id}', path: '{id}.md', \
                 role: '{role}', kind: '{kind}', lifecycle: '', \
                 bounded_context: '', title: 'T', summary: '', \
                 status: 'stable', updated: '2026-06-01', \
                 tags: [], covers: []}})"
        ))
        .unwrap();
    };
    // 3 roadmap-entry docs with empty kind + 1 reference doc.
    seed("roadmap-1", "roadmap-entry", "");
    seed("roadmap-2", "roadmap-entry", "");
    seed("roadmap-3", "roadmap-entry", "");
    seed("ref-1", "doc", "reference");

    let q = SAVED_QUERIES
        .iter()
        .find(|q| q.name == "corpus-narrative-doc-kinds-distribution")
        .expect("corpus-narrative-doc-kinds-distribution must exist");
    let mut result = conn.query(sql_of(q.name)).expect("query runs");
    let as_i64 = |v: &kv::Value| -> i64 {
        match v {
            kv::Value::Int64(n) => *n,
            kv::Value::Int128(n) => *n as i64,
            other => panic!("expected int: {other:?}"),
        }
    };
    let mut rows: Vec<(String, i64)> = Vec::new();
    for row in &mut result {
        let r = row.clone();
        let kind = if let kv::Value::String(s) = &r[0] {
            s.clone()
        } else {
            panic!("kind: {r:?}");
        };
        rows.push((kind, as_i64(&r[1])));
    }
    let by_kind = |k: &str| -> i64 {
        rows.iter()
            .find(|(kind, _)| kind == k)
            .unwrap_or_else(|| panic!("{k} must surface: {rows:?}"))
            .1
    };
    // Empty-kind 3 must surface as 'kind-unset'.
    assert_eq!(by_kind("kind-unset"), 3);
    assert_eq!(by_kind("reference"), 1);
    // Literal empty string must NOT surface (relabeled).
    assert!(
        !rows.iter().any(|(k, _)| k.is_empty()),
        "empty-string kind must be relabeled 'kind-unset': {rows:?}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn corpus_narrative_doc_lifecycles_distribution_per_lifecycle_counts() {
    // Per iter 340 (sibling): per-lifecycle distribution.
    // Fixture: 1 starter + 4 empty + 2 stable + 1 planning.
    let dir = std::env::temp_dir().join(format!(
        "doc-linter-saved-lifecycles-dist-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let db = testkit::open_db(&dir);
    {
        let conn = testkit::connect(&db);
        testkit::reset(&conn);
    }
    let conn = testkit::connect(&db);
    let seed = |id: &str, role: &str, lifecycle: &str| {
        conn.query(&format!(
            "CREATE (:Doc {{id: '{id}', path: '{id}.md', \
                 role: '{role}', kind: '', lifecycle: '{lifecycle}', \
                 bounded_context: '', title: 'T', summary: '', \
                 status: 'stable', updated: '2026-06-01', \
                 tags: [], covers: []}})"
        ))
        .unwrap();
    };
    seed("value-test-1", "ontology-value", "");
    for i in 0..4 {
        seed(&format!("unset-{i:03}"), "doc", "");
    }
    seed("stable-1", "doc", "stable");
    seed("stable-2", "doc", "stable");
    seed("planning-1", "doc", "planning");

    let q = SAVED_QUERIES
        .iter()
        .find(|q| q.name == "corpus-narrative-doc-lifecycles-distribution")
        .expect("corpus-narrative-doc-lifecycles-distribution must exist");
    let mut result = conn.query(sql_of(q.name)).expect("query runs");
    let as_i64 = |v: &kv::Value| -> i64 {
        match v {
            kv::Value::Int64(n) => *n,
            kv::Value::Int128(n) => *n as i64,
            other => panic!("expected int: {other:?}"),
        }
    };
    let mut rows: Vec<(String, i64)> = Vec::new();
    for row in &mut result {
        let r = row.clone();
        let lifecycle = if let kv::Value::String(s) = &r[0] {
            s.clone()
        } else {
            panic!("lifecycle: {r:?}");
        };
        rows.push((lifecycle, as_i64(&r[1])));
    }
    let by_lc = |l: &str| -> i64 {
        rows.iter()
            .find(|(lc, _)| lc == l)
            .unwrap_or_else(|| panic!("{l} must surface: {rows:?}"))
            .1
    };
    // Empty 4 → 'lifecycle-unset'
    assert_eq!(by_lc("lifecycle-unset"), 4);
    assert_eq!(by_lc("stable"), 2);
    assert_eq!(by_lc("planning"), 1);
    // Ordering by count DESC: unset (4) first.
    assert_eq!(rows[0].0, "lifecycle-unset");
    // Literal empty string must NOT surface (relabeled).
    assert!(
        !rows.iter().any(|(lc, _)| lc.is_empty()),
        "empty-string lifecycle must be relabeled: {rows:?}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn authored_docs_by_kind_returns_kind_filtered_authored() {
    // Per iter 338: fixture: 1 starter (filtered) + 3 reference
    // + 2 how-to. Query with $kind='reference' returns 3 ref docs;
    // $kind='how-to' returns 2.
    let dir = std::env::temp_dir().join(format!("doc-linter-saved-by-kind-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let db = testkit::open_db(&dir);
    {
        let conn = testkit::connect(&db);
        testkit::reset(&conn);
    }
    let conn = testkit::connect(&db);
    let seed = |id: &str, role: &str, kind: &str| {
        conn.query(&format!(
            "CREATE (:Doc {{id: '{id}', path: '{id}.md', \
                 role: '{role}', kind: '{kind}', lifecycle: '', \
                 bounded_context: '', title: 'T', summary: '', \
                 status: 'stable', updated: '2026-06-01', \
                 tags: [], covers: []}})"
        ))
        .unwrap();
    };
    seed("value-test-1", "ontology-value", "");
    for i in 0..3 {
        seed(&format!("ref-{i:03}"), "doc", "reference");
    }
    seed("how-1", "doc", "how-to");
    seed("how-2", "doc", "how-to");

    let q = SAVED_QUERIES
        .iter()
        .find(|q| q.name == "authored-docs-by-kind")
        .expect("authored-docs-by-kind must exist");
    let run_for = |kind: &str| -> Vec<String> {
        let cypher = sql_of(q.name).replace("$kind", &format!("'{kind}'"));
        let mut result = conn.query(&cypher).expect("query runs");
        let mut ids: Vec<String> = Vec::new();
        for row in &mut result {
            let r = row.clone();
            if let kv::Value::String(s) = &r[0] {
                ids.push(s.clone());
            }
        }
        ids
    };
    let ref_ids = run_for("reference");
    assert_eq!(ref_ids.len(), 3, "3 reference docs: {ref_ids:?}");
    for id in &ref_ids {
        assert!(id.starts_with("ref-"), "reference id must start ref-: {id}");
    }
    let how_ids = run_for("how-to");
    assert_eq!(how_ids.len(), 2, "2 how-to docs: {how_ids:?}");
    // Starter doc not in output (no matching kind).
    let starter_ids = run_for("");
    assert_eq!(
        starter_ids.len(),
        0,
        "starter not in any authored kind: {starter_ids:?}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn hub_functions_by_caller_breadth_classifies_spread() {
    // Per iter 342: 3 fixture functions with distinct
    // spread shapes:
    // - api-tier: callers spread across 5+ files
    // - mid-tier: callers in 2-4 files
    // - internal-cluster: all callers in 1 file
    let dir = std::env::temp_dir().join(format!(
        "doc-linter-saved-hub-breadth-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let db = testkit::open_db(&dir);
    {
        let conn = testkit::connect(&db);
        testkit::reset(&conn);
    }
    let conn = testkit::connect(&db);
    let seed_fn = |sym: &str, file: &str| {
        conn.query(&format!(
            "CREATE (:Function {{symbol: '{sym}', crate: 'c', \
                 file: '{file}', line: 1, doc_comment: '', \
                 language: 'rust', signature: '', body_excerpt: '', \
                 last_touched: '2026-06-01', repo_id: 'doc-linter'}})"
        ))
        .unwrap();
    };
    // 3 hub functions
    seed_fn("hub_api", "src/api.rs");
    seed_fn("hub_mid", "src/mid.rs");
    seed_fn("hub_internal", "src/cluster.rs");
    // api-tier: 5 distinct callers in 5 distinct files
    for i in 0..5 {
        seed_fn(&format!("caller_api_{i}"), &format!("src/file_{i}.rs"));
        conn.query(&format!(
            "MATCH (a:Function {{symbol: 'caller_api_{i}'}}), \
                 (b:Function {{symbol: 'hub_api'}}) \
                 CREATE (a)-[:CALLS]->(b)"
        ))
        .unwrap();
    }
    // mid-tier: 5 distinct callers in 3 files
    for i in 0..5 {
        seed_fn(&format!("caller_mid_{i}"), &format!("src/mid_f{i:01}.rs"));
        // map i to 3 distinct files via i%3
    }
    for i in 0..5 {
        let file_i = i % 3;
        seed_fn(
            &format!("caller_mid_v_{i}"),
            &format!("src/mid_v_{file_i}.rs"),
        );
        conn.query(&format!(
            "MATCH (a:Function {{symbol: 'caller_mid_v_{i}'}}), \
                 (b:Function {{symbol: 'hub_mid'}}) \
                 CREATE (a)-[:CALLS]->(b)"
        ))
        .unwrap();
    }
    // internal-cluster: 5 callers all in same file
    for i in 0..5 {
        seed_fn(&format!("caller_int_{i}"), "src/cluster.rs");
        conn.query(&format!(
            "MATCH (a:Function {{symbol: 'caller_int_{i}'}}), \
                 (b:Function {{symbol: 'hub_internal'}}) \
                 CREATE (a)-[:CALLS]->(b)"
        ))
        .unwrap();
    }

    let q = SAVED_QUERIES
        .iter()
        .find(|q| q.name == "hub-functions-by-caller-breadth")
        .expect("hub-functions-by-caller-breadth must exist");
    let mut result = conn.query(sql_of(q.name)).expect("query runs");
    let as_i64 = |v: &kv::Value| -> i64 {
        match v {
            kv::Value::Int64(n) => *n,
            kv::Value::Int128(n) => *n as i64,
            other => panic!("expected int: {other:?}"),
        }
    };
    let mut rows: Vec<(String, i64, i64, String)> = Vec::new();
    for row in &mut result {
        let r = row.clone();
        let sym = if let kv::Value::String(s) = &r[0] {
            s.clone()
        } else {
            panic!("symbol: {r:?}");
        };
        let dc = as_i64(&r[2]);
        let cf = as_i64(&r[3]);
        let label = if let kv::Value::String(s) = &r[4] {
            s.clone()
        } else {
            panic!("spread_label: {r:?}");
        };
        rows.push((sym, dc, cf, label));
    }
    let by_sym = |s: &str| -> (i64, i64, String) {
        let r = rows
            .iter()
            .find(|(sym, _, _, _)| sym == s)
            .unwrap_or_else(|| panic!("{s} must surface: {rows:?}"));
        (r.1, r.2, r.3.clone())
    };
    let (dc, cf, label) = by_sym("hub_api");
    assert_eq!(dc, 5);
    assert_eq!(cf, 5);
    assert_eq!(label, "api-tier-hub");
    let (dc, cf, label) = by_sym("hub_mid");
    assert_eq!(dc, 5);
    assert_eq!(cf, 3);
    assert_eq!(label, "mid-tier-hub");
    let (dc, cf, label) = by_sym("hub_internal");
    assert_eq!(dc, 5);
    assert_eq!(cf, 1);
    assert_eq!(label, "internal-cluster-hub");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn authored_docs_sample_excludes_starter_returns_authored() {
    // Per iter 281 (sibling of corpus-doc-starter-ratio):
    // Mixed fixture 3 starter + 2 authored — sample
    // returns exactly the 2 authored docs.
    let dir = std::env::temp_dir().join(format!(
        "doc-linter-saved-authored-sample-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let db = testkit::open_db(&dir);
    {
        let conn = testkit::connect(&db);
        testkit::reset(&conn);
    }
    let conn = testkit::connect(&db);
    for (id, role) in [
        ("value-foo", "ontology-value"),
        ("axis-foo", "ontology-axis"),
        ("ontology-index", "index"),
        ("research-bar", "doc"),
        ("user-probe-001", "doc"),
    ] {
        conn.query(&format!(
            "CREATE (:Doc {{id: '{id}', path: '{id}.md', \
                 role: '{role}', kind: 'reference', lifecycle: '', \
                 bounded_context: '', title: 'T', summary: '', \
                 status: 'stable', updated: '2026-06-01', tags: [], \
                 covers: []}})"
        ))
        .unwrap();
    }
    let q = SAVED_QUERIES
        .iter()
        .find(|q| q.name == "authored-docs-sample")
        .expect("authored-docs-sample must exist");
    let mut result = conn.query(sql_of(q.name)).expect("query runs");
    let mut ids: Vec<String> = Vec::new();
    for row in &mut result {
        if let kv::Value::String(s) = &row[0] {
            ids.push(s.clone());
        }
    }
    // Exactly the 2 non-starter docs.
    assert_eq!(ids.len(), 2, "expected 2 authored rows: {ids:?}");
    assert!(
        ids.contains(&"research-bar".to_string()),
        "must include research-bar: {ids:?}",
    );
    assert!(
        ids.contains(&"user-probe-001".to_string()),
        "must include user-probe-001: {ids:?}",
    );
    for excluded in ["value-foo", "axis-foo", "ontology-index"] {
        assert!(
            !ids.contains(&excluded.to_string()),
            "starter {excluded} must be excluded: {ids:?}",
        );
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn feature_evolution_coupling_hubs_semantic_returns_empty_on_scaffold_only() {
    // Per user-probe-048 Finding A: hub-view at neighbor
    // count ≥ 2 needs ≥3 mutually-coupled feature files;
    // the fixture has only 1 feature pair so hub-view
    // returns 0 rows. Scaffold-clique has 3 nodes each
    // with 2 scaffold neighbors, but they're filtered by
    // commits >= 5. THIS test pins that hub-view CORRECTLY
    // returns 0 on a scaffold-only-evolution-pair corpus.
    let dir = std::env::temp_dir().join(format!(
        "doc-linter-saved-feature-coupling-hubs-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let db = testkit::open_db(&dir);
    {
        let conn = testkit::connect(&db);
        testkit::reset(&conn);
    }
    let conn = testkit::connect(&db);
    seed_feature_vs_scaffold_coupling_fixture(&conn);

    let q = SAVED_QUERIES
        .iter()
        .find(|q| q.name == "feature-evolution-coupling-hubs")
        .expect("feature-evolution-coupling-hubs must exist");
    let mut result = conn.query(sql_of(q.name)).expect("query runs");
    let mut returned_files: Vec<String> = Vec::new();
    for row in &mut result {
        if let kv::Value::String(f) = &row[0] {
            returned_files.push(f.clone());
        }
    }
    for noisy in ["scaffold-a.tsx", "scaffold-b.tsx", "scaffold-c.tsx"] {
        assert!(
            !returned_files.iter().any(|f| f == noisy),
            "scaffold-clique file `{noisy}` must NOT surface as hub: {returned_files:?}",
        );
    }
    // feature-x.py and feature-y.py each have 1 neighbor
    // at commits >= 5 (each other) — does NOT meet the
    // neighbor_count >= 2 hub threshold. Hub-view returns
    // empty on this fixture, which is the correct signal:
    // a 2-file feature evolution is not a hub.
    assert!(
        returned_files.is_empty(),
        "hub-view must return empty on scaffold-only-and-1-feature-pair corpus: {returned_files:?}",
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn file_coupling_degree_ranks_hubs_by_neighbor_count() {
    // Per iter 259 (closes [[user-probe-062]] Finding C+D):
    // file-axis hub detection via COUPLED_WITH degree.
    // Fixture:
    //   hub.tsx couples with 3 neighbors (route-a, route-b,
    //         route-c) — degree=3
    //   pair-a.py couples with pair-b.py only — degree=1 each
    //         (below ≥2 floor, both filtered)
    //   spoke.py has 0 COUPLED_WITH edges — excluded
    // Expected: hub.tsx (degree=3) surfaces alone; pair-a +
    // pair-b filtered.
    let dir = std::env::temp_dir().join(format!(
        "doc-linter-saved-file-coupling-degree-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let db = testkit::open_db(&dir);
    {
        let conn = testkit::connect(&db);
        testkit::reset(&conn);
    }
    let conn = testkit::connect(&db);
    for (path, lang, loc) in [
        ("hub.tsx", "tsx", 100u32),
        ("route-a.tsx", "tsx", 50),
        ("route-b.tsx", "tsx", 50),
        ("route-c.tsx", "tsx", 50),
        ("pair-a.py", "python", 30),
        ("pair-b.py", "python", 30),
        ("spoke.py", "python", 20),
    ] {
        conn.query(&format!(
            "CREATE (:File {{path: '{path}', language: '{lang}', \
                 loc: {loc}, last_touched: '2026-06-01'}})"
        ))
        .expect("seed File");
    }
    for (a, b, commits) in [
        ("hub.tsx", "route-a.tsx", 5u32),
        ("hub.tsx", "route-b.tsx", 4),
        ("hub.tsx", "route-c.tsx", 3),
        ("pair-a.py", "pair-b.py", 8),
    ] {
        conn.query(&format!(
            "MATCH (a:File {{path: '{a}'}}), (b:File {{path: '{b}'}}) \
                 CREATE (a)-[:COUPLED_WITH {{commits: {commits}, \
                 last_co_change_at: '2026-05-01', jaccard: 0.5}}]->(b)"
        ))
        .expect("seed COUPLED_WITH");
    }

    let q = SAVED_QUERIES
        .iter()
        .find(|q| q.name == "file-coupling-degree")
        .expect("file-coupling-degree must exist");
    let mut result = conn.query(sql_of(q.name)).expect("query runs");
    let mut returned: Vec<(String, i64)> = Vec::new();
    for row in &mut result {
        let path = if let kv::Value::String(s) = &row[0] {
            s.clone()
        } else {
            panic!("file_path must be String: {row:?}");
        };
        let degree = match &row[2] {
            kv::Value::Int64(n) => *n,
            kv::Value::Int32(n) => *n as i64,
            kv::Value::UInt64(n) => *n as i64,
            other => panic!("degree must be int: {other:?}"),
        };
        returned.push((path, degree));
    }
    assert!(
        returned.iter().any(|(p, d)| p == "hub.tsx" && *d == 3),
        "hub.tsx must surface at degree=3: {returned:?}",
    );
    // Single-pair files have degree=1, below the ≥2 floor.
    for excluded in ["pair-a.py", "pair-b.py", "spoke.py"] {
        assert!(
            !returned.iter().any(|(p, _)| p == excluded),
            "{excluded} must NOT surface (below degree ≥2 floor): {returned:?}",
        );
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn language_loc_distribution_ranks_by_total_loc() {
    // Per iter 259 (LOC-aware sibling of language-distribution):
    // Fixture:
    //   3 python files at 100 LOC each → total=300
    //   2 tsx files at 200 LOC each → total=400
    //   1 css file with NULL loc → excluded (NULL filter)
    // Expected: tsx (400) > python (300); css absent.
    let dir = std::env::temp_dir().join(format!(
        "doc-linter-saved-language-loc-distribution-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let db = testkit::open_db(&dir);
    {
        let conn = testkit::connect(&db);
        testkit::reset(&conn);
    }
    let conn = testkit::connect(&db);
    for (path, lang, loc_clause) in [
        ("a.py", "python", "100"),
        ("b.py", "python", "100"),
        ("c.py", "python", "100"),
        ("a.tsx", "tsx", "200"),
        ("b.tsx", "tsx", "200"),
        ("zero.css", "css", "0"),
    ] {
        conn.query(&format!(
            "CREATE (:File {{path: '{path}', language: '{lang}', \
                 loc: {loc_clause}, last_touched: '2026-06-01'}})"
        ))
        .expect("seed File");
    }

    let q = SAVED_QUERIES
        .iter()
        .find(|q| q.name == "language-loc-distribution")
        .expect("language-loc-distribution must exist");
    let mut result = conn.query(sql_of(q.name)).expect("query runs");
    let mut returned: Vec<(String, i64, i64)> = Vec::new();
    for row in &mut result {
        let lang = if let kv::Value::String(s) = &row[0] {
            s.clone()
        } else {
            panic!("language must be String: {row:?}");
        };
        let file_count = match &row[1] {
            kv::Value::Int64(n) => *n,
            kv::Value::Int32(n) => *n as i64,
            kv::Value::UInt64(n) => *n as i64,
            other => panic!("file_count must be int: {other:?}"),
        };
        let total_loc = match &row[2] {
            kv::Value::Int64(n) => *n,
            kv::Value::Int32(n) => *n as i64,
            kv::Value::UInt64(n) => *n as i64,
            kv::Value::Int128(n) => *n as i64,
            other => panic!("total_loc must be int: {other:?}"),
        };
        returned.push((lang, file_count, total_loc));
    }
    assert!(
        returned
            .iter()
            .any(|(l, c, t)| l == "tsx" && *c == 2 && *t == 400),
        "tsx must surface at file_count=2 + total_loc=400: {returned:?}",
    );
    assert!(
        returned
            .iter()
            .any(|(l, c, t)| l == "python" && *c == 3 && *t == 300),
        "python must surface at file_count=3 + total_loc=300: {returned:?}",
    );
    // css file has loc=0, filtered by WHERE loc > 0.
    assert!(
        !returned.iter().any(|(l, _, _)| l == "css"),
        "css must NOT surface (loc=0 below the > 0 filter): {returned:?}",
    );
    // Order check: total_loc DESC. tsx (400) must rank
    // before python (300).
    assert_eq!(
        returned[0].0, "tsx",
        "tsx must rank first by total_loc: {returned:?}",
    );
    let _ = std::fs::remove_dir_all(&dir);
}

// Per interrogation-036 Finding G + iter-183 closure: the
// iter-143 syntax-validation test catches the store parser /
// binder bugs (UNWIND scope drop, missing SPLIT function,
// list-slice ban, etc.). It does NOT catch SEMANTIC
// defects like the iter-180 kind='' filter hole. A semantic
// defect produces a valid cypher with wrong rows. The
// two tests below seed a small fixture corpus, run the
// iter-180 freshness queries, and pin the EXPECTED output
// rows (specifically: kind='' and ontology-* roles must
// NOT appear).
fn seed_freshness_semantic_fixture(conn: &testkit::Conn<'_>) {
    // 6 Doc rows covering the role+kind matrix:
    // A: normal narrative reference (must surface)
    // B: kind='' ontology-meta (must be EXCLUDED — the defect)
    // C: explanation (must surface)
    // D: role=ontology-value (must be EXCLUDED — existing filter)
    // E: how-to (must surface)
    // F: kind=NULL (must be EXCLUDED — IS NOT NULL filter)
    for (id, role, kind, updated) in [
        ("a-reference", "doc", "'reference'", "2026-05-01"),
        ("b-empty-kind", "doc", "''", "2026-04-30"),
        ("c-explanation", "doc", "'explanation'", "2026-05-15"),
        (
            "d-ontology-value",
            "ontology-value",
            "'reference'",
            "2026-04-30",
        ),
        ("e-how-to", "doc", "'how-to'", "2026-05-25"),
    ] {
        let cypher = format!(
            "CREATE (:Doc {{id: '{id}', path: '{id}.md', role: '{role}', \
                 kind: {kind}, lifecycle: '', bounded_context: '', title: 'T', \
                 summary: '', status: 'stable', updated: '{updated}', \
                 tags: [], covers: []}})"
        );
        conn.query(&cypher).expect("seed Doc insert");
    }
}

#[test]
fn stale_docs_with_impact_semantic_excludes_empty_kind_and_ontology_roles() {
    // Per interrogation-036 Finding A: iter-180's query
    // surfaced ontology-mig-0001 + ontology-index (role=doc,
    // kind='') as the 'stalest narrative docs' which they
    // aren't. The iter-182 fix added `AND d.kind <> ''`.
    // This SEMANTIC test pins the post-fix behavior end to end.
    let dir = std::env::temp_dir().join(format!(
        "doc-linter-saved-semantic-stale-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let db = testkit::open_db(&dir);
    {
        let conn = testkit::connect(&db);
        testkit::reset(&conn);
    }
    let conn = testkit::connect(&db);
    seed_freshness_semantic_fixture(&conn);

    let q = SAVED_QUERIES
        .iter()
        .find(|q| q.name == "stale-docs-with-impact")
        .expect("stale-docs-with-impact must exist");
    let mut result = conn.query(sql_of(q.name)).expect("query runs");
    let mut returned_ids: Vec<String> = Vec::new();
    for row in &mut result {
        // First column is id per the RETURN.
        if let kv::Value::String(s) = &row[0] {
            returned_ids.push(s.clone());
        }
    }
    // Expected: a-reference, c-explanation, e-how-to.
    // Excluded by kind='' filter: b-empty-kind.
    // Excluded by ontology-* role filter: d-ontology-value.
    assert!(
        !returned_ids.contains(&"b-empty-kind".to_string()),
        "interrogation-036 Finding A regression: kind='' doc surfaced: {returned_ids:?}",
    );
    assert!(
            !returned_ids.contains(&"d-ontology-value".to_string()),
            "feedback_starter_ontology_false_positives regression: ontology-value role surfaced: {returned_ids:?}",
        );
    assert!(
        returned_ids.contains(&"a-reference".to_string()),
        "valid narrative doc must surface: {returned_ids:?}",
    );
    assert!(
        returned_ids.contains(&"c-explanation".to_string()),
        "valid narrative doc must surface: {returned_ids:?}",
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// Per iter 185 (extending iter-183's semantic-validation
/// pattern to a second query): the iter-170
/// `python-untested-files` query uses an OPTIONAL MATCH +
/// WHERE NULL anti-join to surface prod Python files with
/// no `test_X.py` companion. Semantic correctness depends
/// on the anti-join firing for prod-files-without-test
/// AND NOT firing for prod-files-with-test. This test seeds
/// 4 representative File rows + asserts the partition.
fn seed_python_untested_files_fixture(conn: &testkit::Conn<'_>) {
    // File rows covering the 4 cases:
    // - app/models.py prod (no test) → must surface
    // - app/users.py prod (HAS test) → must NOT surface
    // - tests/test_users.py test file → must never be in
    //   the prod-side output regardless
    // - app/trivial.py prod < 20 LOC → must NOT surface
    //   (LOC floor)
    for (path, lang, loc) in [
        ("backend/app/models.py", "python", 129_u32),
        ("backend/app/users.py", "python", 50),
        ("backend/tests/test_users.py", "python", 200),
        ("backend/app/trivial.py", "python", 10),
    ] {
        let cypher = format!(
            "CREATE (:File {{path: '{path}', language: '{lang}', loc: {loc}, \
                 last_touched: '2026-05-30T00:00:00Z'}})"
        );
        conn.query(&cypher).expect("seed File insert");
    }
}

#[test]
fn python_untested_files_semantic_anti_join_partitions_correctly() {
    let dir = std::env::temp_dir().join(format!(
        "doc-linter-saved-semantic-untested-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let db = testkit::open_db(&dir);
    {
        let conn = testkit::connect(&db);
        testkit::reset(&conn);
    }
    let conn = testkit::connect(&db);
    seed_python_untested_files_fixture(&conn);

    let q = SAVED_QUERIES
        .iter()
        .find(|q| q.name == "python-untested-files")
        .expect("python-untested-files must exist");
    let mut result = conn.query(sql_of(q.name)).expect("query runs");
    let mut returned_paths: Vec<String> = Vec::new();
    for row in &mut result {
        if let kv::Value::String(s) = &row[0] {
            returned_paths.push(s.clone());
        }
    }
    // backend/app/models.py: prod ≥ 20 LOC, NO test file
    //   pairs by basename → must surface as UNTESTED.
    assert!(
        returned_paths.contains(&"backend/app/models.py".to_string()),
        "prod file without test_X.py companion must surface: {returned_paths:?}",
    );
    // backend/app/users.py: prod with test_users.py
    // companion → must NOT surface (anti-join excludes).
    assert!(
        !returned_paths.contains(&"backend/app/users.py".to_string()),
        "anti-join regression: prod file WITH companion test surfaced: {returned_paths:?}",
    );
    // backend/tests/test_users.py: a test file (path
    // CONTAINS /tests/) → must never be in the prod-side
    // result regardless.
    assert!(
        !returned_paths.contains(&"backend/tests/test_users.py".to_string()),
        "test file must be excluded from the prod-side output: {returned_paths:?}",
    );
    // backend/app/trivial.py: prod but only 10 LOC → must
    // NOT surface (LOC floor of 20).
    assert!(
        !returned_paths.contains(&"backend/app/trivial.py".to_string()),
        "LOC floor regression: trivial file (<20 LOC) surfaced: {returned_paths:?}",
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn freshness_by_kind_semantic_excludes_empty_kind_bucket() {
    // Per interrogation-036 Finding B: iter-180's
    // freshness-by-kind surfaced a kind='' aggregation
    // bucket because `d.kind IS NOT NULL` doesn't exclude
    // empty strings. The iter-182 fix added `AND d.kind <> ''`.
    let dir = std::env::temp_dir().join(format!(
        "doc-linter-saved-semantic-freshness-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let db = testkit::open_db(&dir);
    {
        let conn = testkit::connect(&db);
        testkit::reset(&conn);
    }
    let conn = testkit::connect(&db);
    seed_freshness_semantic_fixture(&conn);

    let q = SAVED_QUERIES
        .iter()
        .find(|q| q.name == "freshness-by-kind")
        .expect("freshness-by-kind must exist");
    let mut result = conn.query(sql_of(q.name)).expect("query runs");
    let mut returned_kinds: Vec<String> = Vec::new();
    for row in &mut result {
        // First column is `kind` per the RETURN.
        if let kv::Value::String(s) = &row[0] {
            returned_kinds.push(s.clone());
        }
    }
    // Expected: reference, explanation, how-to (the 3 valid kinds).
    // Excluded: '' (the b-empty-kind doc).
    assert!(
        !returned_kinds.contains(&String::new()),
        "interrogation-036 Finding B regression: empty-kind bucket appeared: {returned_kinds:?}",
    );
    for expected in ["reference", "explanation", "how-to"] {
        assert!(
            returned_kinds.iter().any(|k| k == expected),
            "valid kind `{expected}` must surface: {returned_kinds:?}",
        );
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn corpus_ratio_queries_return_a_row_on_an_empty_graph() {
    let dir = std::env::temp_dir().join(format!(
        "doc-linter-saved-empty-ratio-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let db = testkit::open_db(&dir);
    {
        let conn = testkit::connect(&db);
        testkit::reset(&conn);
    }
    for name in [
        "corpus-purpose-classifier",
        "corpus-cold-start-summary",
        "unified-corpus-diagnostic",
        "corpus-entities-per-doc-ratio",
        "corpus-functions-per-file-ratio",
        "corpus-doc-coverage-ratio",
    ] {
        let out = run_saved_query(&db, name, &HashMap::new()).unwrap();
        assert_eq!(out["row_count"], 1, "{name}: {out}");
    }
    drop(db);
    let _ = std::fs::remove_dir_all(&dir);
}
