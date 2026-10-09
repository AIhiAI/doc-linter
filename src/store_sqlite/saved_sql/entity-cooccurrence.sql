-- name: entity-cooccurrence
-- params: 
SELECT e1.id AS entity_a, e2.id AS entity_b, count(*) AS cooccur_count
FROM "COVERS" c1 JOIN "COVERS" c2 ON c2.src = c1.src JOIN Entity e1 ON e1.id = c1.dst JOIN Entity e2 ON e2.id = c2.dst
WHERE e1.id < e2.id GROUP BY e1.id, e2.id HAVING count(*) >= 2 ORDER BY cooccur_count DESC, entity_a, entity_b LIMIT 25;
