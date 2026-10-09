-- name: docs-via-shared-entity-coverage
-- params: doc_id
WITH seed_e AS (SELECT DISTINCT dst AS eid FROM "COVERS" WHERE src = $doc_id),
disc AS (SELECT c.dst AS eid FROM "COVERS" c JOIN seed_e s ON s.eid = c.dst
         GROUP BY c.dst HAVING count(DISTINCT c.src) <= 7)
SELECT d.id AS doc_id, d.title AS title,
       json_group_array(DISTINCT c.dst) AS shared_entities,
       count(DISTINCT c.dst) AS shared_count
FROM "COVERS" c JOIN disc ON disc.eid = c.dst JOIN Doc d ON d.id = c.src
WHERE c.src <> $doc_id
GROUP BY d.id ORDER BY shared_count DESC, d.id LIMIT 25;
