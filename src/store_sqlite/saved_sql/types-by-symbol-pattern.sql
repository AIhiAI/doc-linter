-- name: types-by-symbol-pattern
-- params: pattern
SELECT t.symbol AS symbol, t.kind AS kind, t.file AS file, t.line AS line FROM Type t
WHERE instr(t.symbol, $pattern) > 0 AND NOT (instr(t.file, 'tests/') > 0) AND NOT (instr(t.symbol, 'tests/') > 0)
ORDER BY t.file, t.line LIMIT 50;
