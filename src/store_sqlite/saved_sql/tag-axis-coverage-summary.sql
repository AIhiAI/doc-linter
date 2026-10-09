-- name: tag-axis-coverage-summary
-- params: 
SELECT tag, uses, CASE WHEN uses >= 7 THEN 'load-bearing' WHEN uses >= 4 THEN 'mid-family' WHEN uses >= 2 THEN 'small-family' ELSE 'singleton' END AS health
FROM (SELECT t.value AS tag, count(DISTINCT d.id) AS uses FROM Doc d JOIN doc_tags t ON t.doc_id = d.id
      WHERE d.role = 'doc' AND t.value NOT IN ('research', 'paper', 'tool', 'production', 'framework', 'survey', 'design', 'system', 'substantial-bundle', 'user-probe', 'interrogation') GROUP BY t.value)
ORDER BY uses DESC, tag;
