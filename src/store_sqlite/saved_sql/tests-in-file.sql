-- name: tests-in-file
-- params: path
SELECT f.symbol AS symbol, f.line AS line, f.doc_comment AS doc_comment FROM Function f
WHERE f.file = $path AND instr(f.symbol, 'tests/') > 0 ORDER BY f.line;
