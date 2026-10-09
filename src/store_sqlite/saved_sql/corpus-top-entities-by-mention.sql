-- name: corpus-top-entities-by-mention
-- params: 
SELECT e.id AS entity_id, e.mention_count AS mention_count FROM Entity e ORDER BY mention_count DESC, entity_id LIMIT 15;
