-- name: ontology-vs-narrative-doc-ratio
-- params: 
WITH c AS (SELECT count(*) AS total_docs, count(CASE WHEN d.role IN ('ontology-entity', 'ontology-axis', 'ontology-value', 'ontology-migration', 'index') THEN 1 END) AS ontology_doc_count FROM Doc d)
SELECT ontology_doc_count, total_docs - ontology_doc_count AS narrative_doc_count,
  CASE WHEN total_docs = 0 THEN 0.0 ELSE ontology_doc_count * 100.0 / total_docs END AS ontology_pct,
  CASE WHEN total_docs = 0 THEN 'empty' WHEN ontology_doc_count * 100 / total_docs >= 70 THEN 'rich-ontology-doc-axis-finds-defs'
       WHEN ontology_doc_count * 100 / total_docs >= 30 THEN 'mixed' ELSE 'narrative-doc-axis-finds-howtos' END AS ontology_routing_label
FROM c;
