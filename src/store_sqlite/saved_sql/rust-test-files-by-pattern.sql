-- name: rust-test-files-by-pattern
-- params: 
SELECT * FROM (
  SELECT f.path AS path, f.loc AS loc,
    CASE WHEN instr(f.path, 'tests/common') > 0 THEN 'helper' WHEN substr(f.path, 1, length('tests/')) = 'tests/' OR instr(f.path, '/tests/') > 0 THEN 'integration' ELSE 'sibling' END AS test_kind
  FROM File f WHERE f.language = 'rust' AND (substr(f.path, -9) = '_tests.rs' OR substr(f.path, -8) = '_test.rs' OR substr(f.path, 1, length('tests/')) = 'tests/' OR instr(f.path, '/tests/') > 0))
ORDER BY test_kind, path LIMIT 50;
