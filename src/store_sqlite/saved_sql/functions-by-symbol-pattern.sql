-- name: functions-by-symbol-pattern
-- params: pattern
SELECT f.symbol AS symbol, f.file AS file, f.line AS line, f.language AS language FROM Function f
WHERE instr(f.symbol, $pattern) > 0 AND NOT (instr(f.file, 'tests/') > 0) AND NOT (instr(f.symbol, 'tests/') > 0) ORDER BY f.file, f.line LIMIT 50;
