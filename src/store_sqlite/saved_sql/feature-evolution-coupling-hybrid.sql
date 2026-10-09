-- name: feature-evolution-coupling-hybrid
-- params: 
SELECT a.path AS "a.path", b.path AS "b.path", r.commits AS "r.commits", r.jaccard AS "r.jaccard", r.last_co_change_at AS "r.last_co_change_at"
FROM "COUPLED_WITH" r JOIN File a ON a.path = r.src JOIN File b ON b.path = r.dst WHERE r.commits >= 5 OR r.jaccard <= 0.55 ORDER BY r.commits DESC, r.jaccard DESC LIMIT 50;
