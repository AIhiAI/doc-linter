-- name: entities-via-shared-doc-coverage
-- params: entity_id
WITH seed_d AS (SELECT DISTINCT src AS did FROM "COVERS" WHERE dst = $entity_id)
SELECT o.id AS entity_id, o.display AS display, count(DISTINCT c.src) AS shared_doc_count
FROM "COVERS" c JOIN seed_d s ON s.did = c.src JOIN Entity o ON o.id = c.dst
WHERE o.id <> $entity_id
GROUP BY o.id ORDER BY shared_doc_count DESC, o.id LIMIT 25;
