-- name: types-by-file-pattern
-- params: pattern
SELECT t.file AS file, t.symbol AS symbol, t.kind AS kind, t.line AS line FROM Type t
WHERE instr(t.file, $pattern) > 0 AND NOT (instr(t.file, 'tests/') > 0) AND NOT (instr(t.symbol, 'tests/') > 0)
ORDER BY t.file, t.line LIMIT 50;
