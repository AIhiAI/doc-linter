-- name: entity-pair-comparison
-- params: entity_a, entity_b
SELECT e.id AS entity_id, e.display AS display, e.mention_count AS mention_count, e.description AS description
FROM Entity e WHERE e.id = $entity_a OR e.id = $entity_b ORDER BY e.id;
