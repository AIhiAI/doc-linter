-- name: files-by-pattern-language-split
-- params: pattern
SELECT f.language AS language, count(f.path) AS file_count, sum(f.loc) AS total_loc, max(f.loc) AS max_loc FROM File f
WHERE instr(f.path, $pattern) > 0 AND f.loc IS NOT NULL AND f.loc > 0 GROUP BY f.language ORDER BY total_loc DESC, language;
