-- name: narrative-doc-list
-- params: 
SELECT d.id AS doc_id, d.kind AS kind, d.title AS title, d.summary AS summary FROM Doc d WHERE d.role = 'doc' ORDER BY d.kind, d.id LIMIT 50;
