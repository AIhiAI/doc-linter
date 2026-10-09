-- name: type-hub-files
-- params: 
SELECT file FROM (SELECT DISTINCT t.file AS file FROM Type t JOIN "USES_TYPE" r ON r.dst = t.symbol JOIN Function fn ON fn.symbol = r.src
WHERE NOT (instr(t.file, 'tests/') > 0) AND NOT (instr(t.symbol, 'tests/') > 0) AND NOT (instr(fn.file, 'tests/') > 0) AND NOT (instr(fn.symbol, 'tests/') > 0)
GROUP BY t.symbol HAVING count(DISTINCT fn.symbol) >= 5 AND count(DISTINCT fn.file) >= 5)
ORDER BY file LIMIT 25;
