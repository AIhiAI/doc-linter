-- name: function-hub-files
-- params: 
SELECT file FROM (SELECT DISTINCT fn.file AS file FROM Function fn JOIN "CALLS" r ON r.dst = fn.symbol JOIN Function caller ON caller.symbol = r.src
WHERE NOT (instr(fn.file, 'tests/') > 0) AND NOT (instr(fn.symbol, 'tests/') > 0) AND NOT (substr(fn.file, -4) = '.pyi')
GROUP BY fn.symbol HAVING count(DISTINCT caller.symbol) >= 5 AND count(DISTINCT caller.file) >= 5)
ORDER BY file LIMIT 25;
