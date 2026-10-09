-- name: non-ontology-docs
-- params: 
SELECT d.id AS doc_id, d.role AS role, d.kind AS kind, d.title AS title, d.summary AS summary FROM Doc d
WHERE NOT (d.role IN ('ontology-entity', 'ontology-value', 'ontology-axis', 'ontology-migration', 'index')) ORDER BY d.kind, d.role, d.id LIMIT 30;
