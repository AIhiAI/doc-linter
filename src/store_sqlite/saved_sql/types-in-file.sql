-- name: types-in-file
-- params: path
SELECT t.symbol AS symbol, t.kind AS kind, t.line AS line, t.doc_comment AS doc_comment, t.signature AS signature FROM Type t WHERE t.file = $path ORDER BY t.line;
