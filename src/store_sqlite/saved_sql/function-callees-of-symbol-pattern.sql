-- name: function-callees-of-symbol-pattern
-- params: pattern
SELECT target.symbol AS target_symbol, callee.symbol AS callee_symbol, callee.file AS callee_file
FROM "CALLS" r JOIN Function target ON target.symbol = r.src JOIN Function callee ON callee.symbol = r.dst
WHERE instr(target.symbol, $pattern) > 0 AND NOT (instr(target.file, 'tests/') > 0) AND NOT (instr(target.symbol, 'tests/') > 0) ORDER BY target.symbol, callee.file, callee.symbol LIMIT 100;
