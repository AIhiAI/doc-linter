-- name: concept-work-list
-- params: 
WITH fe AS (SELECT e.id AS eid, (SELECT count(DISTINCT m.src) FROM "FUNCTION_MENTIONS" m WHERE m.dst = e.id) AS functions FROM Entity e)
SELECT eid AS entity, functions,
  (SELECT count(DISTINCT d.id) FROM "COVERS" c JOIN Doc d ON d.id = c.src WHERE c.dst = fe.eid AND NOT d.role IN ('ontology-entity', 'ontology-value', 'ontology-axis', 'ontology-migration', 'index')) AS chapters
FROM fe WHERE functions > 0 ORDER BY chapters ASC, functions DESC, entity LIMIT 25;
