-- name: module-external-callees-by-pattern
-- params: pattern
SELECT t.symbol AS target_symbol, c.symbol AS callee_symbol, c.file AS callee_file
FROM "CALLS" r JOIN Function t ON t.symbol = r.src JOIN Function c ON c.symbol = r.dst
WHERE instr(t.symbol, $pattern) > 0 AND NOT (instr(t.file, 'tests/') > 0) AND NOT (instr(t.symbol, 'tests/') > 0) AND c.file <> t.file ORDER BY t.symbol, c.file, c.symbol LIMIT 100;
