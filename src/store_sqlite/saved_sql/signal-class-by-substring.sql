-- name: signal-class-by-substring
-- params: term
SELECT t.symbol AS type_symbol, t.file AS file_path, t.line AS line, t.body_excerpt AS body_excerpt FROM Type t
WHERE instr(t.symbol, $term) > 0 AND (instr(t.symbol, 'Signal') > 0 OR instr(t.symbol, 'Signature') > 0 OR instr(t.symbol, 'Detector') > 0 OR instr(t.symbol, 'Record') > 0 OR instr(t.symbol, 'Observation') > 0) AND NOT (instr(t.symbol, 'tests') > 0) AND NOT (instr(t.symbol, 'test_') > 0)
ORDER BY t.symbol LIMIT 30;
