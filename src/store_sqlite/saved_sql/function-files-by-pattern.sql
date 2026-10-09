-- name: function-files-by-pattern
-- params: pattern
SELECT fn.file AS file, count(fn.symbol) AS fn_count FROM Function fn WHERE instr(fn.file, $pattern) > 0 GROUP BY fn.file ORDER BY fn_count DESC, file;
