-- name: docs-by-tag
-- params: tag
SELECT d.id AS id, d.role AS role, d.kind AS kind, d.title AS title, (SELECT json_group_array(value) FROM doc_tags WHERE doc_id = d.id) AS tags FROM Doc d WHERE EXISTS (SELECT 1 FROM doc_tags WHERE doc_id = d.id AND value = $tag) ORDER BY d.role, d.id;
