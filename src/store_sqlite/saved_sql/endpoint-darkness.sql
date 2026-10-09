-- name: endpoint-darkness
-- params: 
SELECT e.id AS "e.id", e.kind AS "e.kind", e.method AS "e.method", e.path AS "e.path",
       e.source_file AS "e.source_file", e.source_line AS "e.source_line"
FROM Endpoint e
WHERE NOT EXISTS (SELECT 1 FROM "ENDPOINT_TOUCHES_ENTITY" t WHERE t.src = e.id)
ORDER BY e.id;
