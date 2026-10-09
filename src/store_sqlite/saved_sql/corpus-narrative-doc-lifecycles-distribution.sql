-- name: corpus-narrative-doc-lifecycles-distribution
-- params: 
SELECT CASE WHEN l = '' THEN 'lifecycle-unset' ELSE l END AS lifecycle, narrative_doc_count
FROM (SELECT d.lifecycle AS l, count(d.id) AS narrative_doc_count FROM Doc d WHERE NOT (d.role IN ('ontology-value', 'ontology-axis', 'ontology-entity', 'ontology-migration') OR (d.role = 'index' AND substr(d.id, 1, length('ontology')) = 'ontology')) GROUP BY d.lifecycle)
WHERE narrative_doc_count >= 1 ORDER BY narrative_doc_count DESC, lifecycle LIMIT 20;
