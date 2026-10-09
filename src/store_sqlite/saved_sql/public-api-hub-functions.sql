-- name: public-api-hub-functions
-- params: 
SELECT file, symbol, mention_count FROM (
  SELECT f.symbol AS symbol, f.file AS file, count(*) AS mention_count FROM Function f JOIN "FUNCTION_MENTIONS" r ON r.src = f.symbol
  JOIN Entity en ON en.id = r.dst
  WHERE NOT (instr(f.file, '/tests/') > 0) AND NOT (instr(f.file, '/test_') > 0) AND NOT (instr(f.symbol, '/_') > 0)
  GROUP BY f.symbol, f.file)
WHERE mention_count >= 3 ORDER BY mention_count DESC, file, symbol LIMIT 25;
