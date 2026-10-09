-- name: ingest-pass-status
-- params: 
WITH c AS (SELECT (SELECT count(*) FROM File) AS file_count, (SELECT count(*) FROM Function) AS fn_count,
                  (SELECT count(*) FROM "COUPLED_WITH") AS cw_count, (SELECT count(*) FROM Endpoint) AS ep_count),
p AS (SELECT 'file_walker' AS pass_name, file_count AS row_count FROM c
      UNION ALL SELECT 'scip', fn_count FROM c
      UNION ALL SELECT 'git_coupling', cw_count FROM c
      UNION ALL SELECT 'endpoint_extract', ep_count FROM c)
SELECT pass_name, row_count, row_count > 0 AS populated FROM p ORDER BY pass_name;
