-- name: corpus-functions-per-file-ratio
-- params: 
WITH c AS (SELECT (SELECT count(*) FROM Function) AS total_functions, (SELECT count(*) FROM File) AS n)
SELECT total_functions, n AS total_files, CASE WHEN n = 0 THEN 0.0 ELSE total_functions * 1.0 / n END AS functions_per_file_ratio
FROM c;
