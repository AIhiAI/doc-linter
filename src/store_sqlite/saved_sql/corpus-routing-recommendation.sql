-- name: corpus-routing-recommendation
-- params: 
WITH c AS (SELECT (SELECT count(*) FROM Function) AS fn_count, (SELECT count(*) FROM File) AS file_count,
  (SELECT count(*) FROM Doc d WHERE d.role NOT IN ('ontology-value','ontology-axis','ontology-entity')) AS narrative_doc_count,
  (SELECT count(*) FROM "COUPLED_WITH") AS coupled_with_count)
SELECT fn_count, file_count, narrative_doc_count, coupled_with_count,
  CASE WHEN narrative_doc_count >= 50 AND fn_count >= 100 THEN 'doc-rich-function-rich'
       WHEN narrative_doc_count >= 50 THEN 'doc-rich-function-sparse'
       WHEN coupled_with_count > 0 AND file_count >= 50 THEN 'doc-sparse-coupling-rich'
       WHEN fn_count >= 100 AND file_count >= 50 THEN 'doc-sparse-coupling-empty-function-rich'
       WHEN file_count >= 50 THEN 'doc-sparse-coupling-empty-function-sparse'
       ELSE 'unknown-empty-or-partial' END AS route,
  CASE WHEN narrative_doc_count >= 50 AND fn_count >= 100 THEN 'query_similar (doc + function axes) + research-by-tag + audit_doc_region'
       WHEN narrative_doc_count >= 50 THEN 'query_similar (doc axis) + research-by-tag + audits-by-tag'
       WHEN coupled_with_count > 0 AND file_count >= 50 THEN 'file-coupling-degree + coupled-files + import-fan-out + language-loc-distribution'
       WHEN fn_count >= 100 AND file_count >= 50 THEN 'language-loc-distribution + query_similar type=function + functions-of-shape + callgraph-roots'
       WHEN file_count >= 50 THEN 'language-loc-distribution + language-distribution + import-fan-out'
       ELSE 'corpus appears empty or partial ' || char(8212) || ' check schema; recommend doc-linter check before retrying' END AS recommended_primitives
FROM c;
