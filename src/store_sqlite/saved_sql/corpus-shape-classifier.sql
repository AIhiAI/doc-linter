-- name: corpus-shape-classifier
-- params: 
WITH s AS (SELECT count(*) AS total_docs,
  coalesce(sum(CASE WHEN d.role IN ('ontology-value', 'ontology-axis', 'ontology-entity') THEN 1 ELSE 0 END), 0) AS ontology_docs,
  coalesce(sum(CASE WHEN d.role IN ('ontology-value', 'ontology-axis', 'ontology-entity') THEN 0 ELSE 1 END), 0) AS narrative_docs FROM Doc d)
SELECT total_docs, ontology_docs, narrative_docs,
  CASE WHEN narrative_docs = 0 THEN 'doc-empty-ontology-only' WHEN narrative_docs < 10 THEN 'doc-sparse'
       WHEN narrative_docs >= 50 THEN 'doc-rich' ELSE 'mixed' END AS regime FROM s;
