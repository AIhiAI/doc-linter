-- name: leaf-with-deps-count-by-basename
-- params: basename
WITH tf AS (SELECT t.file AS file_path, t.symbol AS sym FROM Function t WHERE NOT (instr(t.file, 'tests/') > 0) AND NOT (instr(t.symbol, 'tests/') > 0)),
g AS (SELECT file_path, count(*) AS impl_count FROM tf GROUP BY file_path HAVING count(*) <= 2 AND substr(file_path, length(file_path) - length($basename) + 1) = $basename),
cc AS (SELECT g.file_path AS file_path, g.impl_count AS impl_count,
         count(CASE WHEN c.symbol IS NOT NULL AND c.file = tf.file_path THEN 1 END) AS internal_count,
         count(CASE WHEN c.symbol IS NOT NULL AND c.file <> tf.file_path THEN 1 END) AS external_count
       FROM g JOIN tf ON tf.file_path = g.file_path LEFT JOIN "CALLS" r ON r.src = tf.sym LEFT JOIN Function c ON c.symbol = r.dst
       GROUP BY g.file_path),
lw AS (SELECT * FROM cc WHERE internal_count = 0 AND external_count > 0)
SELECT count(file_path) AS template_instance_count FROM lw;
