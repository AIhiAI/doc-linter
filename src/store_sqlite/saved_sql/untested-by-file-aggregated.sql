-- name: untested-by-file-aggregated
-- params: 
SELECT f.file AS file, count(f.symbol) AS untested_count FROM Function f
WHERE NOT EXISTS (SELECT 1 FROM "TEST_FOR" t WHERE t.dst = f.symbol)
  AND NOT (instr(f.file, 'tests/') > 0) AND NOT (instr(f.symbol, 'tests/') > 0) AND NOT (instr(f.file, 'test_') > 0) AND NOT (substr(f.file, -4) = '.pyi')
GROUP BY f.file ORDER BY untested_count DESC LIMIT 25;
