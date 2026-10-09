-- name: files-coupled-to
-- params: path
SELECT b.path AS coupled_path, b.language AS language, r.jaccard AS jaccard, r.commits AS commits, r.last_co_change_at AS last_co_change
FROM (SELECT src, dst, commits, jaccard, last_co_change_at FROM "COUPLED_WITH" UNION ALL SELECT dst, src, commits, jaccard, last_co_change_at FROM "COUPLED_WITH") r JOIN File a ON a.path = r.src JOIN File b ON b.path = r.dst
WHERE a.path = $path ORDER BY r.jaccard DESC, b.path;
