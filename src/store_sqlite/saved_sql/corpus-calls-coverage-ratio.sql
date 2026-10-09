-- name: corpus-calls-coverage-ratio
-- params: 
WITH c AS (SELECT (SELECT count(*) FROM Function) AS total_functions,
  (SELECT count(DISTINCT r.dst) FROM "CALLS" r JOIN Function a ON a.symbol = r.src JOIN Function b ON b.symbol = r.dst) AS functions_with_inbound_calls)
SELECT total_functions, functions_with_inbound_calls,
  CASE WHEN total_functions = 0 THEN 0.0 ELSE functions_with_inbound_calls * 100.0 / total_functions END AS coverage_percent FROM c;
