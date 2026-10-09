-- name: function-direct-callers-count
-- params: function_symbol
SELECT fn.symbol AS function_symbol,
  (SELECT count(DISTINCT r.src) FROM "CALLS" r JOIN Function c ON c.symbol = r.src WHERE r.dst = fn.symbol) AS direct_caller_count
FROM Function fn WHERE fn.symbol = $function_symbol;
