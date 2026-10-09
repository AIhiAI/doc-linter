-- name: function-callers-of-symbol-pattern
-- params: pattern
SELECT target.symbol AS target_symbol, caller.symbol AS caller_symbol, caller.file AS caller_file
FROM "CALLS" r JOIN Function caller ON caller.symbol = r.src JOIN Function target ON target.symbol = r.dst
WHERE instr(target.symbol, $pattern) > 0 AND NOT (instr(target.file, 'tests/') > 0) AND NOT (instr(target.symbol, 'tests/') > 0) ORDER BY target.symbol, caller.file, caller.symbol LIMIT 100;
