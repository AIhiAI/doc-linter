-- name: utility-cluster-pattern-entities
-- params: 
SELECT e.id AS entity_id, e.display AS display, e.mention_count AS mention_count, e.description AS description
FROM Entity e WHERE substr(e.description, 1, length('Cluster anchored on')) = 'Cluster anchored on' AND (instr(e.description, 'across 1 file(s)') > 0 OR instr(e.description, 'across 2 file(s)') > 0)
ORDER BY e.mention_count DESC, e.id LIMIT 30;
