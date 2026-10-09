-- name: feature-doc-connectivity
-- params: 
SELECT d.id AS id, d.title AS title,
  (SELECT count(*) FROM (SELECT src, dst FROM "WIKILINK" UNION ALL SELECT dst, src FROM "WIKILINK") l JOIN Doc o ON o.id = l.dst WHERE l.src = d.id) AS total_connections
FROM Doc d WHERE d.role = 'feature' ORDER BY total_connections DESC, d.id LIMIT 25;
