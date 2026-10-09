-- name: pipeline-stage-coverage
-- params: 
SELECT stage_label, count(id) AS member_count,
  CASE WHEN count(id) >= 3 THEN 'trio-coverage' WHEN count(id) >= 1 THEN 'partial-coverage' ELSE 'gap' END AS coverage_label
FROM (SELECT d.id AS id, CASE WHEN EXISTS (SELECT 1 FROM doc_tags WHERE doc_id = d.id AND value = 'llm-era-query-rewriting') THEN '1-query-rewriting' WHEN EXISTS (SELECT 1 FROM doc_tags WHERE doc_id = d.id AND value = 'chunking') THEN '2-chunking' WHEN EXISTS (SELECT 1 FROM doc_tags WHERE doc_id = d.id AND value = 'deployment-model') THEN '3-embedding' WHEN EXISTS (SELECT 1 FROM doc_tags WHERE doc_id = d.id AND value = 'reranking') THEN '4-reranking' WHEN EXISTS (SELECT 1 FROM doc_tags WHERE doc_id = d.id AND value = 'rag-evaluation') THEN '5-evaluation' ELSE 'NA' END AS stage_label FROM Doc d WHERE EXISTS (SELECT 1 FROM doc_tags WHERE doc_id = d.id AND value = 'research'))
WHERE stage_label <> 'NA' GROUP BY stage_label ORDER BY stage_label;
