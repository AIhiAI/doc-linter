-- name: function-entity-touch-card
-- params: function_symbol
SELECT e.id AS entity_id, e.display AS display, e.is_god_node AS is_god_node, e.mention_count AS mention_count
FROM Function fn JOIN "FUNCTION_MENTIONS" m ON m.src = fn.symbol JOIN Entity e ON e.id = m.dst
WHERE fn.symbol = $function_symbol ORDER BY e.is_god_node DESC, e.mention_count DESC LIMIT 20;
