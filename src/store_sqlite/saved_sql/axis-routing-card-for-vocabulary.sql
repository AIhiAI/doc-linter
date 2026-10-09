-- name: axis-routing-card-for-vocabulary
-- params: term
SELECT 'entity' AS axis, count(e.id) AS axis_count FROM Entity e WHERE instr(e.id, $term) > 0 HAVING count(e.id) > 0
UNION ALL
SELECT 'file', count(f.path) FROM File f WHERE instr(f.path, $term) > 0 HAVING count(f.path) > 0;
