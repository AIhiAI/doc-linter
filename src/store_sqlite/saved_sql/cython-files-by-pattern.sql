-- name: cython-files-by-pattern
-- params: pattern
SELECT f.path AS path, f.loc AS loc,
  CASE WHEN substr(f.path, -4) = '.pyx' THEN 'cython-source' WHEN substr(f.path, -4) = '.pxd' THEN 'cython-header' ELSE 'unknown' END AS cython_kind
FROM File f WHERE instr(f.path, $pattern) > 0 AND (substr(f.path, -4) = '.pyx' OR substr(f.path, -4) = '.pxd') ORDER BY f.path;
