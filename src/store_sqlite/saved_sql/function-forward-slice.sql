-- name: function-forward-slice
-- params: symbol
SELECT DISTINCT callee.symbol AS callee_symbol, callee.file AS callee_file
FROM "CALLS" c JOIN Function target ON target.symbol = c.src JOIN Function callee ON callee.symbol = c.dst
WHERE target.symbol = $symbol AND callee.symbol <> $symbol
ORDER BY callee_symbol;
