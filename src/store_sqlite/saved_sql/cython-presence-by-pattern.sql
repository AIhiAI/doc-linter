-- name: cython-presence-by-pattern
-- params: pattern
WITH c AS (SELECT count(*) AS cython_file_count FROM File f WHERE instr(f.path, $pattern) > 0 AND (substr(f.path, -4) = '.pyx' OR substr(f.path, -4) = '.pxd'))
SELECT cython_file_count, CASE WHEN cython_file_count >= 1 THEN 'yes' ELSE 'no' END AS has_cython FROM c;
