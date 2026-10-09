-- name: function-callees-by-file
-- params: symbol
SELECT callee.file AS callee_file, count(DISTINCT callee.symbol) AS distinct_callees
FROM "CALLS" c JOIN Function target ON target.symbol = c.src JOIN Function callee ON callee.symbol = c.dst
WHERE target.symbol = $symbol AND callee.symbol <> $symbol
GROUP BY callee.file ORDER BY distinct_callees DESC, callee_file LIMIT 25;
