-- name: retrieval-failure-modes
-- params: 
SELECT d.id AS doc_id, d.title AS title, d.kind AS kind, (SELECT json_group_array(DISTINCT t.value) FROM doc_tags t WHERE t.doc_id = d.id AND t.value IN ('vocabulary-mismatch', 'tag-axis-loose-end', 'negation-failure', 'adjacent-family-displacement', 'embedding-backend-not-compiled', 'vocabulary-asymmetry')) AS failure_tags
FROM Doc d WHERE EXISTS (SELECT 1 FROM doc_tags t WHERE t.doc_id = d.id AND t.value IN ('vocabulary-mismatch', 'tag-axis-loose-end', 'negation-failure', 'adjacent-family-displacement', 'embedding-backend-not-compiled', 'vocabulary-asymmetry')) ORDER BY d.id;
