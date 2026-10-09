-- name: corpus-doc-starter-ratio
-- params: 
WITH c AS (SELECT coalesce(sum(CASE WHEN (d.role IN ('ontology-value', 'ontology-axis', 'ontology-entity', 'ontology-migration') OR (d.role = 'index' AND substr(d.id, 1, length('ontology')) = 'ontology')) THEN 1 ELSE 0 END), 0) AS starter_count, count(*) AS total_count FROM Doc d),
c2 AS (SELECT starter_count, total_count, total_count - starter_count AS authored_count FROM c)
SELECT starter_count, authored_count, total_count,
  CASE WHEN total_count = 0 THEN 0.0 ELSE 1.0 * starter_count / total_count END AS starter_ratio,
  CASE WHEN total_count = 0 THEN 'doc-empty' WHEN starter_count = total_count THEN 'doc-null'
       WHEN 1.0 * starter_count / total_count >= 0.8 THEN 'mostly-starter' WHEN 1.0 * starter_count / total_count >= 0.3 THEN 'authored-mixed'
       ELSE 'mostly-authored' END AS shape_label FROM c2;
