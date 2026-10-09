-- name: function-backward-slice
-- params: symbol
SELECT DISTINCT caller.symbol AS caller_symbol, caller.file AS caller_file
FROM "CALLS" c JOIN Function caller ON caller.symbol = c.src JOIN Function target ON target.symbol = c.dst
WHERE target.symbol = $symbol AND caller.symbol <> $symbol
ORDER BY caller_symbol;
