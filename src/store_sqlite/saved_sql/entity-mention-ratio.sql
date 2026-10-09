-- name: entity-mention-ratio
-- params: 
WITH em AS (SELECT e.id AS id, (SELECT count(DISTINCT fn.symbol) FROM "FUNCTION_MENTIONS" m JOIN Function fn ON fn.symbol = m.src WHERE m.dst = e.id AND NOT (instr(fn.file, 'tests/') > 0) AND NOT (instr(fn.symbol, 'tests/') > 0)) AS function_mentions, (SELECT count(DISTINCT t.symbol) FROM "TYPE_MENTIONS" m JOIN Type t ON t.symbol = m.src WHERE m.dst = e.id AND NOT (instr(t.file, 'tests/') > 0) AND NOT (instr(t.symbol, 'tests/') > 0)) AS type_mentions FROM Entity e)
SELECT id AS entity_id, function_mentions, type_mentions,
  CASE WHEN type_mentions = 0 THEN 999.0 ELSE 1.0 * function_mentions / type_mentions END AS ratio, CASE WHEN type_mentions = 0 THEN 'function-heavy' WHEN 1.0 * function_mentions / type_mentions >= 2.0 THEN 'function-heavy' WHEN 1.0 * function_mentions / type_mentions <= 1.5 THEN 'type-heavy' ELSE 'balanced' END AS flavor_label
FROM em WHERE function_mentions >= 1 OR type_mentions >= 1 ORDER BY ratio DESC, id LIMIT 25;
