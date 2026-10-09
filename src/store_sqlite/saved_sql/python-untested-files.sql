-- name: python-untested-files
-- params: 
SELECT prod.path AS path, prod.loc AS loc FROM File prod
WHERE prod.language = 'python' AND prod.loc >= 20 AND NOT (instr(prod.path, '/tests/') > 0) AND NOT (instr(prod.path, '/__pycache__/') > 0)
  AND NOT EXISTS (SELECT 1 FROM File test WHERE instr(test.path, '/tests/') > 0
                  AND substr(test.path, length(test.path) - length(('/test_' || replace(prod.path, rtrim(prod.path, replace(prod.path, '/', '')), ''))) + 1) = ('/test_' || replace(prod.path, rtrim(prod.path, replace(prod.path, '/', '')), '')))
ORDER BY prod.loc DESC, prod.path LIMIT 25;
