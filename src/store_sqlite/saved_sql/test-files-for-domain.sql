-- name: test-files-for-domain
-- params: domain
SELECT f.path AS file_path, f.loc AS loc FROM File f WHERE instr(f.path, $domain) > 0 AND instr(f.path, 'test') > 0 ORDER BY f.loc DESC LIMIT 30;
