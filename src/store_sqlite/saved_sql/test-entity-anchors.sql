-- name: test-entity-anchors
-- params: 
SELECT e.id AS entity_id, e.display AS display, e.mention_count AS mention_count, e.description AS description
FROM Entity e WHERE substr(e.id, 1, length('test-')) = 'test-' ORDER BY e.mention_count DESC, e.id LIMIT 30;
