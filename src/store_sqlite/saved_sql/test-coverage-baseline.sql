-- name: test-coverage-baseline
-- params: 
WITH tot AS (SELECT count(*) AS total_function_count FROM Function),
tst AS (SELECT count(*) AS tested_function_count FROM Function f
        WHERE EXISTS (SELECT 1 FROM "TEST_FOR" t WHERE t.dst = f.symbol)),
c AS (SELECT total_function_count, tested_function_count,
        CASE WHEN total_function_count = 0 THEN 0.0
             ELSE 100.0 * tested_function_count / total_function_count END AS coverage_rate
      FROM tot, tst)
SELECT total_function_count, tested_function_count, coverage_rate,
  CASE WHEN coverage_rate >= 10.0 THEN 'coverage-rich' WHEN coverage_rate >= 1.0 THEN 'coverage-sparse'
       ELSE 'coverage-empty' END AS trust_label
FROM c;
