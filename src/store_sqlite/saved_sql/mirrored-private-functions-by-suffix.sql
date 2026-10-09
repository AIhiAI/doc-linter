-- name: mirrored-private-functions-by-suffix
-- params: suffix
SELECT fn.symbol AS function_symbol FROM Function fn WHERE instr(fn.symbol, $suffix) > 0 AND NOT (instr(fn.symbol, '#') > 0) ORDER BY fn.symbol LIMIT 40;
