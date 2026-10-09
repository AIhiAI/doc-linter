-- name: template-directory-survey
-- params: path_prefix
WITH f AS (SELECT p.path AS file_path FROM File p WHERE substr(p.path, 1, length($path_prefix)) = $path_prefix AND NOT (instr(p.path, 'tests/') > 0)),
ic AS (SELECT f.file_path AS file_path, count(t.symbol) AS impl_count FROM f
       LEFT JOIN Function t ON t.file = f.file_path AND NOT (instr(t.symbol, 'tests/') > 0) GROUP BY f.file_path),
ex AS (SELECT t1.file AS file_path, count(*) AS external_count FROM Function t1
       JOIN "CALLS" r ON r.src = t1.symbol JOIN Function c ON c.symbol = r.dst
       WHERE NOT (instr(t1.symbol, 'tests/') > 0) AND c.file <> t1.file GROUP BY t1.file),
j AS (SELECT ic.file_path AS file_path, ic.impl_count AS impl_count, coalesce(ex.external_count, 0) AS external_count
      FROM ic LEFT JOIN ex ON ex.file_path = ic.file_path)
SELECT file_path, impl_count, external_count,
  CASE WHEN impl_count <= 2 AND external_count > 0 THEN 'leaf-with-deps' WHEN impl_count = 0 THEN 'data-only'
       WHEN impl_count >= 1 AND external_count = 0 THEN 'no-call-or-self-contained' ELSE 'other' END AS sub_shape
FROM j ORDER BY file_path LIMIT 100;
