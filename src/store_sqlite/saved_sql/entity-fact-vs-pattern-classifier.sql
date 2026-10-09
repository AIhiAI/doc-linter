-- name: entity-fact-vs-pattern-classifier
-- params: entity_id
SELECT e.id AS entity_id, e.display AS display, e.mention_count AS mention_count, c.fn_count AS fn_count,
  CASE WHEN c.fn_count > 50 THEN 'FACT' WHEN c.fn_count >= 1 AND c.fn_count <= 30 THEN 'PATTERN'
       WHEN c.fn_count >= 31 AND c.fn_count <= 50 THEN 'MID' ELSE 'unclassified' END AS role_label
FROM Entity e JOIN (SELECT count(DISTINCT m.src) AS fn_count FROM "FUNCTION_MENTIONS" m WHERE m.dst = $entity_id) c
WHERE e.id = $entity_id;
