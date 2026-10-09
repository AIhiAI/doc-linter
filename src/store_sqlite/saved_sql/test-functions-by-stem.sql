-- name: test-functions-by-stem
-- params: stem
SELECT fn.symbol AS function_symbol FROM Function fn WHERE instr(fn.symbol, $stem) > 0 AND instr(fn.symbol, 'test_') > 0 ORDER BY fn.symbol LIMIT 30;
