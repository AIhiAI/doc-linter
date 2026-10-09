-- name: corpus-narrative-doc-roles-distribution
-- params: 
SELECT role, narrative_doc_count FROM (SELECT d.role AS role, count(d.id) AS narrative_doc_count FROM Doc d WHERE NOT (d.role IN ('ontology-value', 'ontology-axis', 'ontology-entity', 'ontology-migration') OR (d.role = 'index' AND substr(d.id, 1, length('ontology')) = 'ontology')) GROUP BY d.role)
WHERE narrative_doc_count >= 1 ORDER BY narrative_doc_count DESC, role LIMIT 20;
