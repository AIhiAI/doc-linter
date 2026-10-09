-- name: competitive-tools
-- params: 
SELECT e.id AS id, e.display AS display, e.description AS description,
  (SELECT json_group_array(value) FROM entity_attributes WHERE entity_id = e.id) AS attributes
FROM Entity e WHERE EXISTS (SELECT 1 FROM entity_attributes WHERE entity_id = e.id) ORDER BY e.id;
