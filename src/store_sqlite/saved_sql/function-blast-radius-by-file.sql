-- name: function-blast-radius-by-file
-- params: symbol
SELECT caller.file AS caller_file, count(DISTINCT caller.symbol) AS distinct_callers
FROM "CALLS" c JOIN Function caller ON caller.symbol = c.src JOIN Function target ON target.symbol = c.dst
WHERE target.symbol = $symbol AND caller.symbol <> $symbol
GROUP BY caller.file ORDER BY distinct_callers DESC, caller_file LIMIT 25;
