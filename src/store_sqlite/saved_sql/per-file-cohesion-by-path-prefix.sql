-- name: per-file-cohesion-by-path-prefix
-- params: path_prefix
WITH tf AS (SELECT t.file AS file_path, t.symbol AS sym FROM Function t WHERE substr(t.file, 1, length($path_prefix)) = $path_prefix AND NOT (instr(t.file, 'tests/') > 0) AND NOT (instr(t.symbol, 'tests/') > 0)),
per AS (SELECT file_path, count(*) AS impl_count FROM tf GROUP BY file_path),
cc AS (SELECT tf.file_path AS file_path,
         count(CASE WHEN c.symbol IS NOT NULL AND c.file = tf.file_path THEN 1 END) AS internal_count,
         count(CASE WHEN c.symbol IS NOT NULL AND c.file <> tf.file_path THEN 1 END) AS external_count
       FROM tf LEFT JOIN "CALLS" r ON r.src = tf.sym LEFT JOIN Function c ON c.symbol = r.dst GROUP BY tf.file_path),
j AS (SELECT per.file_path AS file_path, impl_count, internal_count, external_count FROM per JOIN cc ON cc.file_path = per.file_path)
SELECT file_path, impl_count, internal_count, external_count, CASE WHEN internal_count + external_count = 0 THEN 0.0 ELSE external_count * 100.0 / (internal_count + external_count) END AS external_pct, CASE WHEN impl_count = 0 THEN 'empty' WHEN internal_count + external_count = 0 THEN 'stub-only-cython' WHEN external_count = 0 THEN 'cohesive' WHEN impl_count <= 2 AND internal_count = 0 THEN 'leaf-with-deps' WHEN external_count * 100 / (internal_count + external_count) <= 10 THEN 'mostly-cohesive' WHEN external_count * 100 / (internal_count + external_count) <= 67 THEN 'balanced-dependent' ELSE 'heavily-dependent' END AS cohesion_label
FROM j ORDER BY external_pct DESC, file_path LIMIT 30;
