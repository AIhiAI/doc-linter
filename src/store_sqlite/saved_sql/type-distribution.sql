-- name: type-distribution
-- params: 
SELECT t.kind AS "t.kind", count(t.symbol) AS count FROM Type t GROUP BY t.kind ORDER BY count DESC, t.kind;
