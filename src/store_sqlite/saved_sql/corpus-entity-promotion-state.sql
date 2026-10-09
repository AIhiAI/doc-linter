-- name: corpus-entity-promotion-state
-- params: 
WITH s AS (SELECT count(*) AS total_entities,
  coalesce(sum(CASE WHEN substr(e.description, 1, length('Cluster anchored on')) = 'Cluster anchored on' THEN 1 ELSE 0 END), 0) AS cluster_derived_count,
  coalesce(sum(CASE WHEN substr(e.description, 1, length('Cluster anchored on')) = 'Cluster anchored on' THEN 0 ELSE 1 END), 0) AS promoted_count FROM Entity e)
SELECT total_entities, cluster_derived_count, promoted_count,
  CASE WHEN total_entities = 0 THEN 'no-entities' WHEN cluster_derived_count = 0 THEN 'fully-promoted'
       WHEN promoted_count = 0 THEN 'fully-cluster-derived' WHEN cluster_derived_count > promoted_count THEN 'mostly-cluster-derived'
       ELSE 'mostly-promoted' END AS promotion_state_label FROM s;
