-- name: corpus-cold-start-summary
-- params: 
WITH c AS (SELECT (SELECT count(*) FROM Doc) AS total_docs, (SELECT count(*) FROM Doc d2 WHERE d2.role IN ('ontology-entity', 'ontology-value', 'ontology-axis', 'ontology-migration', 'index')) AS ontology_meta_docs,
  (SELECT count(*) FROM Entity) AS total_entities, (SELECT count(*) FROM Function) AS total_functions, (SELECT count(*) FROM File) AS total_files)
SELECT total_docs, total_entities, total_functions, total_files, CASE WHEN total_docs = 0 THEN 'EMPTY' WHEN ontology_meta_docs * 100 / total_docs >= 90 THEN 'ADOPTION-BRANCH' WHEN ontology_meta_docs * 100 / total_docs <= 30 THEN 'CONTENT-RICH' ELSE 'MIXED' END AS corpus_purpose
FROM c;
