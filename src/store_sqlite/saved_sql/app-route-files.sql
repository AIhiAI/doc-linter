-- name: app-route-files
-- params: 
SELECT * FROM (
  SELECT f.path AS path, f.loc AS loc, CASE WHEN instr(f.path, 'backend/') > 0 THEN 'backend-route' ELSE 'frontend-route' END AS layer
  FROM File f WHERE f.language IS NOT NULL AND (
    (instr(f.path, 'backend/') > 0 AND instr(f.path, '/routes/') > 0 AND NOT (instr(f.path, '/__init__') > 0) AND NOT (instr(f.path, '/test') > 0))
    OR (instr(f.path, 'frontend/src/routes/') > 0 AND NOT (instr(f.path, '_layout.tsx') > 0) AND NOT (instr(f.path, '__root.tsx') > 0))))
ORDER BY layer, path LIMIT 50;
