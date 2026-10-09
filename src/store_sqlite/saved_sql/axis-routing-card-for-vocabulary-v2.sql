-- name: axis-routing-card-for-vocabulary-v2
-- params: term
SELECT (SELECT count(DISTINCT e.id) FROM Entity e WHERE instr(e.id, $term) > 0) AS entity_count,
       (SELECT count(DISTINCT f.path) FROM File f WHERE instr(f.path, $term) > 0) AS file_count,
       (SELECT count(DISTINCT fn.symbol) FROM Function fn WHERE instr(fn.symbol, $term) > 0 AND NOT (instr(fn.symbol, 'tests') > 0) AND NOT (instr(fn.symbol, 'test_') > 0)) AS function_count;
