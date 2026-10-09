-- name: entity-relations
-- params: min_calls=5
SELECT a.id AS "from", b.id AS "to", r.type AS type, 'authored' AS origin, r.frequency AS frequency
FROM "RELATES_TO" r JOIN Entity a ON a.id = r.src JOIN Entity b ON b.id = r.dst
UNION ALL
SELECT a.id, b.id, 'calls', 'derived', r.frequency
FROM "ENTITY_CALLS" r JOIN Entity a ON a.id = r.src JOIN Entity b ON b.id = r.dst
WHERE r.frequency >= CAST($min_calls AS INTEGER);
