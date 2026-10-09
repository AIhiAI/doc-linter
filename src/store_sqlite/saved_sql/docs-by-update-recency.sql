-- name: docs-by-update-recency
-- params: 
SELECT d.id AS id, d.role AS role, d.kind AS kind, d.updated AS updated, d.title AS title FROM Doc d
WHERE d.updated IS NOT NULL ORDER BY d.updated DESC, d.id LIMIT 20;
