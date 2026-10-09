-- name: hub-spread-distribution
-- params: 
SELECT spread_label, count(*) AS hub_count FROM (
  SELECT count(DISTINCT caller.file) AS caller_files, CASE WHEN count(DISTINCT caller.file) >= 5 THEN 'api-tier-hub' WHEN count(DISTINCT caller.file) >= 2 THEN 'mid-tier-hub' ELSE 'internal-cluster-hub' END AS spread_label FROM Function fn JOIN "CALLS" r ON r.dst = fn.symbol JOIN Function caller ON caller.symbol = r.src
WHERE NOT (instr(fn.file, 'tests/') > 0) AND NOT (instr(fn.symbol, 'tests/') > 0) AND NOT (substr(fn.file, -4) = '.pyi')
GROUP BY fn.symbol HAVING count(DISTINCT caller.symbol) >= 5)
GROUP BY spread_label ORDER BY hub_count DESC, spread_label;
