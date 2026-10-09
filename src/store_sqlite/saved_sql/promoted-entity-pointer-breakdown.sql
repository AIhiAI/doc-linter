-- name: promoted-entity-pointer-breakdown
-- params: 
WITH c AS (SELECT count(*) AS total_promoted,
  coalesce(sum(CASE WHEN instr(e.description, '`') > 0 THEN 1 ELSE 0 END), 0) AS promoted_with_pointer_count,
  coalesce(sum(CASE WHEN instr(e.description, '`') > 0 THEN 0 ELSE 1 END), 0) AS promoted_pure_semantic_count
  FROM Entity e WHERE NOT substr(e.description, 1, length('Cluster anchored on')) = 'Cluster anchored on')
SELECT total_promoted, promoted_with_pointer_count, promoted_pure_semantic_count,
  CASE WHEN total_promoted = 0 THEN 'no-promoted' WHEN promoted_with_pointer_count = 0 THEN 'promoted-mostly-pure-semantic'
       WHEN promoted_pure_semantic_count = 0 THEN 'promoted-mostly-with-pointers'
       WHEN promoted_with_pointer_count > promoted_pure_semantic_count THEN 'promoted-mostly-with-pointers'
       WHEN promoted_pure_semantic_count > promoted_with_pointer_count THEN 'promoted-mostly-pure-semantic'
       ELSE 'promoted-mixed' END AS promoted_sub_label FROM c;
