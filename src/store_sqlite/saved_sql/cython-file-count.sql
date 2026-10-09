-- name: cython-file-count
-- params: 
SELECT count(f.path) AS cython_file_count FROM File f WHERE (substr(f.path, -4) = '.pyx' OR substr(f.path, -4) = '.pxd') AND NOT (instr(f.path, 'tests/') > 0);
