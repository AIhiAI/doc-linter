-- name: unstubbed-concepts
-- params: 
SELECT f.message AS term, count(f.id) AS mentions, json_group_array(DISTINCT f.file) AS source_docs FROM Finding f
WHERE f.kind = 'unstubbed-concept' GROUP BY f.message ORDER BY mentions DESC, term;
