-- name: callers-density-hub-test-coverage
-- params: 
WITH hub AS (
  SELECT fn.symbol AS symbol, fn.file AS file, count(*) AS callers
  FROM Function fn JOIN "CALLS" r ON r.dst = fn.symbol
  WHERE NOT (instr(fn.file, 'tests/') > 0) AND NOT (instr(fn.symbol, 'tests/') > 0) AND NOT (substr(fn.file, -4) = '.pyi')
  GROUP BY fn.symbol HAVING count(*) >= 5),
tc AS (
  SELECT h.symbol, h.file, h.callers,
         (SELECT count(*) FROM "TEST_FOR" t WHERE t.dst = h.symbol) AS test_count FROM hub h)
SELECT symbol, file, callers, test_count,
  CASE WHEN test_count = 0 AND callers >= 10 THEN 'critical-untested-hub'
       WHEN test_count = 0 THEN 'untested-hub' ELSE 'tested-hub' END AS classification
FROM tc ORDER BY callers DESC, test_count LIMIT 25;
