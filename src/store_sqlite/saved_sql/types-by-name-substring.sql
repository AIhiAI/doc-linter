-- name: types-by-name-substring
-- params: term
SELECT t.symbol AS type_symbol, t.file AS file_path, t.line AS line, t.kind AS kind, t.signature AS signature, t.body_excerpt AS body_excerpt
FROM Type t WHERE instr(t.symbol, $term) > 0 AND NOT (instr(t.symbol, 'tests') > 0) AND NOT (instr(t.symbol, 'test_') > 0) ORDER BY t.symbol LIMIT 30;
