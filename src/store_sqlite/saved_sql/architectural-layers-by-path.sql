-- name: architectural-layers-by-path
-- params: 
SELECT architectural_layer, count(*) AS file_count, sum(loc) AS total_loc FROM (
  SELECT CASE WHEN instr(f.path, '/routes/') > 0 THEN 'frontend-route' WHEN instr(f.path, '/components/') > 0 THEN 'frontend-component'
    WHEN instr(f.path, '/client/') > 0 THEN 'frontend-client' WHEN instr(f.path, '/api/') > 0 THEN 'backend-api'
    WHEN instr(f.path, '/core/') > 0 THEN 'backend-core' WHEN instr(f.path, '/crud') > 0 THEN 'backend-crud'
    WHEN instr(f.path, '/alembic/') > 0 THEN 'backend-migrations' WHEN instr(f.path, '/scripts/') > 0 THEN 'scripts'
    ELSE 'other' END AS architectural_layer, f.loc AS loc
  FROM File f WHERE f.language IS NOT NULL AND NOT (instr(f.path, 'test') > 0) AND NOT (instr(f.path, '.gen.') > 0) AND NOT (instr(f.path, '/generated/') > 0))
GROUP BY architectural_layer ORDER BY total_loc DESC;
