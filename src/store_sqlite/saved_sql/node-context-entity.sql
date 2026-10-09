-- name: node-context-entity
-- params: entity
SELECT centre.id AS id, centre.display AS display, centre.description AS description,
  (SELECT json_group_array(json_object('id',x.id,'title',x.title,'summary',x.summary,'path',x.path,'kind',x.kind,'line',x.line)) FROM (SELECT d.id, d.title, d.summary, d.path, d.kind, r.line FROM "COVERS" r JOIN Doc d ON d.id = r.src WHERE r.dst = centre.id) x) AS neighbors
FROM Entity centre WHERE centre.id = $entity;
