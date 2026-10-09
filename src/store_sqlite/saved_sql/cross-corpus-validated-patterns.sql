-- name: cross-corpus-validated-patterns
-- params: 
SELECT d.id AS doc_id, d.title AS title, d.kind AS kind, (SELECT json_group_array(value) FROM doc_tags WHERE doc_id = d.id) AS tags FROM Doc d WHERE EXISTS (SELECT 1 FROM doc_tags WHERE doc_id = d.id AND value = 'cross-corpus-validation') ORDER BY d.id;
