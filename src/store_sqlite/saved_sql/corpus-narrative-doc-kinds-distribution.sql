-- name: corpus-narrative-doc-kinds-distribution
-- params: 
SELECT CASE WHEN k = '' THEN 'kind-unset' ELSE k END AS kind, narrative_doc_count
FROM (SELECT d.kind AS k, count(d.id) AS narrative_doc_count FROM Doc d WHERE NOT (d.role IN ('ontology-value', 'ontology-axis', 'ontology-entity', 'ontology-migration') OR (d.role = 'index' AND substr(d.id, 1, length('ontology')) = 'ontology')) GROUP BY d.kind)
WHERE narrative_doc_count >= 1 ORDER BY narrative_doc_count DESC, kind LIMIT 20;
