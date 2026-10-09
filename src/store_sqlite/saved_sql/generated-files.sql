-- name: generated-files
-- params: 
SELECT f.path AS path, f.language AS language, f.loc AS loc FROM File f WHERE instr(f.path, '.gen.') > 0 OR instr(f.path, '/generated/') > 0 OR instr(f.path, '/__generated__/') > 0 OR instr(f.path, '.pb.') > 0 ORDER BY f.path;
