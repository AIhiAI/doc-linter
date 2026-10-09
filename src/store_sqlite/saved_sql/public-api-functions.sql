-- name: public-api-functions
-- params: 
SELECT f.file AS file, count(f.symbol) AS public_fn_count FROM Function f
WHERE NOT (instr(f.file, '/tests/') > 0) AND NOT (instr(f.file, '/test_') > 0) AND NOT (instr(f.symbol, '__init__') > 0) AND NOT (instr(f.symbol, '/_') > 0)
GROUP BY f.file ORDER BY public_fn_count DESC, file LIMIT 25;
