-- name: functions-mentioning-test-entity
-- params: test_entity_id
SELECT fn.symbol AS function_symbol FROM Function fn JOIN "FUNCTION_MENTIONS" m ON m.src = fn.symbol
JOIN Entity e ON e.id = m.dst WHERE e.id = $test_entity_id ORDER BY fn.symbol LIMIT 30;
