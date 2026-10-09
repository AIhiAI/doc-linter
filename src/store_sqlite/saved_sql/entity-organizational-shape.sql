-- name: entity-organizational-shape
-- params: entity_id
WITH c AS (SELECT count(*) AS total_file_matches FROM File f WHERE instr(f.path, $entity_id) > 0 AND NOT (instr(f.path, 'test') > 0))
SELECT total_file_matches,
  CASE WHEN total_file_matches = 0 THEN 'absent' WHEN total_file_matches = 1 THEN 'single-file' ELSE 'multi-file-or-folder' END AS organizational_shape FROM c;
