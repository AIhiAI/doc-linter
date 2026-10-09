-- name: pytest-prod-test-pairs
-- params: 
SELECT prod.path AS production, test.path AS test, prod.loc AS prod_loc, test.loc AS test_loc
FROM File prod, File test
WHERE prod.path < test.path AND instr(test.path, '/tests/') > 0 AND NOT (instr(prod.path, '/tests/') > 0)
  AND substr(test.path, length(test.path) - length(('/test_' || replace(prod.path, rtrim(prod.path, replace(prod.path, '/', '')), ''))) + 1) = ('/test_' || replace(prod.path, rtrim(prod.path, replace(prod.path, '/', '')), ''))
ORDER BY prod.loc DESC, prod.path LIMIT 25;
