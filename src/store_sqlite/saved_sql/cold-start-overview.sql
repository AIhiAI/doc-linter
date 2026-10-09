-- name: cold-start-overview
-- params: 
WITH c AS (SELECT (SELECT count(*) FROM Function) AS fn_count, (SELECT count(*) FROM File) AS file_count,
  (SELECT count(*) FROM Doc) AS doc_count, (SELECT count(*) FROM "COUPLED_WITH") AS coupled_with_count,
  (SELECT count(*) FROM Endpoint) AS endpoint_count,
  (SELECT count(*) FROM Doc nd WHERE NOT nd.role IN ('ontology-value', 'ontology-axis', 'ontology-entity')) AS narrative_doc_count)
SELECT fn_count, file_count, doc_count, narrative_doc_count, coupled_with_count, endpoint_count,
  CASE WHEN fn_count > 0 THEN 'full-scip' WHEN file_count > 0 AND fn_count = 0 THEN 'file-only-no-scip' WHEN file_count = 0 AND doc_count > 0 THEN 'doc-only-by-design' ELSE 'empty' END AS ingest_state,
  CASE WHEN narrative_doc_count = 0 THEN 'doc-empty-ontology-only' WHEN narrative_doc_count < 10 THEN 'doc-sparse'
       WHEN narrative_doc_count >= 50 THEN 'doc-rich' ELSE 'mixed' END AS doc_axis_regime,
  CASE WHEN coupled_with_count > 50 THEN 'coupling-rich' WHEN coupled_with_count > 0 THEN 'coupling-sparse'
       ELSE 'coupling-empty' END AS coupling_axis
FROM c;
