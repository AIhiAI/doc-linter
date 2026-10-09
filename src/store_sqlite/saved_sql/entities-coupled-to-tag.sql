-- name: entities-coupled-to-tag
-- params: tag
SELECT e.id AS entity_id, e.display AS display, count(DISTINCT d.id) AS tag_doc_count
FROM Doc d JOIN "COVERS" c ON c.src = d.id JOIN Entity e ON e.id = c.dst
WHERE EXISTS (SELECT 1 FROM doc_tags WHERE doc_id = d.id AND value = $tag)
GROUP BY e.id ORDER BY tag_doc_count DESC, e.id LIMIT 25;
