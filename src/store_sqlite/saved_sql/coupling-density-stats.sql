-- name: coupling-density-stats
-- params: 
SELECT count(*) AS total_couplings,
  coalesce(sum(CASE WHEN r.commits < 5 AND r.jaccard > 0.55 THEN 1 ELSE 0 END), 0) AS scaffold_count,
  coalesce(sum(CASE WHEN r.commits >= 5 OR r.jaccard <= 0.55 THEN 1 ELSE 0 END), 0) AS real_count
FROM "COUPLED_WITH" r JOIN File a ON a.path = r.src JOIN File b ON b.path = r.dst;
