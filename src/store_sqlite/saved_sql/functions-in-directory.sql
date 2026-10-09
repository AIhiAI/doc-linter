-- name: functions-in-directory
-- params: prefix
SELECT f.symbol AS symbol, f.file AS file, f.line AS line, f.signature AS signature FROM Function f WHERE substr(f.file, 1, length($prefix)) = $prefix ORDER BY f.file, f.line;
