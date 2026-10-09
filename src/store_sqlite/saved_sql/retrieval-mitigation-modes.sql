-- name: retrieval-mitigation-modes
-- params: 
SELECT d.id AS doc_id, d.title AS title, d.kind AS kind, (SELECT json_group_array(DISTINCT t.value) FROM doc_tags t WHERE t.doc_id = d.id AND t.value IN ('vocabulary-mismatch-mitigation', 'audit-as-surface', 'query-reformulation')) AS mitigation_tags
FROM Doc d WHERE EXISTS (SELECT 1 FROM doc_tags t WHERE t.doc_id = d.id AND t.value IN ('vocabulary-mismatch-mitigation', 'audit-as-surface', 'query-reformulation')) ORDER BY d.id;
