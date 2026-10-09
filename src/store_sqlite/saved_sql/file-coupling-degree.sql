-- name: file-coupling-degree
-- params: 
SELECT file_path, language, degree, total_co_change_commits FROM (
  SELECT f.path AS file_path, f.language AS language, count(DISTINCT o.path) AS degree, sum(r.commits) AS total_co_change_commits
  FROM File f JOIN (SELECT src, dst, commits, jaccard, last_co_change_at FROM "COUPLED_WITH" UNION ALL SELECT dst, src, commits, jaccard, last_co_change_at FROM "COUPLED_WITH") r ON r.src = f.path JOIN File o ON o.path = r.dst GROUP BY f.path)
WHERE degree >= 2 ORDER BY degree DESC, total_co_change_commits DESC, file_path LIMIT 50;
