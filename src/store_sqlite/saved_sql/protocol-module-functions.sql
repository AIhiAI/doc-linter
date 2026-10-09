-- name: protocol-module-functions
-- params: protocol_module_substring
SELECT fn.symbol AS function_symbol FROM Function fn WHERE instr(fn.symbol, $protocol_module_substring) > 0 ORDER BY fn.symbol LIMIT 40;
