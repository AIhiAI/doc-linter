-- name: files-by-path-pattern-with-coupling
-- params: pattern
SELECT f.path AS path, f.language AS language, f.loc AS loc, count(DISTINCT o.path) AS degree, sum(r.commits) AS total_co_change_commits
FROM File f LEFT JOIN (SELECT src, dst, commits, jaccard, last_co_change_at FROM "COUPLED_WITH" UNION ALL SELECT dst, src, commits, jaccard, last_co_change_at FROM "COUPLED_WITH") r ON r.src = f.path LEFT JOIN File o ON o.path = r.dst
WHERE instr(f.path, $pattern) > 0 GROUP BY f.path ORDER BY degree DESC, f.loc DESC, f.path LIMIT 50;
