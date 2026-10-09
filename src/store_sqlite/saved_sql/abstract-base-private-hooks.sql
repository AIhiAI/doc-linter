-- name: abstract-base-private-hooks
-- params: class_path_substring
SELECT fn.symbol AS function_symbol FROM Function fn WHERE instr(fn.symbol, $class_path_substring) > 0 AND instr(fn.symbol, '#_') > 0 ORDER BY fn.symbol LIMIT 30;
