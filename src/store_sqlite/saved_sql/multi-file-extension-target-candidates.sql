-- name: multi-file-extension-target-candidates
-- params: 
SELECT e.id AS entity_id, e.display AS display, e.mention_count AS mention_count, e.description AS description
FROM Entity e WHERE substr(e.description, 1, length('Cluster anchored on')) = 'Cluster anchored on' AND (instr(e.description, 'across 3 file(s)') > 0 OR instr(e.description, 'across 4 file(s)') > 0 OR instr(e.description, 'across 5 file(s)') > 0 OR instr(e.description, 'across 6 file(s)') > 0 OR instr(e.description, 'across 7 file(s)') > 0 OR instr(e.description, 'across 8 file(s)') > 0 OR instr(e.description, 'across 9 file(s)') > 0)
ORDER BY e.mention_count DESC, e.id LIMIT 30;
