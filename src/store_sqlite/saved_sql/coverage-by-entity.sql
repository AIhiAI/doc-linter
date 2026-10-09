-- name: coverage-by-entity
-- params: 
SELECT e.id AS "e.id", e.display AS "e.display", e.is_god_node AS "e.is_god_node",
  (SELECT count(DISTINCT m.src) FROM "FUNCTION_MENTIONS" m WHERE m.dst = e.id) AS coverage
FROM Entity e ORDER BY coverage DESC, e.id;
