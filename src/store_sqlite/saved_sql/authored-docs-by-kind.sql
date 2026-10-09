-- name: authored-docs-by-kind
-- params: kind
SELECT d.id AS id, d.role AS role, d.title AS title, d.summary AS summary, d.updated AS updated FROM Doc d
WHERE d.kind = $kind AND NOT (d.role IN ('ontology-value', 'ontology-axis', 'ontology-entity', 'ontology-migration') OR (d.role = 'index' AND substr(d.id, 1, length('ontology')) = 'ontology')) ORDER BY d.updated DESC, d.id LIMIT 10;
