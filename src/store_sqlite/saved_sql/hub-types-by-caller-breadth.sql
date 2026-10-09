-- name: hub-types-by-caller-breadth
-- params: 
SELECT t.symbol AS symbol, t.kind AS kind, t.file AS file, count(DISTINCT fn.symbol) AS distinct_users, count(DISTINCT fn.file) AS user_files,
  CASE WHEN count(DISTINCT fn.file) >= 5 THEN 'api-tier-type' WHEN count(DISTINCT fn.file) >= 2 THEN 'mid-tier-type' ELSE 'internal-cluster-type' END AS spread_label
FROM Type t JOIN "USES_TYPE" r ON r.dst = t.symbol JOIN Function fn ON fn.symbol = r.src
WHERE NOT (instr(t.file, 'tests/') > 0) AND NOT (instr(t.symbol, 'tests/') > 0) AND NOT (instr(fn.file, 'tests/') > 0) AND NOT (instr(fn.symbol, 'tests/') > 0)
GROUP BY t.symbol HAVING count(DISTINCT fn.symbol) >= 5 ORDER BY distinct_users DESC, user_files DESC LIMIT 25;
