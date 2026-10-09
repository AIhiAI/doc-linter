-- name: god-node-entities
-- params: 
SELECT e.id AS entity_id, e.display AS display, e.mention_count AS mention_count, e.description AS description FROM Entity e
WHERE e.is_god_node = 1 ORDER BY e.mention_count DESC LIMIT 20;
