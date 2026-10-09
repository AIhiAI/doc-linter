-- name: entity-belongs-vs-mentions-ratio
-- params: 
WITH em AS (SELECT e.id AS id, (SELECT count(DISTINCT fn.symbol) FROM "FUNCTION_BELONGS_TO" m JOIN Function fn ON fn.symbol = m.src WHERE m.dst = e.id AND NOT (instr(fn.file, 'tests/') > 0) AND NOT (instr(fn.symbol, 'tests/') > 0)) AS belongs_count, (SELECT count(DISTINCT fn2.symbol) FROM "FUNCTION_MENTIONS" m JOIN Function fn2 ON fn2.symbol = m.src WHERE m.dst = e.id AND NOT (instr(fn2.file, 'tests/') > 0) AND NOT (instr(fn2.symbol, 'tests/') > 0)) AS mentions_count FROM Entity e)
SELECT id AS entity_id, belongs_count, mentions_count,
  CASE WHEN mentions_count = 0 AND belongs_count >= 1 THEN 999.0 WHEN mentions_count = 0 THEN 0.0 ELSE 1.0 * belongs_count / mentions_count END AS ratio,
  CASE WHEN belongs_count = 0 THEN 'pure-discussion' WHEN mentions_count = 0 THEN 'pure-structural' WHEN 1.0 * belongs_count / mentions_count >= 2.0 THEN 'implementation-heavy' WHEN 1.0 * belongs_count / mentions_count <= 0.7 THEN 'discussion-heavy' ELSE 'balanced' END AS lifecycle_label
FROM em WHERE belongs_count >= 1 OR mentions_count >= 1 ORDER BY ratio DESC, id LIMIT 25;
