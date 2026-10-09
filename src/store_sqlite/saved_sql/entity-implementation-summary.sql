-- name: entity-implementation-summary
-- params: entity_id
SELECT e.id AS entity_id,
  (SELECT count(DISTINCT fn.symbol) FROM "FUNCTION_MENTIONS" m JOIN Function fn ON fn.symbol = m.src WHERE m.dst = e.id AND NOT (instr(fn.file, 'test') > 0)) AS function_mentions_count,
  (SELECT count(DISTINCT t.symbol) FROM "TYPE_MENTIONS" m JOIN Type t ON t.symbol = m.src WHERE m.dst = e.id AND NOT (instr(t.file, 'test') > 0)) AS type_mentions_count,
  (SELECT count(DISTINCT fb.symbol) FROM "FUNCTION_BELONGS_TO" m JOIN Function fb ON fb.symbol = m.src WHERE m.dst = e.id AND NOT (instr(fb.file, 'test') > 0)) AS function_belongs_count,
  (SELECT count(DISTINCT fn.file) FROM "FUNCTION_MENTIONS" m JOIN Function fn ON fn.symbol = m.src WHERE m.dst = e.id AND NOT (instr(fn.file, 'test') > 0))
  + (SELECT count(DISTINCT t.file) FROM "TYPE_MENTIONS" m JOIN Type t ON t.symbol = m.src WHERE m.dst = e.id AND NOT (instr(t.file, 'test') > 0)) AS distinct_implementation_files
FROM Entity e WHERE e.id = $entity_id;
