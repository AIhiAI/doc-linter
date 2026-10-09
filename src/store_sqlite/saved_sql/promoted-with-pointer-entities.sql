-- name: promoted-with-pointer-entities
-- params: 
SELECT e.id AS entity_id, e.mention_count AS mention_count, e.description AS description FROM Entity e WHERE NOT substr(e.description, 1, length('Cluster anchored on')) = 'Cluster anchored on' AND instr(e.description, '`') > 0 ORDER BY e.mention_count DESC, e.id LIMIT 30;
