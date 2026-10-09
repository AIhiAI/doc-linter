-- name: architectural-patterns
-- params: 
SELECT d.id AS doc_id, d.title AS title, d.kind AS kind, (SELECT json_group_array(DISTINCT t.value) FROM doc_tags t WHERE t.doc_id = d.id AND t.value IN ('absence-as-signal', 'test-co-location', 'query-reformulation', 'cross-corpus-validation', 'cross-workflow-comparison')) AS pattern_tags
FROM Doc d WHERE EXISTS (SELECT 1 FROM doc_tags t WHERE t.doc_id = d.id AND t.value IN ('absence-as-signal', 'test-co-location', 'query-reformulation', 'cross-corpus-validation', 'cross-workflow-comparison')) ORDER BY d.id;
