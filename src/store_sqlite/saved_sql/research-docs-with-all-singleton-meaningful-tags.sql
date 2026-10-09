-- name: research-docs-with-all-singleton-meaningful-tags
-- params: 
WITH mt AS (
  SELECT dt.doc_id, dt.value AS t,
    (SELECT count(*) FROM Doc o WHERE o.id <> dt.doc_id AND EXISTS (SELECT 1 FROM doc_tags WHERE doc_id = o.id AND value = dt.value) AND EXISTS (SELECT 1 FROM doc_tags WHERE doc_id = o.id AND value = 'research')) AS share_count
  FROM doc_tags dt JOIN Doc d ON d.id = dt.doc_id
  WHERE EXISTS (SELECT 1 FROM doc_tags WHERE doc_id = d.id AND value = 'research') AND NOT EXISTS (SELECT 1 FROM doc_tags WHERE doc_id = d.id AND value = 'index') AND dt.value NOT IN ('research', 'paper', 'tool', 'production', 'framework', 'survey', 'design', 'system')),
agg AS (SELECT doc_id, sum(CASE WHEN share_count = 0 THEN 1 ELSE 0 END) AS orphan_tags, count(t) AS total_meaningful
        FROM mt GROUP BY doc_id)
SELECT d.id AS doc_id, d.title AS title, agg.total_meaningful AS tag_count
FROM agg JOIN Doc d ON d.id = agg.doc_id
WHERE agg.total_meaningful > 0 AND agg.orphan_tags = agg.total_meaningful ORDER BY d.id;
