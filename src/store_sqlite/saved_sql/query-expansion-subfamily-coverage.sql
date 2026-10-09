-- name: query-expansion-subfamily-coverage
-- params: 
SELECT subfamily_label, count(id) AS member_count,
  CASE WHEN count(id) >= 2 THEN 'rich' WHEN count(id) >= 1 THEN 'singleton' ELSE 'gap' END AS coverage_label
FROM (SELECT d.id AS id, CASE WHEN EXISTS (SELECT 1 FROM doc_tags WHERE doc_id = d.id AND value = 'aqe-local') THEN 'aqe-local' WHEN EXISTS (SELECT 1 FROM doc_tags WHERE doc_id = d.id AND value = 'aqe-global') THEN 'aqe-global' WHEN EXISTS (SELECT 1 FROM doc_tags WHERE doc_id = d.id AND value = 'aqe-external') THEN 'aqe-external' WHEN EXISTS (SELECT 1 FROM doc_tags WHERE doc_id = d.id AND value = 'llm-era-query-rewriting') THEN 'llm-era' WHEN EXISTS (SELECT 1 FROM doc_tags WHERE doc_id = d.id AND value = 'vocabulary-mismatch') THEN 'vocabulary-mismatch-survey' ELSE 'other' END AS subfamily_label FROM Doc d WHERE EXISTS (SELECT 1 FROM doc_tags WHERE doc_id = d.id AND value = 'research') AND EXISTS (SELECT 1 FROM doc_tags WHERE doc_id = d.id AND value = 'query-expansion'))
GROUP BY subfamily_label ORDER BY subfamily_label;
