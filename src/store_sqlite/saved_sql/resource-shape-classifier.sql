-- name: resource-shape-classifier
-- params: resource_stem
WITH s AS (
  SELECT
    coalesce(sum(CASE WHEN instr(f.path, 'backend/') > 0 AND NOT (instr(f.path, '/test') > 0 OR instr(f.path, '/tests/') > 0) THEN 1 ELSE 0 END), 0) AS backend_src,
    coalesce(sum(CASE WHEN instr(f.path, 'backend/') > 0 AND (instr(f.path, '/test') > 0 OR instr(f.path, '/tests/') > 0) THEN 1 ELSE 0 END), 0) AS backend_test,
    coalesce(sum(CASE WHEN instr(f.path, 'frontend/') > 0 AND NOT (instr(f.path, '/test') > 0 OR instr(f.path, '.spec.') > 0 OR instr(f.path, '/tests/') > 0) THEN 1 ELSE 0 END), 0) AS frontend_src,
    coalesce(sum(CASE WHEN instr(f.path, 'frontend/') > 0 AND (instr(f.path, '/test') > 0 OR instr(f.path, '.spec.') > 0 OR instr(f.path, '/tests/') > 0) THEN 1 ELSE 0 END), 0) AS frontend_test
  FROM File f WHERE instr(f.path, $resource_stem) > 0 AND f.language IS NOT NULL AND NOT (instr(f.path, '/__init__') > 0) AND NOT (instr(f.path, '.gen.') > 0))
SELECT backend_src, backend_test, frontend_src, frontend_test,
  CASE WHEN backend_src >= 1 AND frontend_src >= 1 THEN 'symmetric' WHEN backend_src >= 1 AND frontend_src = 0 THEN 'backend-only'
       WHEN backend_src = 0 AND frontend_src >= 1 THEN 'frontend-only' ELSE 'empty' END AS shape_label
FROM s;
