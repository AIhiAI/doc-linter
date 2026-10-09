-- name: function-axis-token-distribution-for-stem
-- params: stem
SELECT (SELECT count(DISTINCT fn.symbol) FROM Function fn WHERE instr(fn.symbol, $stem) > 0 AND instr(fn.symbol, '#') > 0 AND NOT (instr(fn.symbol, 'tests') > 0) AND NOT (instr(fn.symbol, 'test_') > 0)) AS class_method_count,
       (SELECT count(DISTINCT fn.symbol) FROM Function fn WHERE instr(fn.symbol, $stem) > 0 AND instr(fn.symbol, '/_') > 0 AND NOT (instr(fn.symbol, 'tests') > 0) AND NOT (instr(fn.symbol, 'test_') > 0)) AS private_count,
       (SELECT count(DISTINCT fn.symbol) FROM Function fn WHERE instr(fn.symbol, $stem) > 0 AND instr(fn.symbol, 'test_') > 0) AS test_count;
