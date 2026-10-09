-- name: node-context-doc
-- params: doc
WITH ed AS (SELECT src, dst, line, 'WIKILINK' AS edge_type FROM "WIKILINK" UNION ALL SELECT src, dst, line, 'INFORMED_BY' AS edge_type FROM "INFORMED_BY" UNION ALL SELECT src, dst, line, 'MD_LINK' AS edge_type FROM "MD_LINK" UNION ALL SELECT src, dst, line, 'DEPENDS_ON' AS edge_type FROM "DEPENDS_ON" UNION ALL SELECT src, dst, line, 'SUPERSEDES' AS edge_type FROM "SUPERSEDES")
SELECT centre.id AS id, centre.title AS title, centre.summary AS summary, centre.path AS path, centre.kind AS kind, centre.role AS role,
  (SELECT json_group_array(value) FROM doc_tags WHERE doc_id = centre.id) AS tags,
  (SELECT json_group_array(json_object('id',x.id,'title',x.title,'summary',x.summary,'path',x.path,'kind',x.kind,'line',x.line,'edge_type',x.edge_type)) FROM (SELECT DISTINCT s.id, s.title, s.summary, s.path, s.kind, r.line, r.edge_type FROM ed r JOIN Doc s ON s.id = r.src WHERE r.dst = centre.id) x) AS inbound,
  (SELECT json_group_array(json_object('id',x.id,'title',x.title,'summary',x.summary,'path',x.path,'kind',x.kind,'line',x.line,'edge_type',x.edge_type)) FROM (SELECT DISTINCT s.id, s.title, s.summary, s.path, s.kind, r.line, r.edge_type FROM ed r JOIN Doc s ON s.id = r.dst WHERE r.src = centre.id) x) AS outbound
FROM Doc centre WHERE centre.id = $doc;
