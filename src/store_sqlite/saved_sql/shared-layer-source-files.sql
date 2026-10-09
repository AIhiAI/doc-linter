-- name: shared-layer-source-files
-- params: 
SELECT * FROM (
  SELECT f.path AS path, f.language AS language, f.loc AS loc, CASE WHEN instr(f.path, '/models.py') > 0 THEN 'models-shared' WHEN instr(f.path, '/schema.py') > 0 THEN 'schema-shared' WHEN instr(f.path, '/crud.py') > 0 THEN 'crud-shared' WHEN instr(f.path, '/main.py') > 0 THEN 'app-entry' WHEN instr(f.path, '/router.py') > 0 THEN 'router-aggregator' WHEN instr(f.path, '/routes.py') > 0 THEN 'router-aggregator' WHEN instr(f.path, '/index.tsx') > 0 THEN 'frontend-index' WHEN instr(f.path, '/index.ts') > 0 THEN 'frontend-index' WHEN instr(f.path, '/layout.tsx') > 0 THEN 'frontend-layout' WHEN instr(f.path, '/__init__.py') > 0 THEN 'python-package-init' ELSE 'other' END AS layer_role
  FROM File f
  WHERE f.language IS NOT NULL AND NOT (instr(f.path, '/test') > 0) AND NOT (instr(f.path, '.gen.') > 0) AND NOT (instr(f.path, '/generated/') > 0)
    AND (instr(f.path, '/models.py') > 0 OR instr(f.path, '/crud.py') > 0 OR instr(f.path, '/schema.py') > 0 OR instr(f.path, '/router.py') > 0 OR instr(f.path, '/routes.py') > 0 OR instr(f.path, '/main.py') > 0 OR instr(f.path, '/index.ts') > 0 OR instr(f.path, '/index.tsx') > 0 OR instr(f.path, '/layout.tsx') > 0 OR instr(f.path, '/__init__.py') > 0))
ORDER BY layer_role, path LIMIT 50;
