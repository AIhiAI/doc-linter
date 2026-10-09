-- name: feature-evolution-coupling-hubs
-- params: 
SELECT f.path AS file, count(r.dst) AS neighbor_count, avg(r.jaccard) AS avg_jaccard
FROM File f JOIN "COUPLED_WITH" r ON r.src = f.path JOIN File o ON o.path = r.dst
WHERE r.commits >= 5 GROUP BY f.path HAVING count(r.dst) >= 2 ORDER BY neighbor_count DESC, avg_jaccard DESC LIMIT 25;
