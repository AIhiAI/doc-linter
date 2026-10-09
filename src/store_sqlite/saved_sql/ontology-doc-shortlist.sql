-- name: ontology-doc-shortlist
-- params: 
SELECT d.id AS doc_id, d.role AS role, d.title AS title, d.summary AS summary FROM Doc d
WHERE d.role IN ('ontology-entity', 'ontology-axis', 'ontology-value') ORDER BY d.role, d.id LIMIT 50;
