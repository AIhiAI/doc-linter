-- name: entity-card-summary
-- params: entity_id
SELECT e.id AS entity_id, e.display AS display, e.mention_count AS mention_count, e.description AS description, CASE WHEN substr(e.description, 1, length('Cluster anchored on')) = 'Cluster anchored on' THEN 'cluster-derived' WHEN instr(e.description, '`') > 0 THEN 'promoted-with-pointer' ELSE 'promoted-pure-semantic' END AS promotion_label
FROM Entity e WHERE e.id = $entity_id;
