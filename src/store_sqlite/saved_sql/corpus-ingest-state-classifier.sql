-- name: corpus-ingest-state-classifier
-- params: 
WITH c AS (SELECT (SELECT count(*) FROM Function) AS fn_count, (SELECT count(*) FROM File) AS file_count,
  (SELECT count(*) FROM Doc) AS doc_count, (SELECT count(*) FROM "COUPLED_WITH") AS coupled_with_count,
  (SELECT count(*) FROM Endpoint) AS endpoint_count)
SELECT fn_count, file_count, doc_count, coupled_with_count, endpoint_count, CASE WHEN fn_count > 0 THEN 'full-scip' WHEN file_count > 0 AND fn_count = 0 THEN 'file-only-no-scip' WHEN file_count = 0 AND doc_count > 0 THEN 'doc-only-by-design' ELSE 'empty' END AS ingest_state FROM c;
