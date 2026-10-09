-- name: orphan-entities
-- params: 
SELECT e.id AS "e.id", e.display AS "e.display" FROM Entity e
WHERE NOT EXISTS (SELECT 1 FROM "COVERS" c WHERE c.dst = e.id)
  AND NOT EXISTS (SELECT 1 FROM "FUNCTION_BELONGS_TO" b WHERE b.dst = e.id)
ORDER BY e.id;
