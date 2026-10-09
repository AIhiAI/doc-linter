-- name: tag-cooccurrence
-- params: 
SELECT t1.value AS tag_a, t2.value AS tag_b, count(*) AS cooccur_count
FROM Doc d JOIN doc_tags t1 ON t1.doc_id = d.id JOIN doc_tags t2 ON t2.doc_id = d.id
WHERE d.role = 'doc' AND t1.value NOT IN ('research', 'paper', 'tool', 'production', 'framework', 'survey', 'design', 'system', 'substantial-bundle', 'user-probe', 'interrogation', 'ingest-pass-storyline') AND NOT substr(t1.value, 1, length('iter-')) = 'iter-'
  AND t1.value < t2.value AND t2.value NOT IN ('research', 'paper', 'tool', 'production', 'framework', 'survey', 'design', 'system', 'substantial-bundle', 'user-probe', 'interrogation', 'ingest-pass-storyline') AND NOT substr(t2.value, 1, length('iter-')) = 'iter-'
GROUP BY t1.value, t2.value HAVING count(*) >= 2 ORDER BY cooccur_count DESC, tag_a, tag_b LIMIT 25;
