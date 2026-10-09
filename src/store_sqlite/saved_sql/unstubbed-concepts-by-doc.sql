-- name: unstubbed-concepts-by-doc
-- params: 
SELECT f.file AS doc_id, count(f.id) AS finding_count FROM Finding f WHERE f.kind = 'unstubbed-concept'
GROUP BY f.file ORDER BY finding_count DESC, doc_id LIMIT 20;
