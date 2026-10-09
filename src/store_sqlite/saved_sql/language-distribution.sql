-- name: language-distribution
-- params: 
SELECT f.language AS "f.language", count(f.path) AS count FROM File f GROUP BY f.language ORDER BY count DESC, f.language;
