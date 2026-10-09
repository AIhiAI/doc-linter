-- name: corpus-doc-coverage-ratio
-- params: 
WITH c AS (SELECT (SELECT count(*) FROM Doc) AS total_docs, (SELECT count(*) FROM File) AS n)
SELECT total_docs, n AS total_files, CASE WHEN n = 0 THEN 0.0 ELSE total_docs * 1.0 / n END AS docs_per_file_ratio
FROM c;
