-- name: test-fixture-idiom-classifier
-- params: 
WITH c AS (SELECT (SELECT count(DISTINCT fn.symbol) FROM Function fn WHERE instr(fn.symbol, 'test_') > 0 AND instr(fn.symbol, '/_') > 0 AND NOT (instr(fn.symbol, '#__') > 0)) AS module_helper_count,
                  (SELECT count(DISTINCT fn.symbol) FROM Function fn WHERE instr(fn.symbol, 'test_') > 0 AND instr(fn.symbol, '#__') > 0 AND NOT (instr(fn.symbol, 'tests.test_') > 0)) AS inner_class_helper_count)
SELECT module_helper_count, inner_class_helper_count, CASE WHEN module_helper_count = 0 AND inner_class_helper_count = 0 THEN 'EMPTY' WHEN inner_class_helper_count = 0 THEN 'APP-MODULE-ONLY' WHEN module_helper_count = 0 THEN 'INNER-CLASS-ONLY' ELSE 'FRAMEWORK-MIXED' END AS test_fixture_idiom FROM c;
