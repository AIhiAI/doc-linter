-- name: corpus-test-coverage-ratio
-- params: 
WITH c AS (SELECT (SELECT count(*) FROM Function) AS total_functions,
                  (SELECT count(DISTINCT symbol) FROM Function tfn WHERE instr(tfn.symbol, 'test_') > 0) AS test_function_count)
SELECT total_functions, test_function_count,
  CASE WHEN total_functions = 0 THEN 0.0 ELSE test_function_count * 100.0 / total_functions END AS test_function_pct FROM c;
