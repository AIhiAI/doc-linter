-- name: files-containing-entity-id-in-path
-- params: entity_id
SELECT f.path AS path, f.loc AS loc, f.language AS language FROM File f
WHERE instr(f.path, $entity_id) > 0 AND NOT (instr(f.path, 'test') > 0) ORDER BY f.path, f.loc DESC LIMIT 30;
