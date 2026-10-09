-- name: corpus-entity-axis-shape
-- params: 
SELECT count(e.id) AS total_entity_count, max(e.mention_count) AS max_mention_count, sum(e.mention_count) AS total_mentions,
  CASE WHEN count(e.id) <= 10 THEN 'entity-axis-empty' WHEN count(e.id) <= 50 THEN 'entity-axis-sparse' ELSE 'entity-axis-rich' END AS entity_axis_shape
FROM Entity e;
