-- name: singleton-tags
-- params: 
SELECT value AS tag, min(doc_id) AS only_doc FROM doc_tags
GROUP BY value HAVING count(DISTINCT doc_id) = 1 ORDER BY tag;
