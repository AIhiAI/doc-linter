-- name: files-by-path-substring
-- params: pattern
SELECT f.path AS path, f.language AS language, f.loc AS loc FROM File f WHERE instr(f.path, $pattern) > 0 ORDER BY f.path;
