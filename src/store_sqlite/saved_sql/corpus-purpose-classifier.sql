-- name: corpus-purpose-classifier
-- params: 
WITH c AS (SELECT (SELECT count(*) FROM Doc) AS total_docs, (SELECT count(*) FROM Doc d2 WHERE d2.role IN ('ontology-entity', 'ontology-value', 'ontology-axis', 'ontology-migration', 'index')) AS ontology_meta_docs)
SELECT total_docs, ontology_meta_docs,
  CASE WHEN total_docs = 0 THEN 0.0 ELSE ontology_meta_docs * 100.0 / total_docs END AS ontology_ratio_percent,
  CASE WHEN total_docs = 0 THEN 'EMPTY' WHEN ontology_meta_docs * 100 / total_docs >= 90 THEN 'ADOPTION-BRANCH'
       WHEN ontology_meta_docs * 100 / total_docs <= 30 THEN 'CONTENT-RICH' ELSE 'MIXED' END AS corpus_purpose
FROM c;
