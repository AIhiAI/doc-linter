-- name: corpus-entities-per-doc-ratio
-- params: 
WITH c AS (SELECT (SELECT count(*) FROM Doc) AS total_docs, (SELECT count(*) FROM Entity) AS n)
SELECT total_docs, n AS total_entities, CASE WHEN total_docs = 0 THEN 0.0 ELSE n * 1.0 / total_docs END AS entities_per_doc_ratio
FROM c;
