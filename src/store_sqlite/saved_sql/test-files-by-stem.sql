-- name: test-files-by-stem
-- params: stem
SELECT f.path AS file_path, f.loc AS loc FROM File f WHERE instr(f.path, 'test_') > 0 AND instr(f.path, $stem) > 0 ORDER BY f.loc DESC LIMIT 30;
