-- name: resource-file-cluster
-- params: resource_stem
SELECT * FROM (
  SELECT f.path AS path, f.language AS language, f.loc AS loc,
    CASE WHEN instr(f.path, '/test') > 0 THEN 'test' WHEN instr(f.path, '/routes/') > 0 THEN 'route'
         WHEN instr(f.path, '/api/') > 0 THEN 'backend-api' WHEN instr(f.path, '/components/') > 0 THEN 'frontend-component'
         WHEN instr(f.path, '.spec.') > 0 THEN 'frontend-spec' WHEN instr(f.path, '.tsx') > 0 THEN 'frontend-tsx'
         WHEN instr(f.path, '.ts') > 0 THEN 'frontend-ts' WHEN instr(f.path, '.py') > 0 THEN 'backend-py' ELSE 'other' END AS layer
  FROM File f WHERE instr(f.path, $resource_stem) > 0 AND f.language IS NOT NULL)
ORDER BY layer, path LIMIT 30;
