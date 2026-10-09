-- name: discriminative-entities
-- params: 
SELECT e.id AS entity_id, e.display AS display, count(DISTINCT c.src) AS coverer_count
FROM "COVERS" c JOIN Entity e ON e.id = c.dst
GROUP BY e.id HAVING count(DISTINCT c.src) <= 7
ORDER BY coverer_count ASC, e.id LIMIT 25;
