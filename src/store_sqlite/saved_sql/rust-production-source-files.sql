-- name: rust-production-source-files
-- params: 
SELECT f.path AS path, f.loc AS loc FROM File f
WHERE f.language = 'rust' AND substr(f.path, 1, length('src/')) = 'src/' AND substr(f.path, -3) = '.rs' AND NOT (substr(f.path, -9) = '_tests.rs') AND NOT (substr(f.path, -8) = '_test.rs') AND NOT (substr(f.path, -7) = '/mod.rs')
ORDER BY f.path LIMIT 100;
