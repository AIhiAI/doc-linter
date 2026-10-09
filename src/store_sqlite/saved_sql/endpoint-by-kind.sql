-- name: endpoint-by-kind
-- params: 
SELECT e.kind AS "e.kind", count(e.id) AS count FROM Endpoint e GROUP BY e.kind ORDER BY count DESC, e.kind;
