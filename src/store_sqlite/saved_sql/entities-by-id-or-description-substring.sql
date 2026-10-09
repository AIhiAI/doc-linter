-- name: entities-by-id-or-description-substring
-- params: pattern
SELECT e.id AS entity_id, e.mention_count AS mention_count, e.description AS description FROM Entity e WHERE instr(e.id, $pattern) > 0 OR instr(e.description, $pattern) > 0 ORDER BY e.mention_count DESC, e.id LIMIT 20;
