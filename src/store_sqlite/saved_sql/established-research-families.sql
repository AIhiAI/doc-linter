-- name: established-research-families
-- params: 
WITH rd AS (SELECT d.id FROM Doc d WHERE EXISTS (SELECT 1 FROM doc_tags WHERE doc_id = d.id AND value = 'research') AND NOT EXISTS (SELECT 1 FROM doc_tags WHERE doc_id = d.id AND value = 'index'))
SELECT dt.value AS family_tag, count(DISTINCT dt.doc_id) AS member_count, json_group_array(DISTINCT dt.doc_id) AS members
FROM doc_tags dt JOIN rd ON rd.id = dt.doc_id
WHERE dt.value NOT IN ('research', 'paper', 'tool', 'production', 'framework', 'survey', 'design', 'system', 'ingest-pass-storyline')
GROUP BY dt.value HAVING count(DISTINCT dt.doc_id) >= 3
ORDER BY member_count DESC, family_tag;
