-- name: narrative-doc-count
-- params: 
SELECT count(d.id) AS narrative_doc_count FROM Doc d WHERE NOT d.role IN ('ontology-value', 'ontology-axis', 'ontology-entity');
