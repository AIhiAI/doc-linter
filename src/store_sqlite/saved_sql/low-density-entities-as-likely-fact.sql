-- name: low-density-entities-as-likely-fact
-- params: 
SELECT e.id AS entity_id, e.display AS display, e.mention_count AS mention_count, e.description AS description
FROM Entity e WHERE substr(e.description, 1, length('Cluster anchored on')) = 'Cluster anchored on' AND (instr(e.description, 'density 0.00') > 0 OR instr(e.description, 'density 0.01') > 0 OR instr(e.description, 'density 0.02') > 0 OR instr(e.description, 'density 0.03') > 0 OR instr(e.description, 'density 0.04') > 0 OR instr(e.description, 'density 0.05') > 0 OR instr(e.description, 'density 0.06') > 0 OR instr(e.description, 'density 0.07') > 0 OR instr(e.description, 'density 0.08') > 0 OR instr(e.description, 'density 0.09') > 0) ORDER BY e.mention_count DESC, e.id LIMIT 30;
