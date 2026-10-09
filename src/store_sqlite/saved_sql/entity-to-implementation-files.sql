-- name: entity-to-implementation-files
-- params: entity_id
SELECT file_path, count(*) AS mention_count FROM (
  SELECT DISTINCT fn.file AS file_path FROM "FUNCTION_MENTIONS" m JOIN Function fn ON fn.symbol = m.src
  WHERE m.dst = $entity_id AND NOT (instr(fn.file, 'test') > 0) AND EXISTS (SELECT 1 FROM Entity WHERE id = $entity_id)
  UNION ALL
  SELECT DISTINCT t.file FROM "TYPE_MENTIONS" m JOIN Type t ON t.symbol = m.src
  WHERE m.dst = $entity_id AND NOT (instr(t.file, 'test') > 0) AND EXISTS (SELECT 1 FROM Entity WHERE id = $entity_id))
WHERE file_path IS NOT NULL GROUP BY file_path ORDER BY mention_count DESC, file_path LIMIT 20;
