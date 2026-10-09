-- name: same-name-cross-dir-clones
-- params: 
SELECT * FROM (
  SELECT a.path AS path_a, b.path AS path_b, replace(a.path, rtrim(a.path, replace(a.path, '/', '')), '') AS basename, a.language AS language, a.loc AS loc_a, b.loc AS loc_b
  FROM File a, File b
  WHERE a.path < b.path AND a.language = b.language AND a.language IS NOT NULL AND replace(a.path, rtrim(a.path, replace(a.path, '/', '')), '') = replace(b.path, rtrim(b.path, replace(b.path, '/', '')), '')
    AND NOT (instr(a.path, '.gen.') > 0) AND NOT (instr(a.path, '/generated/') > 0) AND NOT (instr(a.path, '/__generated__/') > 0) AND NOT (instr(a.path, 'test') > 0) AND NOT (instr(b.path, '.gen.') > 0) AND NOT (instr(b.path, '/generated/') > 0) AND NOT (instr(b.path, '/__generated__/') > 0) AND NOT (instr(b.path, 'test') > 0))
ORDER BY basename, path_a LIMIT 25;
