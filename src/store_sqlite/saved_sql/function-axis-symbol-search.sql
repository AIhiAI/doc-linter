-- name: function-axis-symbol-search
-- params: term
SELECT fn.symbol AS function_symbol FROM Function fn WHERE instr(fn.symbol, $term) > 0 AND NOT (instr(fn.symbol, 'tests') > 0) AND NOT (instr(fn.symbol, 'test_') > 0) ORDER BY fn.symbol LIMIT 40;
