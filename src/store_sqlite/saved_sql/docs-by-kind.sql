-- name: docs-by-kind
-- params: kind
SELECT d.id AS id, d.title AS title, d.role AS role, d.summary AS summary FROM Doc d WHERE d.kind = $kind ORDER BY d.id;
