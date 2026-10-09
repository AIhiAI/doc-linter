-- name: corpus-top-doc-tags
-- params: 
SELECT value AS tag, count(*) AS frequency FROM doc_tags GROUP BY value ORDER BY frequency DESC, tag LIMIT 15;
