-- name: graph-similarity-subfamily-coverage
-- params: 
SELECT subfamily_label, count(id) AS member_count,
  CASE WHEN count(id) >= 2 THEN 'rich' WHEN count(id) >= 1 THEN 'singleton' ELSE 'gap' END AS coverage_label
FROM (SELECT d.id AS id, CASE WHEN EXISTS (SELECT 1 FROM doc_tags WHERE doc_id = d.id AND value = 'graph-kernel') THEN 'graph-kernel' WHEN EXISTS (SELECT 1 FROM doc_tags WHERE doc_id = d.id AND value = 'graph-embedding') THEN 'graph-embedding' WHEN EXISTS (SELECT 1 FROM doc_tags WHERE doc_id = d.id AND value = 'neural-graph-matching') THEN 'neural-graph-matching' WHEN EXISTS (SELECT 1 FROM doc_tags WHERE doc_id = d.id AND value = 'cross-attention') THEN 'cross-attention' WHEN EXISTS (SELECT 1 FROM doc_tags WHERE doc_id = d.id AND value = 'bipartite-matching') THEN 'bipartite-matching' ELSE 'other' END AS subfamily_label FROM Doc d WHERE EXISTS (SELECT 1 FROM doc_tags WHERE doc_id = d.id AND value = 'research') AND EXISTS (SELECT 1 FROM doc_tags WHERE doc_id = d.id AND value = 'graph-similarity'))
GROUP BY subfamily_label ORDER BY subfamily_label;
