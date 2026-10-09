-- name: failure-mode-class-catalog
-- params: 
SELECT t.symbol AS type_symbol, t.file AS file_path, t.line AS line, t.body_excerpt AS body_excerpt FROM Type t
WHERE (substr(t.symbol, -6) = 'Error#' OR substr(t.symbol, -10) = 'Exception#' OR substr(t.symbol, -8) = 'Failure#' OR substr(t.symbol, -6) = 'Issue#' OR substr(t.symbol, -6) = 'Fault#' OR substr(t.symbol, -10) = 'Diagnosis#') AND NOT (instr(t.symbol, 'tests') > 0) AND NOT (instr(t.symbol, 'test_') > 0)
ORDER BY t.symbol LIMIT 30;
