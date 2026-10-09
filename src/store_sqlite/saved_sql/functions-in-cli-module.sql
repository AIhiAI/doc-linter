-- name: functions-in-cli-module
-- params: 
SELECT fn.symbol AS function_symbol FROM Function fn
WHERE (instr(fn.symbol, '.cli.') > 0 OR instr(fn.symbol, '/cli/') > 0) AND NOT (instr(fn.symbol, 'tests') > 0) AND NOT (instr(fn.symbol, 'test_') > 0) ORDER BY fn.symbol LIMIT 60;
