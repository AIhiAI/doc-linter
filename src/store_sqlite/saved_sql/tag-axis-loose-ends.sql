-- name: tag-axis-loose-ends
-- params: 
WITH all_tags AS (SELECT DISTINCT value AS candidate FROM doc_tags WHERE length(value) >= 3)
SELECT d.id AS doc_id, d.title AS title, a.candidate AS missing_tag
FROM Doc d CROSS JOIN all_tags a
WHERE d.summary IS NOT NULL AND instr(d.summary, a.candidate) > 0
  AND NOT EXISTS (SELECT 1 FROM doc_tags t WHERE t.doc_id = d.id AND t.value = a.candidate)
ORDER BY d.id, a.candidate;
