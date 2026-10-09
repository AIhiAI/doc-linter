-- name: functions-in-file
-- params: path
SELECT f.symbol AS symbol, f.line AS line, f.doc_comment AS doc_comment, f.signature AS signature FROM Function f WHERE f.file = $path ORDER BY f.line;
