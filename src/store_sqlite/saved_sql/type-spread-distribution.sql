-- name: type-spread-distribution
-- params: 
SELECT spread_label, count(*) AS hub_count FROM (
  SELECT CASE WHEN count(DISTINCT fn.file) >= 5 THEN 'api-tier-type' WHEN count(DISTINCT fn.file) >= 2 THEN 'mid-tier-type' ELSE 'internal-cluster-type' END AS spread_label
  FROM Type t JOIN "USES_TYPE" r ON r.dst = t.symbol JOIN Function fn ON fn.symbol = r.src
WHERE NOT (instr(t.file, 'tests/') > 0) AND NOT (instr(t.symbol, 'tests/') > 0) AND NOT (instr(fn.file, 'tests/') > 0) AND NOT (instr(fn.symbol, 'tests/') > 0)
GROUP BY t.symbol HAVING count(DISTINCT fn.symbol) >= 5)
GROUP BY spread_label ORDER BY hub_count DESC, spread_label;
