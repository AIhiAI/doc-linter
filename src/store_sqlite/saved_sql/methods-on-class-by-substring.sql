-- name: methods-on-class-by-substring
-- params: class_name
SELECT fn.symbol AS function_symbol FROM Function fn WHERE instr(fn.symbol, $class_name) > 0 AND instr(fn.symbol, '#') > 0 AND NOT (instr(fn.symbol, 'tests') > 0) AND NOT (instr(fn.symbol, 'test_') > 0) ORDER BY fn.symbol LIMIT 40;
