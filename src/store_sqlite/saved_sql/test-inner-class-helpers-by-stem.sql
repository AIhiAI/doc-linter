-- name: test-inner-class-helpers-by-stem
-- params: stem
SELECT fn.symbol AS function_symbol FROM Function fn WHERE instr(fn.symbol, $stem) > 0 AND instr(fn.symbol, 'test_') > 0 AND instr(fn.symbol, '#__') > 0 AND NOT (instr(fn.symbol, 'tests.test_') > 0) ORDER BY fn.symbol LIMIT 20;
