-- name: function-shape-summary
-- params: symbol
WITH a AS (
  SELECT count(DISTINCT c.symbol) AS caller_count, count(DISTINCT c.file) AS caller_file_count
  FROM "CALLS" r JOIN Function c ON c.symbol = r.src
  WHERE r.dst = $symbol AND c.symbol <> $symbol AND EXISTS (SELECT 1 FROM Function WHERE symbol = $symbol)),
b AS (
  SELECT count(DISTINCT e.symbol) AS callee_count, count(DISTINCT e.file) AS callee_file_count
  FROM "CALLS" r JOIN Function e ON e.symbol = r.dst
  WHERE r.src = $symbol AND e.symbol <> $symbol AND EXISTS (SELECT 1 FROM Function WHERE symbol = $symbol))
SELECT caller_count, caller_file_count, callee_count, callee_file_count,
  CASE WHEN caller_count >= 5 AND caller_file_count = 1 AND callee_count = 0 THEN 'pure-leaf-factory' WHEN caller_count >= 5 AND caller_file_count >= 2 AND callee_count = 0 THEN 'wide-leaf-utility' WHEN caller_count >= 5 AND caller_file_count >= 3 AND callee_count >= 1 AND callee_count <= 2 THEN 'thin-entry-point' WHEN caller_count >= 5 AND caller_file_count = 1 AND callee_count >= 1 THEN 'subsystem-hub' WHEN caller_count >= 5 AND caller_file_count >= 2 AND callee_count >= 5 THEN 'extension-point' ELSE 'peripheral' END AS shape
FROM a, b;
