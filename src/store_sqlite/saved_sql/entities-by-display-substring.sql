-- name: entities-by-display-substring
-- params: pattern
SELECT e.id AS entity_id, e.display AS display, e.mention_count AS mention_count, e.description AS description
FROM Entity e WHERE instr(e.display, $pattern) > 0 ORDER BY e.mention_count DESC, e.id LIMIT 20;
