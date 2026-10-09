-- name: loc-pair-clone-candidates
-- params: 
SELECT a.path AS path_a, b.path AS path_b, a.loc AS loc_a, b.loc AS loc_b, a.language AS language FROM File a, File b
WHERE a.path < b.path AND a.language = b.language AND a.language IS NOT NULL AND a.loc >= 100 AND b.loc >= 100
  AND a.loc <= b.loc + 20 AND b.loc <= a.loc + 20 AND NOT (instr(a.path, '.gen.') > 0) AND NOT (instr(a.path, '/generated/') > 0) AND NOT (instr(a.path, '/__generated__/') > 0) AND NOT (instr(a.path, '.pb.') > 0) AND NOT (instr(a.path, 'test') > 0) AND NOT (instr(b.path, '.gen.') > 0) AND NOT (instr(b.path, '/generated/') > 0) AND NOT (instr(b.path, '/__generated__/') > 0) AND NOT (instr(b.path, '.pb.') > 0) AND NOT (instr(b.path, 'test') > 0)
ORDER BY a.loc DESC, a.path, b.path LIMIT 25;
