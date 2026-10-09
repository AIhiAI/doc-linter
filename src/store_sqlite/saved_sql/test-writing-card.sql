-- name: test-writing-card
-- params: stem
SELECT (SELECT count(DISTINCT fn.symbol) FROM Function fn WHERE instr(fn.symbol, $stem) > 0 AND instr(fn.symbol, 'test_') > 0) AS test_function_count,
       (SELECT count(DISTINCT f.path) FROM File f WHERE instr(f.path, 'test_') > 0 AND instr(f.path, $stem) > 0) AS test_file_count;
