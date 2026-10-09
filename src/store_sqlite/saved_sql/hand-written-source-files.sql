-- name: hand-written-source-files
-- params: 
SELECT f.path AS path, f.language AS language, f.loc AS loc FROM File f WHERE f.language IS NOT NULL AND NOT (instr(f.path, '.gen.') > 0) AND NOT (instr(f.path, '/generated/') > 0) AND NOT (instr(f.path, '/__generated__/') > 0) AND NOT (instr(f.path, '.pb.') > 0) ORDER BY f.path LIMIT 200;
