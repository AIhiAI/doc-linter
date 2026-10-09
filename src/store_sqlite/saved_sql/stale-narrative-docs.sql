-- name: stale-narrative-docs
-- params: 
SELECT d.id AS id, d.role AS role, d.kind AS kind, d.updated AS updated, d.title AS title FROM Doc d
WHERE d.updated IS NOT NULL AND NOT d.role IN ('ontology-value', 'ontology-axis', 'ontology-entity', 'ontology-migration', 'index') ORDER BY d.updated ASC, d.id LIMIT 20;
